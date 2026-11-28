//! The pairing state machines ([`Pairing`] on the Mac, [`Joiner`] on the phone) end to end through
//! a real folder, including the ways a stranger with write access to that folder can interfere.
mod common;
use common::{h, TempDir};
use milkrice_sync::folder::{DeviceCard, Vault};
use milkrice_sync::pair::{self, Invite, Request, Response};
use milkrice_sync::pairing::{display_name, Joiner, Pairing};
use milkrice_sync::crypto;
use std::time::Duration;

fn phone_card() -> DeviceCard {
    DeviceCard { name: "Test iPhone".into(), kind: "iphone".into(), last_seen: String::new() }
}

fn req_path(v: &Vault, p: &Pairing) -> std::path::PathBuf {
    v.pair_dir().join(format!("{}.req", p.invite().session_hex()))
}

#[test]
fn happy_path_hands_over_the_key() {
    let d = TempDir::new("pair-ok");
    let mac = Vault::create(&d.0).unwrap();
    let mut p = Pairing::start(&mac);
    assert!(p.poll().unwrap().is_none(), "nobody answered yet");

    let phone = Joiner::start(&d.0, &p.qr, &phone_card()).unwrap();
    assert!(phone.poll().unwrap().is_none(), "not granted yet");
    let j = p.poll().unwrap().expect("the Mac sees the phone");
    assert_eq!(j.name, "Test iPhone");
    assert_eq!(j.code, phone.code(), "both screens show the same code");
    assert_eq!(j.code.len(), 6);

    assert_eq!(p.accept(&mac).unwrap(), "Test iPhone");
    assert!(!req_path(&mac, &p).exists(), "request removed");
    let got = phone.poll().unwrap().expect("granted");
    assert_eq!(got.key, mac.key);
    assert_eq!(got.vault_id, mac.vault_id);
    assert_eq!(got.key_id, mac.key_id);
    assert_eq!(std::fs::read_dir(mac.pair_dir()).unwrap().count(), 0, "no pairing files left behind");
}

#[test]
fn accept_before_anyone_answers_shares_nothing() {
    let d = TempDir::new("pair-early");
    let mac = Vault::create(&d.0).unwrap();
    let p = Pairing::start(&mac);
    assert!(p.accept(&mac).is_err());
    assert!(!mac.pair_dir().exists() || std::fs::read_dir(mac.pair_dir()).unwrap().count() == 0);
}

#[test]
fn cancel_shares_nothing() {
    let d = TempDir::new("pair-cancel");
    let mac = Vault::create(&d.0).unwrap();
    let mut p = Pairing::start(&mac);
    let phone = Joiner::start(&d.0, &p.qr, &phone_card()).unwrap();
    assert!(p.poll().unwrap().is_some());
    p.cancel();
    assert!(!req_path(&mac, &p).exists());
    assert!(phone.poll().unwrap().is_none());
}

#[test]
fn an_expired_code_does_nothing() {
    let d = TempDir::new("pair-expired");
    let mac = Vault::create(&d.0).unwrap();
    let mut p = Pairing::start_with_ttl(&mac, Duration::ZERO);
    let _phone = Joiner::start(&d.0, &p.qr, &phone_card()).unwrap();
    assert!(p.expired());
    assert!(p.poll().is_err());
    assert!(p.accept(&mac).is_err());
}

#[test]
fn garbage_or_half_written_requests_are_waited_out() {
    let d = TempDir::new("pair-garbage");
    let mac = Vault::create(&d.0).unwrap();
    let mut p = Pairing::start(&mac);
    std::fs::create_dir_all(mac.pair_dir()).unwrap();
    std::fs::write(req_path(&mac, &p), b"{\"v\":1,\"pu").unwrap();
    assert!(p.poll().unwrap().is_none());
}

#[test]
fn a_stranger_without_the_qr_cannot_get_in_and_cannot_lock_the_phone_out() {
    let d = TempDir::new("pair-stranger");
    let mac = Vault::create(&d.0).unwrap();
    let mut p = Pairing::start(&mac);
    // The stranger sees the session id (it's a file name) but not the Mac's public key, so they
    // guess one; their sealed card doesn't open.
    let guess = Invite { mac_pub: pair::public_key(&h(99)), ..p.invite().clone() };
    let evil_secret: [u8; 32] = h(77);
    let evil_pub = pair::public_key(&evil_secret);
    let ed = pair::derive(&evil_secret, &guess.mac_pub, &guess, &evil_pub).unwrap();
    let evil = pair::make_request(&ed, &guess, &evil_pub, br#"{"name":"Your iPhone","kind":"iphone"}"#, &h(1)).unwrap();
    std::fs::create_dir_all(mac.pair_dir()).unwrap();
    std::fs::write(req_path(&mac, &p), serde_json::to_vec(&evil).unwrap()).unwrap();
    assert!(p.poll().is_err(), "the forged request doesn't authenticate");
    assert!(p.accept(&mac).is_err(), "and nothing can be accepted");
    // The real phone (which did scan the QR) still gets through.
    let phone = Joiner::start(&d.0, &p.qr, &phone_card()).unwrap();
    assert_eq!(p.poll().unwrap().unwrap().code, phone.code());
}

#[test]
fn someone_who_photographed_the_qr_and_answers_first_shows_a_different_code() {
    let d = TempDir::new("pair-mitm");
    let mac = Vault::create(&d.0).unwrap();
    let mut p = Pairing::start(&mac);
    let evil = Joiner::start_with(&d.0, &p.qr, &phone_card(), h(77), &h(1)).unwrap();
    let mac_sees = p.poll().unwrap().unwrap();
    assert_eq!(mac_sees.code, evil.code());
    // The real phone now writes its request too (same file name); the Mac stays with the first.
    let real = Joiner::start_with(&d.0, &p.qr, &phone_card(), h(78), &h(2)).unwrap();
    assert_eq!(p.poll().unwrap().unwrap().code, mac_sees.code);
    assert_ne!(real.code(), mac_sees.code, "the person sees two different codes and cancels");
    p.cancel();
    assert!(real.poll().unwrap().is_none());
}

#[test]
fn a_grant_from_another_session_does_not_open() {
    let d = TempDir::new("pair-replay");
    let mac = Vault::create(&d.0).unwrap();
    let mut first = Pairing::start(&mac);
    let phone1 = Joiner::start(&d.0, &first.qr, &phone_card()).unwrap();
    first.poll().unwrap();
    first.accept(&mac).unwrap();
    let old_resp = std::fs::read(mac.pair_dir().join(format!("{}.resp", first.invite().session_hex()))).unwrap();
    phone1.poll().unwrap().unwrap();

    // A new pairing; someone drops the old grant where the new phone looks for its answer.
    let second = Pairing::start(&mac);
    let phone2 = Joiner::start(&d.0, &second.qr, &phone_card()).unwrap();
    std::fs::write(mac.pair_dir().join(format!("{}.resp", second.invite().session_hex())), &old_resp).unwrap();
    assert!(phone2.poll().is_err());
}

#[test]
fn versions_are_checked() {
    let invite = Invite { vault_id: h(50), session: h(33), mac_pub: pair::public_key(&h(11)) };
    let phone_pub = pair::public_key(&h(22));
    let d = pair::derive(&h(22), &invite.mac_pub, &invite, &phone_pub).unwrap();
    let mut req = pair::make_request(&d, &invite, &phone_pub, b"{}", &h(3)).unwrap();
    assert!(pair::open_request(&h(11), &invite, &req).is_ok());
    req.v = 2;
    assert!(pair::open_request(&h(11), &invite, &req).is_err());
    let mut resp = pair::seal_grant(&d, &invite, 1, &h(1), &h(4)).unwrap();
    assert!(pair::open_grant(&d, &invite, &resp).is_ok());
    resp.v = 0;
    assert!(pair::open_grant(&d, &invite, &resp).is_err());
    assert!(Invite::parse(&invite.to_qr().replace("milkrice-pair:1:", "milkrice-pair:2:")).is_err());
}

#[test]
fn the_wrong_grant_key_or_a_tampered_grant_fails() {
    let invite = Invite { vault_id: h(50), session: h(33), mac_pub: pair::public_key(&h(11)) };
    let phone_pub = pair::public_key(&h(22));
    let d = pair::derive(&h(22), &invite.mac_pub, &invite, &phone_pub).unwrap();
    let resp = pair::seal_grant(&d, &invite, 1, &h(1), &h(4)).unwrap();
    let other = pair::derive(&h(23), &invite.mac_pub, &invite, &pair::public_key(&h(23))).unwrap();
    assert!(pair::open_grant(&other, &invite, &resp).is_err());
    let mut raw = crypto::unb64(&resp.grant).unwrap();
    raw[20] ^= 0x80;
    assert!(pair::open_grant(&d, &invite, &Response { v: 1, grant: crypto::b64(&raw) }).is_err());
    let other_session = Invite { session: h(34), ..invite.clone() };
    assert!(pair::open_grant(&d, &other_session, &resp).is_err(), "the session is bound as associated data");
}

#[test]
fn low_order_public_keys_are_refused() {
    let invite = Invite { vault_id: h(50), session: h(33), mac_pub: pair::public_key(&h(11)) };
    // The all-zero point and the order-2/order-4 points make the shared secret all zeros.
    for bad in [[0u8; 32], { let mut p = [0u8; 32]; p[0] = 1; p }] {
        assert!(pair::derive(&h(11), &bad, &invite, &bad).is_err());
        let req = Request { v: 1, public: crypto::b64(&bad), card: crypto::b64(&[0u8; 40]) };
        assert!(pair::open_request(&h(11), &invite, &req).is_err());
    }
}

#[test]
fn a_qr_for_another_folder_is_refused_by_the_phone() {
    let a = TempDir::new("pair-a");
    let b = TempDir::new("pair-b");
    let va = Vault::create(&a.0).unwrap();
    Vault::create(&b.0).unwrap();
    let p = Pairing::start(&va);
    assert!(Joiner::start(&b.0, &p.qr, &phone_card()).is_err());
}

#[test]
fn names_are_made_safe_to_show() {
    assert_eq!(display_name("Sam's iPhone"), "Sam's iPhone");
    assert_eq!(display_name("a\u{7}\nb\u{202e}c\u{2066}"), "abc");
    assert_eq!(display_name("   "), "Unnamed device");
    assert_eq!(display_name(&"x".repeat(500)).chars().count(), 64);
}
