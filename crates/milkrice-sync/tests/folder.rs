//! The sync folder treated as hostile: whoever can write to it must not be able to redirect
//! writes, relabel records, or make this device read unbounded data.
mod common;
use common::TempDir;
use milkrice_sync::folder::{self, DeviceCard, Vault};
use milkrice_sync::merge::{Record, Stamped};
use serde_json::json;
use std::collections::BTreeMap;

fn record(id: &str, device: &str) -> Record {
    let fields = BTreeMap::from([("title".to_string(), Stamped { v: json!("Hi"), hlc: format!("1763683200000-0000-{device}") })]);
    Record { v: 1, id: id.into(), device: device.into(), fields }
}

#[test]
fn ids_that_are_not_one_path_component_are_refused() {
    let d = TempDir::new("ids");
    let v = Vault::create(&d.0).unwrap();
    for bad in ["../../escape", "..", ".", "a/b", "", "x\0y", &"a".repeat(129)] {
        assert!(v.write_record(&record(bad, "dev1")).is_err(), "meeting id {bad:?}");
        assert!(v.write_record(&record("m1", bad)).is_err(), "device id {bad:?}");
        assert!(v.write_device(bad, &DeviceCard { name: "x".into(), kind: "mac".into(), last_seen: String::new() }).is_err());
    }
    assert!(!d.0.parent().unwrap().join("escape").exists());
    v.write_record(&record("7f3c1d2e-0000-4000-8000-000000000001", "dev1")).unwrap();
}

#[test]
fn a_record_copied_to_another_name_is_rejected() {
    let d = TempDir::new("relabel");
    let v = Vault::create(&d.0).unwrap();
    v.write_record(&record("m1", "phone")).unwrap();
    let sealed = std::fs::read(d.0.join("m/m1/phone.rec")).unwrap();
    // Same meeting, another device's name; and another meeting.
    std::fs::write(d.0.join("m/m1/evil.rec"), &sealed).unwrap();
    std::fs::create_dir_all(d.0.join("m/m2")).unwrap();
    std::fs::write(d.0.join("m/m2/phone.rec"), &sealed).unwrap();
    // The Mac's own record copied under a stranger's name.
    v.write_record(&record("m1", "mac")).unwrap();
    std::fs::copy(d.0.join("m/m1/mac.rec"), d.0.join("m/m1/stranger.rec")).unwrap();

    let (recs, problems) = v.read_records("mac").unwrap();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs["m1"].len(), 1);
    assert_eq!(recs["m1"][0].device, "phone");
    assert_eq!(problems.len(), 3, "{problems:?}");
}

#[cfg(unix)]
#[test]
fn writes_never_follow_symlinks() {
    let d = TempDir::new("symlink");
    let outside = TempDir::new("outside");
    let v = Vault::create(&d.0).unwrap();
    // A planted meeting folder pointing outside the vault.
    std::fs::create_dir_all(d.0.join("m")).unwrap();
    std::os::unix::fs::symlink(&outside.0, d.0.join("m/m1")).unwrap();
    assert!(v.write_record(&record("m1", "mac")).is_err());
    assert_eq!(std::fs::read_dir(&outside.0).unwrap().count(), 0);
    // A planted symlink at the final name is replaced, not written through.
    let victim = outside.0.join("victim.txt");
    std::fs::write(&victim, b"precious").unwrap();
    std::fs::create_dir_all(d.0.join("m/m2")).unwrap();
    std::os::unix::fs::symlink(&victim, d.0.join("m/m2/mac.rec")).unwrap();
    v.write_record(&record("m2", "mac")).unwrap();
    assert_eq!(std::fs::read(&victim).unwrap(), b"precious");
    assert!(!std::fs::symlink_metadata(d.0.join("m/m2/mac.rec")).unwrap().file_type().is_symlink());
    // Reads don't follow symlinks either (think /dev/zero).
    std::os::unix::fs::symlink("/dev/zero", d.0.join("m/m2/phone.rec")).unwrap();
    let (recs, problems) = v.read_records("mac").unwrap();
    assert!(recs.is_empty());
    assert_eq!(problems.len(), 1);
}

#[test]
fn oversized_files_are_not_read() {
    let d = TempDir::new("big");
    let p = d.0.join("big.rec");
    std::fs::write(&p, vec![0u8; 2048]).unwrap();
    assert!(folder::read_capped(&p, 1024).is_err());
    assert_eq!(folder::read_capped(&p, 2048).unwrap().len(), 2048);
}

#[test]
fn vault_json_is_checked_against_what_the_device_paired_with() {
    let d = TempDir::new("vaultjson");
    let v = Vault::create(&d.0).unwrap();
    let id = milkrice_sync::crypto::b64(&v.vault_id);
    Vault::open_expecting(&d.0, v.key, &id).unwrap();

    // Someone rewrites vault.json with another vault id (same key, so the KCV still matches).
    let mut f = folder::read_vault_file(&d.0).unwrap().unwrap();
    f.vault_id = milkrice_sync::crypto::b64(&[9u8; 16]);
    std::fs::write(d.0.join("vault.json"), serde_json::to_vec(&f).unwrap()).unwrap();
    assert!(Vault::open_expecting(&d.0, v.key, &id).is_err());

    // A format this build doesn't know.
    f.vault_id = id.clone();
    f.format = 2;
    std::fs::write(d.0.join("vault.json"), serde_json::to_vec(&f).unwrap()).unwrap();
    assert!(Vault::open(&d.0, v.key).is_err());

    // Wrong key.
    f.format = 1;
    std::fs::write(d.0.join("vault.json"), serde_json::to_vec(&f).unwrap()).unwrap();
    assert!(Vault::open(&d.0, [7u8; 32]).is_err());
}

#[test]
fn a_device_card_without_last_seen_parses() {
    // What a joining phone sends while pairing (see tests/vectors/v1.json, pair.card_plain).
    let c: DeviceCard = serde_json::from_str(r#"{"name":"Test iPhone","kind":"iphone"}"#).unwrap();
    assert_eq!(c.last_seen, "");
}

#[test]
fn device_cards_round_trip() {
    let d = TempDir::new("cards");
    let v = Vault::create(&d.0).unwrap();
    v.write_device("mac1", &DeviceCard { name: "Studio".into(), kind: "mac".into(), last_seen: "2026-11-27T00:00:00Z".into() }).unwrap();
    std::fs::write(d.0.join("devices/junk.dev"), b"not sealed").unwrap();
    let ds = v.devices();
    assert_eq!(ds.len(), 1);
    assert_eq!(ds[0].0, "mac1");
    assert_eq!(ds[0].1.name, "Studio");
}
