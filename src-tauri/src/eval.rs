//! Eval-Metriken (specs Abschnitt 6): normalisierter WER gegen Ground-Truth
//! und Wort-Alignment für die Fehleranalyse — die Grundlage, um aus echten
//! Diktaten Wörterbuch-Regeln abzuleiten („ohne echte Fehler keine guten
//! Regeln").
//!
//! Normalisierung (auf BOTH Seiten symmetrisch angewandt, damit Ziffern- und
//! Wort-Schreibweisen vergleichbar werden):
//! NFC → Kleinbuchstaben → Satz-/Währungszeichen zu Wörtern (% → „prozent",
//! € → „euro") → Uhrzeiten hh:mm → „hh uhr mm" → Zahlen (auch Komma-Dezimal,
//! Tausenderpunkt) in deutsche Zahlwörter → Tokens an Nicht-Buchstaben/-Ziffern.

use std::collections::BTreeMap;

use unicode_normalization::UnicodeNormalization;

/// Ein Wort-Vergleich aus dem Levenshtein-Alignment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlignOp {
    /// Gleiche Wörter (nach Normalisierung).
    Match,
    /// Ersetzung: (Referenz, Hypothese) — der wichtigste Fall für Regeln.
    Sub(String, String),
    /// Wort der Referenz fehlt in der Hypothese.
    Del(String),
    /// Wort der Hypothese steht nicht in der Referenz.
    Ins(String),
}

#[derive(Clone, Debug, Default)]
pub struct WerResult {
    pub matches: usize,
    pub substitutions: usize,
    pub deletions: usize,
    pub insertions: usize,
    /// Referenz-Wortzahl (Nenner).
    pub ref_len: usize,
}

impl WerResult {
    /// WER als Anteil 0..1 (Sub+Del+Ins / Referenzlänge).
    pub fn wer(&self) -> f64 {
        if self.ref_len == 0 {
            return 0.0;
        }
        (self.substitutions + self.deletions + self.insertions) as f64 / self.ref_len as f64
    }
}

/// Diktat-Strukturwörter, die beim Sprechen von „/" bzw. „—" entstehen und
/// auf beiden Seiten rausfallen (das GT „/" normalisiert zu nichts — das
/// gesprochene „Slash" darf dann kein Einfügungs-Fehler sein).
const DROPPED_WORDS: &[&str] = &["slash", "schrägstrich", "bindestrich", "gedankenstrich", "doppelpunkt"];

/// Aufzählungs-Ordinalia („drittens") auf Kardinalformen, damit GT „3."
/// (→ „drei") auf das gesprochene „Drittens" trifft.
const ORDINALS: &[(&str, &str)] = &[
    ("erstens", "eins"),
    ("zweitens", "zwei"),
    ("drittens", "drei"),
    ("viertens", "vier"),
    ("fünftens", "fünf"),
    ("sechstens", "sechs"),
    ("siebtens", "sieben"),
    ("achtens", "acht"),
    ("neuntens", "neun"),
    ("zehntens", "zehn"),
    ("elftens", "elf"),
    ("zwölftens", "zwölf"),
    ("dreizehntens", "dreizehn"),
    ("vierzehntens", "vierzehn"),
    ("fünfzehntens", "fünfzehn"),
    ("sechzehntens", "sechzehn"),
    ("siebzehntens", "siebzehn"),
    ("achtzehntens", "achtzehn"),
    ("neunzehntens", "neunzehn"),
    ("zwanzigstens", "zwanzig"),
];

/// Text → normalisierte Wortliste (siehe Modul-Doku für die Regeln).
pub fn normalize(text: &str) -> Vec<String> {
    let nfc: String = text.nfc().collect();
    // Satz-/Währungszeichen von Ziffern trennen, damit „78%" zwei Wörter werden.
    let spaced = nfc
        .replace(['%', '‰'], " prozent ")
        .replace('€', " euro ")
        .replace(['§'], " paragraph ");
    let mut words = Vec::new();
    for token in spaced.split_whitespace() {
        let lower = token.to_lowercase();
        for part in token_words(&lower) {
            if part.is_empty() || DROPPED_WORDS.contains(&part.as_str()) {
                continue;
            }
            if let Some((_, cardinal)) = ORDINALS.iter().find(|(o, _)| *o == part) {
                words.push(cardinal.to_string());
            } else {
                words.push(part);
            }
        }
    }
    words
}

/// Ein Token → Wörter. Zahlen-Formen mit Komma/Punkt/Doppelpunkt werden als
/// GANZES interpretiert (vor der Zerlegung an Sonderzeichen — sonst wäre
/// „3,5" fälschlich zwei Zahlen), alles Übrige zerfällt an Nicht-Alphanu-
/// merischem (auch „/" und „-": CI/CD → ci, cd).
fn token_words(token: &str) -> Vec<String> {
    // Uhrzeit hh:mm — aber nur mit gültigen Werten (keine Versionsnummern).
    if let Some((h, m)) = token.split_once(':') {
        if let (Ok(hh), Ok(mm)) = (h.parse::<u32>(), m.parse::<u32>()) {
            if hh < 24 && mm < 60 {
                let mut words = vec![number_word(hh as u64), "uhr".to_string()];
                if mm > 0 {
                    words.push(number_word(mm as u64));
                }
                return words;
            }
        }
    }
    // Dezimalzahl mit Komma: "3,5" → „drei komma fünf".
    if let Some((int, frac)) = token.split_once(',') {
        if !int.is_empty()
            && int.chars().all(|c| c.is_ascii_digit())
            && !frac.is_empty()
            && frac.chars().all(|c| c.is_ascii_digit())
        {
            let mut words = vec![number_word(int.parse::<u64>().unwrap_or(0)), "komma".into()];
            if frac.len() <= 2 && !frac.starts_with('0') {
                words.push(number_word(frac.parse::<u64>().unwrap_or(0)));
            } else {
                words.extend(frac.chars().map(digit_word));
            }
            return words;
        }
    }
    // Tausenderpunkt: "40.000" → 40000 (nur 3-stellige Nachkommegruppe).
    if let Some((int, grp)) = token.split_once('.') {
        if !int.is_empty()
            && int.chars().all(|c| c.is_ascii_digit())
            && grp.len() == 3
            && grp.chars().all(|c| c.is_ascii_digit())
        {
            let joined: String = format!("{int}{grp}");
            return vec![number_word(joined.parse::<u64>().unwrap_or(0))];
        }
    }
    // Zerfall an Sonderzeichen; reine Ziffern-Segmente → Zahlwort (auch
    // Ordinal-Punkt „3."). „dritte" bleibt ein eigener Fehler — ehrlich
    // messen, nicht wegnormalisieren.
    token
        .split(|c: char| !c.is_alphanumeric())
        .filter(|p| !p.is_empty())
        .flat_map(|p| {
            let digits = p.trim_end_matches('.');
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                vec![number_word(digits.parse::<u64>().unwrap_or(0))]
            } else {
                vec![p.to_string()]
            }
        })
        .collect()
}

fn digit_word(c: char) -> String {
    match c {
        '0' => "null",
        '1' => "eins",
        '2' => "zwei",
        '3' => "drei",
        '4' => "vier",
        '5' => "fünf",
        '6' => "sechs",
        '7' => "sieben",
        '8' => "acht",
        '9' => "neun",
        _ => "",
    }
    .to_string()
}

const ONES: [&str; 10] = [
    "", "eins", "zwei", "drei", "vier", "fünf", "sechs", "sieben", "acht", "neun",
];
const TEENS: [&str; 10] = [
    "", "elf", "zwölf", "dreizehn", "vierzehn", "fünfzehn", "sechzehn", "siebzehn",
    "achtzehn", "neunzehn",
];
const TENS: [&str; 10] = [
    "", "zehn", "zwanzig", "dreißig", "vierzig", "fünfzig", "sechzig", "siebzig",
    "achtzig", "neunzig",
];

/// Deutsche Zahlwörter 0..=999.999 (darüber ziffernweise). 1 = „eins" —
/// Flexionsformen („eine", „einen") zählen als eigene Wörter; das ist
/// bewusst ehrlich, weil die Normalisierung beide Seiten gleich behandelt.
pub fn number_word(n: u64) -> String {
    if n > 999_999 {
        // Lange Nummern (Telefon) werden ziffernweise gesprochen — das tut die
        // Normalisierung ebenfalls, dadurch treffen Ziffern auf Ziffern.
        let words: Vec<String> = n.to_string().chars().map(digit_word).collect();
        return words.join("");
    }
    let thousand = n / 1000;
    let rest = n % 1000;
    let mut out = String::new();
    if thousand > 1 {
        out.push_str(&small_number(thousand));
        out.push_str("tausend");
    } else if thousand == 1 {
        // 1000 heißt „tausend" — das „eins" fällt wie bei „hundert" weg.
        out.push_str("tausend");
    }
    if rest > 0 || n == 0 {
        out.push_str(&small_number(rest));
    }
    if out.is_empty() {
        out.push_str("null");
    }
    out
}

/// 0..=999.
fn small_number(n: u64) -> String {
    if n == 0 {
        return String::new();
    }
    let hundreds = n / 100;
    let rest = n % 100;
    let mut out = String::new();
    if hundreds > 0 {
        if hundreds > 1 {
            out.push_str(ONES[hundreds as usize]);
        }
        out.push_str("hundert");
    }
    match rest {
        0 => {}
        1..=9 => out.push_str(ONES[rest as usize]),
        10 => out.push_str("zehn"),
        11..=19 => out.push_str(TEENS[(rest - 10) as usize]),
        _ => {
            let ones = rest % 10;
            let tens = rest / 10;
            if ones > 0 {
                // „einundzwanzig", nicht „einsundzwanzig" — nur in der
                // Komposition verliert die Eins ihr s.
                out.push_str(if ones == 1 { "ein" } else { ONES[ones as usize] });
                out.push_str("und");
            }
            out.push_str(TENS[tens as usize]);
        }
    }
    out
}

/// Wort-Level-Levenshtein mit Backtrace: WER-Zähler + Operationen.
pub fn align(reference: &[String], hypothesis: &[String]) -> (WerResult, Vec<AlignOp>) {
    let n = reference.len();
    let m = hypothesis.len();
    // DP-Matrix (n+1)×(m+1) mit Kosten.
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in dp.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=m {
        dp[0][j] = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = if reference[i - 1] == hypothesis[j - 1] { 0 } else { 1 };
            dp[i][j] = (dp[i - 1][j] + 1) // Streichung
                .min(dp[i][j - 1] + 1) // Einfügung
                .min(dp[i - 1][j - 1] + cost); // Ersetzung/Treffer
        }
    }
    // Backtrace.
    let mut ops = Vec::new();
    let mut i = n;
    let mut j = m;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 {
            let cost = if reference[i - 1] == hypothesis[j - 1] { 0 } else { 1 };
            if dp[i][j] == dp[i - 1][j - 1] + cost {
                if cost == 0 {
                    ops.push(AlignOp::Match);
                } else {
                    ops.push(AlignOp::Sub(
                        reference[i - 1].clone(),
                        hypothesis[j - 1].clone(),
                    ));
                }
                i -= 1;
                j -= 1;
                continue;
            }
        }
        if i > 0 && dp[i][j] == dp[i - 1][j] + 1 {
            ops.push(AlignOp::Del(reference[i - 1].clone()));
            i -= 1;
        } else {
            ops.push(AlignOp::Ins(hypothesis[j - 1].clone()));
            j -= 1;
        }
    }
    ops.reverse();
    let mut result = WerResult { ref_len: n, ..Default::default() };
    for op in &ops {
        match op {
            AlignOp::Match => result.matches += 1,
            AlignOp::Sub(..) => result.substitutions += 1,
            AlignOp::Del(..) => result.deletions += 1,
            AlignOp::Ins(..) => result.insertions += 1,
        }
    }
    (result, ops)
}

/// Nur den WER-Wert (ohne Alignment-Speicher).
pub fn wer(reference: &[String], hypothesis: &[String]) -> WerResult {
    align(reference, hypothesis).0
}

/// Ersetzungs-Paare (Referenz → Hypothese) mit Häufigkeit — die Rohdaten für
/// Wörterbuch-Regeln. Deletionen der Referenz separat (Wort → Anzahl).
pub fn error_pairs(ops: &[AlignOp]) -> (BTreeMap<(String, String), usize>, BTreeMap<String, usize>) {
    let mut subs: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut dels: BTreeMap<String, usize> = BTreeMap::new();
    for op in ops {
        match op {
            AlignOp::Sub(r, h) => *subs.entry((r.clone(), h.clone())).or_insert(0) += 1,
            AlignOp::Del(r) => *dels.entry(r.clone()).or_insert(0) += 1,
            _ => {}
        }
    }
    (subs, dels)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(text: &str) -> Vec<String> {
        normalize(text)
    }

    #[test]
    fn normalisierung_grundfaelle() {
        // Satzzeichen raus, Kleinbuchstaben, NFC.
        assert_eq!(n("Hallo, Welt!"), vec!["hallo", "welt"]);
        assert_eq!(n("große Änderung"), vec!["große", "änderung"]);
        // Schrägstrich/Bindestrich trennen.
        assert_eq!(n("CI/CD-Pipeline"), vec!["ci", "cd", "pipeline"]);
    }

    #[test]
    fn ziffern_werden_zu_zahlwoertern() {
        assert_eq!(n("3,5 Millionen"), vec!["drei", "komma", "fünf", "millionen"]);
        assert_eq!(n("40.000 Euro"), vec!["vierzigtausend", "euro"]);
        assert_eq!(n("78 Prozent"), n("78%"));
        assert_eq!(n("149,90 €"), vec!["hundertneunundvierzig", "komma", "neunzig", "euro"]);
        // Uhrzeit: GT-Form „14:30 Uhr" == gesprochene Form.
        assert_eq!(n("14:30 Uhr"), vec!["vierzehn", "uhr", "dreißig", "uhr"]);
        assert_eq!(n("9:15"), vec!["neun", "uhr", "fünfzehn"]);
        // Ordinal-Punkt wird ignoriert, „dritte" bleibt eigener Fehler.
        assert_eq!(n("am 3. März"), vec!["am", "drei", "märz"]);
    }

    #[test]
    fn zahlwoerter_bis_millionen() {
        assert_eq!(number_word(0), "null");
        assert_eq!(number_word(7), "sieben");
        assert_eq!(number_word(16), "sechzehn");
        assert_eq!(number_word(21), "einundzwanzig");
        assert_eq!(number_word(100), "hundert");
        assert_eq!(number_word(101), "hunderteins");
        assert_eq!(number_word(118), "hundertachtzehn");
        assert_eq!(number_word(347), "dreihundertsiebenundvierzig");
        assert_eq!(number_word(1000), "tausend");
        assert_eq!(number_word(2026), "zweitausendsechsundzwanzig");
        assert_eq!(number_word(46_500), "sechsundvierzigtausendfünfhundert");
    }

    #[test]
    fn wer_einfache_faelle() {
        let r = n("hallo welt das ist ein test");
        let h = n("hallo weld das ein test");
        let (res, ops) = align(&r, &h);
        assert_eq!(res.matches, 4);
        assert_eq!(res.substitutions, 1); // weld/welt
        assert_eq!(res.deletions, 1); // ist fehlt
        assert_eq!(res.insertions, 0);
        let (subs, dels) = error_pairs(&ops);
        assert_eq!(subs.get(&("welt".to_string(), "weld".to_string())), Some(&1));
        assert_eq!(dels.get("ist"), Some(&1));
    }

    #[test]
    fn wer_null_und_leer() {
        let r: Vec<String> = vec![];
        assert_eq!(wer(&r, &n("etwas")).insertions, 1);
        let empty = wer(&n("etwas"), &r);
        assert_eq!(empty.deletions, 1);
        assert_eq!(WerResult::default().wer(), 0.0);
    }

    #[test]
    fn ziffern_gegen_ausgeschrieben_symmetrisch() {
        // Genau der Zweck: 3,5 (GT) soll „drei Komma fünf" (ASR) entsprechen.
        assert_eq!(n("3,5"), n("drei Komma fünf"));
        assert_eq!(n("2026"), n("zweitausendsechsundzwanzig"));
        assert_eq!(n("40.000"), n("vierzigtausend"));
    }

    #[test]
    fn diktat_strukturwoerter_fallen_weg() {
        // GT „A / B" (normalisiert ohne /) gegen gesprochenes „A Slash B".
        assert_eq!(n("Hausenblas / Buyny"), n("Hausenblas Slash Buyny"));
        assert_eq!(n("Repro-Ziel"), n("Repro Bindestrich Ziel"));
    }

    #[test]
    fn ordinal_zaehler_werden_kardinal() {
        // GT „3." vs. gesprochenes „Drittens" — kein Fehler, gleiche Form.
        assert_eq!(n("3. Punkt"), n("Drittens, Punkt"));
        assert_eq!(n("15."), n("fünfzehntens"));
        // „zwanzigstens" wie „20."
        assert_eq!(n("20. Stelle"), n("zwanzigstens Stelle"));
    }
}
