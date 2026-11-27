//! The Mac side of iPhone sync, shared by the Milkrice app and the `milkrice-sync` command:
//! setting up the vault, pairing, one sync pass against the Mac's store, and status.
//!
//! Local state lives in `<data dir>/sync/` (`config.json`, `state.json`). The vault key is kept
//! in the macOS login keychain, never in the sync folder.
use crate::{
    crypto::{self, Key},
    folder::{DeviceCard, Vault},
    merge::{self, Clock, MeetingState, Record},
};
use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use milkrice_core::{llm, paths, store::Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub folder: PathBuf,
    pub device_id: String,
    pub vault_id: String,
    pub device_name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SyncState {
    clock: Clock,
    meetings: BTreeMap<String, MeetingState>,
    /// Hash of the record last written per meeting, so unchanged records aren't rewritten
    /// (each rewrite is an upload for iCloud/Drive).
    #[serde(default)]
    written: BTreeMap<String, String>,
    /// When this Mac's device card was last refreshed (unix seconds).
    #[serde(default)]
    device_written_at: i64,
    /// Where each meeting was first recorded ("mac", "iphone"), as the records say.
    #[serde(default)]
    origins: BTreeMap<String, String>,
    /// How each meeting was recorded ("mac", "room", "voice note"…), as the records say.
    #[serde(default)]
    modes: BTreeMap<String, String>,
}

pub fn sync_dir() -> PathBuf {
    let d = paths::data_dir().join("sync");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn load<T: for<'a> Deserialize<'a> + Default>(name: &str) -> T {
    std::fs::read(sync_dir().join(name)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save<T: Serialize>(name: &str, v: &T) -> Result<()> {
    let p = sync_dir().join(name);
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
    std::fs::rename(tmp, p)?;
    Ok(())
}

/// This Mac's sync setup, or an error when sync was never set up.
pub fn config() -> Result<Config> {
    let b = std::fs::read(sync_dir().join("config.json")).context("sync isn't set up on this Mac yet")?;
    Ok(serde_json::from_slice(&b)?)
}

// ---------- keychain

pub const KEYCHAIN_SERVICE: &str = "Milkrice Sync";

fn store_key(vault_id: &str, key: &Key) -> Result<()> {
    if let Ok(path) = std::env::var("MILKRICE_SYNC_KEYFILE") {
        return Ok(std::fs::write(path, hex::encode(key))?); // tests only
    }
    #[cfg(target_os = "macos")]
    {
        security_framework::passwords::set_generic_password(KEYCHAIN_SERVICE, vault_id, hex::encode(key).as_bytes())
            .context("couldn't save the sync key to the keychain")
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (vault_id, key);
        bail!("the keychain is only available on macOS; set MILKRICE_SYNC_KEYFILE")
    }
}

fn load_key(vault_id: &str) -> Result<Key> {
    let hex_key = if let Ok(path) = std::env::var("MILKRICE_SYNC_KEYFILE") {
        std::fs::read_to_string(path)?.into_bytes()
    } else {
        #[cfg(target_os = "macos")]
        {
            security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, vault_id)
                .context("the sync key isn't in this Mac's keychain")?
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = vault_id;
            bail!("the keychain is only available on macOS; set MILKRICE_SYNC_KEYFILE")
        }
    };
    hex::decode(String::from_utf8(hex_key)?.trim())?.try_into().ok().context("bad key in keychain")
}

fn delete_key(vault_id: &str) {
    if std::env::var("MILKRICE_SYNC_KEYFILE").is_ok() {
        return;
    }
    #[cfg(target_os = "macos")]
    let _ = security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, vault_id);
    #[cfg(not(target_os = "macos"))]
    let _ = vault_id;
}

pub fn open_vault(cfg: &Config) -> Result<Vault> {
    Vault::open_expecting(&cfg.folder, load_key(&cfg.vault_id)?, &cfg.vault_id)
}

/// iCloud Drive › Milkrice Sync.
pub fn default_folder() -> PathBuf {
    icloud_drive().join("Milkrice Sync")
}

pub fn icloud_drive() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join("Library/Mobile Documents/com~apple~CloudDocs")
}

pub fn host_name() -> String {
    std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Mac".into())
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn wall_ms() -> u64 {
    Utc::now().timestamp_millis() as u64
}

// ---------- setup

/// Create a new encrypted vault in `folder` (default: iCloud Drive › Milkrice Sync).
pub fn init(folder: Option<PathBuf>) -> Result<Config> {
    if let Ok(c) = config() {
        bail!("sync is already set up here (folder: {})", c.folder.display());
    }
    let folder = folder.unwrap_or_else(default_folder);
    if folder.starts_with(dirs::home_dir().unwrap_or_default().join("Library/Mobile Documents")) && !icloud_drive().exists() {
        bail!("iCloud Drive isn't turned on for this Mac. Turn it on in System Settings › Apple Account › iCloud, or choose another folder.");
    }
    std::fs::create_dir_all(&folder).with_context(|| format!("couldn't create {}", folder.display()))?;
    let vault = Vault::create(&folder)?;
    let cfg = Config {
        folder: folder.clone(),
        device_id: hex::encode(crypto::random_bytes::<16>()),
        vault_id: crypto::b64(&vault.vault_id),
        device_name: host_name(),
    };
    store_key(&cfg.vault_id, &vault.key)?;
    save("config.json", &cfg)?;
    vault.write_device(&cfg.device_id, &DeviceCard { name: cfg.device_name.clone(), kind: "mac".into(), last_seen: now_rfc3339() })?;
    Ok(cfg)
}

/// Stop syncing on this Mac: forget the key and the local sync state. Meetings stay on the Mac,
/// and the vault folder is left for the other devices.
pub fn leave() -> Result<()> {
    let cfg = config()?;
    delete_key(&cfg.vault_id);
    let _ = std::fs::remove_file(cfg.folder.join("devices").join(format!("{}.dev", cfg.device_id)));
    for f in ["config.json", "state.json"] {
        let _ = std::fs::remove_file(sync_dir().join(f));
    }
    Ok(())
}

// ---------- pairing

pub use crate::pairing::{Joining, Pairing};

/// The pairing invite as an SVG QR code.
pub fn qr_svg(text: &str) -> Result<String> {
    let code = qrcode::QrCode::new(text.as_bytes())?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .quiet_zone(true)
        .min_dimensions(220, 220)
        .dark_color(qrcode::render::svg::Color("#1d1b16"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}

/// The invite rendered for a terminal.
pub fn qr_text(text: &str) -> Result<String> {
    let code = qrcode::QrCode::new(text.as_bytes())?;
    Ok(code.render::<qrcode::render::unicode::Dense1x2>().quiet_zone(true).build())
}

// ---------- one pass

/// The Mac's view of one meeting, as sync fields.
/// `origin`/`mode` aren't stored on the Mac, so they're carried from what the records said
/// (always present in the record, or a device paired later would never learn them).
fn local_fields(
    store: &Store,
    id: &str,
    known: Option<&MeetingState>,
    origin: Option<&str>,
    mode: Option<&str>,
) -> Result<Option<BTreeMap<String, Value>>> {
    let Some(m) = store.get_meeting(id)? else { return Ok(None) };
    let segs = store.segments(id)?;
    let transcript: Vec<Value> =
        segs.iter().map(|s| json!([s.start_ms, s.end_ms, llm::speaker_name(&s.speaker, &m.speakers), s.text])).collect();
    let mut f = BTreeMap::new();
    f.insert("title".into(), json!(m.title));
    f.insert("notes".into(), json!(m.notes));
    f.insert("enhanced".into(), json!(m.enhanced));
    f.insert("enhanced_by".into(), json!(m.enhanced_by));
    f.insert("transcript".into(), Value::Array(transcript));
    f.insert("transcribed_by".into(), json!(m.stt_model));
    f.insert("created_at".into(), json!(m.created_at.to_rfc3339_opts(SecondsFormat::Secs, true)));
    f.insert("duration_ms".into(), json!(m.duration_ms));
    f.insert("deleted".into(), json!(false));
    for (k, v) in [("origin", origin), ("mode", mode)] {
        match v {
            // Came from another device that didn't say where it was recorded: don't claim it.
            Some("") => {}
            Some(v) => {
                f.insert(k.into(), json!(v));
            }
            // Seen before but the value wasn't kept (state from an older version): leave it to
            // the records rather than claim "mac".
            None if known.map(|s| s.contains_key(k)).unwrap_or(false) => {}
            None => {
                f.insert(k.into(), json!("mac"));
            }
        }
    }
    Ok(Some(f))
}

fn db_conn() -> Result<rusqlite::Connection> {
    let c = rusqlite::Connection::open(paths::db_path())?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(c)
}

/// Apply another device's changes to the Mac's store.
fn apply(store: &Store, id: &str, changes: &BTreeMap<String, Value>, merged: &Record) -> Result<()> {
    let s = |k: &str| merged.fields.get(k).and_then(|x| x.v.as_str()).unwrap_or("").to_string();
    if merged.fields.get("deleted").and_then(|x| x.v.as_bool()) == Some(true) {
        if store.get_meeting(id)?.is_some() {
            store.delete_meeting(id)?;
        }
        return Ok(());
    }
    if store.get_meeting(id)?.is_none() {
        let created = chrono::DateTime::parse_from_rfc3339(&s("created_at")).map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now());
        let m = milkrice_core::store::Meeting {
            id: id.into(),
            title: s("title"),
            created_at: created,
            started_at: Some(created),
            ended_at: Some(created),
            duration_ms: merged.fields.get("duration_ms").and_then(|x| x.v.as_u64()).unwrap_or(0),
            notes: s("notes"),
            enhanced: s("enhanced"),
            template: "auto".into(),
            status: "done".into(),
            ..Default::default()
        };
        store.create_meeting(&m)?;
        store.set_field(id, "enhanced_by", &s("enhanced_by"))?;
        store.set_field(id, "stt_model", &s("transcribed_by"))?;
        replace_transcript(id, merged.fields.get("transcript").map(|x| &x.v))?;
        return Ok(());
    }
    for (k, v) in changes {
        match k.as_str() {
            "title" | "notes" | "enhanced" | "enhanced_by" => store.set_field(id, k, v.as_str().unwrap_or(""))?,
            "transcribed_by" => store.set_field(id, "stt_model", v.as_str().unwrap_or(""))?,
            "transcript" => replace_transcript(id, Some(v))?,
            "duration_ms" => {
                db_conn()?.execute("UPDATE meetings SET duration_ms=?1 WHERE id=?2", rusqlite::params![v.as_i64().unwrap_or(0), id])?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn replace_transcript(id: &str, v: Option<&Value>) -> Result<()> {
    let mut c = db_conn()?;
    let tx = c.transaction()?;
    tx.execute("DELETE FROM segments WHERE meeting_id=?1", [id])?;
    for row in v.and_then(|v| v.as_array()).into_iter().flatten() {
        let Some(r) = row.as_array() else { continue };
        tx.execute(
            "INSERT INTO segments (meeting_id,start_ms,end_ms,source,speaker,text) VALUES (?1,?2,?3,'mic',?4,?5)",
            rusqlite::params![
                id,
                r.first().and_then(|x| x.as_i64()).unwrap_or(0),
                r.get(1).and_then(|x| x.as_i64()).unwrap_or(0),
                r.get(2).and_then(|x| x.as_str()).unwrap_or("Speaker"),
                r.get(3).and_then(|x| x.as_str()).unwrap_or("")
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

#[derive(Debug, Default, Serialize)]
pub struct PassReport {
    /// Records this Mac wrote to the folder.
    pub exported: usize,
    /// Meetings changed (or created, or deleted) on this Mac from another device.
    pub imported: Vec<String>,
    /// Meetings an iPhone asked this Mac to write notes for.
    pub requests: Vec<String>,
    /// Files that couldn't be read (another vault, a damaged file…).
    pub problems: Vec<String>,
}

/// One sync pass: export this Mac's meetings, import the other devices'.
pub fn sync_pass(cfg: &Config, vault: &Vault, store: &Store) -> Result<PassReport> {
    let mut st: SyncState = load("state.json");
    if st.clock.device.is_empty() {
        st.clock = Clock::new(&cfg.device_id);
    }
    let (remote, problems) = vault.read_records(&cfg.device_id)?;
    let mut ids: Vec<String> = store
        .list_meetings(None, 100_000)?
        .into_iter()
        .filter(|m| m.status != "recording")
        .map(|m| m.id)
        .collect();
    ids.extend(remote.keys().cloned());
    ids.extend(st.meetings.keys().cloned());
    ids.sort();
    ids.dedup();

    let mut report = PassReport { problems, ..Default::default() };
    for id in ids {
        let known = st.meetings.get(&id).cloned();
        let here_before = store.get_meeting(&id)?.is_some();
        let local = match local_fields(store, &id, known.as_ref(), st.origins.get(&id).map(|s| s.as_str()), st.modes.get(&id).map(|s| s.as_str()))? {
            Some(f) => f,
            // Known before but gone now: deleted on this Mac (unless it was already a tombstone).
            None if known.as_ref().map(|k| k.contains_key("title")).unwrap_or(false)
                && !remote.get(&id).map(|rs| rs.iter().any(|r| r.fields.get("deleted").map(|d| d.v == json!(true)).unwrap_or(false))).unwrap_or(false)
                && !tombstoned(known.as_ref()) =>
            {
                BTreeMap::from([("deleted".to_string(), json!(true))])
            }
            None => BTreeMap::new(),
        };
        let mut state = known.unwrap_or_default();
        let remotes = remote.get(&id).cloned().unwrap_or_default();
        let out = merge::merge(&id, &local, &mut state, &remotes, &mut st.clock, wall_ms());
        let view = merged_view(&out.record, &remotes, &state);
        if let Some(o) = view.fields.get("origin").and_then(|o| o.v.as_str()) {
            st.origins.insert(id.clone(), o.to_string());
        } else if !here_before && !out.apply.is_empty() {
            // Created here from another device's record: it wasn't recorded on this Mac.
            st.origins.entry(id.clone()).or_default();
        }
        if let Some(m) = view.fields.get("mode").and_then(|o| o.v.as_str()) {
            st.modes.insert(id.clone(), m.to_string());
        }
        if !out.apply.is_empty() {
            apply(store, &id, &out.apply, &view)?;
            report.imported.push(id.clone());
        }
        let request = remotes.iter().filter_map(|r| r.fields.get("notes_request")).max_by(|a, b| a.hlc.cmp(&b.hlc)).map(|s| s.v.clone());
        if merge::notes_requested(&state, request.as_ref()) && store.get_meeting(&id)?.is_some() {
            report.requests.push(id.clone());
        }
        if !out.record.fields.is_empty() {
            let h = hex::encode(&<sha2::Sha256 as sha2::Digest>::digest(serde_json::to_vec(&out.record)?)[..8]);
            if st.written.get(&id) != Some(&h) || !vault.own_record_exists(&id, &cfg.device_id) {
                vault.write_record(&out.record)?;
                st.written.insert(id.clone(), h);
                report.exported += 1;
            }
        }
        st.meetings.insert(id, state);
    }
    if Utc::now().timestamp() - st.device_written_at > 600 {
        vault.write_device(&cfg.device_id, &DeviceCard { name: cfg.device_name.clone(), kind: "mac".into(), last_seen: now_rfc3339() })?;
        st.device_written_at = Utc::now().timestamp();
    }
    save("state.json", &st)?;
    Ok(report)
}

fn tombstoned(state: Option<&MeetingState>) -> bool {
    state.map(|s| s.get("deleted").map(|d| d.hash == merge::value_hash(&json!(true))).unwrap_or(false)).unwrap_or(false)
}

/// Every field's winning value: our record plus newer remote values (for creating a meeting).
fn merged_view(own: &Record, remotes: &[Record], state: &MeetingState) -> Record {
    let mut r = own.clone();
    for rem in remotes {
        for (k, s) in &rem.fields {
            if state.get(k).map(|st| st.hlc == s.hlc).unwrap_or(false) {
                r.fields.insert(k.clone(), s.clone());
            }
        }
    }
    r
}

/// Meetings first recorded on an iPhone.
pub fn iphone_meetings() -> std::collections::BTreeSet<String> {
    let st: SyncState = load("state.json");
    st.origins
        .iter()
        .filter(|(id, o)| *o == "iphone" || (o.is_empty() && st.modes.get(*id).map(|m| m != "mac").unwrap_or(false)))
        .map(|(id, _)| id.clone())
        .collect()
}

// ---------- status

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub last_seen: String,
    pub this_mac: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub folder: PathBuf,
    pub in_icloud: bool,
    pub device_name: String,
    pub devices: Vec<Device>,
    pub meetings_from_others: usize,
    pub unreadable: usize,
}

pub fn status(cfg: &Config, vault: &Vault) -> Result<Status> {
    let (recs, problems) = vault.read_records(&cfg.device_id)?;
    let mut devices: Vec<Device> = vault
        .devices()
        .into_iter()
        .map(|(id, c)| Device { this_mac: id == cfg.device_id, id, name: c.name, kind: c.kind, last_seen: c.last_seen })
        .collect();
    devices.sort_by(|a, b| b.this_mac.cmp(&a.this_mac).then(b.last_seen.cmp(&a.last_seen)));
    Ok(Status {
        in_icloud: cfg.folder.starts_with(icloud_drive()),
        folder: cfg.folder.clone(),
        device_name: cfg.device_name.clone(),
        devices,
        meetings_from_others: recs.len(),
        unreadable: problems.len(),
    })
}

/// True when `folder` already holds a Milkrice vault (another Mac set it up).
pub fn folder_has_vault(folder: &Path) -> bool {
    crate::folder::read_vault_file(folder).ok().flatten().is_some()
}
