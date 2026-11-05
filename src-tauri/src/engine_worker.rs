//! Engine-Worker-Isolation (specs 4.1, Handy-`engine_supervisor`-Muster als
//! Konzept-Blaupause, Eigenimplementierung): Die STT-Engine läuft in einem
//! Kindprozess — `private-wispr.exe --engine-worker`. Ein GPU-Fault killt
//! nur den Worker: Das Diktat bekommt einen klaren Fehler, die App bleibt
//! am Leben, der Supervisor startet den Worker neu und lädt das Modell
//! erneut — sichtbar im Log und in EngineInfo, kein stiller Fallback.
//!
//! Protokoll: JSON-Zeilen auf stdin/stdout des Kindes; die Audio-Samples
//! folgen als roher little-endian-f32-Block (Länge im Request) — ohne
//! Base64-Overhead (16 kHz mono ≈ 64 kB/s).
//!
//! ```text
//! → {"cmd":"load","model":"<pfad>"}
//! → {"cmd":"transcribe","vocabulary":[…],"samplesBytes":N} + N Bytes
//! ← {"type":"loaded","info":{…}} | {"type":"result",…} | {"type":"error",…}
//! ```
//!
//! Sequenzierung: eine Anfrage zur Zeit (die App serialisiert über den
//! Engine-Mutex), daher reicht ein Antwort-Kanal pro Worker-Instanz.

use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::stt::{self, EngineInfo, SttEngine, TranscriptResult};

// ─── Worker-Seite (Kindprozess) ────────────────────────────────────────

/// Worker-Modus (main.rs leitet `--engine-worker` hierher). Läuft bis stdin
/// geschlossen wird (App beendet sich) und antwortet zeilenweise auf stdout.
pub fn run_worker() -> i32 {
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stdout = std::io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let mut engine: Option<stt::Engine> = None;

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF: App beendet
            Ok(_) => {}
            Err(_) => break,
        }
        let cmd: WorkerCommand = match serde_json::from_str(line.trim()) {
            Ok(c) => c,
            Err(e) => {
                let _ = write_reply(&mut out, &serde_json::json!({"type": "error", "message": e.to_string()}));
                continue;
            }
        };
        match cmd {
            WorkerCommand::Load { model } => {
                match stt::Engine::load(Path::new(&model)) {
                    Ok(loaded) => {
                        let info = loaded.info().clone();
                        engine = Some(loaded);
                        let _ = write_reply(&mut out, &serde_json::json!({"type": "loaded", "info": info}));
                    }
                    Err(e) => {
                        let _ = write_reply(
                            &mut out,
                            &serde_json::json!({"type": "error", "message": format!("Modell-Load fehlgeschlagen: {e:#}")}),
                        );
                    }
                }
            }
            WorkerCommand::Transcribe { vocabulary, samples_bytes } => {
                // Binärblock direkt hinter der Zeile lesen.
                let mut buf = vec![0u8; samples_bytes];
                if reader.read_exact(&mut buf).is_err() {
                    break;
                }
                let Some(engine) = engine.as_mut() else {
                    let _ = write_reply(&mut out, &serde_json::json!({"type": "error", "message": "Modell nicht geladen"}));
                    continue;
                };
                let samples = bytes_to_f32(&buf);
                match engine.transcribe(&samples, &vocabulary) {
                    Ok(r) => {
                        let _ = write_reply(
                            &mut out,
                            &serde_json::json!({
                                "type": "result",
                                "text": r.text,
                                "language": r.language,
                                "ms": r.transcription_ms as u64,
                            }),
                        );
                    }
                    Err(e) => {
                        let _ = write_reply(
                            &mut out,
                            &serde_json::json!({"type": "error", "message": format!("Transkription fehlgeschlagen: {e:#}")}),
                        );
                    }
                }
            }
        }
    }
    0
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "cmd")]
enum WorkerCommand {
    Load { model: String },
    #[serde(rename_all = "camelCase")]
    Transcribe { vocabulary: Vec<String>, samples_bytes: usize },
}

fn write_reply(out: &mut BufWriter<std::io::StdoutLock<'_>>, value: &serde_json::Value) -> std::io::Result<()> {
    out.write_all(value.to_string().as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()
}

fn bytes_to_f32(buf: &[u8]) -> Vec<f32> {
    buf.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn f32_to_bytes(samples: &[f32]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

// ─── Supervisor-Seite (App) ────────────────────────────────────────────

enum WorkerMsg {
    Loaded(EngineInfo),
    Result(TranscriptResult),
    Error(String),
}
impl WorkerMsg {
    fn parse(line: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        match v.get("type")?.as_str()? {
            "loaded" => Some(WorkerMsg::Loaded(serde_json::from_value(v["info"].clone()).ok()?)),
            "result" => Some(WorkerMsg::Result(TranscriptResult {
                text: v["text"].as_str().unwrap_or_default().to_string(),
                language: v["language"].as_str().map(str::to_string),
                transcription_ms: v["ms"].as_u64().unwrap_or_default() as u128,
            })),
            "error" => Some(WorkerMsg::Error(v["message"].as_str().unwrap_or_default().to_string())),
            _ => None,
        }
    }
}

/// STT-Engine hinter dem Worker-Prozess (implementiert `SttEngine`).
pub struct WorkerEngine {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    /// Antworten des Readers-Threads (eine Anfrage zur Zeit).
    rx: Receiver<WorkerMsg>,
    info: EngineInfo,
    model_path: PathBuf,
}

impl WorkerEngine {
    /// Default-Modell in einem Worker laden (App-Start, nach Modell-Download).
    pub fn start_default() -> Result<Self> {
        let model_path = stt::default_model_path();
        anyhow::ensure!(
            model_path.exists(),
            "Modell fehlt unter {} — erst Einstellungen → Modell herunterladen",
            model_path.display()
        );
        Self::start(&model_path)
    }

    fn start(model_path: &Path) -> Result<Self> {
        // Eigene exe als Worker (App spawnt sich selbst). Die Umgebungs-
        // variable erlaubt Tests/Debug, eine konkrete Binary vorzugeben.
        let exe = match std::env::var_os("PW_ENGINE_WORKER_EXE") {
            Some(p) => PathBuf::from(p),
            None => std::env::current_exe().context("Eigene exe nicht ermittelbar")?,
        };
        let mut child = Command::new(exe)
            .arg("--engine-worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // transcribe.cpp loggt eigenständig; Meldungen des Kindprozesses
            // laufen über die Reply- und Exit-Pfade, nicht über stderr.
            .stderr(Stdio::null())
            .spawn()
            .context("Engine-Worker-Prozess nicht startbar")?;
        let stdin = child.stdin.take().context("Worker-stdin fehlt")?;
        let stdout = child.stdout.take().context("Worker-stdout fehlt")?;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("pw-engine-reader".into())
            .spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    match WorkerMsg::parse(&line) {
                        Some(msg) => {
                            if tx.send(msg).is_err() {
                                break; // Supervisor verworfen
                            }
                        }
                        None => continue,
                    }
                }
                // Kanal zu (tx gedroppt) → recv liefert Disconnected → Tod.
            })
            .context("Worker-Reader-Thread nicht startbar")?;

        let mut engine = WorkerEngine {
            child,
            stdin: BufWriter::new(stdin),
            rx,
            info: EngineInfo::default(),
            model_path: model_path.to_path_buf(),
        };
        engine.send_load()?;
        Ok(engine)
    }

    fn send_load(&mut self) -> Result<()> {
        let req = serde_json::json!({"cmd": "load", "model": self.model_path.display().to_string()});
        self.write_request(&req, &[])?;
        match self.recv(Duration::from_secs(120))? {
            WorkerMsg::Loaded(mut info) => {
                info.loaded = true;
                self.info = info;
                Ok(())
            }
            WorkerMsg::Error(e) => bail!("Modell-Load im Worker fehlgeschlagen: {e}"),
            _ => bail!("Unerwartete Worker-Antwort auf load"),
        }
    }

    fn write_request(&mut self, head: &serde_json::Value, body: &[u8]) -> Result<()> {
        self.stdin.write_all(head.to_string().as_bytes())?;
        self.stdin.write_all(b"\n")?;
        if !body.is_empty() {
            self.stdin.write_all(body)?;
        }
        self.stdin.flush().context("Engine-Worker nicht erreichbar (stdin zu)")
    }

    /// Auf die nächste Antwort warten; unbekannte Zwischennachrichten werden
    /// überlesen, nach Ablauf der Frist klar abgebrochen.
    fn recv(&mut self, timeout: Duration) -> Result<WorkerMsg> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.checked_duration_since(Instant::now()).unwrap_or_default();
            match self.rx.recv_timeout(remaining) {
                Ok(msg) => return Ok(msg),
                Err(mpsc::RecvTimeoutError::Timeout) => bail!("Engine-Worker antwortet nicht (Timeout)"),
                Err(mpsc::RecvTimeoutError::Disconnected) => bail!("Engine-Worker-Prozess wurde beendet"),
            }
        }
    }

    /// Nach einem Absturz: Prozess endgültig einsammeln, neu starten, Modell
    /// neu laden. Der Fehler des abgestürzten Diktats bleibt beim Aufrufer —
    /// kein stiller Wiederholungsversuch.
    fn restart(&mut self) -> Result<()> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let fresh = Self::start(&self.model_path.clone())?;
        self.child = fresh.child;
        self.stdin = fresh.stdin;
        self.rx = fresh.rx;
        self.info = fresh.info;
        log::warn!("Engine-Worker neu gestartet und Modell erneut geladen");
        Ok(())
    }
}

impl SttEngine for WorkerEngine {
    fn transcribe(&mut self, samples: &[f32], vocabulary: &[String]) -> Result<TranscriptResult> {
        let bytes = f32_to_bytes(samples);
        let req = serde_json::json!({
            "cmd": "transcribe",
            "vocabulary": vocabulary,
            "samplesBytes": bytes.len(),
        });
        // RTF ~1: Audio-Sekunden + Puffer; kurze Diktate bekommen ein
        // Mindestbudget für Modell-Warmup.
        let audio_secs = samples.len() as f64 / 16_000.0;
        let timeout = Duration::from_secs(audio_secs as u64 + 90);

        if let Err(e) = self.write_request(&req, &bytes) {
            // Prozess tot (stdin gebrochen): Neustart wagen, Diktat klar
            // fehlschlagen — App bleibt leben (der Sinn der Isolation).
            log::error!("Engine-Worker nicht erreichbar ({e:#}) — Neustart");
            if let Err(r) = self.restart() {
                log::error!("Worker-Neustart fehlgeschlagen: {r:#}");
            }
            bail!("Engine-Worker abgestürzt — bitte Diktat wiederholen (Worker wurde neu gestartet)");
        }
        match self.recv(timeout) {
            Ok(WorkerMsg::Result(result)) => Ok(result),
            Ok(WorkerMsg::Error(e)) => {
                // Engine-Fehler ohne Prozess-Tod: Diktat fehlschlagen, Worker
                // läuft weiter.
                self.info.error = Some(e.clone());
                bail!("Transkription im Worker fehlgeschlagen: {e}")
            }
            Ok(WorkerMsg::Loaded(_)) => bail!("Unerwartete Worker-Antwort (loaded) auf transcribe"),
            Err(e) => {
                log::error!("Engine-Worker-Fehler ({e:#}) — Neustart");
                if let Err(r) = self.restart() {
                    log::error!("Worker-Neustart fehlgeschlagen: {r:#}");
                }
                bail!("Engine-Worker abgestürzt (GPU-Fehler?) — bitte Diktat wiederholen")
            }
        }
    }

    fn info(&self) -> &EngineInfo {
        &self.info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_block_roundtrip() {
        let samples = vec![0.0f32, -1.0, 0.5, 1234.5];
        let bytes = f32_to_bytes(&samples);
        assert_eq!(bytes.len(), samples.len() * 4);
        assert_eq!(bytes_to_f32(&bytes), samples);
    }

    #[test]
    fn worker_nachrichten_parsen() {
        let loaded = WorkerMsg::parse(r#"{"type":"loaded","info":{"loaded":true,"model_name":"parakeet","backend":"vulkan","on_gpu":true}}"#);
        assert!(matches!(loaded, Some(WorkerMsg::Loaded(ref i)) if i.backend == "vulkan"));

        let result = WorkerMsg::parse(r#"{"type":"result","text":"Hallo","language":null,"ms":42}"#);
        match result {
            Some(WorkerMsg::Result(r)) => {
                assert_eq!(r.text, "Hallo");
                assert_eq!(r.transcription_ms, 42);
            }
            _ => panic!("result erwartet"),
        }

        let error = WorkerMsg::parse(r#"{"type":"error","message":"boom"}"#);
        assert!(matches!(error, Some(WorkerMsg::Error(ref m)) if m == "boom"));

        assert!(WorkerMsg::parse(r#"{"type":"unbekannt"}"#).is_none());
    }

    #[test]
    fn befehl_serialisierung_matcht_tags() {
        // Der Supervisor sendet camelCase + cmd-Tag — muss in WorkerCommand
        // aufgehen (internes Vertragsstück des Protokolls).
        let load: WorkerCommand = serde_json::from_str(r#"{"cmd":"load","model":"C:\\m.gguf"}"#).unwrap();
        assert!(matches!(load, WorkerCommand::Load { .. }));
        let tr: WorkerCommand =
            serde_json::from_str(r#"{"cmd":"transcribe","vocabulary":["a"],"samplesBytes":8}"#).unwrap();
        assert!(matches!(tr, WorkerCommand::Transcribe { samples_bytes: 8, .. }));
    }
}
