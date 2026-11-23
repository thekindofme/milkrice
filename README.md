# Milkrice

Local-first AI meeting notes for macOS, written in Rust. An open-source alternative to tools like Granola, with no account and no sign-up:

- **Listens on your Mac.** It captures your mic ("Me") and system audio (everyone else on the call), then transcribes live with an **open-weights model you download once**. The model choice follows Handy: Parakeet v2/v3, Canary, Cohere Transcribe, Whisper, Moonshine and SenseVoice. Audio is never uploaded, and it's only kept on disk if you turn on **Keep recordings**.
- **Knows who's talking.** A local speaker-embedding model (WeSpeaker ResNet34) separates the remote voices. Names come from the calendar invite and from cues in the conversation ("thanks, Priya"), and you can rename or merge speakers with a click.
- **Hooks into your calendar.** It reads the calendars already synced to macOS (iCloud, Google, Exchange) through EventKit. You get a notification (and a "Next: … · Start notes" item in the menu bar) as a meeting starts, or it can record automatically. While auto-record is on, Milkrice shows it in the sidebar and menu bar, with a one-click "Turn off". It stops when the meeting is over and the room goes quiet. It also notices unscheduled Zoom, Teams, Webex, Slack and FaceTime calls that are using the mic.
- **Writes your notes with the AI you already pay for.** It uses **Claude Haiku** through the Claude Code CLI or **Codex Luna** through the Codex CLI, reusing their existing subscription logins. A fully offline Ollama option is also available. No API keys are needed. You choose the provider once during setup and Milkrice remembers it. Until you do, nothing is sent anywhere and notes aren't written.
- **Granola-style workflow:**
  - Jot rough notes during the call. Afterwards Milkrice expands them with detail from the transcript, using templates for 1:1s, stand-ups, customer calls, interviews and more.
  - The AI notes are yours to edit: action items tick off, and the note says which model wrote it. Speaker names the AI guessed are marked until you confirm them.
  - Ask questions about the meeting ("draft a follow-up email").
  - Search across every note and transcript.
  - Export notes as Markdown.

## For agents: CLI, MCP and skill

Milkrice exposes your meetings to Claude Code, Codex, Cursor, OpenCode, VS Code (Copilot) and any MCP client. Access is read-only, and every piece of content is labelled with its source:

- **NOTES**: what you typed.
- **AI SUMMARY**: written by an LLM, which is named.
- **TRANSCRIPT**: speech-to-text from a named model. Your own lines are marked `(user)`.

**Connect from the app:** Settings → Agents → **Connect**. It installs the `milkrice` command if needed, registers the MCP server and installs the skill ([`skills/milkrice/SKILL.md`](skills/milkrice/SKILL.md)).

**Connect from a terminal:**

| Agent | Command |
|---|---|
| Claude Code (plugin) | `claude plugin marketplace add thekindofme/milkrice` then `claude plugin install milkrice@milkrice` |
| Claude Code (direct) | `milkrice agents install claude` |
| Codex | `milkrice agents install codex` |
| Cursor | `milkrice agents install cursor` |
| OpenCode | `milkrice agents install opencode` |
| VS Code (Copilot) | `milkrice agents install vscode` |
| Any agent that reads `~/.agents/skills` | `milkrice agents install skill` |
| Any MCP client | `{"command": "milkrice", "args": ["mcp"]}` |

On Claude Code 2.1.275 or later, `claude plugin install milkrice --marketplace thekindofme/milkrice` (or `/plugin install milkrice --marketplace thekindofme/milkrice` inside a session) does the plugin install in one step. The plugin needs the Milkrice app installed; it runs the app's `milkrice mcp`.

`milkrice agents install all` connects every agent it finds. `milkrice agents status` shows what is connected and flags problems, and `milkrice agents uninstall <agent|all>` reverses it. The installer only adds or removes its own `milkrice` entry, backs a config file up (`<file>.milkrice-backup-<time>`) before changing it, and can be re-run safely. Use the plugin or the direct install for Claude Code, not both.

If you installed the DMG, `milkrice` goes in `~/.local/bin`, which a new zsh doesn't put on PATH. `milkrice agents status` tells you when that's the case and how to fix it; MCP works either way because it is registered with the full path. Homebrew links `milkrice` into `/opt/homebrew/bin`.

| Command | What it gives you |
|---|---|
| `milkrice ls [-q TEXT] [--since 7d] [-f FOLDER]` | One line per meeting, with which content exists |
| `milkrice folders` | Your folders and how many meetings each holds |
| `milkrice show ID [notes,ai,transcript\|all]` | Labelled sections; transcript left out by default |
| `milkrice tx ID [--from mm:ss --to mm:ss --speaker NAME --grep TEXT]` | Only the transcript lines you need |
| `milkrice search TEXT [--in notes\|ai\|transcript]` | Hits say where they matched |
| `milkrice mcp` | MCP server on stdio with 5 tools: `list_meetings`, `list_folders`, `get_meeting`, `get_transcript`, `search` |

## Install

Apple Silicon Macs, macOS 14.2 or later.

**Homebrew**

```bash
brew install --cask thekindofme/tap/milkrice
```

**Direct download.** Get the DMG from the [latest release](https://github.com/thekindofme/milkrice/releases/latest), open it and drag Milkrice to Applications. Checksums are in `SHA256SUMS.txt` on the release page.

**Build from source.** See [Build and run](#build-and-run) and [CONTRIBUTING.md](CONTRIBUTING.md).

Milkrice checks GitHub for new releases shortly after launch (and from "Check for updates…" in the menu bar) and asks before installing one.

## Layout

```
crates/milkrice-core   the engine (no UI)
  audio/             mic (cpal) + system audio (Core Audio process tap, ScreenCaptureKit fallback) → 16 kHz mono
  vad.rs             adaptive energy VAD, cuts streams into utterances
  stt.rs, catalog.rs transcribe-rs engines + model catalog
  models.rs          download / unpack / delete models
  diarize/           Kaldi fbank + WeSpeaker embeddings + online clustering
  session.rs         live pipeline: capture → VAD → STT → speakers → echo removal
  calendar.rs        EventKit (events, attendees, call links)
  detect.rs          "is a meeting app using the mic?"
  llm.rs             Claude Code / Codex / Ollama runners + prompts
  store.rs           SQLite (WAL) storage
  agents_install.rs  installs the CLI, MCP entries and skill for each agent
  mcp.rs             the read-only MCP server
  bin/milkrice-cli.rs  headless tool, including a full pipeline simulator
src-tauri            the desktop app: commands, scheduler, tray, notifications
ui                   the interface: plain HTML/CSS/JS, no build step
skills/milkrice      the agent skill (also embedded in the CLI)
.claude-plugin       Claude Code plugin + marketplace (scripts/milkrice-mcp is its MCP entry)
design               brand notes and Codex-generated artwork
```

## Build and run

```bash
cargo tauri build --bundles app
```

```bash
open target/release/bundle/macos/Milkrice.app
```

Requirements: macOS 14.2+, Rust, `cargo install tauri-cli`, and cmake (for whisper.cpp).
On first launch, Milkrice asks for permission to use the microphone, calendars and "Screen & System Audio Recording". The last one is needed to hear the other participants; your screen is never captured.

### Headless

```bash
cargo run --release -p milkrice-core --bin milkrice-cli -- models
```

```bash
cargo run --release -p milkrice-core --bin milkrice-cli -- simulate parakeet-v2 mic.wav system.wav claude
```

`simulate` streams two 16 kHz mono WAVs (your mic and the call audio) through the exact live pipeline. It then names the speakers, titles the meeting, writes enhanced notes and answers a question with the chosen AI.

### Automated end-to-end checks

These run the real app binary with test audio in place of the live devices:

```bash
MILKRICE_SELFTEST=dir/ target/release/milkrice
```

Plays `dir/mic.wav` and `dir/system.wav` through the recorder, storage and AI pass, then quits.

```bash
MILKRICE_TEST_EVENT=50 MILKRICE_QUIET_SECS=15 MILKRICE_TEST_AUDIO=dir/ target/release/milkrice
```

Injects a calendar meeting. The real scheduler auto-starts recording, auto-stops after the event ends and the room goes quiet, then names speakers and writes the notes.

## Performance (M5 MacBook Air, CPU int8)

| Model | 5 s utterance | Peak RAM |
|---|---|---|
| Parakeet v2 | 0.27 s (~19× real time) | 1.9 GB |
| Canary 180M Flash | 0.34 s | 0.7 GB |
| Moonshine Base | 0.25 s | 0.6 GB |
| Whisper Small (Metal) | 1.1 s | 0.8 GB |

- **Full pipeline (two streams, diarization, echo removal):** 26× real time, so a 44 s meeting takes 1.7 s.
- **Model load:** 0.5 s.
- **Idle app:** ~0% CPU, ~100 MB RAM.

## Privacy

- **Stays on your Mac:** audio, transcripts, notes and models live in `~/Library/Application Support/Milkrice`. There is no account, no telemetry and no Milkrice server.
- **Recordings:** audio is processed in memory and discarded. With **Keep recordings** on, Milkrice keeps a compressed copy per meeting and deletes it after the retention period you choose.
- **Speaker voice prints:** these are kept in memory only, never written to disk.
- **What goes over the network:**
  - Model downloads.
  - Your calendar feed, if you add a secret iCal address.
  - A check for new Milkrice releases on GitHub (the app asks before installing anything).
  - When notes are written, the transcript text and your notes go to the AI you chose (Claude Code or Codex, using your own subscription). Choose Ollama to keep even that on-device.
- **Obsidian:** export writes plain Markdown files into the folder you pick.
- **iPhone sync (optional, off by default):** once you pair an iPhone, meetings sync through a folder in your iCloud Drive (or any folder you choose). Every file is end-to-end encrypted with a key that stays in each device's keychain, so Apple, or whoever hosts the folder, sees only sealed files with random names.

## License

MIT. See [LICENSE](LICENSE). The speech models Milkrice downloads keep their own licenses, some of which require attribution; see the acknowledgements page on the website (`site/acknowledgements.html`).

Milkrice is an independent project. It isn't affiliated with Granola, Anthropic, OpenAI or the Handy project. Their names are used only to describe compatibility.
