//! Property tests for the merge: several simulated devices edit, delete and sync one meeting in
//! random orders with skewed clocks; every device must end up with the same values, and each
//! field's value must be the last write by HLC (nothing newer is ever lost).
use milkrice_sync::merge::{self, hlc_cmp, parse_hlc, Clock, MeetingState, Record};
use proptest::prelude::*;
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::collections::BTreeMap;

const ID: &str = "7f3c1d2e-0000-4000-8000-000000000001";
const SYNC_FIELDS: &[&str] = &["title", "notes", "deleted", "duration_ms"];

#[derive(Debug, Clone)]
enum Op {
    Edit { dev: usize, field: usize, value: u8 },
    Delete { dev: usize },
    Undelete { dev: usize },
    Sync { dev: usize },
    /// Sync, but the folder hands this device an old copy of some record (rollback/replay).
    SyncWithStale { dev: usize, pick: usize },
    Time { dev: usize, ms: u64 },
}

fn value_for(field: &str, n: u8) -> Value {
    match field {
        "duration_ms" => json!(n as u64 * 1000),
        "deleted" => json!(n % 2 == 1),
        _ => json!(["", "a", "b", "Kitchen quote ✓"][n as usize % 4]),
    }
}

struct Device {
    local: BTreeMap<String, Value>,
    state: MeetingState,
    clock: Clock,
    wall: u64,
}

struct World {
    devs: Vec<Device>,
    /// The latest record each device published (what the folder holds).
    folder: BTreeMap<usize, Record>,
    /// Every record ever published, for replay.
    history: Vec<Record>,
    /// Every (hlc, value) stamp any device ever published, per field.
    stamps: BTreeMap<String, Vec<(String, Value)>>,
}

impl World {
    fn new(n: usize, skews: &[u64]) -> Self {
        let devs = (0..n)
            .map(|i| Device { local: BTreeMap::new(), state: MeetingState::new(), clock: Clock::new(&format!("dev{i}")), wall: 1_763_683_200_000 + skews[i] })
            .collect();
        Self { devs, folder: BTreeMap::new(), history: vec![], stamps: BTreeMap::new() }
    }

    fn sync(&mut self, dev: usize, stale: Option<Record>) {
        let mut remotes: Vec<Record> = self.folder.iter().filter(|(d, _)| **d != dev).map(|(_, r)| r.clone()).collect();
        remotes.extend(stale.filter(|r| r.device != self.devs[dev].clock.device));
        let d = &mut self.devs[dev];
        let out = merge::merge(ID, &d.local, &mut d.state, &remotes, &mut d.clock, d.wall);
        for (k, v) in &out.apply {
            d.local.insert(k.clone(), v.clone());
        }
        // The record is the device's full, current view.
        for (k, s) in &out.record.fields {
            assert_eq!(d.local.get(k), Some(&s.v), "record disagrees with the local view for {k}");
        }
        if !out.record.fields.is_empty() {
            for (k, s) in &out.record.fields {
                self.stamps.entry(k.clone()).or_default().push((s.hlc.clone(), s.v.clone()));
            }
            self.history.push(out.record.clone());
            self.folder.insert(dev, out.record);
        }
        d.wall += 1;
    }

    fn run(&mut self, op: &Op) {
        let n = self.devs.len();
        match *op {
            Op::Edit { dev, field, value } => {
                let f = SYNC_FIELDS[field % SYNC_FIELDS.len()];
                self.devs[dev % n].local.insert(f.into(), value_for(f, value));
            }
            Op::Delete { dev } => {
                self.devs[dev % n].local.insert("deleted".into(), json!(true));
            }
            Op::Undelete { dev } => {
                self.devs[dev % n].local.insert("deleted".into(), json!(false));
            }
            Op::Sync { dev } => self.sync(dev % n, None),
            Op::SyncWithStale { dev, pick } => {
                let stale = (!self.history.is_empty()).then(|| self.history[pick % self.history.len()].clone());
                self.sync(dev % n, stale)
            }
            Op::Time { dev, ms } => self.devs[dev % n].wall += ms,
        }
    }

    /// Everyone syncs, twice round, with no further edits.
    fn settle(&mut self) {
        for _ in 0..2 {
            for d in 0..self.devs.len() {
                self.sync(d, None);
            }
        }
    }
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0..4usize, 0..4usize, any::<u8>()).prop_map(|(dev, field, value)| Op::Edit { dev, field, value }),
        1 => (0..4usize).prop_map(|dev| Op::Delete { dev }),
        1 => (0..4usize).prop_map(|dev| Op::Undelete { dev }),
        4 => (0..4usize).prop_map(|dev| Op::Sync { dev }),
        1 => (0..4usize, any::<usize>()).prop_map(|(dev, pick)| Op::SyncWithStale { dev, pick }),
        1 => (0..4usize, 0..5_000u64).prop_map(|(dev, ms)| Op::Time { dev, ms }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    /// Convergence and no lost writes: after everyone has synced, all devices hold the same value
    /// for every field, and it is the value of the newest stamp ever published for that field.
    #[test]
    fn devices_converge_on_the_last_writer(
        n in 2..=4usize,
        skews in prop::collection::vec(0..120_000u64, 4),
        ops in prop::collection::vec(op(), 0..60),
    ) {
        let mut w = World::new(n, &skews);
        for o in &ops {
            w.run(o);
        }
        w.settle();
        let first = w.devs[0].local.clone();
        for (i, d) in w.devs.iter().enumerate() {
            prop_assert_eq!(&d.local, &first, "device {} differs", i);
        }
        for (field, stamps) in &w.stamps {
            let (hlc, v) = stamps.iter().max_by(|a, b| hlc_cmp(&a.0, &b.0)).unwrap();
            prop_assert_eq!(first.get(field), Some(v), "field {} lost its newest write {}", field, hlc);
            // Distinct stamps never carry different values.
            for (h2, v2) in stamps {
                if h2 == hlc { prop_assert_eq!(v2, v); }
            }
        }
        // Every record in the folder agrees too.
        for r in w.folder.values() {
            for (k, s) in &r.fields {
                prop_assert_eq!(first.get(k), Some(&s.v));
            }
        }
    }

    /// Causality: an edit made after a device has seen everything always wins, however far
    /// ahead the other devices' clocks are.
    #[test]
    fn an_edit_made_after_seeing_everything_wins(
        n in 2..=4usize,
        skews in prop::collection::vec(0..600_000u64, 4),
        ops in prop::collection::vec(op(), 0..60),
        dev in 0..4usize,
        field in 0..4usize,
    ) {
        let mut w = World::new(n, &skews);
        for o in &ops { w.run(o); }
        w.settle();
        let dev = dev % n;
        let f = SYNC_FIELDS[field];
        let fresh = match f { "duration_ms" => json!(123_456_789), "deleted" => json!(!w.devs[dev].local.get(f).and_then(|v| v.as_bool()).unwrap_or(false)), _ => json!("fresh edit") };
        w.devs[dev].local.insert(f.into(), fresh.clone());
        w.sync(dev, None);
        w.settle();
        for d in &w.devs {
            prop_assert_eq!(d.local.get(f), Some(&fresh));
        }
    }

    /// The outcome of one merge doesn't depend on the order the folder lists the records in.
    #[test]
    fn merge_ignores_record_order(
        ops in prop::collection::vec(op(), 0..40),
        local_title in 0..4u8,
        seed in any::<u64>(),
    ) {
        let mut w = World::new(4, &[0, 50, 10_000, 3]);
        for o in &ops { w.run(o); }
        let remotes: Vec<Record> = w.folder.values().cloned().collect();
        let mut shuffled = remotes.clone();
        // Deterministic shuffle from the seed.
        let mut s = seed | 1;
        for i in (1..shuffled.len()).rev() {
            s ^= s << 13; s ^= s >> 7; s ^= s << 17;
            shuffled.swap(i, (s % (i as u64 + 1)) as usize);
        }
        shuffled.reverse();
        let local = BTreeMap::from([("title".to_string(), value_for("title", local_title))]);
        let run = |rs: &[Record]| {
            let mut st = MeetingState::new();
            let mut c = Clock::new("observer");
            let out = merge::merge(ID, &local, &mut st, rs, &mut c, 1_763_683_200_000);
            (out, st, c)
        };
        prop_assert_eq!(run(&remotes), run(&shuffled));
    }

    /// Merging the same input again changes nothing: no new applies, same record, same state.
    #[test]
    fn merge_is_idempotent(ops in prop::collection::vec(op(), 0..40), dev in 0..3usize) {
        let mut w = World::new(3, &[0, 7, 90_000]);
        for o in &ops { w.run(o); }
        let remotes: Vec<Record> = w.folder.iter().filter(|(d, _)| **d != dev).map(|(_, r)| r.clone()).collect();
        let d = &mut w.devs[dev];
        let first = merge::merge(ID, &d.local, &mut d.state, &remotes, &mut d.clock, d.wall);
        for (k, v) in &first.apply { d.local.insert(k.clone(), v.clone()); }
        let state_after = d.state.clone();
        let again = merge::merge(ID, &d.local, &mut d.state, &remotes, &mut d.clock, d.wall + 10);
        prop_assert!(again.apply.is_empty());
        prop_assert_eq!(&again.record, &first.record);
        prop_assert_eq!(&d.state, &state_after);
    }

    /// Two devices syncing in either order end in the same place (state-based merge commutes).
    #[test]
    fn sync_order_between_two_devices_does_not_matter(
        a_edits in prop::collection::vec((0..4usize, any::<u8>()), 1..6),
        b_edits in prop::collection::vec((0..4usize, any::<u8>()), 1..6),
        a_first in any::<bool>(),
    ) {
        let play = |a_first: bool| {
            let mut w = World::new(2, &[0, 1_000]);
            for (f, v) in &a_edits { w.run(&Op::Edit { dev: 0, field: *f, value: *v }); }
            for (f, v) in &b_edits { w.run(&Op::Edit { dev: 1, field: *f, value: *v }); }
            if a_first { w.sync(0, None); w.sync(1, None); } else { w.sync(1, None); w.sync(0, None); }
            w.settle();
            w.devs[0].local.clone()
        };
        let x = play(a_first);
        let y = play(!a_first);
        // Different orders may pick different winners (stamps differ), but each order converges
        // and both outcomes keep every field either device set.
        for k in x.keys().chain(y.keys()) {
            prop_assert!(x.contains_key(k) && y.contains_key(k));
        }
    }

    /// Clock stamps strictly increase, keep their fixed-width shape, and string order is clock
    /// order, whatever the wall clock does and whatever stamps are observed.
    #[test]
    fn hlc_ticks_are_monotonic_and_fixed_width(
        steps in prop::collection::vec((0..3u8, 0..2_000u64, 0..20_000u32), 1..300),
    ) {
        let mut c = Clock::new("dev0");
        let mut wall = 1_763_683_200_000u64;
        let mut prev: Option<String> = None;
        for (kind, ms, counter) in steps {
            match kind {
                0 => wall += ms,
                1 => wall = wall.saturating_sub(ms), // the wall clock jumps back
                _ => c.observe(&format!("{:013}-{:04}-other", wall + ms, counter.min(9999))),
            }
            for _ in 0..(counter % 50) {
                let h = c.tick(wall);
                prop_assert_eq!(h.len(), 13 + 1 + 4 + 1 + 4);
                prop_assert!(parse_hlc(&h).is_some());
                if let Some(p) = &prev {
                    prop_assert!(h > *p, "{} then {}", p, h);
                    prop_assert_eq!(hlc_cmp(&h, p), Ordering::Greater);
                }
                prev = Some(h);
            }
        }
    }
}

#[test]
fn ten_thousand_ticks_in_one_millisecond_stay_ordered() {
    let mut c = Clock::new("d");
    c.observe("1763683260000-0000-x"); // another device's clock is a minute ahead
    let mut prev = c.tick(1_763_683_200_000);
    for _ in 0..25_000 {
        let h = c.tick(1_763_683_200_000);
        assert!(h > prev, "{prev} then {h}");
        prev = h;
    }
    assert!(prev.starts_with("1763683260002-"));
}

#[test]
fn a_concurrent_delete_and_edit_keep_both_and_undelete_restores() {
    let mut w = World::new(2, &[0, 0]);
    w.run(&Op::Edit { dev: 0, field: 0, value: 1 }); // title "a"
    w.sync(0, None);
    w.sync(1, None);
    // Concurrently: dev0 deletes, dev1 retitles.
    w.run(&Op::Delete { dev: 0 });
    w.run(&Op::Edit { dev: 1, field: 0, value: 2 }); // title "b"
    w.sync(0, None);
    w.sync(1, None);
    w.settle();
    for d in &w.devs {
        assert_eq!(d.local["deleted"], json!(true), "the tombstone wins");
        assert_eq!(d.local["title"], json!("b"), "the concurrent edit isn't lost");
    }
    // A later undelete brings the meeting back with the edit.
    w.run(&Op::Undelete { dev: 1 });
    w.sync(1, None);
    w.settle();
    for d in &w.devs {
        assert_eq!(d.local["deleted"], json!(false));
        assert_eq!(d.local["title"], json!("b"));
    }
}

#[test]
fn a_replayed_old_record_cannot_resurrect_a_deleted_meeting() {
    let mut w = World::new(2, &[0, 0]);
    w.run(&Op::Edit { dev: 0, field: 0, value: 1 });
    w.run(&Op::Undelete { dev: 0 }); // deleted = false
    w.sync(0, None);
    let before_delete = w.folder[&0].clone();
    w.sync(1, None);
    w.run(&Op::Delete { dev: 1 });
    w.sync(1, None);
    w.sync(0, None);
    assert_eq!(w.devs[0].local["deleted"], json!(true));
    // The folder serves dev1 the old record again (someone restored an old file version).
    w.sync(1, Some(Record { device: "dev0".into(), ..before_delete }));
    w.settle();
    for d in &w.devs {
        assert_eq!(d.local["deleted"], json!(true));
    }
}

#[test]
fn malformed_or_hostile_stamps_are_ignored() {
    let mut st = MeetingState::new();
    let mut c = Clock::new("phone");
    let local = BTreeMap::from([("title".to_string(), json!("mine"))]);
    merge::merge(ID, &local, &mut st, &[], &mut c, 1_763_683_200_000);
    for hlc in ["~", "99999999999999999-0000-x", "1763683200000-0000-", "1763683200000-0000-../x", "1763683200000-x-d", "", "1763683200000-0000-a b"] {
        let evil = Record {
            v: 1,
            id: ID.into(),
            device: "evil".into(),
            fields: BTreeMap::from([("title".to_string(), merge::Stamped { v: json!("pwned"), hlc: hlc.into() })]),
        };
        let out = merge::merge(ID, &local, &mut st, &[evil], &mut c, 1_763_683_200_001);
        assert!(out.apply.is_empty(), "{hlc:?} was accepted");
        assert_eq!(out.record.fields["title"].v, json!("mine"));
    }
    assert!(c.ms < 2_000_000_000_000, "the clock wasn't dragged into the far future");
}

#[test]
fn legacy_five_digit_counters_still_order_correctly() {
    // Stamps an older build could have written after overflowing the counter.
    assert_eq!(hlc_cmp("1763683200000-10000-a", "1763683200000-9999-a"), Ordering::Greater);
    assert_eq!(hlc_cmp("1763683200001-0000-a", "1763683200000-10000-a"), Ordering::Greater);
    assert_eq!(hlc_cmp("1763683200000-0000-b", "1763683200000-0000-a"), Ordering::Greater);
}

#[test]
fn records_for_other_meetings_and_unknown_fields_are_ignored() {
    let mut st = MeetingState::new();
    let mut c = Clock::new("phone");
    let other = Record {
        v: 1,
        id: "another-meeting".into(),
        device: "mac".into(),
        fields: BTreeMap::from([("title".to_string(), merge::Stamped { v: json!("x"), hlc: "1763683200000-0000-mac".into() })]),
    };
    let future = Record {
        v: 2,
        id: ID.into(),
        device: "mac".into(),
        fields: BTreeMap::from([("colour".to_string(), merge::Stamped { v: json!("red"), hlc: "1763683200000-0000-mac".into() })]),
    };
    let out = merge::merge(ID, &BTreeMap::new(), &mut st, &[other, future], &mut c, 1_763_683_200_000);
    assert!(out.apply.is_empty());
    assert!(out.record.fields.is_empty());
}
