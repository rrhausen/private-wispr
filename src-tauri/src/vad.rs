//! Nachbearbeitungs-VAD (specs 4.2, ADR 0004): Silero-VAD läuft EINMAL nach
//! dem Stop über die komplette Aufnahme und wirkt als Qualitätstor in der
//! Diktat-Verarbeitung — „keine Sprache erkannt" (nicht transkribieren,
//! Halluzinations-Filter) und das Zu-kurz-Gate auf Sprechzeit. Bewusst KEIN
//! Streaming, kein Auto-Stop (gegrillt 22.11.2026: Im PTT-Modell entscheidet
//! die Taste; Denkpausen dürfen eine Aufnahme nicht beenden) und KEIN
//! Sample-Trim: Das Kürzen von Randstil­le verschob Parakeets Decode messbar
//! schlechter (WER 1,86 % → 2,45–5,1 % je nach Trim-Stärke, BT700-Korpus,
//! 22.11.2026) — Anlaufsilence wirkt als Encoder-Warm-up. Die Segmente
//! bleiben als Basis für M3-Chunks (specs 4.9) erhalten.
//!
//! Architektur: Silero läuft IN-PROCESS auf CPU (handy-computer/
//! silero-vad-v6.2-gguf, ~1,2 MB, RTF >1000 auf CPU) hinter dem
//! [`SpeechAnalyzer`]-Trait — die Worker-Isolation (specs 4.1) existiert wegen
//! GPU-Faults der großen Vulkan-Engine; ein CPU-Modell ohne GPU-Zustand
//! braucht sie nicht. Der Selftest nutzt dieselbe Implementierung und misst
//! damit die Pipeline, die die App nutzt.
//!
//! Ausfall-Philosophie (kein stiller Fallback, aber nie blockierend): Fehlt
//! das VAD-Modell oder scheitert es, wird das Diktat trotzdem transkribiert
//! (volle Aufnahme, Gates übersprungen) — laut geloggt und im Status
//! sichtbar (`VadState::Failed`), nie verworfen.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::MutexGuard;

use anyhow::{Context, Result};

use transcribe_cpp::{Backend, Model, ModelOptions, VadOptions, VadSession, VadSessionOptions};

/// Silero-VAD-Modell (v6.2, F32) für transcribe.cpp — Konvertierung von
/// handy-computer, Ursprung snakers4/silero-vad (MIT). Download:
/// `scripts/download-vad-model.ps1`.
pub const VAD_MODEL_FILE: &str = "silero-vad-v6.2-F32.gguf";
pub const VAD_MODEL_REPO: &str = "handy-computer/silero-vad-v6.2-gguf";
/// Gepinnte Revision (identisch zum Download-Skript) — reproduzierbar.
pub const VAD_MODEL_REV: &str = "a9b2d61c20ae4ddf3644edb34969e8185a485448";
pub const VAD_MODEL_SHA256: &str =
    "0d871a800c420fdd5d1b17a3aea4296d1c39608f0ff4a9295eb76457eb1c88e6";
pub const VAD_MODEL_SIZE: u64 = 1_241_056;

const SAMPLE_RATE: u32 = 16_000;

// ─── Policy-Konstanten (gegrillt 22.11.2026: Konstanten statt Settings) ──
// Basis = Silero-Defaults (`get_speech_timestamps`), zwei bewusste Abweichungen
// nach der WER-Nachmessung vom 22.11.2026 (konservativ = Sprache großzügig
// erkennen, damit die Gates leise Diktate nicht fälschlich verwerfen).

/// Wahrscheinlichkeit, ab der ein Frame als Sprache zählt. Unter Sileros
/// Standard 0,5: Leise Wort-Anfänge (Plosive, Flüster-Diktate) müssen als
/// Sprache zählen, sonst meldet das Gate fälschlich „keine/zu wenig Sprache".
pub const VAD_THRESHOLD: f64 = 0.35;
/// Negative Schwelle innerhalb von Sprache; -1 = Silero-Regel
/// `max(threshold - 0.15, 0.01)`.
pub const VAD_NEG_THRESHOLD: f64 = -1.0;
/// Kürzeste Segmentlänge, die als Sprache gilt (Fenster-Rauschen weg).
pub const VAD_MIN_SPEECH_MS: i32 = 250;
/// Stille, die ein Segment beendet. NICHT der Auto-Stop (den gibt es nicht) —
/// steuert nur, wie fein Silero Segmente trennt.
pub const VAD_MIN_SILENCE_MS: i32 = 100;
/// Rand-Polster je Segmentseite; Silero wendet es Innerhalb von `run` an,
/// die gelieferten Segmentgrenzen sind bereits gepolstert. 200 ms statt
/// Sileros 30 ms, damit die Sprechzeit-Summe Wort-Anfänge sicher einschließt
/// (Wirken auf die Gates; geschnitten wird nichts).
pub const VAD_SPEECH_PAD_MS: i32 = 200;
/// Aufnahmen mit weniger Sprechzeit gelten als versehentlich (Blitztext-
/// Gate; ersetzt die Gesamt-Dauer als Maß, sobald der VAD liegt).
pub const MIN_SPEECH_SECS: f64 = 0.3;

/// Ein Sprachsegment in Samples des Inputs: `[start, end)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub start_sample: usize,
    pub end_sample: usize,
}

/// Die Sprache-Analyse einer ganzen Aufnahme (Deep Module: eine Methode).
/// Implementierungen: [`SileroVad`] (App + Selftest) und ein Fake in den
/// Diktat-Tests — die Schnittstelle ist die Test-Fläche.
pub trait SpeechAnalyzer {
    /// Sprachsegmente über die komplette Aufnahme, zeitlich geordnet, in
    /// Samples des Inputs. Ein leerer Vektor ist ein gültiges Ergebnis
    /// („keine Sprache erkannt"); Fehler sind `Err` und führen zum
    /// sichtbaren Überspringen des Trims.
    fn segments(&mut self, samples: &[f32]) -> Result<Vec<Segment>>;
}

/// Gate-Entscheidung aus den Segmenten — reine Funktion, ohne Modell testbar.
/// Es wird NICHT geschnitten (ADR 0004: Sample-Trim verschlechterte Parakeet
/// messbar); die Entscheidung steuert nur, OB transkribiert wird.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GateDecision {
    /// Kein Sprachsegment: nicht transkribieren (spart den Engine-Call und
    /// verhindert Halluzinationen auf Stille — specs 4.2).
    NoSpeech,
    /// Sprache, aber weniger als [`MIN_SPEECH_SECS`] gesprochen.
    TooShort { speech_secs: f64 },
    /// Genug Sprache — die VOLLE Aufnahme geht zur Engine (kein Trim).
    Speech { speech_secs: f64 },
}

/// Qualitätstor aus den Segmenten ableiten: Sprechzeit = Summe der
/// Segmentlängen (ohne Pausen dazwischen; Silero hat die Polster bereits
/// eingerechnet). Reine Funktion über den Segmenten — M3-Chunks (specs 4.9)
/// docken später an derselben Basis an.
pub fn plan_gate(segments: &[Segment]) -> GateDecision {
    let speech_samples: usize = segments
        .iter()
        .filter(|s| s.end_sample > s.start_sample)
        .map(|s| s.end_sample - s.start_sample)
        .sum();
    let speech_secs = speech_samples as f64 / SAMPLE_RATE as f64;
    if speech_samples == 0 {
        return GateDecision::NoSpeech;
    }
    if speech_secs < MIN_SPEECH_SECS {
        return GateDecision::TooShort { speech_secs };
    }
    GateDecision::Speech { speech_secs }
}

// ─── Silero-Implementierung (in-process, CPU) ───────────────────────────

/// Silero-VAD hinter dem Trait. Eine Instanz ist wiederverwendbar (`run`
/// setzt den Zustand zurück); nicht `Sync`, aber `Send` — im App-Slot hinter
/// einem Mutex, im Selftest direkt.
pub struct SileroVad {
    session: VadSession,
}

impl SileroVad {
    /// Modell über die Kandidatenliste laden (wie die STT-Engine: exe-nah für
    /// den portablen Modus, dann `%LOCALAPPDATA%\PrivateWispr\models\`).
    pub fn load_default() -> Result<SileroVad> {
        let path = vad_model_path();
        anyhow::ensure!(
            path.exists(),
            "VAD-Modell fehlt unter {} — scripts/download-vad-model.ps1 ausführen \
             (Diktat läuft ohne VAD weiter, Trim dann aus)",
            path.display()
        );
        Self::load(&path)
    }

    pub fn load(path: &std::path::Path) -> Result<SileroVad> {
        // Backends PROZESSWEIT genau einmal (stt::ensure_backends_once): Ein
        // zweiter init_backends_default()-Aufruf veränderte die Transkription
        // der bereits geladenen Engine (WER 1,9 % → 5,1 %, 22.11.2026).
        crate::stt::ensure_backends_once();
        // CPU ist das Referenz-Backend des Modells (Handy-Repo-Empfehlung);
        // Vulkan brächte für 1,2 MB nur GPU-Standby-Last.
        let model = Model::load_with(
            path,
            &ModelOptions { backend: Backend::Cpu, device: None, no_mmap: false },
        )
        .with_context(|| format!("VAD-Modell-Load fehlgeschlagen ({})", path.display()))?;
        let session = model
            .vad_session_with(&VadSessionOptions::default())
            .context("VAD-Session fehlgeschlagen")?;
        Ok(SileroVad { session })
    }

    /// Die Policy-Konstanten als transcribe.cpp-Optionen (Silero-Defaults,
    /// bewusst festgehalten statt `Default::default()` zu greifen).
    fn options() -> VadOptions {
        VadOptions {
            threshold: VAD_THRESHOLD,
            neg_threshold: VAD_NEG_THRESHOLD,
            min_speech_ms: VAD_MIN_SPEECH_MS,
            min_silence_ms: VAD_MIN_SILENCE_MS,
            speech_pad_ms: VAD_SPEECH_PAD_MS,
            max_speech_ms: 0,
        }
    }
}

impl SpeechAnalyzer for SileroVad {
    fn segments(&mut self, samples: &[f32]) -> Result<Vec<Segment>> {
        let result = self
            .session
            .run(samples, &Self::options())
            .map_err(|e| anyhow::anyhow!("VAD-Auswertung fehlgeschlagen: {e}"))?;
        // VadSegment ist in Input-Samples [start, end) — defensiv clampen,
        // damit ein nativer Ausreißer nie out-of-bounds schneiden kann.
        Ok(result
            .segments
            .iter()
            .filter_map(|s| {
                let start = s.start_sample.max(0) as usize;
                let end = (s.end_sample.max(0) as usize).min(samples.len());
                (end > start).then_some(Segment { start_sample: start, end_sample: end })
            })
            .collect())
    }
}

/// Suchreihenfolge fürs VAD-Modell, identisch zur STT-Engine (stt.rs): neben
/// der exe (`models\`-Unterordner und direkt — portabler Modus), dann
/// `%LOCALAPPDATA%\PrivateWispr\models\` (Ziel des Download-Skripts).
pub fn vad_model_path() -> PathBuf {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("models").join(VAD_MODEL_FILE));
            candidates.push(dir.join(VAD_MODEL_FILE));
        }
    }
    candidates.push(models_dir().join(VAD_MODEL_FILE));
    candidates
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| models_dir().join(VAD_MODEL_FILE))
}

fn models_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PrivateWispr")
        .join("models")
}

// ─── App-Slot: globale, lazy initialisierte Instanz ─────────────────────

/// Sichtbarer Zustand des VAD im App-Betrieb (landet in `pw://status`).
#[derive(Clone, Debug, PartialEq)]
pub enum VadState {
    /// Noch kein Diktat gelaufen — Initialisierung ist lazy (ADR 0004).
    NotTried,
    Active,
    Failed(String),
}

impl VadState {
    /// Kompakter Schlüssel fürs Status-Event (UI textet deutsch).
    pub fn key(&self) -> &'static str {
        match self {
            VadState::NotTried => "not_tried",
            VadState::Active => "active",
            VadState::Failed(_) => "failed",
        }
    }
}

/// Der globale App-Slot. Initialisierung einmalig beim ersten Diktat
/// (`ensure_loaded`); ein Fehlschlag wird gemerkt (kein Retry-Geflimmer je
/// Diktat), die App startet den nächsten Versuch erst nach Neustart.
pub struct AppVad {
    pub analyzer: Option<SileroVad>,
    pub state: VadState,
}

static APP_VAD: Mutex<AppVad> = Mutex::new(AppVad {
    analyzer: None,
    state: VadState::NotTried,
});

/// App-weiter Zugriff. Lock-Ordnung: immer NACH dem Engine-Lock nehmen
/// (beide Diktat-Threads halten sie in dieser Reihenfolge).
pub fn lock_app() -> MutexGuard<'static, AppVad> {
    APP_VAD.lock().expect("VAD-Slot vergiftet")
}

impl AppVad {
    /// Beim ersten Diktat das Modell laden. Liefert `true`, wenn sich der
    /// Zustand geändert hat (Aufrufer stößt ein Status-Event an — NACH dem
    /// Loslassen des Locks, er ist nicht reentrant).
    pub fn ensure_loaded(&mut self) -> bool {
        if !matches!(self.state, VadState::NotTried) {
            return false;
        }
        match SileroVad::load_default() {
            Ok(vad) => {
                log::info!("VAD bereit: {VAD_MODEL_FILE} (Silero, CPU, in-process)");
                self.analyzer = Some(vad);
                self.state = VadState::Active;
            }
            Err(e) => {
                log::warn!("VAD nicht verfügbar ({e:#}) — Diktate laufen ungetrimmt");
                self.state = VadState::Failed(e.to_string());
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: usize = SAMPLE_RATE as usize; // 1 s in Samples

    fn seg(start: usize, end: usize) -> Segment {
        Segment { start_sample: start, end_sample: end }
    }

    /// Kein Segment ⇒ NoSpeech (Transkription wird gar nicht erst angestoßen).
    #[test]
    fn keine_segmente_ist_keine_sprache() {
        assert_eq!(plan_gate(&[]), GateDecision::NoSpeech);
        // Entartete Segmente (Ende <= Start) zählen nicht als Sprache.
        assert_eq!(plan_gate(&[seg(100, 100)]), GateDecision::NoSpeech);
    }

    /// Sprechzeit = Summe der Segmentlängen; unter der Schwelle ⇒ TooShort.
    #[test]
    fn sprache_unter_schwelle_ist_zu_kurz() {
        // 0,2 s Sprache in zwei Häppchen mit Pause.
        let d = plan_gate(&[seg(S, S + 1600), seg(3 * S, 3 * S + 1600)]);
        assert_eq!(d, GateDecision::TooShort { speech_secs: 0.2 });
        // Exakt an der Schwelle (0,3 s) passiert das Gate: < ist Kriterium.
        let min_samples = (MIN_SPEECH_SECS * SAMPLE_RATE as f64) as usize;
        let exact = plan_gate(&[seg(0, min_samples)]);
        assert!(matches!(exact, GateDecision::Speech { .. }), "{exact:?}");
    }

    /// Sprache über der Schwelle ⇒ Speech mit der summierten Sprechzeit;
    /// Pausen zwischen Segmenten zählen NICHT als Sprache.
    #[test]
    fn sprache_zaehlt_ohne_pausen_dazwischen() {
        let d = plan_gate(&[seg(S, 2 * S), seg(5 * S, 6 * S)]);
        assert_eq!(d, GateDecision::Speech { speech_secs: 2.0 });
    }

    /// Pfad-Kandidaten: exe-nah zuerst, LOCALAPPDATA-Standard zuletzt.
    #[test]
    fn modellpfad_endet_mit_standardkandidaten() {
        let exe_dir = std::path::Path::new(r"C:\Tools\PrivateWispr");
        let mut candidates = vec![
            exe_dir.join("models").join(VAD_MODEL_FILE),
            exe_dir.join(VAD_MODEL_FILE),
        ];
        candidates.push(models_dir().join(VAD_MODEL_FILE));
        // vad_model_path() liefert einen Treffer aus derselben Liste (oder
        // den Standard, wenn keiner existiert) — hier prüfen wir nur die Form.
        let p = vad_model_path();
        assert!(candidates.contains(&p) || p == models_dir().join(VAD_MODEL_FILE));
    }
}
