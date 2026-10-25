//! Text-Fitting (specs 4.6, Blitztext-Konzept, regelbasiert — kein LLM):
//! Leerzeichen an Nahtstellen, Groß-/Kleinschreibung am Satzanfang,
//! Funktionswörter klein mitten im Satz, Schluss-Punkt entfernen, wenn
//! danach weitergeschrieben wird.
//!
//! Nahtstellen: `SeamContext` beschreibt die Zeichen links und rechts des
//! Cursors in der Ziel-App. Das Lesen des Zieltexts (UIA o. ä.) folgt mit
//! der Paste-Transaktion (M1 Sprint 3) — bis dahin läuft die App mit
//! `SeamContext::none()`: dann wirken die satzinternen Regeln (Abstände,
//! Kommata, Funktionswörter, Erstbuchstabe), und der Schluss-Punkt bleibt
//! sicherheitshalber stehen.

/// Zeichen am Cursor der Ziel-App (None = unbekannt, keine Naht-Regeln).
#[derive(Clone, Copy, Debug, Default)]
pub struct SeamContext {
    pub char_before: Option<char>,
    pub char_after: Option<char>,
}

impl SeamContext {
    pub fn none() -> Self {
        SeamContext { char_before: None, char_after: None }
    }
}

const SENTENCE_END: [char; 4] = ['.', '!', '?', '…'];
const CLOSERS: [char; 7] = ['"', '»', '«', '\'', ')', ']', '}'];
const OPENERS: [char; 5] = ['(', '[', '{', '„', '»'];

/// Deutsche Funktionswörter, die mitten im Satz kleingeschrieben werden
/// (ASR schreibt sie gern groß). Bewusst ohne Anredepronomen: „Sie/Ihr/
/// Ihnen" bleiben immer groß (Höflichkeitsform), „du" ist informell klein.
const FUNCTION_WORDS: &[&str] = &[
    "und", "oder", "aber", "doch", "denn", "sondern", "weil", "wenn", "ob", "obwohl",
    "während", "damit", "dass", "als", "wie", "was", "wer", "wann", "wo", "warum",
    "wieso", "weshalb", "der", "die", "das", "den", "dem", "des", "ein", "eine",
    "einen", "einem", "einer", "eines", "ist", "sind", "war", "waren", "wird",
    "werden", "wurde", "wurden", "hat", "habe", "haben", "hatte", "hatten", "nicht",
    "kein", "keine", "keinen", "noch", "nur", "auch", "sehr", "so", "dann", "schon",
    "wieder", "immer", "nie", "oft", "vielleicht", "eigentlich", "einfach", "wirklich",
    "bitte", "mal", "ja", "nein", "hier", "da", "dort", "jetzt", "gleich", "ich",
    "du", "wir", "ihr", "er", "es", "man", "mich", "dir", "dich", "mir", "uns",
    "euch", "ihm", "ihn", "für", "mit", "von", "zu", "zum", "zur", "in", "im", "auf",
    "aus", "bei", "nach", "vor", "über", "unter", "um", "durch", "gegen", "ohne",
    "bis", "seit", "an", "am", "hinter", "neben", "trotz",
];

pub fn fit(text: &str, ctx: &SeamContext) -> String {
    let spaced = normalize_spacing(text.trim());
    let cased = fit_casing(&spaced, ctx);
    fit_seams(&cased, ctx)
}

/// Satzinterne Abstände: Leerzeichen-Läufe auf eins, kein Leerzeichen vor
/// Satz-/Lesezeichen, Leerzeichen nach Komma/Semikolon und nach Punkt, wenn
/// ein Großbuchstabe direkt anschließt (verpasstes Leerzeichen, z. B.
/// „Wort.Nächste"). Zahlen („1,5", „3.14159") und Abkürzungen ohne
/// Großbuchstaben („usw.") bleiben unberührt.
fn normalize_spacing(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == ' ' || c == '\t' {
            // Nur ein Leerzeichen, und keins vor ,.;:!?…
            let mut j = i;
            while j < chars.len() && (chars[j] == ' ' || chars[j] == '\t') {
                j += 1;
            }
            if j < chars.len() && ",.;:!?…)]".contains(chars[j]) {
                i = j; // Leerzeichen vor Satzzeichen fällt weg
            } else {
                out.push(' ');
                i = j;
            }
            continue;
        }
        out.push(c);
        // Leerzeichen nach Interpunktion ergänzen, wenn direkt ein Buchstabe
        // folgt (Komma/Semikolon immer; Punkt nur bei Großbuchstabe mit
        // vorangestelltem Kleinbuchstaben — Zahlen/Abkürzungen unberührt).
        let needs_space =
            matches!(c, ',' | ';') && i + 1 < chars.len() && chars[i + 1].is_alphabetic()
                || c == '.'
                    && i + 1 < chars.len()
                    && chars[i + 1].is_uppercase()
                    && i > 0
                    && chars[i - 1].is_lowercase();
        if needs_space {
            out.push(' ');
        }
        i += 1;
    }
    out
}

/// Erstbuchstabe + Funktionswörter: Den ersten Buchstaben groß schreiben,
/// wenn ein neuer Satz beginnt — am Textanfang ohne Naht-Kontext oder wenn
/// links vom Cursor ein Satzende steht. Funktionswörter mitten im Satz
/// klein; „Sie/Ihr/Ihnen" stehen nicht auf der Liste und bleiben groß.
/// Die Satzmitte vertraut der ASR-Großschreibung (Parakeet interpunktiert
/// und kasusiert selbst); hier wird nur aufgeräumt, nichts umgebaut.
fn fit_casing(text: &str, ctx: &SeamContext) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());

    // Großschreibung des ersten Worts nur bei bekanntem Satzende links vom
    // Cursor — ohne Kontext könnte das Diktat mitten im Satz weiterschreiben
    // („und dann …") oder mit Abkürpfung/Einheit beginnen („usw.", „1,5 kg");
    // dann gilt die ASR-Schreibweise (Parakeet kasusiert selbst).
    let starts_sentence = ctx.char_before.map_or(false, |c| SENTENCE_END.contains(&c));
    let first_at_start = next_word(&chars, 0).map_or(false, |(s, _)| {
        chars[..s].iter().all(|c| *c == ' ' || OPENERS.contains(c))
    });
    let capitalize_first = first_at_start && starts_sentence;

    let mut i = 0;
    let mut at_text_start = true;
    while i < chars.len() {
        if chars[i].is_alphabetic() {
            let (ws, we) = next_word(&chars, i).expect("Buchstabenfolge existiert ab hier");
            let word: Vec<char> = chars[ws..we].to_vec();
            if at_text_start {
                if capitalize_first && word[0].is_lowercase() {
                    out.extend(word[0].to_uppercase());
                    out.extend(word.iter().skip(1).copied());
                } else {
                    out.extend(word.iter().copied());
                }
                at_text_start = false;
            } else {
                let lowered: String = word.iter().collect::<String>().to_lowercase();
                let sentence_initial = is_sentence_initial(&chars, ws);
                if !sentence_initial
                    && word[0].is_uppercase()
                    && FUNCTION_WORDS.contains(&lowered.as_str())
                {
                    out.extend(lowered.chars());
                } else {
                    out.extend(word.iter().copied());
                }
            }
            i = we;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out.into_iter().collect()
}

/// Nächste zusammenhängende Buchstabenfolge ab `from` (Start, Ende).
fn next_word(chars: &[char], from: usize) -> Option<(usize, usize)> {
    let start = (from..chars.len()).find(|&i| chars[i].is_alphabetic())?;
    let end = (start..chars.len())
        .find(|&i| !chars[i].is_alphabetic())
        .unwrap_or(chars.len());
    Some((start, end))
}

/// Steht der Wortanfang an einem Satzanfang? Blick nach links: Leerzeichen
/// und schließende Anführungszeichen/Klammern überspringen, dann zählt ein
/// Satzende-Zeichen (. ! ? …) oder der Textanfang.
fn is_sentence_initial(chars: &[char], word_start: usize) -> bool {
    let mut i = word_start;
    loop {
        if i == 0 {
            return true;
        }
        i -= 1;
        let c = chars[i];
        if c == ' ' || c == '\t' || CLOSERS.contains(&c) {
            continue;
        }
        return SENTENCE_END.contains(&c);
    }
}

/// Nahtstellen-Leerzeichen + Schluss-Punkt: Abstand zum Text links/rechts
/// sicherstellen; den einzelnen Schluss-Punkt entfernen, wenn rechts vom
/// Cursor weitergeschrieben wird (Buchstabe/Ziffer folgt). „..." und „…"
/// bleiben ebenso wie ? und !.
fn fit_seams(text: &str, ctx: &SeamContext) -> String {
    let mut text = text.trim().to_string();

    // Leerzeichen am Anfang: nach Buchstabe/Ziffer/schließendem Zeichen
    // gehört eines hin, nach öffnenden Klammern/Anführungszeichen keins.
    if let Some(before) = ctx.char_before {
        let first = text.chars().next();
        let starts_with_punct =
            first.is_some_and(|c| !c.is_alphabetic() && !c.is_numeric());
        if !starts_with_punct
            && !before.is_whitespace()
            && (before.is_alphabetic() || before.is_numeric() || CLOSERS.contains(&before))
        {
            text.insert(0, ' ');
        }
    }

    // Schluss-Punkt entfernen, wenn danach weitergeschrieben wird.
    if let Some(after) = ctx.char_after {
        if (after.is_alphabetic() || after.is_numeric())
            && text.ends_with('.')
            && !text.ends_with("...")
        {
            text.pop();
        }
    }

    // Leerzeichen am Ende: Trennung, wenn rechts direkt Text weitergeht.
    if let Some(after) = ctx.char_after {
        let last = text.chars().next_back();
        let continues_text = after.is_alphabetic() || after.is_numeric();
        let last_opener = last.is_some_and(|c| OPENERS.contains(&c));
        if continues_text && !last_opener && last != Some(' ') {
            text.push(' ');
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fit_none(text: &str) -> String {
        fit(text, &SeamContext::none())
    }

    #[test]
    fn leerzeichen_vor_satzzeichen_faellt_weg() {
        assert_eq!(fit_none("Hallo , Welt"), "Hallo, Welt");
        assert_eq!(fit_none("Gut !"), "Gut!");
        assert_eq!(fit_none("eins  zwei   drei"), "eins zwei drei");
    }

    #[test]
    fn leerzeichen_nach_komma_und_verpasstem_punkt() {
        assert_eq!(fit_none("eins,zwei"), "eins, zwei");
        assert_eq!(fit_none("Wort.Nächste"), "Wort. Nächste");
        // Zahlen und Abkürzungen bleiben unberührt
        assert_eq!(fit_none("1,5 kg wiegen"), "1,5 kg wiegen");
        assert_eq!(fit_none("3.14159 ist Pi"), "3.14159 ist Pi");
        assert_eq!(fit_none("usw. usf."), "usw. usf.");
    }

    #[test]
    fn satzanfang_wird_gross() {
        // Ohne Naht-Kontext: ASR-Schreibweise vertrauen (Kontinuation,
        // Abkürzungen, Einheiten sind denkbar).
        assert_eq!(fit_none("hallo welt"), "hallo welt");
        // Naht: Nach Satzende links vom Cursor beginnt sicher ein Satz —
        // auch Funktionswörter werden dann großgeschrieben.
        let ctx = SeamContext { char_before: Some('.'), char_after: None };
        assert_eq!(fit("das ist gut", &ctx), "Das ist gut");
        assert_eq!(fit("hallo welt", &ctx), "Hallo welt");
    }

    #[test]
    fn funktionswoerter_mitten_im_satz_klein() {
        assert_eq!(fit_none("Ich Denke, Das War Gut"), "Ich Denke, das war Gut");
        assert_eq!(fit_none("Er Kam. Das Passt"), "Er Kam. Das Passt");
    }

    #[test]
    fn sie_bleibt_gross() {
        assert_eq!(
            fit_none("Können Sie Das Bitte Mal Machen"),
            "Können Sie das bitte mal Machen"
        );
    }

    #[test]
    fn schluss_punkt_nur_bei_fortsetzung_weg() {
        let cont = SeamContext { char_before: None, char_after: Some('w') };
        assert_eq!(fit("Fertig.", &cont), "Fertig ");
        // Ohne Naht-Kontext bleibt der Punkt sicherheitshalber stehen
        assert_eq!(fit_none("Fertig."), "Fertig.");
        // „…" und „..." sowie ? ! bleiben immer
        assert_eq!(fit("Und zwar…", &cont), "Und zwar… ");
        assert_eq!(fit("Wirklich?", &cont), "Wirklich? ");
    }

    #[test]
    fn nahtstellen_bekommen_leerzeichen() {
        // links Buchstabe, rechts Buchstabe
        let ctx = SeamContext { char_before: Some('r'), char_after: Some('w') };
        assert_eq!(fit("geht weiter", &ctx), " geht weiter ");
        // rechts Komma: kein Schluss-Leerzeichen, Schluss-Punkt bleibt (kein Buchstabe rechts)
        let ctx = SeamContext { char_before: Some('r'), char_after: Some(',') };
        assert_eq!(fit("geht.", &ctx), " geht.");
        // links öffnende Klammer: kein Leerzeichen
        let ctx = SeamContext { char_before: Some('('), char_after: None };
        assert_eq!(fit("hallo", &ctx), "hallo");
    }

    #[test]
    fn trimmt_und_haelt_sauber_text_stehen() {
        assert_eq!(fit_none("  Ganz  sauber , Danke  . "), "Ganz sauber, Danke.");
    }
}
