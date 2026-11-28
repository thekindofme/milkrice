//! Pairing a new device to the vault, through the sync folder itself.
//!
//! 1. The Mac makes an ephemeral X25519 key and shows a QR code:
//!    `milkrice-pair:1:<vault_id>:<session>:<mac_pub>` (base64url).
//! 2. The phone scans it, makes its own ephemeral key and writes `pair/<session>.req`
//!    (its public key, plus its device card sealed with the pairing key).
//! 3. Both derive the pairing key and a 6-digit code from the X25519 secret, with the vault id,
//!    session and both public keys as HKDF salt. Both screens show the code; a swapped QR or a
//!    swapped request gives different codes.
//! 4. Only after the person confirms the codes match on the Mac does the Mac write
//!    `pair/<session>.resp`: `key_id (u32 BE) || vault_key` sealed with the pairing key.
use crate::crypto::{self, Key, VaultId, NONCE_LEN};
use anyhow::{bail, ensure, Context, Result};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

pub const QR_PREFIX: &str = "milkrice-pair:1:";
pub type Session = [u8; 8];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub vault_id: VaultId,
    pub session: Session,
    pub mac_pub: [u8; 32],
}

impl Invite {
    pub fn to_qr(&self) -> String {
        format!("{QR_PREFIX}{}:{}:{}", crypto::b64(&self.vault_id), crypto::b64(&self.session), crypto::b64(&self.mac_pub))
    }

    pub fn parse(s: &str) -> Result<Self> {
        let rest = s.trim().strip_prefix(QR_PREFIX).context("not a Milkrice pairing code")?;
        let parts: Vec<&str> = rest.split(':').collect();
        ensure!(parts.len() == 3, "pairing code has the wrong shape");
        Ok(Self {
            vault_id: crypto::unb64(parts[0])?.try_into().ok().context("bad vault id")?,
            session: crypto::unb64(parts[1])?.try_into().ok().context("bad session")?,
            mac_pub: crypto::unb64(parts[2])?.try_into().ok().context("bad public key")?,
        })
    }

    pub fn session_hex(&self) -> String {
        hex::encode(self.session)
    }
}

/// What both sides derive once they hold each other's public keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derived {
    pub pair_key: Key,
    pub code: String,
}

pub fn derive(own_secret: &[u8; 32], peer_pub: &[u8; 32], invite: &Invite, phone_pub: &[u8; 32]) -> Result<Derived> {
    let shared = StaticSecret::from(*own_secret).diffie_hellman(&PublicKey::from(*peer_pub));
    if shared.as_bytes().iter().all(|b| *b == 0) {
        bail!("invalid public key");
    }
    let salt = [invite.vault_id.as_slice(), &invite.session, &invite.mac_pub, phone_pub].concat();
    let hk = Hkdf::<Sha256>::new(Some(&salt), shared.as_bytes());
    let mut pair_key = [0u8; 32];
    hk.expand(b"milkrice-pair-key-v1", &mut pair_key).unwrap();
    let mut c = [0u8; 4];
    hk.expand(b"milkrice-pair-code-v1", &mut c).unwrap();
    Ok(Derived { pair_key, code: format!("{:06}", u32::from_be_bytes(c) % 1_000_000) })
}

pub fn public_key(secret: &[u8; 32]) -> [u8; 32] {
    PublicKey::from(&StaticSecret::from(*secret)).to_bytes()
}

/// `pair/<session>.req`, written by the new device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub v: u32,
    /// base64url X25519 public key of the joining device.
    #[serde(rename = "pub")]
    pub public: String,
    /// base64url box: the joining device's card (JSON), sealed with the pairing key.
    pub card: String,
}

/// `pair/<session>.resp`, written by the Mac after the person confirms the code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub v: u32,
    /// base64url box: key_id (u32 BE) || vault key, sealed with the pairing key, aad = session.
    pub grant: String,
}

/// The joining device's half of step 2: its request, with `card` (JSON) sealed with the pairing key.
pub fn make_request(d: &Derived, invite: &Invite, phone_pub: &[u8; 32], card: &[u8], nonce: &[u8; NONCE_LEN]) -> Result<Request> {
    Ok(Request { v: 1, public: crypto::b64(phone_pub), card: crypto::b64(&crypto::box_seal(&d.pair_key, &invite.session, nonce, card)?) })
}

/// The Mac's half of step 3: derive from a request and open the card it carries.
pub fn open_request(mac_secret: &[u8; 32], invite: &Invite, r: &Request) -> Result<(Derived, Vec<u8>)> {
    ensure!(r.v == 1, "unsupported pairing request version {}", r.v);
    let phone_pub: [u8; 32] = crypto::unb64(&r.public)?.try_into().ok().context("bad public key in pairing request")?;
    let d = derive(mac_secret, &phone_pub, invite, &phone_pub)?;
    let card = crypto::box_open(&d.pair_key, &invite.session, &crypto::unb64(&r.card)?)?;
    Ok((d, card))
}

pub fn seal_grant(d: &Derived, invite: &Invite, key_id: u32, vault_key: &Key, nonce: &[u8; NONCE_LEN]) -> Result<Response> {
    let msg = [key_id.to_be_bytes().as_slice(), vault_key].concat();
    Ok(Response { v: 1, grant: crypto::b64(&crypto::box_seal(&d.pair_key, &invite.session, nonce, &msg)?) })
}

pub fn open_grant(d: &Derived, invite: &Invite, r: &Response) -> Result<(u32, Key)> {
    ensure!(r.v == 1, "unsupported pairing response version {}", r.v);
    let msg = crypto::box_open(&d.pair_key, &invite.session, &crypto::unb64(&r.grant)?)?;
    ensure!(msg.len() == 36, "grant has the wrong length");
    Ok((u32::from_be_bytes(msg[..4].try_into().unwrap()), msg[4..].try_into().unwrap()))
}
