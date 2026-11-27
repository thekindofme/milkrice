# Privacy audit, November 2026

Date: 2026-11-27. Scope: the Mac app (`src-tauri/`, `ui/`), the engine (`crates/milkrice-core`), iPhone sync on the Mac side (`crates/milkrice-sync`), the agent integration (`skills/`, `.claude-plugin/`, `scripts/milkrice-mcp`), the website (`site/`, `functions/`) and the privacy docs. The iPhone app lives in another repository and was not reviewed.

Method: static review on Linux. The app cannot be built here (it needs macOS frameworks, and the ONNX Runtime download is blocked), so nothing was run end to end. The changed functions were compiled and unit tested on their own (see "Verification" at the end). Line numbers in the claims table refer to `main` at `5291d7d`, before the fixes on this branch.

## Summary

The core promise holds. Audio never leaves the Mac, there is no Milkrice server, no telemetry SDK and no analytics in the app, the webview CSP only allows local content, and every speech model download is pinned to a commit and checked against a SHA-256 before use. Nothing reaches an AI provider until the user picks one.

The gaps are in the details the copy left out, and in two places where untrusted text meets tools:

| # | Finding | Severity | Status |
|---|---|---|---|
| 1 | Consent page told users to say "nothing is shared" while the transcript goes to Anthropic or OpenAI | High (users repeat it to others) | Fixed |
| 2 | Ollama auto-selected the first listed model, which can be an Ollama cloud model running on ollama.com | High (contradicts "nothing leaves it") | Fixed |
| 3 | Folder suggestion sends attendee email addresses, folder names and other notes' titles; not documented anywhere | Medium | Fixed (docs) |
| 4 | A secret iCal feed URL could be written to `milkrice.log` and shown in the UI through a reqwest error | Medium | Fixed |
| 5 | Prompt injection: meeting transcripts reach tool-enabled agents (MCP, skill) and the Codex CLI, which can read files | Medium | Partly fixed (agent instructions); Codex remains |
| 6 | Privacy page named the wrong host for the Moonshine streaming model (`download.moonshine.ai`) | Low | Fixed |
| 7 | Downloads followed redirects to plain HTTP (integrity was still protected by the hash) | Low | Fixed |
| 8 | Unused `models::download_file` fetched URLs with no checksum | Low | Removed |
| 9 | Codex output file left in the temp folder when the run failed or timed out | Low | Fixed |
| 10 | Data folder permissions relied on `~/Library` being private | Low | Fixed (0700) |
| 11 | Update check runs on every launch with no off switch, and its downloads are counted as an install metric | Low | Open |
| 12 | Deleted meetings and nulled voice prints can survive in free SQLite pages | Low | Open |
| 13 | Claude Code and Codex CLIs run with their own telemetry settings | Low | Open |

## 1. Every network request and spawned process

### Network requests from the Mac app and CLI

| What | Code | Data that leaves | When | Opt-in | Documented |
|---|---|---|---|---|---|
| Speech model download (Hugging Face) | `crates/milkrice-core/src/models.rs` `download_verified`, URLs in `catalog.rs:80-125` | IP, user agent `Milkrice/0.1`, file requested | User clicks Download (or `milkrice-cli download`) | Yes | Yes (privacy page; host was wrong, fixed) |
| Speaker model download (GitHub releases, sherpa-onnx) | `models.rs:206-219`, URL `diarize/mod.rs:7` | IP, file requested | First recording with speaker recognition on, or after a model download | Implicit (speaker recognition is on by default) | Yes |
| Update check and install (GitHub releases `latest.json`, then the update bundle) | `src-tauri/src/lib.rs:67-73`, `updater.rs`, `tauri.conf.json:77-82` | IP, request; GitHub counts `latest.json` downloads, which `docs/ANALYTICS.md:15` uses as "roughly how many Macs still run Milkrice" | 8 s after every launch, and from the menu | No off switch | Yes (README, privacy page, ANALYTICS.md) |
| Calendar feed (iCal URL the user pastes) | `crates/milkrice-core/src/ics.rs:11-30`, `src-tauri/src/ics.rs` (every 10 minutes) | IP, the secret URL, to the calendar host | Only if the user adds a feed | Yes | Yes |
| Ollama | `llm.rs:221-230`, `llm.rs:271-280` | Nothing off the Mac (`127.0.0.1:11434`), unless the model is an Ollama cloud model | Settings, and after meetings if Ollama is chosen | Yes | Yes (cloud model caveat added) |
| iPhone sync folder (iCloud Drive or another folder) | `crates/milkrice-sync`, `src-tauri/src/sync.rs` | Sealed files (ChaCha20-Poly1305, padded to 1 KB, random names), uploaded by iCloud, not by Milkrice | Only once sync is set up | Yes, off by default | Yes |
| Obsidian export | `src-tauri/src/export.rs`, `obsidian.rs` | Plain Markdown, written to a folder the user picks. If that vault is in iCloud or another synced folder, it is uploaded unencrypted by that service | Only once a vault is chosen | Yes | Partly (README says plain Markdown; does not warn that a synced vault uploads it) |

There are no other HTTP clients. `reqwest` is used only in `models.rs`, `ics.rs` and `llm.rs` (Ollama). The UI makes no `fetch` calls; `ui/mock.js` only runs outside Tauri (`ui/mock.js:3`).

### Processes the app spawns

| Process | Code | Purpose | Data passed |
|---|---|---|---|
| `claude -p ... --tools "" --no-session-persistence --setting-sources "" --strict-mcp-config` | `llm.rs:236-255` | AI notes, titles, speaker names, Q&A, folder pick | Prompt on stdin (not argv); fixed system prompt as argv. Goes to Anthropic |
| `claude auth status`, `codex login status` | `llm.rs:159-203` | Detect signed-in providers in Settings | Nothing from meetings |
| `codex exec --ephemeral --sandbox read-only -o <tmp file> -` | `llm.rs:256-270` | Same as Claude | Prompt on stdin. Goes to OpenAI |
| `$SHELL -ilc 'printf ...$PATH'` | `llm.rs:34-50` | Read the login PATH | None; runs the user's shell startup files |
| `claude mcp add/remove` | `agents_install.rs:595-640` | Register the MCP server | None |
| `/usr/bin/afconvert` | `audio_rec.rs:108` | WAV to AAC for kept recordings | Local files only |
| `/usr/bin/open` | `permissions.rs:76`, `commands.rs:111,865` | Open System Settings, Finder, links | URLs and paths |
| `scutil` | `milkrice-sync/src/desktop.rs:133` | Computer name for the sync device card | Name goes into the encrypted vault |

### Webview security

- CSP (`tauri.conf.json:38`): `default-src 'self'`, so `connect-src` falls back to `'self'` and the UI cannot call out. `style-src 'unsafe-inline'` is the only loosening and does not allow script. Not overly broad; no change made.
- Asset protocol scope is limited to `Application Support/Milkrice/audio/**` (`tauri.conf.json:42`).
- Capabilities (`capabilities/default.json`): `opener:default` (opens http(s), mailto and tel links in the browser), `dialog:allow-open`, `notification:default`, `updater:default`. Reasonable.
- Updates are verified against the minisign public key in `tauri.conf.json:78` before install.

### Website

- Cloudflare Web Analytics beacon, injected by Cloudflare for the zone (`docs/ANALYTICS.md:11`). No cookies.
- `functions/download.js` writes one Analytics Engine data point per Download click: source tag, campaign, button, page path (internal referrers only), referring domain (external), country, coarse platform. It does not store IP or user agent. Matches the privacy page.
- Google Fonts on every page (`site/*.html:12-14`). Disclosed on the privacy page.

## 2. Privacy claims

TRUE: the code matches. MISLEADING: true in a narrow reading but leaves out something a reader would want to know. FALSE: the code contradicts it.

### README.md

| Line | Claim | Verdict | Evidence |
|---|---|---|---|
| 5 | Audio is never uploaded; kept only with Keep recordings | TRUE | `recorder.rs:288` sets `record_to` only when `keep_audio`; no code path sends audio |
| 5 | Models include Cohere Transcribe and SenseVoice | FALSE (not privacy) | `catalog.rs` `MODELS` has no SenseVoice or Cohere entry; `COHERE_14` is `#[allow(dead_code)]`. Left for the maintainer |
| 8 | Until you choose a provider, "nothing is sent anywhere" | MISLEADING, fixed | True for AI (`recorder.rs:342-343,372-377`), but model downloads and the update check still run. Now "nothing is sent to any AI" |
| 150 | No account, no telemetry, no Milkrice server | TRUE | No telemetry code; the only outbound calls are listed above |
| 151 | Kept recordings deleted after the retention period | TRUE | `audio_rec.rs:166-183`, run at launch and hourly (`lib.rs:51-57`, `scheduler.rs:147`) |
| 152 | Voice prints only in memory | TRUE | `state.rs:109` in-memory map; `store.rs:190-191` nulls old ones (see finding 12) |
| 153-157 | What goes over the network | MISLEADING, fixed | Missed the folder-suggestion data and iPhone sync; now listed |
| 159 | Sync is end-to-end encrypted, key in the keychain | TRUE | `milkrice-sync/src/crypto.rs`, `desktop.rs:71-117` |

### site/privacy.html

| Line | Claim | Verdict | Evidence |
|---|---|---|---|
| 48 | No usage data or crash reports; no licence check | TRUE | No such code |
| 53-55 | Website analytics description | TRUE | `functions/download.js:43-55` |
| 62 | Settings, including feed addresses, in `milkrice.sqlite` | TRUE | `state.rs:142-143` |
| 64 | Recordings: off by default, 30-day default retention, deleted with the meeting | TRUE | `state.rs:67-68`, `commands.rs:566` |
| 64 | About 15 MB an hour | Unverified | `audio_rec.rs:2` says about 20 MB an hour |
| 66 | AI requests run in `milkrice-llm` temp folder; Codex output deleted | MISLEADING, fixed | Was not deleted on failure (finding 9) |
| 79 | What each AI request contains | MISLEADING, fixed | Folder pick also sends attendee emails, folder names and note titles (`recorder.rs:192-232`); prompts include the user's name (`recorder.rs:526-532`) |
| 81 | Claude: tools off, no session saving | TRUE | `llm.rs:245-250` |
| 82 | Codex: read-only sandbox, ephemeral | TRUE | `llm.rs:261`. Note that read-only still lets Codex read any file the user can (finding 5) |
| 83 | Ollama: nothing leaves your computer | MISLEADING, fixed | Cloud models were eligible (finding 2) |
| 88 | Models from huggingface.co or download.moonshine.ai, checksummed | FALSE in part, fixed | All URLs are huggingface.co (`catalog.rs:119-125`); checksums TRUE (`models.rs:145-149`) |
| 91 | Update check sends no identifiers; updates signed | TRUE | `updater.rs`, `tauri.conf.json:78` |
| 94 | Feed fetched over HTTPS, stored locally | TRUE | `ics.rs:14-16` |
| 97 | Agents read on the Mac, read-only | TRUE | `mcp.rs` exposes five read tools; DB opened read-only (`mcp.rs:131`, `store.rs:204-205`) |
| 106 | Sync off until paired; Mac sends to the chosen AI when the iPhone asks | TRUE | `sync.rs:53-55`, `sync.rs:118` |

### site/index.html

| Line | Claim | Verdict | Evidence |
|---|---|---|---|
| 44, 351 | Transcribed on-device; audio never leaves your Mac | TRUE | |
| 378 | No account, no analytics, no telemetry | MISLEADING (not changed) | The app has none, but `latest.json` downloads from the update check are counted as an install metric (`docs/ANALYTICS.md:15`). Suggest adding "The app only checks GitHub for updates" |
| 414 | Until you pick an AI, nothing is sent anywhere | TRUE in context (AI section) | `recorder.rs:342-343` |
| 424, 373 | Ollama: nothing leaves your computer | MISLEADING, fixed in code | Finding 2 |
| 438 | Agent access is read-only | TRUE | |
| 567 | FAQ: what is sent to the AI | MISLEADING, fixed | Finding 3 |

### Other pages and docs

| Where | Claim | Verdict | Evidence |
|---|---|---|---|
| `site/consent.html:54` | Suggested script: "It transcribes on my Mac and nothing is shared" | FALSE for Claude Code and Codex users, fixed | `llm.rs:233-270` sends the transcript to the provider |
| `site/compare.html:36,72` | Ollama: nothing leaves your machine | MISLEADING before the Ollama fix, now TRUE for automatic selection | |
| `site/compare.html:74` | Voiceprints never written to disk | TRUE | See finding 12 for old builds |
| `site/agents.html:58` | Nothing leaves your Mac unless your agent sends it to its own model | TRUE | |
| `site/agents.html:126` | MCP opens no port; stdio only | TRUE | `mcp.rs:84-110` |
| `SECURITY.md:12` | Audio, transcripts and notes stay on your Mac | MISLEADING (not changed) | Transcripts and notes go to the chosen AI provider. Suggest "audio stays on your Mac; transcripts and notes leave it only for the AI provider you choose" |
| `docs/ANALYTICS.md:3` | The apps collect nothing | TRUE, with the update-check note on line 15 | |
| Footer on every page | No cookies, doesn't track you | TRUE | Aggregate, cookieless analytics only |

### In-app strings (`ui/app.js`)

| Line | String | Verdict |
|---|---|---|
| 538-547 | Badge: "On this Mac" only for Ollama or no provider; otherwise "notes via Claude/Codex" | TRUE (Ollama cloud models now excluded from automatic selection) |
| 668 | Audio is processed on this Mac and never uploaded | TRUE |
| 843, 1635 | No AI chosen: nothing has left this Mac | TRUE for meeting content |
| 852, 1377, 1387, 1733, 1741 | Transcript text, never audio, goes to the chosen AI | MISLEADING (minor): also title, attendee names and emails, folder names. Suggest "the meeting's text" |
| 853 | Ollama: "On this Mac. Nothing leaves it." | TRUE for automatically chosen models; FALSE if the user names an Ollama cloud model in Settings |
| 1483 | iCloud folder holds only sealed files; Apple can't read them | TRUE |

## 3. Data at rest

- **Location.** `~/Library/Application Support/Milkrice` (`paths.rs:53-71`): `milkrice.sqlite` (+ WAL/SHM), `models/`, `audio/<meeting-uuid>/`, `milkrice.log`, `milkrice.log.1`. Settings, including secret feed URLs and the Obsidian vault path, are a JSON row in the database (`state.rs:142-143`).
- **Permissions.** Files are created with the default umask. `~/Library` is 0700 on macOS, so other local users cannot read them, but the data folder itself is now set to 0700 at launch (fixed).
- **Encryption.** None beyond FileVault. The SQLite database and recordings are plain files readable by any process running as the user, which `site/agents.html:126` states honestly.
- **Retention.** Recordings: per-file mtime, default 30 days (`audio_rec.rs:166`). Transcripts and notes: kept until the user deletes the meeting. Deleting a meeting removes its recordings (`commands.rs:566`) and rows (cascade), but SQLite does not overwrite freed pages without `secure_delete` (finding 12).
- **Temporary files.** Uncompressed `*.mic.wav` / `*.sys.wav` sit in the audio folder during a kept recording, and stay there after a crash until retention removes them. AI runs use `$TMPDIR/milkrice-llm` (per-user on macOS); Codex writes its output there and Milkrice now deletes it on every path.
- **Log.** `src-tauri/src/lib.rs:202-222`. I reviewed all 63 `log::` calls. None write transcript text or notes (`session.rs:681` deliberately logs only lengths). Logged items: device names, calendar feed display names and host only (`src-tauri/src/ics.rs:46`, `commands.rs:352`), meeting UUIDs and audio paths, update versions, AI error lines. Two leaks were possible: a reqwest body-read error with the full secret feed URL (fixed), and an AI CLI failure message, which is the last stderr line or the stdout text (`llm.rs:131-133`); that could in rare cases quote model output. Rotation only happens at launch (over 5 MB), so a long-running session can grow past that.
- **AI prompts.** Built in `llm.rs:354-469` and `recorder.rs:192-232`. Notes: title, date, invitee names (marked "(me)"), template, user notes, transcript up to 60,000 characters (middle cut). Q&A: same plus up to 6 previous turns. Ask-all: up to 70,000 characters across meetings. Titles: 6,000 characters. Speaker names: candidate names and 30,000 characters of raw labelled transcript. Folder pick: folder names, up to 4 note titles per folder, title, attendee names with email addresses, 2,500 characters of note text. Up to four requests per meeting run automatically when `auto_enhance` is on (default true).

### Prompt injection

Anyone who speaks on a recorded call controls part of the transcript.

- **Claude Code path:** `--tools ""`, `--setting-sources ""` and `--strict-mcp-config` with no MCP config, so the model has no tools. Injection can only change the notes text. Low risk.
- **Codex path:** `--sandbox read-only` blocks writes and network, but Codex can still run read-only shell commands, for example reading `~/.ssh` or browser profiles, and put the result into the notes. Those notes are then mirrored to Obsidian (possibly a synced vault), synced to the iPhone, and sent to the AI again later. The only guard is a sentence at the end of the prompt (`llm.rs:265`). Medium risk. Recommended: find a Codex option that disables the shell tool, or run Codex with a scratch `HOME`/sandbox profile that cannot read the user's files; at minimum wrap the transcript in clear delimiters and say it is untrusted data.
- **Agents through MCP or the skill:** these agents usually have shell, file and network tools, and a transcript is exactly the kind of content they will read in full. The MCP server and skill now tell agents to treat meeting content as data and never act on requests inside it (fixed). This reduces risk; it does not remove it.
- **iPhone requests:** a paired iPhone can ask the Mac to write notes for a meeting (`sync.rs:118`), which runs the same AI path. Only paired devices with the vault key can do this.

### CLI and agent plugin permissions

- The MCP server (`milkrice mcp`) exposes five read-only tools, opens the database read-only and listens on stdio only. The `milkrice` CLI read commands are the same; `milkrice agents install` writes agent config files with backups (`agents_install.rs:1-11`).
- The Claude Code plugin (`.claude-plugin/plugin.json`) declares only the MCP server. `scripts/milkrice-mcp` searches fixed locations first, then falls back to `command -v milkrice`, so a `milkrice` earlier on PATH would be run. That is the normal trust model for PATH, but the fallback could be dropped.
- `SKILL.md` has no `allowed-tools`; it relies on the agent's own permission prompts to run `milkrice`.

## 4. Model download integrity

- Every speech model file is pinned to a Hugging Face commit hash in the URL and to a size and SHA-256 (`catalog.rs:80-125`). The hash is computed while streaming and the file is only moved into place if both match (`models.rs:103-153`). TRUE.
- The speaker model URL points at a GitHub release tag that can be changed upstream (`diarize/mod.rs:7`), but its size and SHA-256 are pinned (`models.rs:209-214`), so a changed file is rejected. TRUE.
- All URLs are HTTPS. Redirects to plain HTTP were followed before; clients now use `https_only(true)` (fixed).
- `is_downloaded` checks only file sizes after install (`models.rs:24-35`). Tampering would need local write access to the user's files, so this is acceptable.
- `models::download_file`, which had no hash check, was unused and has been removed.
- App updates are signature-checked by the Tauri updater; DMGs have `SHA256SUMS.txt` on the release.

## 5. Prioritised fixes

Done on this branch (one commit each):

1. Strip the secret URL from calendar fetch errors (`ics.rs`).
2. HTTPS-only downloads and calendar fetches; remove the unverified download helper (`models.rs`, `ics.rs`).
3. Ollama: never auto-select a cloud model (`llm.rs`, with a unit test).
4. MCP instructions and SKILL.md: treat meeting content as data, not instructions.
5. Data folder set to 0700 (`paths.rs`, with a unit test).
6. Privacy page: correct the Moonshine host, list folder-suggestion data, Ollama cloud caveat, all agents, `milkrice.log`.
7. Consent page: the suggested script no longer says "nothing is shared".
8. README and site FAQ: exact list of what goes to the AI.
9. Codex output file deleted on failure too (`llm.rs`).

Still open, in priority order:

1. **P1. Codex tool access.** Stop Codex from running shell commands on meeting content (see Prompt injection). Until then, say on the privacy page that Codex runs in a read-only sandbox that can still read files.
2. **P1. Ollama model the user names.** If `provider_model` is an Ollama cloud model, show the "notes via Ollama cloud" badge instead of "On this Mac".
3. **P2. Folder suggestion data.** Consider not sending attendee email addresses (names are enough to match folders), or make the folder suggestion a separate setting.
4. **P2. Update check toggle.** Add a setting to turn off the launch check, and mention on the home page that the app checks GitHub for updates (index.html:378).
5. **P2. SQLite secure delete.** Set `PRAGMA secure_delete = ON` on every writable connection (`store.rs` and `milkrice-sync/src/desktop.rs:307`), and run one `VACUUM` after the voice print migration so old voice prints and deleted meetings do not linger in free pages.
6. **P2. SECURITY.md and in-app strings.** Reword "audio, transcripts and notes stay on your Mac" and "the transcript text is sent" to match what is sent.
7. **P3. Provider CLI telemetry.** Consider setting `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` (and the Codex equivalent, if any) in the environment of the spawned CLIs, and say on the privacy page that those tools have their own telemetry settings.
8. **P3. Log hygiene.** Truncate AI CLI error text before logging, and rotate the log while running, not only at launch.
9. **P3. Obsidian.** Warn that a vault inside iCloud Drive or Dropbox uploads notes and transcripts unencrypted.
10. **P3. Website fonts.** Self-host the Fraunces font to drop the Google Fonts request.
11. **P3. README model list.** Remove SenseVoice and Cohere Transcribe, or add them to the catalog.
12. **P3. Plugin launcher.** Drop the `command -v milkrice` fallback in `scripts/milkrice-mcp`.

## Verification

- `cargo check` of the workspace could not run here: `ort-sys` needs to download ONNX Runtime (blocked by the network policy) and `alsa-sys` needs ALSA headers. CI on macOS will build and test the branch.
- The changed functions (`is_cloud_model`, `first_local_model`, `owner_only`, `ics::fetch`) were copied into a scratch crate against the same `reqwest 0.12.28` and `serde_json` and tested: cloud models are skipped, the folder ends up 0700, and a failed fetch of a URL containing a secret produces an error without it.
- Site changes are copy only; no layout changed.
