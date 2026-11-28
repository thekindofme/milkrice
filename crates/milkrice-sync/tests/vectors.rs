//! Shared test vectors. The iOS app's MilkriceSync package checks the same file
//! (`docs/sync-vectors/v1.json` there), so both sides must produce identical bytes.
//! Regenerate with `MILKRICE_WRITE_VECTORS=1 cargo test -p milkrice-sync --test vectors`.
use milkrice_sync::{crypto, merge, pair};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn h<const N: usize>(seed: u8) -> [u8; N] {
    std::array::from_fn(|i| seed.wrapping_add((i as u8).wrapping_mul(7)))
}

fn rec(id: &str, device: &str, fields: Value) -> merge::Record {
    let fields = fields
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), merge::Stamped { v: v[0].clone(), hlc: v[1].as_str().unwrap().into() }))
        .collect();
    merge::Record { v: 1, id: id.into(), device: device.into(), fields }
}

fn merge_case(name: &str, local: Value, state: Value, remotes: Vec<merge::Record>, clock_ms: u64, clock_counter: u32, wall: u64) -> Value {
    let id = "7f3c1d2e-0000-4000-8000-000000000001";
    let local_map: BTreeMap<String, Value> = serde_json::from_value(local.clone()).unwrap();
    let mut st: merge::MeetingState = serde_json::from_value(state.clone()).unwrap();
    let mut clock = merge::Clock { device: "phone0000000000000000000000000001".into(), ms: clock_ms, counter: clock_counter };
    let out = merge::merge(id, &local_map, &mut st, &remotes, &mut clock, wall);
    json!({
        "name": name, "id": id, "wall_ms": wall,
        "clock": {"device": clock.device, "ms": clock_ms, "counter": clock_counter},
        "local": local, "state": state, "remotes": remotes,
        "expect": {"apply": out.apply, "record": out.record, "state": st, "clock": {"ms": clock.ms, "counter": clock.counter}}
    })
}

fn build() -> Value {
    let key: crypto::Key = h(1);
    let vault: crypto::VaultId = h(50);
    let nonce: [u8; 12] = h(90);
    let payload = r#"{"v":1,"id":"m1","device":"d1","fields":{"title":{"v":"Kitchen quote ✓ / déjà vu","hlc":"0001763683200000-0000-d1"}}}"#;
    let file = crypto::seal_with_nonce(&key, &vault, 3, &nonce, payload.as_bytes()).unwrap();

    let mac_secret: [u8; 32] = h(11);
    let phone_secret: [u8; 32] = h(22);
    let invite = pair::Invite { vault_id: vault, session: h(33), mac_pub: pair::public_key(&mac_secret) };
    let phone_pub = pair::public_key(&phone_secret);
    let dm = pair::derive(&mac_secret, &phone_pub, &invite, &phone_pub).unwrap();
    let dp = pair::derive(&phone_secret, &invite.mac_pub, &invite, &phone_pub).unwrap();
    assert_eq!(dm, dp, "both sides must derive the same pairing key and code");
    let grant_nonce: [u8; 12] = h(44);
    let grant = pair::seal_grant(&dm, &invite, 3, &key, &grant_nonce).unwrap();
    let card_nonce: [u8; 12] = h(55);
    let card = crypto::box_seal(&dm.pair_key, &invite.session, &card_nonce, br#"{"name":"Test iPhone","kind":"iphone"}"#).unwrap();

    let mac = "mac00000000000000000000000000002";
    let cases = vec![
        merge_case("fresh local meeting", json!({"title": "Standup", "duration_ms": 95777, "deleted": false}), json!({}), vec![], 0, 0, 1763683200000),
        merge_case(
            "remote title newer, local notes edited",
            json!({"title": "Standup", "notes": "ask Sam", "transcript": [[0, 4500, "Room", "Okay, quick standup."]]}),
            json!({"title": {"hash": merge::value_hash(&json!("Standup")), "hlc": "1763683200000-0000-phone0000000000000000000000000001"},
                   "notes": {"hash": merge::value_hash(&json!("")), "hlc": "1763683200000-0001-phone0000000000000000000000000001"},
                   "transcript": {"hash": merge::value_hash(&json!([[0, 4500, "Room", "Okay, quick standup."]])), "hlc": "1763683200000-0002-phone0000000000000000000000000001"}}),
            vec![rec("7f3c1d2e-0000-4000-8000-000000000001", mac, json!({
                "title": ["Team standup", "1763683260000-0000-mac00000000000000000000000000002"],
                "enhanced": ["Notes by the Mac.\n- [ ] Ship beta", "1763683260000-0001-mac00000000000000000000000000002"],
                "unknown_future_field": [1, "1763683260000-0002-mac00000000000000000000000000002"]}))],
            1763683200000, 2, 1763683230000,
        ),
        merge_case(
            "stale remote loses",
            json!({"title": "Phone title"}),
            json!({"title": {"hash": merge::value_hash(&json!("Phone title")), "hlc": "1763683300000-0000-phone0000000000000000000000000001"}}),
            vec![rec("7f3c1d2e-0000-4000-8000-000000000001", mac, json!({"title": ["Old Mac title", "1763683290000-0000-mac00000000000000000000000000002"]}))],
            1763683300000, 0, 1763683310000,
        ),
        merge_case(
            "deleted on the Mac",
            json!({"title": "Gone soon", "deleted": false}),
            json!({"title": {"hash": merge::value_hash(&json!("Gone soon")), "hlc": "1763683200000-0000-phone0000000000000000000000000001"},
                   "deleted": {"hash": merge::value_hash(&json!(false)), "hlc": "1763683200000-0001-phone0000000000000000000000000001"}}),
            vec![rec("7f3c1d2e-0000-4000-8000-000000000001", mac, json!({"deleted": [true, "1763683400000-0000-mac00000000000000000000000000002"]}))],
            1763683200000, 1, 1763683300000,
        ),
        merge_case(
            "malformed and out-of-range remote stamps are ignored",
            json!({"title": "Mine"}),
            json!({"title": {"hash": merge::value_hash(&json!("Mine")), "hlc": "1763683200000-0000-phone0000000000000000000000000001"}}),
            vec![
                rec("7f3c1d2e-0000-4000-8000-000000000001", mac, json!({"title": ["Tilde", "~"]})),
                rec("7f3c1d2e-0000-4000-8000-000000000001", "mac00000000000000000000000000003", json!({"title": ["Far future", "99999999999999-0000-mac00000000000000000000000000003"]})),
                rec("7f3c1d2e-0000-4000-8000-000000000001", "mac00000000000000000000000000004", json!({"notes": ["Bad device", "1763683300000-0000-../evil"]})),
            ],
            1763683200000, 0, 1763683200500,
        ),
        merge_case(
            "legacy five-digit counter orders after 9999",
            json!({}),
            json!({"title": {"hash": merge::value_hash(&json!("Older")), "hlc": "1763683200000-9999-mac00000000000000000000000000002"}}),
            vec![rec("7f3c1d2e-0000-4000-8000-000000000001", mac, json!({"title": ["Newer", "1763683200000-10000-mac00000000000000000000000000002"]}))],
            1763683200000, 9999, 1763683100000,
        ),
        merge_case(
            "counter rolls into the next millisecond",
            json!({"title": "A", "notes": "B", "mode": "room"}),
            json!({}),
            vec![],
            1763683200000, 9998, 1763683100000,
        ),
    ];

    // Padding edges: the length prefix plus payload exactly fills, then just overflows, one block.
    let pad: Vec<Value> = [0usize, 1020, 1021]
        .iter()
        .map(|&n| {
            let payload = vec![b'x'; n];
            let f = crypto::seal_with_nonce(&key, &vault, 1, &nonce, &payload).unwrap();
            json!({"payload_len": n, "payload_byte": "x", "file_len": f.len(), "sha256": hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&f))})
        })
        .collect();

    // HLCs: ticks across the counter limit, observe, parse and compare.
    let mut c = merge::Clock { device: "d1".into(), ms: 1763683200000, counter: 9998 };
    let ticks: Vec<String> = (0..3).map(|_| c.tick(1763683100000)).collect();
    let mut o = merge::Clock { device: "d1".into(), ms: 1763683200000, counter: 5 };
    o.observe("1763683200000-0007-x");
    let after_observe = o.tick(1763683200000);
    o.observe("garbage");
    let valid = ["1763683200000-0000-d1", "0000000000001-0000-a", "9999999999999-9999-x.y_z-w", "1763683200000-10000-legacy"];
    let invalid = ["", "~", "1763683200000-0000-", "1763683200000-0000-..", "1763683200000-0000-a/b", "1763683200000-x-d", "10000000000000-0000-d", "1763683200000-0000-a b"];
    for v in valid { assert!(merge::parse_hlc(v).is_some(), "{v}"); }
    for v in invalid { assert!(merge::parse_hlc(v).is_none(), "{v}"); }
    let order = [
        ("1763683200000-0001-a", "1763683200000-0000-b", 1),
        ("1763683200000-0000-b", "1763683200000-0000-a", 1),
        ("1763683200000-10000-a", "1763683200000-9999-a", 1),
        ("1763683200000-0000-a", "~", 1),
        ("~", "garbage", 0),
        ("1763683200000-0000-a", "1763683200000-0000-a", 0),
    ];
    for (a, b, want) in order { assert_eq!(merge::hlc_cmp(a, b) as i8, want, "{a} vs {b}"); }

    // The exact pairing messages a joining device writes and the Mac answers with.
    let request = pair::make_request(&dp, &invite, &phone_pub, br#"{"name":"Test iPhone","kind":"iphone"}"#, &card_nonce).unwrap();
    assert_eq!(request.card, crypto::b64(&card));

    let canon = json!(["Room", 4500, true, null, {"b": "/", "a": "déjà"}]);
    let mut canon_bytes = Vec::new();
    merge::canonical(&canon, &mut canon_bytes);

    json!({
        "format": 1,
        "seal": {"key": hex::encode(key), "vault_id": hex::encode(vault), "key_id": 3, "nonce": hex::encode(nonce), "payload": payload, "file": hex::encode(&file)},
        "kcv": {"key": hex::encode(key), "kcv": crypto::kcv(&key)},
        "canonical": {"value": canon, "encoding_hex": hex::encode(&canon_bytes), "hash": merge::value_hash(&canon)},
        "pair": {
            "vault_id": hex::encode(vault), "session": hex::encode(invite.session),
            "mac_secret": hex::encode(mac_secret), "phone_secret": hex::encode(phone_secret),
            "mac_pub": hex::encode(invite.mac_pub), "phone_pub": hex::encode(phone_pub),
            "qr": invite.to_qr(), "pair_key": hex::encode(dm.pair_key), "code": dm.code,
            "grant_nonce": hex::encode(grant_nonce), "key_id": 3, "vault_key": hex::encode(key), "grant": grant.grant,
            "card_nonce": hex::encode(card_nonce), "card_plain": r#"{"name":"Test iPhone","kind":"iphone"}"#, "card": crypto::b64(&card)
        },
        "merge": cases,
        "pad": pad,
        "hlc": {
            "tick_overflow": {"device": "d1", "ms": 1763683200000u64, "counter": 9998, "wall_ms": 1763683100000u64, "ticks": ticks},
            "observe": {"start": {"ms": 1763683200000u64, "counter": 5}, "observed": "1763683200000-0007-x", "then_garbage": "garbage", "next_tick_at_wall_1763683200000": after_observe},
            "valid": valid, "invalid": invalid,
            "compare": order.iter().map(|(a, b, w)| json!([a, b, w])).collect::<Vec<_>>(),
        },
        "pair_messages": {
            "req_json": serde_json::to_string(&request).unwrap(),
            "resp_json": serde_json::to_string(&grant).unwrap(),
        },
    })
}

#[test]
fn vectors_match() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/v1.json");
    let built = build();
    if std::env::var("MILKRICE_WRITE_VECTORS").is_ok() {
        std::fs::write(&path, serde_json::to_string_pretty(&built).unwrap() + "\n").unwrap();
    }
    let committed: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("vectors file")).unwrap();
    assert_eq!(committed, built, "sync format changed: regenerate the vectors and update the iOS copy");
}

#[test]
fn seal_round_trip_and_tamper() {
    let key: crypto::Key = h(1);
    let vault: crypto::VaultId = h(50);
    let sealed = crypto::seal(&key, &vault, 1, b"hello").unwrap();
    assert_eq!(crypto::open(&key, &vault, &sealed).unwrap(), b"hello");
    let mut bad = sealed.clone();
    let last = bad.len() - 1;
    bad[last] ^= 1;
    assert!(crypto::open(&key, &vault, &bad).is_err());
    let mut relabelled = sealed.clone();
    relabelled[23] ^= 1; // key id is authenticated
    assert!(crypto::open(&key, &vault, &relabelled).is_err());
    assert!(crypto::open(&h::<32>(2), &vault, &sealed).is_err());
}

#[test]
fn swapped_phone_key_changes_the_code() {
    let invite = pair::Invite { vault_id: h(50), session: h(33), mac_pub: pair::public_key(&h(11)) };
    let real = pair::public_key(&h(22));
    let evil = pair::public_key(&h(23));
    let honest = pair::derive(&h(22), &invite.mac_pub, &invite, &real).unwrap();
    let mac_sees_evil = pair::derive(&h(11), &evil, &invite, &evil).unwrap();
    assert_ne!(honest.code, mac_sees_evil.code);
}
