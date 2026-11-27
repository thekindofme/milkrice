//! Local SQLite storage for meetings, transcripts, notes and settings.
use crate::types::{AudioSource, Attendee, Segment};
use anyhow::Result;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Meeting {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    /// Total recorded audio in ms (across pauses/resumes).
    pub duration_ms: u64,
    pub event_id: Option<String>,
    pub attendees: Vec<Attendee>,
    /// The user's own notes (markdown-ish plain text).
    pub notes: String,
    /// AI-enhanced notes (markdown).
    pub enhanced: String,
    pub template: String,
    pub status: String,
    /// speaker key ("me", "s1", ...) -> display name
    pub speakers: BTreeMap<String, String>,
    /// Saved diarization state so a meeting can be resumed.
    pub clusterer: Option<serde_json::Value>,
    pub meeting_url: Option<String>,
    /// Who wrote `enhanced`, e.g. "claude:haiku" or "codex:gpt-6-luna".
    #[serde(default)]
    pub enhanced_by: String,
    /// Speech model that produced the transcript, e.g. "parakeet-v2".
    #[serde(default)]
    pub stt_model: String,
    /// Last file written by the Obsidian/folder export (to follow renames).
    #[serde(default)]
    pub export_path: String,
    /// The user edited the AI notes after they were written (cleared when regenerated).
    #[serde(default)]
    pub enhanced_edited: bool,
    /// Speaker keys whose names were guessed by the AI and not yet confirmed by the user.
    #[serde(default)]
    pub speakers_guessed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingSummary {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub status: String,
    pub attendees: Vec<Attendee>,
    pub snippet: String,
    /// Ids of the folders this note is in.
    #[serde(default)]
    pub folders: Vec<String>,
}

/// Folder colours, in the order new folders take them.
pub const FOLDER_COLORS: [&str; 8] = ["ember", "sage", "blue", "amber", "violet", "teal", "rose", "slate"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Folder {
    pub id: String,
    pub name: String,
    /// One of [`FOLDER_COLORS`].
    #[serde(default)]
    pub color: String,
    /// Notes in the folder.
    #[serde(default)]
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

pub struct Store {
    conn: Mutex<Connection>,
    /// Optional columns present in this database (a read-only handle can't migrate an older one).
    cols: [bool; 5],
}

/// Optional columns added after the first release, in `cols` order.
const OPT_COLS: [&str; 5] = ["enhanced_by", "stt_model", "export_path", "enhanced_edited", "speakers_guessed"];

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS meetings (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  started_at TEXT,
  ended_at TEXT,
  duration_ms INTEGER NOT NULL DEFAULT 0,
  event_id TEXT,
  attendees TEXT NOT NULL DEFAULT '[]',
  notes TEXT NOT NULL DEFAULT '',
  enhanced TEXT NOT NULL DEFAULT '',
  template TEXT NOT NULL DEFAULT 'auto',
  status TEXT NOT NULL DEFAULT 'draft',
  speakers TEXT NOT NULL DEFAULT '{}',
  clusterer TEXT,
  meeting_url TEXT
);
CREATE INDEX IF NOT EXISTS meetings_created ON meetings(created_at DESC);
CREATE INDEX IF NOT EXISTS meetings_event ON meetings(event_id);
CREATE TABLE IF NOT EXISTS segments (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  start_ms INTEGER NOT NULL,
  end_ms INTEGER NOT NULL,
  source TEXT NOT NULL,
  speaker TEXT NOT NULL,
  text TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS segments_meeting ON segments(meeting_id, start_ms);
CREATE TABLE IF NOT EXISTS chats (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  role TEXT NOT NULL,
  content TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS event_flags (event_id TEXT PRIMARY KEY, flag TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS folders (id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL, color TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS meeting_folders (
  meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  folder_id TEXT NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
  PRIMARY KEY (meeting_id, folder_id)
);
CREATE INDEX IF NOT EXISTS meeting_folders_folder ON meeting_folders(folder_id);
-- "Auto-add future meetings": calendar notes whose series key matches go to the folder.
CREATE TABLE IF NOT EXISTS folder_series (
  series TEXT NOT NULL,
  folder_id TEXT NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
  PRIMARY KEY (series, folder_id)
);
-- Per-note folder suggestion state: 'dismissed', 'ai:<folder id>' or 'ai-none'.
CREATE TABLE IF NOT EXISTS folder_hints (meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE, hint TEXT NOT NULL);
"#;

fn src_str(s: AudioSource) -> &'static str {
    match s {
        AudioSource::Mic => "mic",
        AudioSource::System => "system",
    }
}

fn parse_dt(s: Option<String>) -> Option<DateTime<Utc>> {
    s.and_then(|s| DateTime::parse_from_rfc3339(&s).ok()).map(|d| d.with_timezone(&Utc))
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        // Migrations: provenance columns.
        for (col, ddl) in [
            ("enhanced_by", "ALTER TABLE meetings ADD COLUMN enhanced_by TEXT NOT NULL DEFAULT ''"),
            ("stt_model", "ALTER TABLE meetings ADD COLUMN stt_model TEXT NOT NULL DEFAULT ''"),
            ("export_path", "ALTER TABLE meetings ADD COLUMN export_path TEXT NOT NULL DEFAULT ''"),
            ("enhanced_edited", "ALTER TABLE meetings ADD COLUMN enhanced_edited INTEGER NOT NULL DEFAULT 0"),
            ("speakers_guessed", "ALTER TABLE meetings ADD COLUMN speakers_guessed TEXT NOT NULL DEFAULT '[]'"),
        ] {
            let has: bool = conn
                .prepare("SELECT 1 FROM pragma_table_info('meetings') WHERE name=?1")?
                .exists([col])?;
            if !has {
                conn.execute_batch(ddl)?;
            }
        }
        // Folder colours arrived after folders.
        let has_color: bool = conn.prepare("SELECT 1 FROM pragma_table_info('folders') WHERE name='color'")?.exists([])?;
        if !has_color {
            conn.execute_batch("ALTER TABLE folders ADD COLUMN color TEXT NOT NULL DEFAULT ''")?;
        }
        // Older builds stored speaker voice prints; never keep biometric data on disk.
        conn.execute("UPDATE meetings SET clusterer=NULL WHERE clusterer IS NOT NULL", [])?;
        // A crash or force-quit can leave a meeting marked as recording; nothing is recording now.
        conn.execute("UPDATE meetings SET status='done', ended_at=COALESCE(ended_at, started_at) WHERE status='recording'", [])?;
        Ok(Self { conn: Mutex::new(conn), cols: [true; 5] })
    }

    fn has_col(conn: &Connection, col: &str) -> bool {
        conn.prepare("SELECT 1 FROM pragma_table_info('meetings') WHERE name=?1")
            .and_then(|mut st| st.exists([col]))
            .unwrap_or(false)
    }

    /// Read-only handle for the CLI / MCP server: agents can never modify meetings.
    pub fn open_readonly(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let cols = OPT_COLS.map(|c| Self::has_col(&conn, c));
        Ok(Self { conn: Mutex::new(conn), cols })
    }

    pub fn open_default() -> Result<Self> {
        Self::open(&crate::paths::db_path())
    }

    pub fn create_meeting(&self, m: &Meeting) -> Result<()> {
        let c = self.conn.lock();
        c.execute(
            "INSERT INTO meetings (id,title,created_at,started_at,ended_at,duration_ms,event_id,attendees,notes,enhanced,template,status,speakers,clusterer,meeting_url)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            params![
                m.id,
                m.title,
                m.created_at.to_rfc3339(),
                m.started_at.map(|d| d.to_rfc3339()),
                m.ended_at.map(|d| d.to_rfc3339()),
                m.duration_ms as i64,
                m.event_id,
                serde_json::to_string(&m.attendees)?,
                m.notes,
                m.enhanced,
                m.template,
                m.status,
                serde_json::to_string(&m.speakers)?,
                m.clusterer.as_ref().map(|v| v.to_string()),
                m.meeting_url,
            ],
        )?;
        Ok(())
    }

    pub fn get_meeting(&self, id: &str) -> Result<Option<Meeting>> {
        let c = self.conn.lock();
        let fallback = |col: &str| match col {
            "enhanced_edited" => "0",
            "speakers_guessed" => "'[]'",
            _ => "''",
        };
        let opt: Vec<String> = OPT_COLS
            .iter()
            .zip(self.cols)
            .map(|(col, present)| if present { col.to_string() } else { format!("{} AS {col}", fallback(col)) })
            .collect();
        let sql = format!(
            "SELECT id,title,created_at,started_at,ended_at,duration_ms,event_id,attendees,notes,enhanced,template,status,speakers,clusterer,meeting_url,{} FROM meetings WHERE id=?1",
            opt.join(",")
        );
        let m = c
            .query_row(
                &sql,
                [id],
                |r| {
                    Ok(Meeting {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        created_at: parse_dt(r.get(2)?).unwrap_or_else(Utc::now),
                        started_at: parse_dt(r.get(3)?),
                        ended_at: parse_dt(r.get(4)?),
                        duration_ms: r.get::<_, i64>(5)? as u64,
                        event_id: r.get(6)?,
                        attendees: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or_default(),
                        notes: r.get(8)?,
                        enhanced: r.get(9)?,
                        template: r.get(10)?,
                        status: r.get(11)?,
                        speakers: serde_json::from_str(&r.get::<_, String>(12)?).unwrap_or_default(),
                        clusterer: r
                            .get::<_, Option<String>>(13)?
                            .and_then(|s| serde_json::from_str(&s).ok()),
                        meeting_url: r.get(14)?,
                        enhanced_by: r.get(15)?,
                        stt_model: r.get(16)?,
                        export_path: r.get(17)?,
                        enhanced_edited: r.get::<_, i64>(18)? != 0,
                        speakers_guessed: serde_json::from_str(&r.get::<_, String>(19)?).unwrap_or_default(),
                    })
                },
            )
            .optional()?;
        Ok(m)
    }

    pub fn find_by_event(&self, event_id: &str) -> Result<Option<String>> {
        let c = self.conn.lock();
        Ok(c.query_row(
            "SELECT id FROM meetings WHERE event_id=?1 ORDER BY created_at DESC LIMIT 1",
            [event_id],
            |r| r.get(0),
        )
        .optional()?)
    }

    /// Update a single editable text/json field.
    pub fn set_field(&self, id: &str, field: &str, value: &str) -> Result<()> {
        const ALLOWED: &[&str] =
            &["title", "notes", "enhanced", "template", "status", "speakers", "attendees", "clusterer", "meeting_url", "enhanced_by", "stt_model", "export_path", "enhanced_edited", "speakers_guessed"];
        anyhow::ensure!(ALLOWED.contains(&field), "field {field} is not editable");
        let c = self.conn.lock();
        c.execute(&format!("UPDATE meetings SET {field}=?1 WHERE id=?2"), params![value, id])?;
        Ok(())
    }

    pub fn mark_started(&self, id: &str) -> Result<()> {
        let c = self.conn.lock();
        c.execute(
            "UPDATE meetings SET started_at=COALESCE(started_at, ?1), status='recording' WHERE id=?2",
            params![Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }

    pub fn mark_stopped(&self, id: &str, duration_ms: u64) -> Result<()> {
        let c = self.conn.lock();
        c.execute(
            "UPDATE meetings SET ended_at=?1, status='done', duration_ms=?2 WHERE id=?3",
            params![Utc::now().to_rfc3339(), duration_ms as i64, id],
        )?;
        Ok(())
    }

    pub fn list_meetings(&self, query: Option<&str>, limit: usize) -> Result<Vec<MeetingSummary>> {
        let c = self.conn.lock();
        let q = query.map(|s| s.trim()).filter(|s| !s.is_empty());
        let sql = if q.is_some() {
            "SELECT m.id,m.title,m.created_at,m.duration_ms,m.status,m.attendees,
               COALESCE(NULLIF(substr(m.enhanced,1,240),''), substr(m.notes,1,240)) FROM meetings m
             WHERE m.title LIKE ?1 OR m.notes LIKE ?1 OR m.enhanced LIKE ?1 OR m.attendees LIKE ?1
               OR EXISTS (SELECT 1 FROM segments s WHERE s.meeting_id=m.id AND s.text LIKE ?1)
             ORDER BY m.created_at DESC LIMIT ?2"
        } else {
            "SELECT m.id,m.title,m.created_at,m.duration_ms,m.status,m.attendees,
               COALESCE(NULLIF(substr(m.enhanced,1,240),''), substr(m.notes,1,240)) FROM meetings m
             WHERE ?1 IS NULL OR ?1 IS NOT NULL ORDER BY m.created_at DESC LIMIT ?2"
        };
        let like = q.map(|s| format!("%{}%", s.replace('%', "\\%")));
        let mut st = c.prepare(sql)?;
        let rows = st.query_map(params![like, limit as i64], |r| {
            Ok(MeetingSummary {
                id: r.get(0)?,
                title: r.get(1)?,
                created_at: parse_dt(r.get(2)?).unwrap_or_else(Utc::now),
                duration_ms: r.get::<_, i64>(3)? as u64,
                status: r.get(4)?,
                attendees: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
                snippet: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                folders: vec![],
            })
        })?;
        let mut out: Vec<MeetingSummary> = rows.filter_map(|r| r.ok()).collect();
        let mut by: std::collections::HashMap<String, Vec<String>> = Default::default();
        // An older database opened read-only may not have folders yet.
        if let Ok(mut st) = c.prepare("SELECT meeting_id, folder_id FROM meeting_folders") {
            if let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
                for (m, f) in rows.flatten() {
                    by.entry(m).or_default().push(f);
                }
            }
        }
        for m in &mut out {
            m.folders = by.remove(&m.id).unwrap_or_default();
        }
        Ok(out)
    }

    /// Resolve a full or prefix id (agents use short 8-char ids).
    pub fn resolve_id(&self, prefix: &str) -> Result<Option<String>> {
        let c = self.conn.lock();
        let mut st = c.prepare("SELECT id FROM meetings WHERE id LIKE ?1 ORDER BY created_at DESC LIMIT 2")?;
        let ids: Vec<String> = st.query_map([format!("{prefix}%")], |r| r.get(0))?.filter_map(|r| r.ok()).collect();
        anyhow::ensure!(ids.len() <= 1, "id prefix '{prefix}' is ambiguous");
        Ok(ids.into_iter().next())
    }

    /// Meetings created at or after `since`, newest first.
    pub fn meetings_since(&self, since: DateTime<Utc>, limit: usize) -> Result<Vec<MeetingSummary>> {
        Ok(self.list_meetings(None, 10_000)?.into_iter().filter(|m| m.created_at >= since).take(limit).collect())
    }

    pub fn delete_meeting(&self, id: &str) -> Result<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM meetings WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn add_segment(&self, meeting_id: &str, s: &Segment) -> Result<i64> {
        let c = self.conn.lock();
        c.execute(
            "INSERT INTO segments (meeting_id,start_ms,end_ms,source,speaker,text) VALUES (?1,?2,?3,?4,?5,?6)",
            params![meeting_id, s.start_ms as i64, s.end_ms as i64, src_str(s.source), s.speaker, s.text],
        )?;
        Ok(c.last_insert_rowid())
    }

    /// Total transcribed speech in a meeting so far (both sides), in ms.
    pub fn speech_ms(&self, meeting_id: &str) -> Result<i64> {
        let c = self.conn.lock();
        Ok(c.query_row(
            "SELECT COALESCE(SUM(end_ms - start_ms), 0) FROM segments WHERE meeting_id=?1",
            [meeting_id],
            |r| r.get(0),
        )?)
    }

    pub fn segments(&self, meeting_id: &str) -> Result<Vec<Segment>> {
        let c = self.conn.lock();
        let mut st = c.prepare(
            "SELECT id,start_ms,end_ms,source,speaker,text FROM segments WHERE meeting_id=?1 ORDER BY start_ms, id",
        )?;
        let rows = st.query_map([meeting_id], |r| {
            Ok(Segment {
                id: r.get(0)?,
                start_ms: r.get::<_, i64>(1)? as u64,
                end_ms: r.get::<_, i64>(2)? as u64,
                source: if r.get::<_, String>(3)? == "mic" { AudioSource::Mic } else { AudioSource::System },
                speaker: r.get(4)?,
                text: r.get(5)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn update_segment_text(&self, id: i64, text: &str) -> Result<()> {
        self.conn.lock().execute("UPDATE segments SET text=?1 WHERE id=?2", params![text, id])?;
        Ok(())
    }

    /// Move every segment of one speaker key to another (merging speakers).
    pub fn reassign_speaker(&self, meeting_id: &str, from: &str, to: &str) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE segments SET speaker=?1 WHERE meeting_id=?2 AND speaker=?3",
            params![to, meeting_id, from],
        )?;
        Ok(())
    }

    pub fn add_chat(&self, meeting_id: &str, role: &str, content: &str) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO chats (meeting_id,role,content,created_at) VALUES (?1,?2,?3,?4)",
            params![meeting_id, role, content, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn chats(&self, meeting_id: &str) -> Result<Vec<ChatMessage>> {
        let c = self.conn.lock();
        let mut st = c.prepare("SELECT role,content,created_at FROM chats WHERE meeting_id=?1 ORDER BY id")?;
        let rows = st.query_map([meeting_id], |r| {
            Ok(ChatMessage {
                role: r.get(0)?,
                content: r.get(1)?,
                created_at: parse_dt(r.get(2)?).unwrap_or_else(Utc::now),
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn get_setting(&self, key: &str) -> Option<String> {
        self.conn
            .lock()
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO settings (key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Per-calendar-event flags such as "notified" or "skipped".
    pub fn event_flag(&self, event_id: &str) -> Option<String> {
        self.conn
            .lock()
            .query_row("SELECT flag FROM event_flags WHERE event_id=?1", [event_id], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_event_flag(&self, event_id: &str, flag: &str) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO event_flags (event_id,flag) VALUES (?1,?2) ON CONFLICT(event_id) DO UPDATE SET flag=excluded.flag",
            params![event_id, flag],
        )?;
        Ok(())
    }

    /// Delete notes that never got anything in them: drafts with no notes, transcript, AI notes,
    /// chat or folder (opening an upcoming event creates one; reopening the event recreates it).
    /// A note the user titled by hand counts as kept. `keep` is the note on screen.
    pub fn purge_empty_drafts(&self, keep: Option<&str>) -> Result<usize> {
        let n = self.conn.lock().execute(
            "DELETE FROM meetings WHERE status='draft' AND duration_ms=0
               AND trim(notes)='' AND enhanced=''
               AND (event_id IS NOT NULL OR trim(title)='')
               AND id <> COALESCE(?1, '')
               AND NOT EXISTS (SELECT 1 FROM segments s WHERE s.meeting_id=meetings.id)
               AND NOT EXISTS (SELECT 1 FROM chats c WHERE c.meeting_id=meetings.id)
               AND NOT EXISTS (SELECT 1 FROM meeting_folders f WHERE f.meeting_id=meetings.id)",
            params![keep],
        )?;
        Ok(n)
    }

    // ---------------------------------------------------------------- folders

    /// All folders, alphabetical, with note counts. Empty on a database without folders.
    pub fn list_folders(&self) -> Result<Vec<Folder>> {
        let c = self.conn.lock();
        let Ok(mut st) = c.prepare(
            "SELECT f.id, f.name, (SELECT COUNT(*) FROM meeting_folders mf WHERE mf.folder_id=f.id), f.color
             FROM folders f ORDER BY f.name COLLATE NOCASE",
        ) else {
            return Ok(vec![]);
        };
        let rows = st.query_map([], |r| {
            Ok(Folder { id: r.get(0)?, name: r.get(1)?, count: r.get::<_, i64>(2)? as usize, color: r.get::<_, String>(3).unwrap_or_default() })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Create a folder, or return the existing one with the same name (ignoring case).
    pub fn create_folder(&self, name: &str) -> Result<Folder> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "folder name is empty");
        anyhow::ensure!(name.chars().count() <= 60, "folder name is too long");
        if let Some(f) = self.list_folders()?.into_iter().find(|f| f.name.eq_ignore_ascii_case(name)) {
            return Ok(f);
        }
        // The first colour no folder uses yet (cycling once all are taken).
        let used: Vec<String> = self.list_folders()?.into_iter().map(|f| f.color).collect();
        let color = FOLDER_COLORS
            .iter()
            .find(|c| !used.iter().any(|u| u == *c))
            .unwrap_or(&FOLDER_COLORS[used.len() % FOLDER_COLORS.len()])
            .to_string();
        let f = Folder { id: uuid::Uuid::new_v4().to_string(), name: name.to_string(), count: 0, color };
        self.conn.lock().execute(
            "INSERT INTO folders (id,name,created_at,color) VALUES (?1,?2,?3,?4)",
            params![f.id, f.name, Utc::now().to_rfc3339(), f.color],
        )?;
        Ok(f)
    }

    pub fn rename_folder(&self, id: &str, name: &str) -> Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "folder name is empty");
        anyhow::ensure!(name.chars().count() <= 60, "folder name is too long");
        if self.list_folders()?.iter().any(|f| f.id != id && f.name.eq_ignore_ascii_case(name)) {
            anyhow::bail!("there's already a folder called {name}");
        }
        self.conn.lock().execute("UPDATE folders SET name=?2 WHERE id=?1", params![id, name])?;
        Ok(())
    }

    pub fn set_folder_color(&self, id: &str, color: &str) -> Result<()> {
        anyhow::ensure!(FOLDER_COLORS.contains(&color), "unknown colour {color}");
        self.conn.lock().execute("UPDATE folders SET color=?2 WHERE id=?1", params![id, color])?;
        Ok(())
    }

    /// Delete a folder. Its notes are kept; they just leave the folder.
    pub fn delete_folder(&self, id: &str) -> Result<()> {
        self.conn.lock().execute("DELETE FROM folders WHERE id=?1", [id])?;
        Ok(())
    }

    /// Folders a note is in.
    pub fn folders_of(&self, meeting_id: &str) -> Result<Vec<Folder>> {
        let c = self.conn.lock();
        let Ok(mut st) = c.prepare(
            "SELECT f.id, f.name, f.color FROM folders f JOIN meeting_folders mf ON mf.folder_id=f.id
             WHERE mf.meeting_id=?1 ORDER BY f.name COLLATE NOCASE",
        ) else {
            return Ok(vec![]);
        };
        let rows = st.query_map([meeting_id], |r| Ok(Folder { id: r.get(0)?, name: r.get(1)?, count: 0, color: r.get::<_, String>(2).unwrap_or_default() }))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn set_meeting_folder(&self, meeting_id: &str, folder_id: &str, on: bool) -> Result<()> {
        let c = self.conn.lock();
        if on {
            c.execute("INSERT OR IGNORE INTO meeting_folders (meeting_id, folder_id) VALUES (?1,?2)", params![meeting_id, folder_id])?;
        } else {
            c.execute("DELETE FROM meeting_folders WHERE meeting_id=?1 AND folder_id=?2", params![meeting_id, folder_id])?;
        }
        Ok(())
    }

    /// Every filed note as `(folder_id, info)`, for suggestions. Text is capped per note.
    pub fn filed_notes(&self) -> Result<Vec<(String, crate::folders::NoteInfo)>> {
        let c = self.conn.lock();
        let Ok(mut st) = c.prepare(
            "SELECT mf.folder_id, m.title, m.attendees, substr(CASE WHEN m.enhanced<>'' THEN m.enhanced ELSE m.notes END, 1, 4000), m.event_id IS NOT NULL
             FROM meeting_folders mf JOIN meetings m ON m.id=mf.meeting_id",
        ) else {
            return Ok(vec![]);
        };
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                crate::folders::NoteInfo {
                    title: r.get(1)?,
                    attendees: serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or_default(),
                    text: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    from_calendar: r.get(4)?,
                },
            ))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Folders that auto-collect calendar notes with this series key.
    pub fn series_folders(&self, series: &str) -> Result<Vec<String>> {
        if series.is_empty() {
            return Ok(vec![]);
        }
        let c = self.conn.lock();
        let Ok(mut st) = c.prepare("SELECT folder_id FROM folder_series WHERE series=?1") else { return Ok(vec![]) };
        let rows = st.query_map([series], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn set_series_folder(&self, series: &str, folder_id: &str, on: bool) -> Result<()> {
        anyhow::ensure!(!series.is_empty(), "this note has no recurring series");
        let c = self.conn.lock();
        if on {
            c.execute("INSERT OR IGNORE INTO folder_series (series, folder_id) VALUES (?1,?2)", params![series, folder_id])?;
        } else {
            c.execute("DELETE FROM folder_series WHERE series=?1 AND folder_id=?2", params![series, folder_id])?;
        }
        Ok(())
    }

    pub fn folder_hint(&self, meeting_id: &str) -> Option<String> {
        self.conn
            .lock()
            .query_row("SELECT hint FROM folder_hints WHERE meeting_id=?1", [meeting_id], |r| r.get(0))
            .ok()
    }

    pub fn set_folder_hint(&self, meeting_id: &str, hint: &str) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO folder_hints (meeting_id, hint) VALUES (?1,?2) ON CONFLICT(meeting_id) DO UPDATE SET hint=excluded.hint",
            params![meeting_id, hint],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders() {
        let dir = std::env::temp_dir().join(format!("milkrice-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = Store::open(&dir.join("t.sqlite")).unwrap();
        let m = Meeting { id: "m1".into(), title: "Acme kickoff".into(), created_at: Utc::now(), template: "auto".into(), status: "draft".into(), ..Default::default() };
        s.create_meeting(&m).unwrap();
        let f = s.create_folder(" Acme ").unwrap();
        assert_eq!(s.create_folder("acme").unwrap().id, f.id, "same name reuses the folder");
        s.set_meeting_folder("m1", &f.id, true).unwrap();
        s.set_meeting_folder("m1", &f.id, true).unwrap();
        assert_eq!(s.list_folders().unwrap()[0].count, 1);
        assert_eq!(s.list_meetings(None, 10).unwrap()[0].folders, vec![f.id.clone()]);
        assert_eq!(s.filed_notes().unwrap()[0].1.title, "Acme kickoff");
        let g = s.create_folder("Hiring").unwrap();
        assert_ne!(f.color, g.color, "new folders take different colours");
        s.set_folder_color(&g.id, "teal").unwrap();
        assert!(s.set_folder_color(&g.id, "neon").is_err());
        assert!(s.rename_folder(&g.id, "ACME").is_err(), "no duplicate names");
        s.set_series_folder("acme kickoff", &f.id, true).unwrap();
        assert_eq!(s.series_folders("acme kickoff").unwrap(), vec![f.id.clone()]);
        s.set_folder_hint("m1", "dismissed").unwrap();
        assert_eq!(s.folder_hint("m1").as_deref(), Some("dismissed"));
        s.delete_folder(&f.id).unwrap();
        assert!(s.folders_of("m1").unwrap().is_empty());
        assert!(s.get_meeting("m1").unwrap().is_some(), "deleting a folder keeps its notes");
        assert!(s.series_folders("acme kickoff").unwrap().is_empty());
    }

    #[test]
    fn purges_only_empty_drafts() {
        let dir = std::env::temp_dir().join(format!("milkrice-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = Store::open(&dir.join("t.sqlite")).unwrap();
        let mk = |id: &str, title: &str, ev: bool| Meeting {
            id: id.into(), title: title.into(), created_at: Utc::now(), template: "auto".into(), status: "draft".into(),
            event_id: ev.then(|| format!("ev-{id}")), ..Default::default()
        };
        for m in [mk("empty-ev", "Yoga", true), mk("empty", "", false), mk("titled", "Ideas", false), mk("noted", "", false), mk("open", "", false)] {
            s.create_meeting(&m).unwrap();
        }
        s.set_field("noted", "notes", "buy milk").unwrap();
        assert_eq!(s.purge_empty_drafts(Some("open")).unwrap(), 2);
        let left: Vec<String> = s.list_meetings(None, 10).unwrap().into_iter().map(|m| m.id).collect();
        assert!(left.contains(&"titled".to_string()) && left.contains(&"noted".to_string()) && left.contains(&"open".to_string()));
        assert!(!left.contains(&"empty-ev".to_string()) && !left.contains(&"empty".to_string()));
    }

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("milkrice-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = Store::open(&dir.join("t.sqlite")).unwrap();
        let m = Meeting {
            id: "m1".into(),
            title: "Weekly sync".into(),
            created_at: Utc::now(),
            template: "auto".into(),
            status: "draft".into(),
            ..Default::default()
        };
        s.create_meeting(&m).unwrap();
        s.add_segment(
            "m1",
            &Segment { id: 0, start_ms: 0, end_ms: 1000, source: AudioSource::Mic, speaker: "me".into(), text: "budget is approved".into() },
        )
        .unwrap();
        s.set_field("m1", "notes", "hello").unwrap();
        assert_eq!(s.get_meeting("m1").unwrap().unwrap().notes, "hello");
        assert_eq!(s.list_meetings(Some("budget"), 10).unwrap().len(), 1);
        assert_eq!(s.list_meetings(None, 10).unwrap().len(), 1);
        assert!(s.set_field("m1", "id; DROP", "x").is_err());
        s.set_field("m1", "enhanced_edited", "1").unwrap();
        s.set_field("m1", "speakers_guessed", r#"["s1"]"#).unwrap();
        let got = s.get_meeting("m1").unwrap().unwrap();
        assert!(got.enhanced_edited);
        assert_eq!(got.speakers_guessed, vec!["s1".to_string()]);
        s.delete_meeting("m1").unwrap();
        assert!(s.segments("m1").unwrap().is_empty());
    }
}

/// Migrations from older databases and query edge cases.
#[cfg(test)]
mod edge_tests {
    use super::*;

    fn tmp_db() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("milkrice-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("t.sqlite")
    }

    /// The schema of the first release: no provenance columns, no folders at all.
    const V1_SCHEMA: &str = "
        CREATE TABLE meetings (id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL,
          started_at TEXT, ended_at TEXT, duration_ms INTEGER NOT NULL DEFAULT 0, event_id TEXT,
          attendees TEXT NOT NULL DEFAULT '[]', notes TEXT NOT NULL DEFAULT '', enhanced TEXT NOT NULL DEFAULT '',
          template TEXT NOT NULL DEFAULT 'auto', status TEXT NOT NULL DEFAULT 'draft', speakers TEXT NOT NULL DEFAULT '{}',
          clusterer TEXT, meeting_url TEXT);
        CREATE TABLE segments (id INTEGER PRIMARY KEY AUTOINCREMENT, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
          start_ms INTEGER NOT NULL, end_ms INTEGER NOT NULL, source TEXT NOT NULL, speaker TEXT NOT NULL, text TEXT NOT NULL);
        CREATE TABLE chats (id INTEGER PRIMARY KEY AUTOINCREMENT, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
          role TEXT NOT NULL, content TEXT NOT NULL, created_at TEXT NOT NULL);
        CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        INSERT INTO meetings (id,title,created_at,started_at,status,speakers,clusterer,attendees)
          VALUES ('old1','Kick-off 🚀','2025-01-02T03:04:05+00:00','2025-01-02T03:04:05+00:00','recording',
                  '{\"s1\":\"Priya\"}','{\"centroids\":[[0.1,0.2]]}','[{\"name\":\"Priya\",\"email\":null,\"is_self\":false}]');
        INSERT INTO segments (meeting_id,start_ms,end_ms,source,speaker,text) VALUES ('old1',0,1500,'system','s1','hello from v1');
        INSERT INTO settings (key,value) VALUES ('settings','{\"language\":\"auto\"}');
    ";

    fn v1_db() -> std::path::PathBuf {
        let p = tmp_db();
        Connection::open(&p).unwrap().execute_batch(V1_SCHEMA).unwrap();
        p
    }

    #[test]
    fn migrates_a_first_release_database() {
        let p = v1_db();
        let s = Store::open(&p).unwrap();
        let m = s.get_meeting("old1").unwrap().expect("old meeting readable");
        assert_eq!(m.title, "Kick-off 🚀");
        assert_eq!(m.speakers.get("s1").map(String::as_str), Some("Priya"));
        assert_eq!(m.attendees.len(), 1);
        // New columns get their defaults.
        assert_eq!((m.enhanced_by.as_str(), m.stt_model.as_str(), m.export_path.as_str()), ("", "", ""));
        assert!(!m.enhanced_edited && m.speakers_guessed.is_empty());
        // Stored voice prints are wiped; a meeting left "recording" by a crash is closed.
        assert!(m.clusterer.is_none());
        assert_eq!(m.status, "done");
        assert_eq!(m.ended_at, m.started_at);
        // Old data stays usable with the new features.
        assert_eq!(s.segments("old1").unwrap()[0].text, "hello from v1");
        assert_eq!(s.get_setting("settings").as_deref(), Some("{\"language\":\"auto\"}"));
        let f = s.create_folder("Clients").unwrap();
        assert!(FOLDER_COLORS.contains(&f.color.as_str()));
        s.set_meeting_folder("old1", &f.id, true).unwrap();
        assert_eq!(s.list_meetings(None, 10).unwrap()[0].folders, vec![f.id.clone()]);
        s.set_field("old1", "speakers_guessed", "[\"s1\"]").unwrap();
        drop(s);
        // Opening again is a no-op migration and keeps everything.
        let s = Store::open(&p).unwrap();
        assert_eq!(s.get_meeting("old1").unwrap().unwrap().speakers_guessed, vec!["s1".to_string()]);
        assert_eq!(s.list_folders().unwrap().len(), 1);
    }

    #[test]
    fn folders_table_without_colour_column_is_migrated() {
        let p = v1_db();
        Connection::open(&p)
            .unwrap()
            .execute_batch(
                "CREATE TABLE folders (id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL);
                 INSERT INTO folders VALUES ('f1','Hiring','2025-01-01T00:00:00+00:00');",
            )
            .unwrap();
        let s = Store::open(&p).unwrap();
        let fs = s.list_folders().unwrap();
        assert_eq!(fs.len(), 1);
        assert_eq!(fs[0].color, "");
        s.set_folder_color("f1", "sage").unwrap();
        assert_eq!(s.list_folders().unwrap()[0].color, "sage");
    }

    #[test]
    fn read_only_handle_reads_an_unmigrated_database() {
        // The CLI / MCP server can open an old database the app hasn't migrated yet.
        let p = v1_db();
        let s = Store::open_readonly(&p).unwrap();
        let m = s.get_meeting("old1").unwrap().unwrap();
        assert_eq!(m.title, "Kick-off 🚀");
        assert_eq!(m.enhanced_by, "");
        assert!(m.speakers_guessed.is_empty());
        let list = s.list_meetings(None, 10).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].folders.is_empty());
        assert!(s.list_folders().unwrap().is_empty());
        assert!(s.folders_of("old1").unwrap().is_empty());
        assert!(s.filed_notes().unwrap().is_empty());
        assert!(s.series_folders("anything").unwrap().is_empty());
        assert_eq!(s.list_meetings(Some("v1"), 10).unwrap().len(), 1, "transcript search");
        // And it really is read-only.
        assert!(s.set_field("old1", "title", "x").is_err());
    }

    fn meeting(id: &str, title: &str) -> Meeting {
        Meeting { id: id.into(), title: title.into(), created_at: Utc::now(), template: "auto".into(), status: "done".into(), ..Default::default() }
    }

    #[test]
    fn search_treats_percent_and_underscore_literally() {
        let s = Store::open(&tmp_db()).unwrap();
        s.create_meeting(&meeting("a", "Budget: cut 50% of travel")).unwrap();
        s.create_meeting(&meeting("b", "Budget: cut 500 laptops")).unwrap();
        s.create_meeting(&meeting("c", "rename user_id column")).unwrap();
        s.create_meeting(&meeting("d", "rename userXid column")).unwrap();
        s.create_meeting(&meeting("e", "path C:\\temp cleanup")).unwrap();
        let ids = |q: &str| -> Vec<String> {
            let mut v: Vec<String> = s.list_meetings(Some(q), 10).unwrap().into_iter().map(|m| m.id).collect();
            v.sort();
            v
        };
        assert_eq!(ids("50%"), vec!["a"]);
        assert_eq!(ids("user_id"), vec!["c"]);
        assert_eq!(ids("C:\\temp"), vec!["e"]);
        assert_eq!(ids("budget"), vec!["a", "b"], "ASCII search ignores case");
        assert_eq!(ids("   "), vec!["a", "b", "c", "d", "e"], "blank query lists everything");
    }

    #[test]
    fn unicode_titles_round_trip_and_are_searchable() {
        let s = Store::open(&tmp_db()).unwrap();
        let titles = ["週次定例 — 予算", "Café ☕ avec Zoë", "Встреча по проекту", "🎉🎉🎉", "مراجعة الميزانية"];
        for (i, t) in titles.iter().enumerate() {
            s.create_meeting(&meeting(&format!("u{i}"), t)).unwrap();
        }
        for (i, t) in titles.iter().enumerate() {
            assert_eq!(s.get_meeting(&format!("u{i}")).unwrap().unwrap().title, *t);
        }
        assert_eq!(s.list_meetings(Some("予算"), 10).unwrap()[0].id, "u0");
        assert_eq!(s.list_meetings(Some("Zoë"), 10).unwrap()[0].id, "u1");
        assert_eq!(s.list_meetings(Some("🎉"), 10).unwrap()[0].id, "u3");
        // Folder names are capped by characters, not bytes: 60 CJK chars (180 bytes) is fine.
        let long: String = "会".repeat(60);
        assert!(s.create_folder(&long).is_ok());
        assert!(s.create_folder(&"会".repeat(61)).is_err());
    }

    #[test]
    fn empty_and_very_long_transcripts() {
        let s = Store::open(&tmp_db()).unwrap();
        s.create_meeting(&meeting("empty", "Nothing said")).unwrap();
        assert!(s.segments("empty").unwrap().is_empty());
        assert_eq!(s.speech_ms("empty").unwrap(), 0);
        assert!(s.chats("empty").unwrap().is_empty());

        // A 9-hour workshop: segment times past u32 ms-of-an-int32 range and many rows.
        s.create_meeting(&meeting("long", "All-day workshop")).unwrap();
        let n = 6_000u64;
        let step = 9 * 3_600_000 / n;
        for i in (0..n).rev() {
            let spk = if i % 2 == 0 { "me" } else { "s1" };
            s.add_segment("long", &Segment { id: 0, start_ms: i * step, end_ms: i * step + step - 100, source: AudioSource::System, speaker: spk.into(), text: format!("line {i}") })
                .unwrap();
        }
        // A segment beyond i32::MAX ms (~24.8 days) must not wrap.
        let far = i32::MAX as u64 + 10_000;
        s.add_segment("long", &Segment { id: 0, start_ms: far, end_ms: far + 1_000, source: AudioSource::Mic, speaker: "me".into(), text: "far".into() })
            .unwrap();
        let segs = s.segments("long").unwrap();
        assert_eq!(segs.len(), n as usize + 1);
        assert!(segs.windows(2).all(|w| w[0].start_ms <= w[1].start_ms), "segments come back in time order");
        assert_eq!(segs.last().unwrap().start_ms, far);
        assert_eq!(s.speech_ms("long").unwrap(), (n * (step - 100) + 1_000) as i64);
        s.reassign_speaker("long", "s1", "s2").unwrap();
        assert!(s.segments("long").unwrap().iter().all(|x| x.speaker != "s1"));
        // Deleting the meeting removes its transcript (foreign keys are on).
        s.delete_meeting("long").unwrap();
        assert!(s.segments("long").unwrap().is_empty());
    }

    #[test]
    fn id_prefixes_resolve_and_ambiguity_is_an_error() {
        let s = Store::open(&tmp_db()).unwrap();
        s.create_meeting(&meeting("abc-111", "One")).unwrap();
        s.create_meeting(&meeting("abc-222", "Two")).unwrap();
        assert_eq!(s.resolve_id("abc-1").unwrap().as_deref(), Some("abc-111"));
        assert!(s.resolve_id("abc").is_err());
        assert_eq!(s.resolve_id("zzz").unwrap(), None);
    }
}
