#![allow(dead_code)]
use std::path::PathBuf;

/// A fresh empty folder under the system temp dir, removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("milkrice-sync-{tag}-{}", hex::encode(milkrice_sync::crypto::random_bytes::<8>())));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Deterministic bytes for fixtures: `seed, seed+7, seed+14, …`.
pub fn h<const N: usize>(seed: u8) -> [u8; N] {
    std::array::from_fn(|i| seed.wrapping_add((i as u8).wrapping_mul(7)))
}
