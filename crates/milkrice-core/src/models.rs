//! Downloading, unpacking and deleting local model files.
use crate::catalog::{self, ModelSpec};
use crate::paths;
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct ModelStatus {
    #[serde(flatten)]
    pub spec: ModelSpec,
    pub downloaded: bool,
    pub language_label: String,
}

pub fn local_path(spec: &ModelSpec) -> PathBuf {
    paths::models_dir().join(spec.local_name)
}

pub fn is_downloaded(spec: &ModelSpec) -> bool {
    let p = local_path(spec);
    if !spec.files.is_empty() {
        // Every pinned file present with the right size (hashes were checked at download).
        return if spec.is_dir() {
            p.is_dir() && spec.files.iter().all(|f| fs::metadata(p.join(f.name)).map(|m| m.len() == f.size).unwrap_or(false))
        } else {
            fs::metadata(&p).map(|m| m.len() == spec.files[0].size).unwrap_or(false)
        };
    }
    false
}

pub fn statuses() -> Vec<ModelStatus> {
    catalog::MODELS
        .iter()
        .map(|m| ModelStatus {
            spec: m.clone(),
            downloaded: is_downloaded(m),
            language_label: m.language_label(),
        })
        .collect()
}

/// Download one pinned file, hashing as it streams; the file is only kept if size and SHA-256
/// match. `progress(done)` reports bytes of this file.
fn download_verified(f: &crate::catalog::ModelFile, dest: &Path, cancel: &AtomicBool, mut progress: impl FnMut(u64)) -> Result<()> {
    use sha2::{Digest, Sha256};
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(None)
        // Model hosts redirect to CDNs; refuse any hop that isn't HTTPS.
        .https_only(true)
        .user_agent("Milkrice/0.1 (local meeting notes)")
        .build()?;
    let mut resp = client.get(f.url).send().with_context(|| format!("connecting for {}", f.name))?;
    if !resp.status().is_success() {
        bail!("{}: HTTP {}", f.name, resp.status());
    }
    let part = dest.with_extension("part");
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut out = std::io::BufWriter::with_capacity(1 << 20, fs::File::create(&part)?);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    let mut last = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(out);
            let _ = fs::remove_file(&part);
            bail!("cancelled");
        }
        let n = resp.read(&mut buf).context("reading download")?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        done += n as u64;
        if last.elapsed() > Duration::from_millis(100) {
            progress(done);
            last = Instant::now();
        }
    }
    out.flush()?;
    drop(out);
    let digest = format!("{:x}", hasher.finalize());
    if done != f.size || digest != f.sha256 {
        let _ = fs::remove_file(&part);
        bail!("{} failed verification (got {done} bytes, sha256 {}…); the source may have changed", f.name, &digest[..12]);
    }
    progress(done);
    fs::rename(&part, dest)?;
    Ok(())
}

/// Download every pinned file of a model into a staging folder, then move it into place.
fn download_files(spec: &ModelSpec, cancel: &AtomicBool, mut progress: impl FnMut(u64, u64)) -> Result<()> {
    let total: u64 = spec.files.iter().map(|f| f.size).sum();
    let mut base = 0u64;
    if !spec.is_dir() {
        let f = &spec.files[0];
        download_verified(f, &local_path(spec), cancel, |d| progress(d, total))?;
        return Ok(());
    }
    let models = paths::models_dir();
    let staging = models.join(format!(".{}.download", spec.local_name));
    fs::create_dir_all(&staging)?;
    for f in spec.files {
        let dest = staging.join(f.name);
        // Resume across app restarts: keep files that already verified.
        let have = fs::metadata(&dest).map(|m| m.len() == f.size).unwrap_or(false);
        if !have {
            let b = base;
            download_verified(f, &dest, cancel, |d| progress(b + d, total))?;
        }
        base += f.size;
        progress(base, total);
    }
    let dest = local_path(spec);
    let _ = fs::remove_dir_all(&dest);
    fs::rename(&staging, &dest)?;
    Ok(())
}

/// Download and install a catalog model.
pub fn download(spec: &ModelSpec, cancel: &AtomicBool, progress: impl FnMut(u64, u64)) -> Result<()> {
    if is_downloaded(spec) {
        return Ok(());
    }
    if !spec.files.is_empty() {
        return download_files(spec, cancel, progress);
    }
    Err(anyhow!("model {} has no download files", spec.id))
}

pub fn delete(spec: &ModelSpec) -> Result<()> {
    let p = local_path(spec);
    if p.is_dir() {
        fs::remove_dir_all(&p)?;
    } else if p.exists() {
        fs::remove_file(&p)?;
    }
    Ok(())
}

/// Path of the speaker-embedding model, downloading it on first use.
pub fn ensure_speaker_model(cancel: &AtomicBool, progress: impl FnMut(u64, u64)) -> Result<PathBuf> {
    let p = paths::models_dir().join(crate::diarize::EMBED_MODEL_FILE);
    if !p.is_file() {
        let f = crate::catalog::ModelFile {
            url: crate::diarize::EMBED_MODEL_URL,
            name: crate::diarize::EMBED_MODEL_FILE,
            size: 26_530_550,
            sha256: "e9848563da86f263117134dfd7ad63c92355b37de492b55e325400c9d9c39012",
        };
        let mut progress = progress;
        download_verified(&f, &p, cancel, |d| progress(d, f.size)).map_err(|e| anyhow!("speaker model: {e}"))?;
    }
    Ok(p)
}

pub fn speaker_model_present() -> bool {
    paths::models_dir().join(crate::diarize::EMBED_MODEL_FILE).is_file()
}
