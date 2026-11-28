//! Milkrice sync: end-to-end encrypted iPhone <-> Mac sync through a folder both devices can see
//! (iCloud Drive, Google Drive, Dropbox, Syncthing…). There is no Milkrice server; the folder only
//! ever holds sealed files with random names. Design: `docs/SYNC.md` in the iOS repo.
//!
//! ```text
//! <folder>/vault.json                     public: format, vault id, key id, key check value
//! <folder>/devices/<device>.dev           sealed device card
//! <folder>/m/<meeting id>/<device>.rec    sealed record: that device's view of the meeting
//! <folder>/pair/<session>.req|.resp       pairing hand-shake (short-lived)
//! ```
//! Each device only writes files named after itself, so cloud providers never create
//! "conflicted copy" files.
pub mod crypto;
#[cfg(feature = "desktop")]
pub mod desktop;
pub mod folder;
pub mod merge;
pub mod pair;
pub mod pairing;
