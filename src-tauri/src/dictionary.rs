//! Wörterbuch-/Regel-Pipeline (specs 4.5): deterministische Phrasen-Regeln
//! mit Alias-Listen aus YAML im Benutzerprofil.
//!
//! Ausführungs-Reihenfolge innerhalb `apply` (fix, testbar): Unicode-NFC →
//! Filler-Entfernung (de-aware) → Phrasen-Regeln. Längste Phrase zuerst,
//! echte Wortgrenzen (`\p{L}`-basiert — kein Treffer mitten im Wort,
//! Komposita bleiben unangetastet), case-insensitiv inkl. Umlauten/ß,
//! Profile ein-/ausschaltbar. Jede angewandte Regel wird mit einer
//! Versionskennung der Datei protokolliert (Historie, specs 4.8).
//!
//! Die Kanones aktiver Regeln wirken zusätzlich als `vocabulary` im Decode
//! (stt.rs); Fuzzy/Phonetik (Kölner Phonetik) folgt in v2. Liegt die Datei
//! nicht vor, wird einmalig eine mit exemplarischen Regeln angelegt (echte
//! Regelfälle kommen aus den Eval-Clips, specs Abschnitt 6).

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

/// Budget fürs `vocabulary` im Decode (specs 4.5: Budget-Begrenzung, damit
/// der Decoder nicht überläuft — bei Überlauf loggen, nicht still kappen).
const VOCABULARY_CAP: usize = 64;

/// Deutsche Filler als Ganzwort (case-insensitiv). Bewusst kurz: „halt",
/// „eben" u. ä. sind echte Wörter und bleiben stehen.
const FILLERS: &[&str] = &["äh", "ähm", "ehm", "öh", "öhm", "hm", "hmm", "mhm"];

/// YAML-Format (specs 4.5), editierbar per Editor:
/// ```yaml
/// locale: de-DE
/// profiles: [it]
/// terms:
///   - canonical: "SharePoint"
///     aliases: ["Share Point", "Scherpoint"]
///     profile: it        # optional; ohne = immer aktiv
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DictionaryFile {
    #[serde(default = "default_locale")]
    pub locale: String,
    /// Aktive Profile; Terme mit Profil sind nur dann aktiv.
    #[serde(default)]
    pub profiles: Vec<String>,
    #[serde(default)]
    pub terms: Vec<Term>,
}

fn default_locale() -> String {
    "de-DE".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Term {
    pub canonical: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// Profil-Zugehörigkeit; `None` = in jedem Profil aktiv.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

/// Eine angewandte Regel fürs Historie-Protokoll (`applied_rules`-JSON).
#[derive(Clone, Debug, Serialize)]
pub struct AppliedRule {
    pub canonical: String,
    /// Erkannte Phrase im Original (z. B. „Scherpoint").
    pub matched: String,
    pub count: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ApplyOutcome {
    pub text: String,
    pub applied: Vec<AppliedRule>,
}

struct CompiledRule {
    /// Ersatztext (NFC, kanonische Schreibweise).
    canonical: String,
}

pub struct Dictionary {
    pub locale: String,
    /// In der Datei aktive Profile.
    pub active_profiles: Vec<String>,
    /// Alle Profile, die bei Termen vorkommen (für die UI).
    pub profiles: Vec<String>,
    /// SHA256-Präfix der Datei — Version der angewandten Regeln.
    pub version: String,
    pub term_count: usize,
    pub alias_count: usize,
    /// Ein Matcher über alle Phrasen (längste zuerst), None = keine Regeln.
    matcher: Option<regex::Regex>,
    /// Klein gefaltete Phrase → Index in `rules`.
    phrase_map: HashMap<String, usize>,
    rules: Vec<CompiledRule>,
    /// Kanones aktiver Regeln (dedupliziert) fürs `vocabulary` im Decode.
    vocabulary: Vec<String>,
}

impl Dictionary {
    /// Kompiliert eine Datei-Struktur (auch für Unit-Tests, ohne Dateisystem).
    pub fn compile(file: DictionaryFile) -> Dictionary {
        let mut dict = Dictionary {
            locale: file.locale,
            active_profiles: file.profiles.clone(),
            profiles: Vec::new(),
            version: String::new(),
            term_count: 0,
            alias_count: 0,
            matcher: None,
            phrase_map: HashMap::new(),
            rules: Vec::new(),
            vocabulary: Vec::new(),
        };

        // Profile sammeln (deklariert oder an Termen hängend), sortiert für
        // eine stabile UI-Anzeige.
        let mut profiles: Vec<String> = file.profiles.clone();
        for term in &file.terms {
            if let Some(p) = &term.profile {
                if !profiles.iter().any(|x| x == p) {
                    profiles.push(p.clone());
                }
            }
        }
        profiles.sort();
        dict.profiles = profiles;

        // Phrasen aktiver Terme sammeln: Kanone + Aliase, NFC, non-leer.
        let mut phrases: Vec<String> = Vec::new();
        let mut active_canonicals: Vec<String> = Vec::new();
        for term in &file.terms {
            let canonical: String = term.canonical.trim().nfc().collect();
            if canonical.is_empty() {
                log::warn!("Wörterbuch: Term ohne Kanone übersprungen ({term:?})");
                continue;
            }
            let active = term
                .profile
                .as_ref()
                .map_or(true, |p| file.profiles.iter().any(|x| x == p));
            if !active {
                continue;
            }
            dict.term_count += 1;
            dict.alias_count += term.aliases.len();
            active_canonicals.push(canonical.clone());
            phrases.push(canonical);
            for alias in &term.aliases {
                let alias: String = alias.trim().nfc().collect();
                if alias.is_empty() {
                    continue;
                }
                phrases.push(alias);
            }
        }

        // Dedupliziertes Vocabulary (Kanones aktiver Regeln).
        active_canonicals.sort();
        active_canonicals.dedup();
        if active_canonicals.len() > VOCABULARY_CAP {
            log::warn!(
                "Wörterbuch: {} aktive Fachwörter übersteigen das Vocabulary-Budget von {} — \
                 nur die ersten kommen in den Decode (spezifischere Regeln zuerst ergänzen)",
                active_canonicals.len(),
                VOCABULARY_CAP
            );
            active_canonicals.truncate(VOCABULARY_CAP);
        }
        dict.vocabulary = active_canonicals;

        if phrases.is_empty() {
            return dict;
        }

        // Längste Phrase zuerst: die regex-Alternation ist leftmost-first,
        // also muss die längste Alternative an früher Stelle stehen, damit
        // sie an derselben Position gewinnt.
        phrases.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()));
        let pattern = phrases
            .iter()
            .map(|p| regex::escape(p))
            .collect::<Vec<_>>()
            .join("|");
        let matcher = match regex::RegexBuilder::new(&format!("(?i)(?:{pattern})"))
            .size_limit(4 * 1024 * 1024)
            .build()
        {
            Ok(re) => re,
            Err(e) => {
                log::error!("Wörterbuch: Phrasen-Matcher nicht kompilierbar ({e}) — Regeln deaktiviert");
                return dict;
            }
        };

        // Kanone + Aliase zeigen auf dieselbe Regel; erste Regel gewinnt bei
        // Phrase-Kollisionen zwischen Termen.
        for term in &file.terms {
            let canonical: String = term.canonical.trim().nfc().collect();
            if canonical.is_empty() {
                continue;
            }
            let active = term
                .profile
                .as_ref()
                .map_or(true, |p| file.profiles.iter().any(|x| x == p));
            if !active {
                continue;
            }
            let idx = dict.rules.len();
            dict.rules.push(CompiledRule { canonical });
            for phrase in std::iter::once(&term.canonical).chain(term.aliases.iter()) {
                let key: String = phrase.trim().nfc().collect::<String>().to_lowercase();
                if key.is_empty() {
                    continue;
                }
                dict.phrase_map.entry(key).or_insert(idx);
            }
        }
        dict.matcher = Some(matcher);
        dict
    }

    /// Lädt das Wörterbuch aus dem Benutzerprofil. Fehler werden geloggt und
    /// führen zu einem leeren Wörterbuch (Diktat funktioniert ohne Regeln
    /// weiter — die Datei ist vom Nutzer editierbar, ein Syntaxfehler darf
    /// das Diktat nicht blockieren; der Fehler steht im Log).
    pub fn load() -> Dictionary {
        match Self::load_inner() {
            Ok(d) => d,
            Err(e) => {
                log::error!("Wörterbuch nicht ladbar — Regeln deaktiviert: {e:#}");
                Dictionary::default()
            }
        }
    }

    fn load_inner() -> Result<Dictionary> {
        let path = file_path();
        if !path.exists() {
            write_default_file(&path)?;
            log::info!("Wörterbuch angelegt mit exemplarischen Regeln: {}", path.display());
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("Wörterbuch nicht lesbar: {}", path.display()))?;
        let file: DictionaryFile = serde_yaml::from_str(&raw)
            .with_context(|| format!("Wörterbuch-YAML ungültig: {}", path.display()))?;
        let mut dict = Self::compile(file);
        dict.version = content_version(&raw);
        Ok(dict)
    }

    /// Kanones aktiver Regeln fürs `vocabulary` im Decode (stt.rs).
    pub fn vocabulary(&self) -> &[String] {
        &self.vocabulary
    }

    /// NFC → Filler-Entfernung → Phrasen-Regeln. Gibt den neuen Text und die
    /// protokollierten Regeln zurück.
    pub fn apply(&self, text: &str) -> ApplyOutcome {
        let nfc: String = text.trim().nfc().collect();
        let without_fillers = remove_fillers(&nfc);

        let Some(matcher) = &self.matcher else {
            return ApplyOutcome { text: without_fillers, applied: Vec::new() };
        };

        // Eigener Scan statt replace_all: Wortgrenzen werden manuell über
        // \p{L} geprüft (regex-lookarounds kennt die Crate nicht), und bei
        // einem Kompositum-Treffer (Grenze schlägt fehl) wird innerhalb des
        // Treffers weitersucht, damit kürzere Phrasen dort noch passen.
        let src = without_fillers.as_str();
        let mut out = String::with_capacity(src.len() + 16);
        let mut counts: HashMap<usize, (String, usize)> = HashMap::new();
        let mut pos = 0;
        while pos < src.len() {
            let Some(m) = matcher.find_at(src, pos) else { break };
            let (s, e) = (m.start(), m.end());
            let left_ok = src[..s].chars().next_back().is_none_or(|c| !c.is_alphabetic());
            let right_ok = src[e..].chars().next().is_none_or(|c| !c.is_alphabetic());
            if left_ok && right_ok {
                let key = m.as_str().to_lowercase();
                if let Some(&idx) = self.phrase_map.get(&key) {
                    out.push_str(&src[pos..s]);
                    out.push_str(&self.rules[idx].canonical);
                    let entry = counts.entry(idx).or_insert((m.as_str().to_string(), 0));
                    entry.1 += 1;
                    pos = e;
                    continue;
                }
            }
            // Kein Treffer mitten im Wort (Komposita-Toleranz): erstes Zeichen
            // des Treffers übernehmen und dahinter weitersuchen.
            let ch = src[s..].chars().next().unwrap();
            out.push_str(&src[pos..s + ch.len_utf8()]);
            pos = s + ch.len_utf8();
        }
        out.push_str(&src[pos..]);

        let mut applied: Vec<AppliedRule> = counts
            .into_iter()
            .map(|(idx, (matched, count))| AppliedRule {
                canonical: self.rules[idx].canonical.clone(),
                matched,
                count,
            })
            .collect();
        applied.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.canonical.cmp(&b.canonical)));
        ApplyOutcome { text: out, applied }
    }
}

impl Default for Dictionary {
    fn default() -> Self {
        Self::compile(DictionaryFile::default())
    }
}

impl Default for DictionaryFile {
    fn default() -> Self {
        DictionaryFile { locale: default_locale(), profiles: Vec::new(), terms: Vec::new() }
    }
}

/// Ablageort laut specs 4.5: `%APPDATA%\PrivateWispr\dictionary\`. (Modelle
/// und settings.json liegen bewusst in `%LOCALAPPDATA%` — historisch aus M0;
/// ein Umzug der Settings würde Nutzereinstellungen verlieren.)
pub fn dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PrivateWispr")
        .join("dictionary")
}

fn file_path() -> PathBuf {
    dir().join("dictionary.yaml")
}

/// Kurze Versionskennung (SHA256-Präfix) des Dateiinhalts fürs Protokoll.
fn content_version(raw: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(raw.as_bytes());
    digest.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// Profile in der UI umschalten: einzige Änderung ist die `profiles`-Zeile,
/// Terme bleiben erhalten. (Round-trip über serde — handgeschriebene
/// Kommentare in der Datei gehen dabei verloren, Terme nicht.)
pub fn set_active_profiles(active: &[String]) -> Result<Dictionary> {
    let path = file_path();
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("Wörterbuch nicht lesbar: {}", path.display()))?;
    let mut file: DictionaryFile = serde_yaml::from_str(&raw)
        .with_context(|| format!("Wörterbuch-YAML ungültig: {}", path.display()))?;
    let mut profiles = active.to_vec();
    profiles.sort();
    profiles.dedup();
    file.profiles = profiles;
    let yaml = serde_yaml::to_string(&file)?;
    std::fs::write(&path, yaml)
        .with_context(|| format!("Wörterbuch nicht schreibbar: {}", path.display()))?;
    Ok(Dictionary::load())
}

/// Ordner im Explorer öffnen (pfad<Tab>-Ersatz für die Wörterbuch-Karte).
pub fn open_dir() -> Result<()> {
    std::fs::create_dir_all(dir())?;
    std::process::Command::new("explorer").arg(dir()).spawn()?;
    Ok(())
}

fn write_default_file(path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, DEFAULT_FILE)?;
    Ok(())
}

/// Exemplarische Startregeln (specs-Beispiele + verbreitete ASR-Hörfehler).
/// Ersetzt/erweitert sie durch echte Fälle aus den Eval-Clips (specs 6).
const DEFAULT_FILE: &str = r#"# Private Wispr — Wörterbuch (specs 4.5)
# Erweitert aus den BT700-Eval-Clips vom 22.11.2026 (11 Clips, Baseline-WER
# 4,8%): jeder neue Eintrag entspricht einem echten Erkennungsfehler.
#   canonical: kanonische Schreibweise (ersetzter Text)
#   aliases:   erkannte Schreibweisen/Sprachfehler des Modells
# Datei jederzeit editierbar — wirkt ab dem nächsten Diktat.
locale: de-DE
terms:
  # --- KI-Werkzeuge (Eval bericht/mix/braindump) ---
  - canonical: Copilot
    aliases: [Cobalt, Cobilet]
  - canonical: Copilot Studio
    aliases: [Cobalt Studio, Cobilet Studio]
  - canonical: GitHub
    aliases: [GipTab, Giptab]
  - canonical: Coding Agent
    aliases: [Recording Agent]
  - canonical: Human in the Loop
    aliases: [Jungen in the Loop]
  # --- Projekt-/IT-Begriffe ---
  - canonical: Cutover
    aliases: [Cut Over, Cut-Over]
  - canonical: UAT
    aliases: [UET, UIT]
  - canonical: RACI-Matrix
    aliases: [RAIC-Matrix]
  - canonical: RICE-Scoring
    aliases: [Rise Scoring, Rise-Scoring]
  - canonical: PostgreSQL
    aliases: [SquadsSQL, Brust-SquadsSQL]
  - canonical: VPN
    aliases: [VBN]
  - canonical: VPN-Gateway
    aliases: [VBN-Gateway]
  - canonical: BIS Hub
    aliases: [Bisab, Bisaab]
  - canonical: IDocs
    aliases: [Idocks]
  - canonical: IDoc
    aliases: [Idock]
  - canonical: Serverauslastung
    aliases: [Serva-Auslastung, Servaauslastung]
  - canonical: Seeburger
    aliases: [Seeborger]
  - canonical: Tagging
    aliases: [Taging]
  - canonical: Workaround
    aliases: [Workround]
  - canonical: "Weber & Partner"
    aliases: [Web und Butler, Weber und Partner]
  - canonical: E-Mail
    aliases: [E-Meil]
  - canonical: Toggle
    aliases: [Tocl, TOCL]
  - canonical: Anbei
    aliases: [Anleih]
  - canonical: PBC
    aliases: [PPC]
  # --- Personen (Eval namen/notiz/braindump) ---
  - canonical: Reimar Hausenblas
    aliases: [Reimer Hausenblas, Reimerhausenblas, Reimer-Hausenblas]
  - canonical: Klimmek
    aliases: [Klimmeck, Klimmet]
  - canonical: Philipp Buyny
    aliases: [Philipp Beiney, Philipp Beine]
  - canonical: Buyny
    aliases: [Beiney, Beini]
  - canonical: Marko Einecke
    aliases: [Marco Einecke, Marco Eineke, Marko Eineke]
  - canonical: Einecke
    aliases: [Eineke]
  - canonical: Beate
    aliases: [Arte, Beati]
  - canonical: Seuling
    aliases: [Süling]
  - canonical: Beate Seuling
    aliases: [Beate Süling, Arte Seuling, Arte Süling, Beati Seuling, Beate Suling]
  - canonical: Romich
    aliases: [Rohmig, Romig]
  - canonical: Rozewicki
    aliases: [Rosovicki]
  - canonical: Wieprecht
    aliases: [Wiebrecht]
  - canonical: Jaroslaw Depta
    aliases: [Joslav Dapta, Jaroslaw Dapta, Joslav Depta]
  - canonical: Depta
    aliases: [Dapta]
  - canonical: Fisnik
    aliases: [Fisnick]
  - canonical: Bauer
    aliases: [Verbau, Verbauer]
  # --- Wortformen-Fehler des Modells ---
  - canonical: Abnahme
    aliases: [Abnahmung]
  - canonical: Freieinweisung
    aliases: [Freienweisung]
  - canonical: Datenbank
    aliases: [Datenanbanken]
  - canonical: Eingabetätigkeiten
    aliases: [Eingabentätigkeiten]
  - canonical: Generierungsanweisungen
    aliases: [Generierungsanweisen]
  - canonical: Heft
    aliases: [Teft]
  - canonical: im Hintergrund
    aliases: [im Grund, in der Grund]
  # --- Ursprüngliche Beispiel-Regeln ---
  - canonical: SharePoint
    aliases: [Share Point, Scherpoint]
  - canonical: OCuLink
    aliases: [Ocu Link, Okkulink]
  - canonical: GitLab
    aliases: [Git Lab]
  - canonical: TypeScript
    aliases: [Type Script]
  - canonical: Kubernetes
    aliases: [Kubernetis, Kuber Netes]
  - canonical: Markdown
    aliases: [Mark down]
"#;

/// Filler-Ganzwörter entfernen (de-aware): „ähm", „ähm," fallen inklusive
/// anhängendem Komma weg, umgebende Leerzeichen fallen nicht doppelt an.
/// Kein Treffer mitten im Wort („Hmmm" bleibt), echte Wörter bleiben.
fn remove_fillers(nfc: &str) -> String {
    let mut phrases: Vec<&str> = FILLERS.to_vec();
    phrases.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()));
    let pattern = phrases.iter().map(|p| regex::escape(p)).collect::<Vec<_>>().join("|");
    let Ok(re) = regex::RegexBuilder::new(&format!("(?i)(?:{pattern})")).build() else {
        return nfc.to_string();
    };

    let src = nfc;
    let mut out = String::with_capacity(src.len());
    let mut pos = 0;
    while pos < src.len() {
        let Some(m) = re.find_at(src, pos) else { break };
        let (s, e) = (m.start(), m.end());
        let left_ok = src[..s].chars().next_back().is_none_or(|c| !c.is_alphabetic());
        let right_ok = src[e..].chars().next().is_none_or(|c| !c.is_alphabetic());
        if left_ok && right_ok {
            out.push_str(&src[pos..s]);
            pos = e;
            // Ein Komma direkt am Filler gehört zum Filler („ich ähm, glaube").
            let rest = &src[pos..];
            let comma = rest.trim_start().starts_with(',');
            if comma {
                pos = src[pos..]
                    .find(',')
                    .map(|off| pos + off + 1)
                    .unwrap_or(pos);
            }
            // Kein doppeltes Leerzeichen an der Nahtstelle (auch am Textanfang).
            let out_ends_space = out.chars().next_back() == Some(' ');
            let rest_starts_space = src[pos..].starts_with(' ');
            if (out_ends_space || out.is_empty()) && rest_starts_space {
                pos += 1;
            }
            continue;
        }
        let ch = src[s..].chars().next().unwrap();
        out.push_str(&src[pos..s + ch.len_utf8()]);
        pos = s + ch.len_utf8();
    }
    out.push_str(&src[pos..]);
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(profiles: &[&str], terms: &[(&str, &[&str], Option<&str>)]) -> Dictionary {
        Dictionary::compile(DictionaryFile {
            locale: "de-DE".into(),
            profiles: profiles.iter().map(|s| s.to_string()).collect(),
            terms: terms
                .into_iter()
                .map(|(canonical, aliases, profile)| Term {
                    canonical: canonical.to_string(),
                    aliases: aliases.iter().map(|s| s.to_string()).collect(),
                    profile: profile.map(|s| s.to_string()),
                })
                .collect(),
        })
    }

    #[test]
    fn yaml_format_der_spec_ist_parsebar() {
        let yaml = r#"
locale: de-DE
profiles: [it]
terms:
  - canonical: "SharePoint"
    aliases: ["Share Point", "Scherpoint"]
  - canonical: "OCuLink"
    aliases: ["Ocu Link", "Okkulink"]
"#;
        let file: DictionaryFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(file.locale, "de-DE");
        assert_eq!(file.profiles, vec!["it".to_string()]);
        assert_eq!(file.terms.len(), 2);
        assert_eq!(file.terms[0].aliases, vec!["Share Point", "Scherpoint"]);
        assert!(file.terms[0].profile.is_none());
    }

    #[test]
    fn fehlende_felder_fallen_auf_defaults() {
        let file: DictionaryFile = serde_yaml::from_str("terms: []").unwrap();
        assert_eq!(file.locale, "de-DE");
        assert!(file.profiles.is_empty());
    }

    #[test]
    fn nfc_normalisiert_vor_dem_match() {
        // NFD (A + kombinierender Umlaut) muss die NFC-Alias treffen.
        let d = dict(&[], &[("Änderungsantrag", &["\u{c4}nderungsantrag"], None)]);
        let nfd = "Ich stelle einen A\u{308}nderungsantrag";
        let out = d.apply(nfd);
        assert!(out.text.contains("\u{c4}nderungsantrag"));
        assert_eq!(out.applied.len(), 1);
        assert_eq!(out.applied[0].count, 1);
    }

    #[test]
    fn case_insensitiv_inklusive_umlauten() {
        let d = dict(&["it"], &[("SharePoint", &["Scherpoint"], Some("it"))]);
        for input in ["Scherpoint", "scherpoint", "SCHERPOINT", "sChErPoInT"] {
            let out = d.apply(&format!("Wir nutzen {input}"));
            assert_eq!(out.text, "Wir nutzen SharePoint", "Input: {input}");
        }
    }

    #[test]
    fn laengste_phrase_zuerst() {
        // Zwei Regeln, deren Phrasen am gleichen Wort beginnen: Die längere
        // muss gewinnen (regex-Alternation ist leftmost-first, also muss die
        // Kompilierung längere Phrasen voranstellen).
        let d = dict(
            &[],
            &[("GitLab", &["Git Lab"], None), ("Git", &["Git"], None)],
        );
        let out = d.apply("Der Code liegt im Git Lab");
        assert_eq!(out.text, "Der Code liegt im GitLab");
        assert_eq!(out.applied.len(), 1);
        assert_eq!(out.applied[0].canonical, "GitLab");
    }

    #[test]
    fn kein_treffer_mitten_im_wort() {
        let d = dict(&["it"], &[("OCuLink", &["Okkulink"], Some("it"))]);
        // Kompositum: Okkulink endet mitten in Okkulinkliste -> unangetastet.
        let out = d.apply("Die Okkulinkliste ist defekt");
        assert_eq!(out.text, "Die Okkulinkliste ist defekt");
        assert!(out.applied.is_empty());
        // Bindestrich ist eine echte Wortgrenze -> Ersetzung erlaubt.
        let out = d.apply("Die Okkulink-Liste ist defekt");
        assert_eq!(out.text, "Die OCuLink-Liste ist defekt");
        assert_eq!(out.applied[0].count, 1);
    }

    #[test]
    fn kompositum_mit_korrekter_kanone_bleibt_unangetastet() {
        // specs-Beispiel: „SharePoint-Liste" bleibt unverändert. Die Kanone
        // matcht auch sich selbst (case-insensitiv) und wird protokolliert —
        // Trefferzähler für die Fachbegriff-Quote (specs 6), Text ändert sich
        // aber nicht.
        let d = dict(&["it"], &[("SharePoint", &["Share Point"], Some("it"))]);
        let out = d.apply("in der SharePoint-Liste");
        assert_eq!(out.text, "in der SharePoint-Liste");
        assert_eq!(out.applied.len(), 1);
        assert_eq!(out.applied[0].canonical, "SharePoint");
        assert_eq!(out.applied[0].matched, "SharePoint");
    }

    #[test]
    fn kanone_erzwingt_ihre_grossschreibung() {
        let d = dict(&["it"], &[("SharePoint", &["Share Point"], Some("it"))]);
        assert_eq!(d.apply("wir nutzen sharepoint").text, "wir nutzen SharePoint");
    }

    #[test]
    fn profile_filtern_regeln() {
        let d_it = dict(&["it"], &[("SharePoint", &["Scherpoint"], Some("it"))]);
        assert_eq!(d_it.apply("Wir nutzen Scherpoint").text, "Wir nutzen SharePoint");

        let d_med = dict(&["medizin"], &[("SharePoint", &["Scherpoint"], Some("it"))]);
        let out = d_med.apply("Wir nutzen Scherpoint");
        assert_eq!(out.text, "Wir nutzen Scherpoint");
        assert!(out.applied.is_empty());
        assert!(d_med.vocabulary().is_empty());
    }

    #[test]
    fn terme_ohne_profil_sind_immer_aktiv() {
        let d = dict(&["medizin"], &[("Hausarzt", &["Haus Arzt"], None)]);
        assert_eq!(d.apply("Ich gehe zum Haus Arzt").text, "Ich gehe zum Hausarzt");
    }

    #[test]
    fn vocabulary_enthält_kanones_aktiver_regeln_dedupliziert() {
        let d = dict(
            &["it"],
            &[
                ("SharePoint", &["Share Point"], Some("it")),
                ("GitLab", &["Git Lab", "Gitlab"], Some("it")), // Gitlab alias ≠ kanone
                ("Stethoskop", &["Stetho Skop"], Some("medizin")),
            ],
        );
        assert_eq!(d.vocabulary(), &["GitLab".to_string(), "SharePoint".to_string()]);
        // Kanone kann selbst auch als Phrase matchen:
        assert_eq!(d.apply("im Git Lab").text, "im GitLab");
    }

    #[test]
    fn filler_werden_als_ganzwort_entfernt() {
        let d = Dictionary::default();
        let cases = [
            ("Hallo äh Welt", "Hallo Welt"),
            ("Ich ähm, glaube schon", "Ich glaube schon"),
            ("Er, ähm, sagt nichts", "Er, sagt nichts"),
            ("Ähm hallo", "hallo"),
            ("hm das passt", "das passt"),
        ];
        for (input, want) in cases {
            assert_eq!(d.apply(input).text, want, "Input: {input}");
        }
    }

    #[test]
    fn filler_nicht_mitten_im_wort_und_keine_echten_woerter() {
        let d = Dictionary::default();
        // „Hmmm" ist länger als „hmm" (kein Ganzwort), „Halt" steht nicht
        // auf der Liste, „Hardware" enthält kein Grenzwort-hm.
        for input in ["Hmmm das", "Halt stop", "Hardware bleibt", "Ohm ist eine Einheit"] {
            assert_eq!(d.apply(input).text, input, "Input: {input}");
        }
    }

    #[test]
    fn applied_regeln_werden_zaehlend_protokolliert() {
        let d = dict(&["it"], &[("SharePoint", &["Scherpoint", "Share Point"], Some("it"))]);
        let out = d.apply("Scherpoint und nochmal scherpoint im Share Point Projekt");
        assert_eq!(
            out.text,
            "SharePoint und nochmal SharePoint im SharePoint Projekt"
        );
        assert_eq!(out.applied.len(), 1);
        assert_eq!(out.applied[0].canonical, "SharePoint");
        assert_eq!(out.applied[0].count, 3);
        assert_eq!(out.applied[0].matched, "Scherpoint", "erste erkannte Phrase");
    }

    #[test]
    fn leeres_woerterbuch_laesst_text_unveraendert() {
        // Interne Leerzeichen normalisiert erst das Text-Fitting, nicht die
        // Regel-Pipeline — hier zählt nur trim + NFC + keine Regeln.
        let d = Dictionary::default();
        let out = d.apply("  Völlig  unverändert  ");
        assert_eq!(out.text, "Völlig  unverändert");
        assert!(out.applied.is_empty());
    }
}
