# milkrice-sync security review, November 2026

* **Scope:** `crates/milkrice-sync` (`crypto.rs`, `pair.rs`, `merge.rs`, `folder.rs`, `desktop.rs`)
  as of `main` at `5291d7d`. Line numbers below refer to that commit.
* **Branch with fixes:** `cloud/sync-hardening`.
* **Method:** manual review of every line of the crate, then each suspected issue reproduced with
  a failing test before fixing it. The crate was built and tested on Linux without the `desktop`
  feature (that feature needs `milkrice-core`, which has macOS-only dependencies), so the
  `desktop.rs` changes are small and were checked by review and `rustfmt` parsing only.
* **Threat model:**
  1. **Folder attacker:** can read, write, delete, rename, roll back and plant files (including
     symlinks, on providers that sync them) in the shared folder, but does not hold the vault key.
     Examples: the storage provider, another app with folder access, someone holding the cloud login.
  2. **QR observer:** a folder attacker who also saw or photographed the pairing QR code.
  3. **Key holder:** a paired device that is compromised or simply buggy. The iPhone app is a
     separate Swift implementation, so "buggy peer" is a realistic case, not only a malicious one.

## Summary

| ID | Severity | Title | Status |
| --- | --- | --- | --- |
| H1 | High | HLC counter overflow reverses clock order and loses edits | Fixed |
| C1 | High (correctness) | Mac can't parse the device card an iPhone sends while pairing | Fixed |
| M1 | Medium | Path traversal through a decrypted meeting id | Fixed |
| M2 | Medium | Temp-file writes follow planted symlinks | Fixed |
| M3 | Medium | One malformed HLC freezes a field on every device forever | Fixed |
| M4 | Medium | No device removal or key rotation | **Not fixed** (needs a product decision) |
| L1 | Low | Records aren't bound to their file path | Fixed |
| L2 | Low | `vault.json` is unauthenticated and not checked against the paired vault | Fixed |
| L3 | Low | Unbounded reads; symlinks to special files hang the sync loop | Fixed |
| L4 | Low | Pairing invites never expire | Fixed |
| L5 | Low | Pairing message versions aren't checked | Fixed |
| L6 | Low | Device name in the confirmation prompt is attacker-chosen, unsanitised | Fixed |
| L7 | Low | Stale pairing files are never cleaned up | Fixed |
| L8 | Low | Both pairing directions use the same key and associated data | Not fixed (pairing v2) |
| L9 | Low | Device cards aren't bound to a device id | Not fixed (format 2) |
| L10 | Low | Rolled-back own record isn't noticed by its author | Not fixed |
| L11 | Low | Test key-file hook ships in release builds and writes the key world-readable | Not fixed |
| L12 | Low | Key material isn't zeroized | Partly fixed (pairing secrets) |
| I1–I7 | Info | Nonces, KCV, subkeys, metadata, code bias, error messages, constant time | No action |

Nothing found lets a folder attacker read or forge sync contents. The AEAD construction and the
pairing key exchange are sound. The serious problems were in the merge clock and in how the crate
handles the file system around the cryptography.

## High

### H1: HLC counter overflow reverses clock order and loses edits

* **Where:** `src/merge.rs:54-61` (`Clock::tick`), `src/merge.rs:163` and `:189` (string comparison of HLCs).
* **Problem:** HLCs are `"<ms:013>-<counter:04>-<device>"` and are compared as strings. `tick`
  incremented the counter without a limit. From the 10,000th tick within one millisecond the
  counter had five digits, and `"…-10000-…"` sorts before `"…-9999-…"`. The counter only resets
  when the wall clock passes the clock's `ms`, so this is easy to reach: one device observes a
  stamp from another device whose clock is ahead, then ticks once per changed field. A first sync
  of a library with about 1,000 meetings (11 fields each) while a peer's clock is a minute ahead
  is enough. In debug builds the `u32` overflow would eventually panic too.
* **Scenario:** a Mac with 2,000 meetings turns on sync. The phone's clock is 30 s ahead. During
  the first pass every Mac stamp after the 9,999th sorts as older than earlier ones. Later edits
  lose to earlier ones on every device, and all devices converge on the wrong value, so the loss
  is silent. Reproduced: `order broke at tick 9999: 1763683260000-9999-d then 1763683260000-10000-d`.
* **Fix:** `tick` moves to the next millisecond when the counter would pass 9999, so written HLCs
  keep the fixed width and string order stays clock order. Received HLCs are compared as parsed
  `(ms, counter, device)` tuples (`hlc_cmp`), which also orders any five-digit stamps an older
  build already wrote. No format change; existing stamps keep their meaning.
* **Tests:** `merge_props.rs`: `ten_thousand_ticks_in_one_millisecond_stay_ordered`,
  `hlc_ticks_are_monotonic_and_fixed_width` (proptest), `legacy_five_digit_counters_still_order_correctly`;
  vectors `hlc.tick_overflow` and merge cases "counter rolls into the next millisecond" and
  "legacy five-digit counter orders after 9999".

### C1: the Mac can't parse the device card an iPhone sends while pairing

Not a security issue, but it would have stopped iPhone pairing from working at all.

* **Where:** `src/folder.rs:24` (`DeviceCard.last_seen` required), `src/desktop.rs:224`.
* **Problem:** the shared vectors (`pair.card_plain`) define the pairing card as
  `{"name":…,"kind":"iphone"}`, with no `last_seen`. The Mac deserialized it into `DeviceCard`,
  where `last_seen` was required, so `poll` failed with `missing field last_seen` for any phone
  following the vectors. The Mac's own end-to-end test passed only because its simulated phone
  sent a `last_seen`.
* **Fix:** `#[serde(default)]` on `last_seen`. Test: `folder.rs::a_device_card_without_last_seen_parses`
  and the `Joiner` (reference phone) used by every pairing test sends the vector shape.

## Medium

### M1: path traversal through a decrypted meeting id

* **Where:** `src/folder.rs:79-81` (`record_path` joins ids unchecked), `:107` (records grouped by the
  id *inside* the plaintext), `src/desktop.rs:403` (those ids join the sync list) and `:447`
  (`write_record` for each).
* **Attacker:** key holder.
* **Scenario:** a compromised or buggy device writes a validly sealed record whose `id` is
  `../../../../Library/Application Support/x`. On the next pass the Mac merges it, writes its own
  record to `<vault>/m/../../../../Library/Application Support/x/<device>.rec`, creating folders
  and files outside the sync folder anywhere the user can write, and creates a meeting with that id
  in its database. A `device` value containing `/` has the same effect through the file name.
* **Fix:** meeting and device ids must be one safe path component (`[A-Za-z0-9._-]{1,128}`, not
  `.`/`..`) before they are used in a path (`folder::valid_id`); `record_path`, `write_record` and
  `write_device` refuse anything else, and `read_records` skips folders with invalid names. Meeting
  ids are UUIDs, so no real id is affected.
* **Tests:** `folder.rs::ids_that_are_not_one_path_component_are_refused`.

### M2: temp-file writes follow planted symlinks

* **Where:** `src/folder.rs:35-42` (`write_atomic`).
* **Attacker:** folder attacker on a provider that syncs symbolic links (Dropbox, Syncthing and
  others; the crate explicitly supports "any synced folder").
* **Problem:** writes went to the predictable `<name>.tmp<pid>` via `std::fs::write`, which
  follows symlinks and truncates the target, and `create_dir_all` followed symlinked folders.
* **Scenario:** the attacker plants `m/<meeting>/<mac id>.tmp<pid>` → `~/.zshrc` for each likely
  pid (pids are small and the Mac app's is long-lived). The next time the Mac writes that record,
  `~/.zshrc` is replaced with ciphertext. Or `m/<meeting>` → `~/Documents/Important`, and records
  are written into that folder.
* **Fix:** the temp file gets a random name and is opened with `create_new` (O_EXCL, which fails
  on any existing path, including a symlink), then renamed over the final name (rename replaces a
  symlink rather than following it). Every folder from the vault root down is checked with
  `symlink_metadata` and the write is refused if any is a symlink.
* **Tests:** `folder.rs::writes_never_follow_symlinks`.

### M3: one malformed HLC freezes a field on every device forever

* **Where:** `src/merge.rs:163` (`s.hlc > cur.hlc` on raw strings), `src/merge.rs:65-74`
  (`observe` silently ignores what it can't parse).
* **Attacker:** key holder, which includes a buggy iPhone build.
* **Scenario:** a record carries `{"title": {"v": "x", "hlc": "~"}}`, or a 14-digit `ms`. As a
  string it sorts after every real stamp, so every device adopts it. `observe` can't parse it, so
  no device's clock ever moves past it, and no later edit can win. The title is frozen on every
  device with no way to recover short of editing every device's state by hand.
* **Fix:** remote fields whose HLC is malformed (`parse_hlc`: digits only, `ms` ≤ 13 digits'
  worth, a valid device id) are ignored, `observe` only follows valid stamps, and comparison uses
  the parsed tuple. The filter is deterministic (it doesn't depend on the local wall clock), so
  devices still converge.
* **Tests:** `merge_props.rs::malformed_or_hostile_stamps_are_ignored`; vector merge case
  "malformed and out-of-range remote stamps are ignored"; `hlc.valid`/`hlc.invalid`.

### M4: no device removal or key rotation (not fixed)

* **Where:** design; `src/desktop.rs:178-186` (`leave` forgets only the local key), `key_id`
  (`crypto.rs`, `vault.json`) is carried everywhere but never changes.
* **Scenario:** an iPhone is lost or sold without being wiped, or a family member's device was
  paired once. It keeps the vault key forever. With access to the folder (on iCloud, the same
  Apple ID the phone is signed into) it can read every future meeting and forge records. There is
  no way to revoke it short of deleting the vault and starting over.
* **Recommendation:** a "Remove device" action that creates a new key and `key_id`, re-seals every
  file the Mac can read, updates `vault.json`, and asks the remaining devices to pair again (or
  hands them the new key sealed under the old one, which a removed device could also read, so
  re-pairing is the safe default). `key_id_of` already exists to pick the key during the switch.
* **Why not fixed here:** it needs UI and a decision about re-pairing the remaining devices, and
  the `desktop` code can't be built in this environment. It should land before the iPhone app ships.

## Low

### L1: records aren't bound to their file path

* **Where:** `src/folder.rs:89-110`. The associated data covers magic, vault id and key id, but
  not the meeting id, device id or file kind. Readers used the ids inside the plaintext and
  skipped only the file named after their own device.
* **Scenario:** a folder attacker copies the Mac's own `m/A/<mac>.rec` to `m/A/other.rec`; the
  Mac reads its own (possibly old) record back as another device's. Merge is last-writer-wins,
  so stale copies can't win against newer stamps, which limits the impact.
* **Fix (compatible):** a record must sit at `m/<id>/<device>.rec` matching its own authenticated
  `id` and `device` (`Vault::read_record_at`). Putting the path in the associated data is planned
  for format 2. **Test:** `folder.rs::a_record_copied_to_another_name_is_rejected`.

### L2: `vault.json` is unauthenticated and not checked against the paired vault

* **Where:** `src/folder.rs:70-76`, `src/desktop.rs:119-120`.
* **Scenario:** a folder attacker edits `vault_id` in `vault.json` (the KCV still matches). The
  Mac opens the vault and seals every new file under the attacker's vault id; every other device
  rejects them as "another vault" and vice versa, and sync silently stops. `format` was never
  checked, so a future format bump couldn't stop old apps writing old files into a new vault.
* **Fix:** `Vault::open` refuses unknown formats; `Vault::open_expecting` also checks the vault id
  against the one this device paired with, and the Mac now uses it. The phone-side `Joiner` checks
  the QR's vault id and the granted key's KCV against `vault.json`. **Test:**
  `folder.rs::vault_json_is_checked_against_what_the_device_paired_with`.

### L3: unbounded reads; symlinks to special files hang the sync loop

* **Where:** `src/folder.rs:50`, `:102`, `:131` (`std::fs::read`).
* **Scenario:** a folder attacker plants `m/x/y.rec` → `/dev/zero`, or a multi-gigabyte file; the
  sync loop reads forever or runs out of memory on every pass.
* **Fix:** `read_capped` reads only regular files (not symlinks) up to a limit (64 MiB records,
  1 MiB cards, 16 KiB pairing files, 64 KiB `vault.json`). **Tests:**
  `folder.rs::oversized_files_are_not_read`, `writes_never_follow_symlinks`.

### L4: pairing invites never expire

* **Where:** `src/desktop.rs:207-243`. The app keeps a pairing open until it's cancelled; the CLI
  has its own 10-minute timeout.
* **Scenario:** the pairing sheet is left open; someone who photographs the QR code later can
  still answer it (they still need the person to accept a mismatched code).
* **Fix:** `Pairing` refuses to poll or accept 10 minutes after it was started. **Test:**
  `pairing.rs::an_expired_code_does_nothing`.

### L5: pairing message versions aren't checked

* **Where:** `src/desktop.rs:221-224`, `src/pair.rs:100-103`. `Request.v` and `Response.v` were
  ignored, so a future v2 message would be misread as v1 instead of refused.
* **Fix:** `pair::open_request` and `pair::open_grant` require `v == 1`. **Test:** `pairing.rs::versions_are_checked`.

### L6: device name in the confirmation prompt is attacker-chosen and unsanitised

* **Where:** `src/desktop.rs:224-225`. A QR observer chooses the name shown next to the code, e.g.
  with newlines, control characters or right-to-left overrides that reorder the prompt.
* **Fix:** `pairing::display_name` drops control and bidi-override characters and caps the name at
  64 characters. **Test:** `pairing.rs::names_are_made_safe_to_show`.

### L7: stale pairing files are never cleaned up

* **Where:** `src/desktop.rs:231-242`. `.resp` files (the vault key sealed under an ephemeral
  pairing key) stayed in the folder forever; requests from abandoned attempts too.
* **Fix:** starting a pairing sweeps `pair/` files older than an hour; cancel removes what it owns;
  the joining device deletes the response after reading it.

### L8: both pairing directions use the same key and associated data (not fixed)

* **Where:** `src/pair.rs:94-102` and `src/desktop.rs:224`: the phone's card and the Mac's grant
  are both sealed with `pair_key` and `aad = session`. Reflecting one as the other fails only
  because the plaintexts have different shapes (JSON vs 36 bytes).
* **Recommendation:** for pairing v2, derive one key per direction or use distinct associated
  data (`"milkrice-pair-req-v2" ‖ session`, `"…-resp-v2" ‖ session`). Changing it now would break
  the shared vectors the iPhone code is being written against, for no current exploit.

### L9: device cards aren't bound to a device id (not fixed)

* **Where:** `src/folder.rs:125-136`. Cards don't contain their id, so a folder attacker can swap
  two devices' card files and the Mac's device list shows the wrong names and last-seen times.
  Cards are display-only. **Recommendation:** add `id` to cards in format 2 (adding a field to
  `DeviceCard` now would also break the `desktop` code, which can't be built here).

### L10: a rolled-back own record isn't noticed by its author (not fixed)

* **Where:** `src/desktop.rs:445-450`. The Mac rewrites its own record only if the file is missing
  or the record changed since its last write, judged by a hash cached in `state.json`.
* **Scenario:** a folder attacker replaces the Mac's record with an older version of itself.
  Devices that already synced ignore the stale stamps (tested in
  `merge_props.rs::a_replayed_old_record_cannot_resurrect_a_deleted_meeting`), but a device paired
  afterwards sees the old values until the Mac next changes that meeting.
* **Recommendation:** have the Mac occasionally read back and verify its own records, or include
  the newest stamp in the cached hash and compare against the file. Full rollback protection
  against a folder attacker who also deletes newer files isn't possible without a trusted
  monotonic counter; document it as a limitation.

### L11: test key-file hook ships in release builds (not fixed)

* **Where:** `src/desktop.rs:75-78`, `:91-94`. If `MILKRICE_SYNC_KEYFILE` is set, the vault key is
  written in hex with default permissions (usually 0644) instead of the keychain. Whoever can set
  the app's environment already controls it, so the risk is low. **Recommendation:** compile it
  only under `cfg(any(test, debug_assertions))` or a `test-keyfile` feature, and create the file
  with mode 0600.

### L12: key material isn't zeroized (partly fixed)

* `Key` is a plain `[u8; 32]`, `Vault` derives `Clone`, and keys are hex-encoded into short-lived
  `String`s on their way to the keychain. The pairing state machines now clear their ephemeral
  secrets on drop. **Recommendation:** wrap `Key` in `zeroize::Zeroizing` when the `desktop` code
  can be rebuilt and tested.

## Info (no action needed)

* **I1 nonces:** random 96-bit nonces with one long-lived key shared by every device
  (`crypto.rs:78-79`). Collision probability stays below 2⁻³² until about 2³² sealed files,
  far beyond any realistic library. XChaCha20-Poly1305 (192-bit nonces) would remove the question
  in format 2. `seal_with_nonce` and the explicit-nonce pairing helpers exist for test vectors;
  production paths always pass fresh random nonces (checked in `crypto_props.rs::nonces_are_fresh`).
* **I2 KCV:** `HMAC-SHA256(key, "milkrice-kcv-v1")[..8]` in the public `vault.json` reveals nothing
  about a 256-bit random key. Its comparison isn't constant-time, but both sides are public.
* **I3 subkeys:** the vault key is used directly for ChaCha20-Poly1305 and as the HMAC key for the
  KCV. No known interaction, but format 2 should derive per-purpose subkeys with HKDF.
* **I4 metadata:** the folder reveals the number of meetings and devices, meeting ids (random
  UUIDs), sizes to 1 KiB, and edit times through modification times. Inherent to a file-based
  design; worth stating in the privacy docs.
* **I5 code bias:** `u32 mod 10⁶` for the 6-digit code has a relative bias of about 2×10⁻⁴; irrelevant.
* **I6 errors:** `open` distinguishes "too short / not a sync file / other vault / failed
  authentication", all before or at the tag check; no plaintext is processed before
  authentication, so there is no padding or format oracle.
* **I7 constant time:** tag verification is constant-time inside `chacha20poly1305`; the
  all-zero shared-secret check and the vault-id comparison involve public values only.

## Pairing protocol assessment

The QR code is the authentic channel (it carries `mac_pub`), and the folder is untrusted. HKDF
binds the vault id, session and both public keys, and the 6-digit code is derived from the X25519
secret. A folder attacker without the QR can't produce a request the Mac can open. A QR observer
can answer first, but can only compute the code the Mac shows, not the code the real phone
shows, so the person sees two different codes (success chance 10⁻⁶ per attempt). There is no
way to grind codes offline because the phone's code depends on a secret the attacker never sees.
Replay of an old grant fails because each session derives a new key and binds the session as
associated data. Low-order public keys are rejected. These properties are covered by
`tests/pairing.rs`, which runs both state machines through a real folder, including the stranger,
QR-observer and replay cases.

## Format compatibility

No change to any byte on disk or on the wire: every existing vector in `tests/vectors/v1.json`
is unchanged and still checked. New vectors were added (padding edges, HLC rules, hostile-stamp
merge cases, exact pairing messages). Behaviour changes a Swift implementation must match are in
`docs/SYNC-FORMAT.md`: the counter rollover in `tick`, tuple comparison of HLCs, ignoring
malformed stamps, path binding of records, and the version checks.

## Tests added

| File | What |
| --- | --- |
| `tests/merge_props.rs` | proptest: 2–4 simulated devices, skewed clocks, random edits/deletes/undeletes/syncs/replays: convergence, newest stamp wins, causality, order independence, idempotence, HLC monotonicity; plus fixed cases for tombstones and hostile stamps |
| `tests/crypto_props.rs` | proptest: seal round trip and padding size, every single-bit flip, truncation/extension, wrong key/vault, box AAD binding, QR round trip, both pairing sides agree; nonce freshness; malformed QR codes |
| `tests/pairing.rs` | the Mac and phone state machines end to end through a folder, early accept, cancel, expiry, half-written and forged requests, QR observer, grant replay, versions, tampering, low-order keys, display names |
| `tests/folder.rs` | id validation, path binding, symlinks, size caps, `vault.json` checks, device cards |
| `tests/vectors.rs` | the shared vectors, extended |
