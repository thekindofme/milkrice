//! The sync folder on disk.
//!
//! Anyone who can write to the folder (the storage provider, another app, a stolen cloud login)
//! is treated as hostile: file names are never trusted, a record must sit at the path its own
//! (authenticated) contents name, nothing is followed through a symlink, and reads are capped.
use crate::crypto::{self, Key, VaultId};
use crate::merge::{valid_device_id, Record};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The vault format this build reads and writes (`vault.json` → `format`).
pub const FORMAT: u32 = 1;
/// Largest sealed file this build reads (a long meeting's record is a few hundred KB).
pub const MAX_FILE_BYTES: u64 = 64 << 20;

/// Is `id` safe as a meeting or device id, i.e. as one path component?
/// Same rule as device ids in HLCs: 1–128 of `[A-Za-z0-9._-]`, not `.`/`..`.
pub fn valid_id(id: &str) -> bool {
    valid_device_id(id)
}

/// Records from other devices, keyed by meeting id.
pub type RecordsByMeeting = BTreeMap<String, Vec<Record>>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultFile {
    pub format: u32,
    pub vault_id: String,
    pub key_id: u32,
    pub kcv: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCard {
    pub name: String,
    pub kind: String,
    /// RFC 3339. Absent in the card a joining phone sends while pairing.
    #[serde(default)]
    pub last_seen: String,
}

#[derive(Clone)]
pub struct Vault {
    pub root: PathBuf,
    pub vault_id: VaultId,
    pub key_id: u32,
    pub key: Key,
}

/// Create the directories from `root` down to `dir`, refusing to go through a symlink
/// (a planted `m/<id>` → `~/Library/...` link would otherwise redirect our writes).
fn safe_dirs(root: &Path, dir: &Path) -> Result<()> {
    let rel = dir.strip_prefix(root).context("path outside the sync folder")?;
    let mut cur = root.to_path_buf();
    for c in rel.components() {
        let std::path::Component::Normal(c) = c else { bail!("unexpected path component in {}", dir.display()) };
        cur.push(c);
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => bail!("{} is a symlink; refusing to write through it", cur.display()),
            Ok(m) if !m.is_dir() => bail!("{} is not a folder", cur.display()),
            Ok(_) => {}
            Err(_) => std::fs::create_dir(&cur).or_else(|e| if cur.is_dir() { Ok(()) } else { Err(e) })?,
        }
    }
    Ok(())
}

/// Write `bytes` to `path` (under `root`) via a fresh, randomly named temp file opened with
/// `create_new` (O_EXCL: never follows or reuses a planted file or symlink), then rename.
fn write_atomic(root: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("no parent folder")?;
    safe_dirs(root, dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).context("bad file name")?;
    let tmp = dir.join(format!(".{name}.{}.tmp", hex::encode(crypto::random_bytes::<8>())));
    let written = (|| -> Result<()> {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        Ok(())
    })();
    if let Err(e) = written.and_then(|_| Ok(std::fs::rename(&tmp, path)?)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Read a regular file (not a symlink, not a device), at most `max` bytes.
pub fn read_capped(path: &Path, max: u64) -> Result<Vec<u8>> {
    let m = std::fs::symlink_metadata(path)?;
    ensure!(m.file_type().is_file(), "{} is not a regular file", path.display());
    ensure!(m.len() <= max, "{} is too large ({} bytes)", path.display(), m.len());
    let mut buf = Vec::with_capacity(m.len() as usize);
    std::fs::File::open(path)?.take(max + 1).read_to_end(&mut buf)?;
    ensure!(buf.len() as u64 <= max, "{} is too large", path.display());
    Ok(buf)
}

pub fn read_vault_file(root: &Path) -> Result<Option<VaultFile>> {
    let p = root.join("vault.json");
    if !p.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&read_capped(&p, 64 << 10)?).context("vault.json is unreadable")?))
}

impl Vault {
    /// Create a brand-new vault in an empty folder.
    pub fn create(root: &Path) -> Result<Self> {
        if read_vault_file(root)?.is_some() {
            bail!("{} already holds a Milkrice vault; pair with it instead", root.display());
        }
        let v = Self { root: root.into(), vault_id: crypto::random_bytes(), key_id: 1, key: crypto::random_bytes() };
        v.write_vault_file()?;
        Ok(v)
    }

    pub fn write_vault_file(&self) -> Result<()> {
        let f = VaultFile { format: FORMAT, vault_id: crypto::b64(&self.vault_id), key_id: self.key_id, kcv: crypto::kcv(&self.key) };
        write_atomic(&self.root, &self.root.join("vault.json"), &serde_json::to_vec_pretty(&f)?)
    }

    /// Open an existing vault with a key from the keychain, checking it really is this vault's key.
    ///
    /// `vault.json` is not authenticated, so prefer [`Vault::open_expecting`] with the vault id
    /// this device was paired with: otherwise whoever edits `vault.json` chooses the vault id
    /// this device seals new files under.
    pub fn open(root: &Path, key: Key) -> Result<Self> {
        let f = read_vault_file(root)?.context("no Milkrice vault in that folder")?;
        ensure!(f.format == FORMAT, "this sync folder uses format {}; this version of Milkrice reads format {FORMAT}. Update Milkrice.", f.format);
        if crypto::kcv(&key) != f.kcv {
            bail!("this device's key doesn't match the vault (it was reset or re-keyed); pair again");
        }
        let vault_id: VaultId = crypto::unb64(&f.vault_id)?.try_into().ok().context("bad vault id")?;
        Ok(Self { root: root.into(), vault_id, key_id: f.key_id, key })
    }

    /// [`Vault::open`], also checking `vault.json` names the vault (base64url id) this device
    /// was paired with.
    pub fn open_expecting(root: &Path, key: Key, vault_id_b64: &str) -> Result<Self> {
        let expected: VaultId = crypto::unb64(vault_id_b64)?.try_into().ok().context("bad vault id in config")?;
        let v = Self::open(root, key)?;
        ensure!(v.vault_id == expected, "the sync folder now holds a different vault than the one this device was paired with; pair again");
        Ok(v)
    }

    fn record_path(&self, meeting: &str, device: &str) -> Result<PathBuf> {
        ensure!(valid_id(meeting), "bad meeting id {meeting:?}");
        ensure!(valid_id(device), "bad device id {device:?}");
        Ok(self.root.join("m").join(meeting).join(format!("{device}.rec")))
    }

    pub fn write_record(&self, r: &Record) -> Result<()> {
        let path = self.record_path(&r.id, &r.device)?;
        let sealed = crypto::seal(&self.key, &self.vault_id, self.key_id, &serde_json::to_vec(r)?)?;
        write_atomic(&self.root, &path, &sealed)
    }

    /// Open one record file found at `m/<meeting>/<device>.rec`, checking its sealed contents
    /// name that same meeting and device (so a file can't be copied to another name).
    pub fn read_record_at(&self, meeting: &str, device: &str, bytes: &[u8]) -> Result<Record> {
        let r: Record = serde_json::from_slice(&crypto::open(&self.key, &self.vault_id, bytes)?)?;
        ensure!(r.id == meeting, "record for meeting {:?} found under {meeting:?}", r.id);
        ensure!(r.device == device, "record from device {:?} found under {device:?}", r.device);
        Ok(r)
    }

    /// Every other device's records, grouped by meeting. Unreadable files are reported, not fatal.
    pub fn read_records(&self, own_device: &str) -> Result<(RecordsByMeeting, Vec<String>)> {
        let mut out: BTreeMap<String, Vec<Record>> = BTreeMap::new();
        let mut problems = Vec::new();
        let dir = self.root.join("m");
        let Ok(meetings) = std::fs::read_dir(&dir) else { return Ok((out, problems)) };
        for m in meetings.flatten() {
            let meeting = m.file_name().to_str().unwrap_or("").to_string();
            if !valid_id(&meeting) || m.file_type().map(|t| t.is_symlink()).unwrap_or(true) {
                continue;
            }
            let Ok(files) = std::fs::read_dir(m.path()) else { continue };
            for f in files.flatten() {
                let p = f.path();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let Some(device) = name.strip_suffix(".rec") else { continue };
                if device == own_device {
                    continue;
                }
                if !valid_id(device) {
                    problems.push(format!("{}: bad file name", p.display()));
                    continue;
                }
                let parsed = read_capped(&p, MAX_FILE_BYTES).and_then(|b| self.read_record_at(&meeting, device, &b));
                match parsed {
                    Ok(r) => out.entry(r.id.clone()).or_default().push(r),
                    Err(e) => problems.push(format!("{}: {e}", p.display())),
                }
            }
        }
        Ok((out, problems))
    }

    /// This device's own records (to recover meetings it knows but has lost locally).
    pub fn own_record_exists(&self, meeting: &str, device: &str) -> bool {
        self.record_path(meeting, device).map(|p| p.exists()).unwrap_or(false)
    }

    pub fn write_device(&self, device: &str, card: &DeviceCard) -> Result<()> {
        ensure!(valid_id(device), "bad device id {device:?}");
        let sealed = crypto::seal(&self.key, &self.vault_id, self.key_id, &serde_json::to_vec(card)?)?;
        write_atomic(&self.root, &self.root.join("devices").join(format!("{device}.dev")), &sealed)
    }

    pub fn devices(&self) -> Vec<(String, DeviceCard)> {
        let Ok(rd) = std::fs::read_dir(self.root.join("devices")) else { return vec![] };
        rd.flatten()
            .filter_map(|e| {
                let p = e.path();
                let id = p.file_name()?.to_str()?.strip_suffix(".dev")?.to_string();
                if !valid_id(&id) {
                    return None;
                }
                let plain = crypto::open(&self.key, &self.vault_id, &read_capped(&p, 1 << 20).ok()?).ok()?;
                Some((id, serde_json::from_slice(&plain).ok()?))
            })
            .collect()
    }

    pub fn pair_dir(&self) -> PathBuf {
        self.root.join("pair")
    }

    /// Write an unsealed file (pairing messages) at a path relative to the folder.
    pub fn write_plain(&self, rel: &str, bytes: &[u8]) -> Result<()> {
        ensure!(Path::new(rel).components().all(|c| matches!(c, std::path::Component::Normal(_))), "bad path {rel:?}");
        write_atomic(&self.root, &self.root.join(rel), bytes)
    }
}
