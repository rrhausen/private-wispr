//! Historie (specs 4.8): SQLite pro Diktat mit Rohtranskript, Endtext und
//! angewandten Regeln — Nachvollziehbarkeit, was die Pipeline verändert hat.
//!
//! Ablage: `%APPDATA%\PrivateWispr\history.db` (Roaming, laut Spec; Modelle
//! und settings.json liegen in `%LOCALAPPDATA%`, siehe dictionary.rs).
//! Audio wird standardmäßig direkt nach der Transkription gelöscht
//! (Ort: lib.rs) — optionales Archiv mit Retention folgt später.
//! Aufbewahrung der Einträge: default 90 Tage, per Einstellung änderbar
//! (0 = unbegrenzt). Für Tests laufen Insert/List/Delete gegen eine
//! übergebene Connection (In-Memory), die Datei-DB ist nur ein dünnere
//! Hülle darüber.

use anyhow::{Context, Result};
use rusqlite::Connection;

#[derive(Clone, serde::Serialize)]
pub struct Entry {
    pub id: i64,
    /// RFC3339 UTC.
    pub ts: String,
    /// Was mit dem Diktat passierte: „cursor", „handoff" oder „clipboard"
    /// (Fokus-Wechsel-Fallback).
    pub mode: String,
    pub raw_transcript: String,
    pub final_text: String,
    /// JSON: { dictionary_version, rules: [{canonical, matched, count}] }.
    pub applied_rules: serde_json::Value,
    /// Prozess-EXE der Ziel-App (z. B. „Code.exe"), wenn ermittelbar.
    pub target_app: Option<String>,
    pub duration_s: f64,
    pub model: String,
    pub engine: String,
}

/// Neuer Historien-Eintrag (roh + verarbeitet + angewandte Regeln).
pub struct NewEntry {
    pub mode: String,
    pub raw_transcript: String,
    pub final_text: String,
    pub dictionary_version: String,
    pub applied: Vec<crate::dictionary::AppliedRule>,
    pub target_app: Option<String>,
    pub duration_s: f64,
    pub model: String,
    pub engine: String,
}

pub fn db_path() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("PrivateWispr")
        .join("history.db")
}

fn open() -> Result<Connection> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(&path)
        .with_context(|| format!("History-DB nicht öffnbar: {}", path.display()))?;
    init(&conn)?;
    Ok(conn)
}

fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE IF NOT EXISTS dictations (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            ts             TEXT    NOT NULL,
            mode           TEXT    NOT NULL,
            raw_transcript TEXT    NOT NULL,
            final_text     TEXT    NOT NULL,
            applied_rules  TEXT,
            target_app     TEXT,
            duration_s     REAL    NOT NULL,
            model          TEXT,
            engine         TEXT
         );",
    )?;
    Ok(())
}

/// Eintrag schreiben; danach Aufbewahrung durchsetzen (Tage, 0 = unbegrenzt).
fn insert_into(conn: &Connection, entry: &NewEntry, retention_days: u64) -> Result<()> {
    let ts = chrono::Utc::now().to_rfc3339();
    let applied = applied_rules_json(&entry.dictionary_version, &entry.applied).to_string();
    conn.execute(
        "INSERT INTO dictations
           (ts, mode, raw_transcript, final_text, applied_rules, target_app, duration_s, model, engine)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            ts,
            entry.mode,
            entry.raw_transcript,
            entry.final_text,
            applied,
            entry.target_app,
            entry.duration_s,
            entry.model,
            entry.engine,
        ],
    )
    .context("Historien-Eintrag nicht schreibbar")?;

    if retention_days > 0 {
        let cutoff =
            (chrono::Utc::now() - chrono::Duration::days(retention_days as i64)).to_rfc3339();
        let removed = conn
            .execute("DELETE FROM dictations WHERE ts < ?1", rusqlite::params![cutoff])
            .context("Aufbewahrung nicht durchsetzbar")?;
        if removed > 0 {
            log::info!("Historie: {removed} Einträge älter als {retention_days} Tage gelöscht");
        }
    }
    Ok(())
}

fn applied_rules_json(
    dictionary_version: &str,
    applied: &[crate::dictionary::AppliedRule],
) -> serde_json::Value {
    serde_json::json!({
        "dictionary_version": dictionary_version,
        "rules": applied,
    })
}

fn list_from(conn: &Connection, limit: u32) -> Result<Vec<Entry>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, mode, raw_transcript, final_text, applied_rules, target_app,
                duration_s, model, engine
         FROM dictations ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |row| {
        let applied: Option<String> = row.get(5)?;
        Ok(Entry {
            id: row.get(0)?,
            ts: row.get(1)?,
            mode: row.get(2)?,
            raw_transcript: row.get(3)?,
            final_text: row.get(4)?,
            applied_rules: applied
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(serde_json::Value::Null),
            target_app: row.get(6)?,
            duration_s: row.get(7)?,
            model: row.get(8).unwrap_or_default(),
            engine: row.get(9).unwrap_or_default(),
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>().context("Historie-Zeilen nicht lesbar")
}

fn delete_from(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM dictations WHERE id = ?1", rusqlite::params![id])
        .with_context(|| format!("Eintrag {id} nicht löschbar"))?;
    Ok(())
}

fn clear_in(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM dictations", []).context("Historie nicht löschbar")?;
    Ok(())
}

// ─── Datei-DB-Hülle für die App ────────────────────────────────────────

pub fn insert(entry: &NewEntry) -> Result<()> {
    let conn = open()?;
    insert_into(&conn, entry, crate::settings::current().history_retention_days)
}

pub fn list(limit: u32) -> Vec<Entry> {
    try_list(limit).unwrap_or_else(|e| {
        log::error!("Historie nicht lesbar: {e:#}");
        Vec::new()
    })
}

fn try_list(limit: u32) -> Result<Vec<Entry>> {
    let conn = open()?;
    list_from(&conn, limit)
}

pub fn delete(id: i64) -> Result<()> {
    let conn = open()?;
    delete_from(&conn, id)
}

pub fn clear() -> Result<()> {
    let conn = open()?;
    clear_in(&conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::AppliedRule;

    fn entry(mode: &str) -> NewEntry {
        NewEntry {
            mode: mode.into(),
            raw_transcript: "Wir nutzen Scherpoint und scherpoint".into(),
            final_text: "Wir nutzen SharePoint und SharePoint".into(),
            dictionary_version: "ab12cd34".into(),
            applied: vec![AppliedRule {
                canonical: "SharePoint".into(),
                matched: "Scherpoint".into(),
                count: 2,
            }],
            target_app: Some("Code.exe".into()),
            duration_s: 4.2,
            model: "parakeet-tdt-0.6b-v3-Q8_0".into(),
            engine: "vulkan".into(),
        }
    }

    #[test]
    fn eintrag_rundreise_mit_regeln() {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        insert_into(&conn, &entry("cursor"), 0).unwrap();
        insert_into(&conn, &entry("clipboard"), 0).unwrap();

        let list = list_from(&conn, 10).unwrap();
        assert_eq!(list.len(), 2);
        // Neueste zuerst, Roh/Endtext getrennt, Regeln als JSON protokolliert
        assert_eq!(list[0].mode, "clipboard");
        assert_eq!(list[0].raw_transcript, "Wir nutzen Scherpoint und scherpoint");
        assert_eq!(list[0].final_text, "Wir nutzen SharePoint und SharePoint");
        assert_eq!(list[0].applied_rules["dictionary_version"], "ab12cd34");
        assert_eq!(list[0].applied_rules["rules"][0]["canonical"], "SharePoint");
        assert_eq!(list[0].applied_rules["rules"][0]["count"], 2);
        assert_eq!(list[0].target_app.as_deref(), Some("Code.exe"));
        assert_eq!(list[0].engine, "vulkan");
    }

    #[test]
    fn loeschen_einzelnen_und_alle() {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        insert_into(&conn, &entry("cursor"), 0).unwrap();
        insert_into(&conn, &entry("handoff"), 0).unwrap();

        let list = list_from(&conn, 10).unwrap();
        delete_from(&conn, list[0].id).unwrap();
        assert_eq!(list_from(&conn, 10).unwrap().len(), 1);

        clear_in(&conn).unwrap();
        assert!(list_from(&conn, 10).unwrap().is_empty());
    }

    #[test]
    fn limit_begrenzt_die_liste() {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        insert_into(&conn, &entry("cursor"), 0).unwrap();
        insert_into(&conn, &entry("cursor"), 0).unwrap();
        assert_eq!(list_from(&conn, 1).unwrap().len(), 1);
    }
}
