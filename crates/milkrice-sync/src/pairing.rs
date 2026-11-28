//! The pairing hand-shake as two small state machines over the sync folder: [`Pairing`] (the Mac,
//! which holds the vault key) and [`Joiner`] (the new device; a reference for the iPhone's Swift
//! code and for tests). Message formats are in [`crate::pair`] and `docs/SYNC-FORMAT.md`.
//!
//! ```text
//!  Mac (Pairing)                       folder                        Phone (Joiner)
//!  start: ephemeral key, QR  ─────────── QR on screen ───────────►  start(qr): ephemeral key
//!  poll: Waiting                ◄── pair/<session>.req ──────────  writes request, shows code
//!  poll: Joining{name, code}                                         (person compares codes)
//!  accept ───────────────────────► pair/<session>.resp ─────────►  poll: Some(vault)
//!  cancel / expiry: nothing shared
//! ```
use crate::crypto::{self, NONCE_LEN};
use crate::folder::{read_capped, read_vault_file, DeviceCard, Vault, FORMAT};
use crate::pair::{self, Derived, Invite, Request, Response};
use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// How long a QR code stays good. A photo of the screen is useless after this.
pub const PAIRING_TTL: Duration = Duration::from_secs(10 * 60);
/// Pairing files older than this are swept when a new pairing starts.
pub const STALE_PAIR_FILES: Duration = Duration::from_secs(60 * 60);
const MAX_PAIR_FILE: u64 = 16 << 10;

/// One pairing attempt on the Mac: show `qr`, `poll` until a phone answers, compare codes, then
/// `accept` (or `cancel`). Refuses to do anything once [`PAIRING_TTL`] has passed.
pub struct Pairing {
    secret: [u8; 32],
    invite: Invite,
    pub qr: String,
    req_path: PathBuf,
    resp_path: PathBuf,
    deadline: Instant,
    pending: Option<(Derived, DeviceCard)>,
}

impl Drop for Pairing {
    fn drop(&mut self) {
        self.secret = [0; 32];
    }
}

/// A phone that scanned the code and is waiting for the person to compare codes.
#[derive(Debug, Clone, Serialize)]
pub struct Joining {
    pub name: String,
    pub code: String,
}

/// A device name as the other side chose it, made safe to show: no control or bidi-override
/// characters (which could make "Evil" render as something else), at most 64 characters.
pub fn display_name(s: &str) -> String {
    let bidi = |c: char| matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    let n: String = s.chars().filter(|c| !c.is_control() && !bidi(*c)).take(64).collect();
    let n = n.trim();
    if n.is_empty() {
        "Unnamed device".into()
    } else {
        n.into()
    }
}

fn sweep_stale(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .map(|age| age > STALE_PAIR_FILES)
            .unwrap_or(false);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

impl Pairing {
    pub fn start(vault: &Vault) -> Self {
        Self::start_with_ttl(vault, PAIRING_TTL)
    }

    pub fn start_with_ttl(vault: &Vault, ttl: Duration) -> Self {
        sweep_stale(&vault.pair_dir());
        let secret: [u8; 32] = crypto::random_bytes();
        let invite = Invite { vault_id: vault.vault_id, session: crypto::random_bytes(), mac_pub: pair::public_key(&secret) };
        let qr = invite.to_qr();
        let req_path = vault.pair_dir().join(format!("{}.req", invite.session_hex()));
        let resp_path = vault.pair_dir().join(format!("{}.resp", invite.session_hex()));
        Self { secret, invite, qr, req_path, resp_path, deadline: Instant::now() + ttl, pending: None }
    }

    pub fn invite(&self) -> &Invite {
        &self.invite
    }

    pub fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }

    /// The phone that answered, once one has. The first request that authenticates is kept;
    /// later ones (another phone, someone who photographed the QR) are not looked at.
    pub fn poll(&mut self) -> Result<Option<Joining>> {
        if let Some((d, card)) = &self.pending {
            return Ok(Some(Joining { name: display_name(&card.name), code: d.code.clone() }));
        }
        if self.expired() {
            bail!("this pairing code has expired; start again");
        }
        let Ok(b) = read_capped(&self.req_path, MAX_PAIR_FILE) else { return Ok(None) };
        let Ok(req) = serde_json::from_slice::<Request>(&b) else { return Ok(None) }; // still being written
        let (d, card) = pair::open_request(&self.secret, &self.invite, &req)?;
        let card: DeviceCard = serde_json::from_slice(&card).context("bad device card in pairing request")?;
        let j = Joining { name: display_name(&card.name), code: d.code.clone() };
        self.pending = Some((d, card));
        Ok(Some(j))
    }

    /// The person confirmed the codes match: hand the vault key to the phone.
    pub fn accept(&self, vault: &Vault) -> Result<String> {
        let (d, card) = self.pending.as_ref().context("no iPhone has answered yet")?;
        ensure!(!self.expired(), "this pairing code has expired; start again");
        ensure!(vault.vault_id == self.invite.vault_id, "pairing was started for another vault");
        let resp = pair::seal_grant(d, &self.invite, vault.key_id, &vault.key, &crypto::random_bytes())?;
        vault.write_plain(&format!("pair/{}.resp", self.invite.session_hex()), &serde_json::to_vec(&resp)?)?;
        let _ = std::fs::remove_file(&self.req_path);
        Ok(display_name(&card.name))
    }

    /// Give up (or the codes didn't match): nothing is shared.
    pub fn cancel(&self) {
        let _ = std::fs::remove_file(&self.req_path);
        if self.pending.is_none() {
            let _ = std::fs::remove_file(&self.resp_path);
        }
    }
}

/// The joining device's side: scan, write the request, show the code, wait for the key.
pub struct Joiner {
    secret: [u8; 32],
    invite: Invite,
    derived: Derived,
    root: PathBuf,
    resp_path: PathBuf,
}

impl Drop for Joiner {
    fn drop(&mut self) {
        self.secret = [0; 32];
    }
}

impl Joiner {
    /// Scan `qr`, write `pair/<session>.req` with `card` into the folder at `root`.
    pub fn start(root: &Path, qr: &str, card: &DeviceCard) -> Result<Self> {
        Self::start_with(root, qr, card, crypto::random_bytes(), &crypto::random_bytes())
    }

    /// [`Joiner::start`] with a chosen secret and nonce (test vectors).
    pub fn start_with(root: &Path, qr: &str, card: &DeviceCard, secret: [u8; 32], nonce: &[u8; NONCE_LEN]) -> Result<Self> {
        let invite = Invite::parse(qr)?;
        if let Some(f) = read_vault_file(root)? {
            ensure!(f.format == FORMAT, "the sync folder uses format {}", f.format);
            ensure!(crypto::unb64(&f.vault_id)? == invite.vault_id, "that QR code is for a different sync folder");
        }
        let public = pair::public_key(&secret);
        let derived = pair::derive(&secret, &invite.mac_pub, &invite, &public)?;
        let card = serde_json::to_vec(&serde_json::json!({"name": card.name, "kind": card.kind}))?;
        let req = pair::make_request(&derived, &invite, &public, &card, nonce)?;
        let probe = Vault { root: root.into(), vault_id: invite.vault_id, key_id: 0, key: [0; 32] };
        probe.write_plain(&format!("pair/{}.req", invite.session_hex()), &serde_json::to_vec(&req)?)?;
        let resp_path = probe.pair_dir().join(format!("{}.resp", invite.session_hex()));
        Ok(Self { secret, invite, derived, root: root.into(), resp_path })
    }

    /// The 6-digit code to show next to the Mac's.
    pub fn code(&self) -> &str {
        &self.derived.code
    }

    /// The vault, once the Mac granted it. The key is checked against `vault.json` (KCV) and the
    /// vault id against the QR code before it is returned.
    pub fn poll(&self) -> Result<Option<Vault>> {
        let Ok(b) = read_capped(&self.resp_path, MAX_PAIR_FILE) else { return Ok(None) };
        let Ok(resp) = serde_json::from_slice::<Response>(&b) else { return Ok(None) };
        let (key_id, key) = pair::open_grant(&self.derived, &self.invite, &resp)?;
        let v = Vault::open(&self.root, key)?;
        ensure!(v.vault_id == self.invite.vault_id, "the sync folder holds a different vault than the QR code named");
        let _ = std::fs::remove_file(&self.resp_path);
        Ok(Some(Vault { key_id, ..v }))
    }
}
