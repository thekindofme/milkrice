//! Sealed files and pairing boxes: round trip, tamper detection, wrong keys, length hiding.
use milkrice_sync::crypto::{self, HEADER_LEN, NONCE_LEN, PAD};
use milkrice_sync::pair::{self, Invite};
use proptest::prelude::*;
use std::collections::HashSet;

const TAG: usize = 16;

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn seal_round_trips_and_hides_length(
        key in any::<[u8; 32]>(), vault in any::<[u8; 16]>(), key_id in any::<u32>(),
        payload in prop::collection::vec(any::<u8>(), 0..5000),
    ) {
        let f = crypto::seal(&key, &vault, key_id, &payload).unwrap();
        prop_assert_eq!(crypto::open(&key, &vault, &f).unwrap(), payload.clone());
        prop_assert_eq!(crypto::key_id_of(&f), Some(key_id));
        prop_assert_eq!(&f[..4], b"MRS1");
        let body = f.len() - HEADER_LEN - NONCE_LEN - TAG;
        prop_assert_eq!(body % PAD, 0, "plaintext is padded to a multiple of {}", PAD);
        prop_assert_eq!(body, (4 + payload.len()).div_ceil(PAD) * PAD);
    }

    #[test]
    fn any_single_bit_flip_is_detected(
        key in any::<[u8; 32]>(), vault in any::<[u8; 16]>(),
        payload in prop::collection::vec(any::<u8>(), 0..300),
        pos in any::<prop::sample::Index>(), bit in 0..8u8,
    ) {
        let mut f = crypto::seal(&key, &vault, 1, &payload).unwrap();
        let i = pos.index(f.len());
        f[i] ^= 1 << bit;
        prop_assert!(crypto::open(&key, &vault, &f).is_err(), "flip at byte {} went unnoticed", i);
    }

    #[test]
    fn truncation_and_extension_are_detected(
        key in any::<[u8; 32]>(), vault in any::<[u8; 16]>(),
        payload in prop::collection::vec(any::<u8>(), 0..300),
        cut in 1..64usize, extra in prop::collection::vec(any::<u8>(), 1..64),
    ) {
        let f = crypto::seal(&key, &vault, 1, &payload).unwrap();
        prop_assert!(crypto::open(&key, &vault, &f[..f.len().saturating_sub(cut)]).is_err());
        let mut longer = f.clone();
        longer.extend_from_slice(&extra);
        prop_assert!(crypto::open(&key, &vault, &longer).is_err());
        prop_assert!(crypto::open(&key, &vault, &[]).is_err());
    }

    #[test]
    fn wrong_key_or_vault_fails(
        key in any::<[u8; 32]>(), other in any::<[u8; 32]>(),
        vault in any::<[u8; 16]>(), other_vault in any::<[u8; 16]>(),
        payload in prop::collection::vec(any::<u8>(), 0..300),
    ) {
        let f = crypto::seal(&key, &vault, 1, &payload).unwrap();
        if other != key { prop_assert!(crypto::open(&other, &vault, &f).is_err()); }
        if other_vault != vault {
            prop_assert!(crypto::open(&key, &other_vault, &f).is_err());
            // Relabelling the header with the other vault id doesn't help either.
            let mut moved = f.clone();
            moved[4..20].copy_from_slice(&other_vault);
            prop_assert!(crypto::open(&key, &other_vault, &moved).is_err());
        }
        if other != key { prop_assert_ne!(crypto::kcv(&key), crypto::kcv(&other)); }
    }

    #[test]
    fn boxes_round_trip_and_bind_their_associated_data(
        key in any::<[u8; 32]>(), aad in prop::collection::vec(any::<u8>(), 0..32),
        other_aad in prop::collection::vec(any::<u8>(), 0..32),
        nonce in any::<[u8; NONCE_LEN]>(), msg in prop::collection::vec(any::<u8>(), 1..200),
        pos in any::<prop::sample::Index>(),
    ) {
        let b = crypto::box_seal(&key, &aad, &nonce, &msg).unwrap();
        prop_assert_eq!(crypto::box_open(&key, &aad, &b).unwrap(), msg);
        if other_aad != aad { prop_assert!(crypto::box_open(&key, &other_aad, &b).is_err()); }
        let mut t = b.clone();
        let i = pos.index(t.len());
        t[i] ^= 0x01;
        prop_assert!(crypto::box_open(&key, &aad, &t).is_err());
    }

    #[test]
    fn invites_round_trip_through_the_qr_text(vault in any::<[u8; 16]>(), session in any::<[u8; 8]>(), mac_pub in any::<[u8; 32]>()) {
        let i = Invite { vault_id: vault, session, mac_pub };
        prop_assert_eq!(Invite::parse(&i.to_qr()).unwrap(), i.clone());
        prop_assert_eq!(Invite::parse(&format!("  {}\n", i.to_qr())).unwrap(), i);
    }

    #[test]
    fn both_sides_derive_the_same_key_and_code(mac in any::<[u8; 32]>(), phone in any::<[u8; 32]>(), vault in any::<[u8; 16]>(), session in any::<[u8; 8]>()) {
        let invite = Invite { vault_id: vault, session, mac_pub: pair::public_key(&mac) };
        let phone_pub = pair::public_key(&phone);
        let a = pair::derive(&mac, &phone_pub, &invite, &phone_pub).unwrap();
        let b = pair::derive(&phone, &invite.mac_pub, &invite, &phone_pub).unwrap();
        prop_assert_eq!(&a, &b);
        prop_assert_eq!(a.code.len(), 6);
        prop_assert!(a.code.bytes().all(|c| c.is_ascii_digit()));
    }
}

#[test]
fn nonces_are_fresh() {
    let key = [1u8; 32];
    let vault = [2u8; 16];
    let mut seen = HashSet::new();
    for _ in 0..10_000 {
        let f = crypto::seal(&key, &vault, 1, b"same").unwrap();
        assert!(seen.insert(f[HEADER_LEN..HEADER_LEN + NONCE_LEN].to_vec()), "nonce repeated");
    }
}

#[test]
fn malformed_qr_codes_are_refused() {
    let good = Invite { vault_id: [1; 16], session: [2; 8], mac_pub: [3; 32] }.to_qr();
    let parts: Vec<&str> = good.trim_start_matches("milkrice-pair:1:").split(':').collect();
    for bad in [
        String::new(),
        "milkrice-pair:1:".into(),
        good.replace("milkrice-pair:1:", "milkrice-pair:0:"),
        format!("{good}:extra"),
        format!("milkrice-pair:1:{}:{}", parts[0], parts[1]),
        format!("milkrice-pair:1:{}:{}:{}", parts[0], parts[1], &parts[2][1..]),
        format!("milkrice-pair:1:{}:{}:{}", &parts[0][2..], parts[1], parts[2]),
        format!("milkrice-pair:1:{}:{}:{}", parts[0], "!!!", parts[2]),
    ] {
        assert!(Invite::parse(&bad).is_err(), "{bad:?}");
    }
}
