# Changelog

All notable changes to Milkrice are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## Unreleased

- Privacy: Ollama "cloud" models (which run on ollama.com) are no longer picked automatically; with no local model, notes fail with a message saying how to fix it
- Privacy: secret iCal feed addresses no longer appear in errors or `milkrice.log`; model, speaker-model and calendar downloads require HTTPS; the data folder is created owner-only (0700); Codex's temporary output file is deleted when a run fails
- Privacy: agents reading meetings through the MCP server or skill are told to treat meeting content as data, never as instructions
- Fix: search treated `_` in a query as a wildcard; recurring meetings whose UNTIL didn't match DTSTART's type showed only once; speaker maps with non-text values failed to load
- Sync: hardened ahead of the iPhone app (stamp counter overflow, pairing card fields, record ids limited to safe file names, no following planted symlinks); see docs/audits/sync-security-review-2026-11.md and docs/SYNC-FORMAT.md
- iPhone sync (Settings › iPhone sync): pair Milkrice for iPhone (beta) with a QR code and a matching 6-digit code, then meetings sync both ways through iCloud Drive (or any shared folder), end-to-end encrypted with a key kept in each device's keychain. Meetings from the iPhone are marked in the sidebar, mirrored to Obsidian, and the Mac writes their notes with your chosen AI when the iPhone asks
- Fix: the Obsidian section in Settings never finished loading ("Looking for vaults…"), so a vault could only be chosen during onboarding
- Sync: the Mac keeps sending where each meeting was recorded, so a device paired later learns it, and never labels a meeting it received as recorded on the Mac

## 0.1.0 (2026-11-21)

First public release.

- Live transcription of mic and system audio with downloadable open-weights models (Parakeet, Canary, Whisper, Moonshine, SenseVoice, Cohere Transcribe)
- Local speaker separation, with names from calendar invites and conversation cues
- Calendar integration through EventKit and optional iCal feeds, with notify or auto-record modes
- AI-written notes through Claude Code, Codex or Ollama
- Search, folders, Markdown and Obsidian export
- `milkrice` command-line tool, MCP server and agent skill for read-only access to meetings
- `milkrice agents status|install|uninstall` for Claude Code, Codex, Cursor, OpenCode, VS Code (Copilot) and the shared skill; Settings → Agents uses the same installer and also installs the `milkrice` command
- Claude Code plugin and marketplace in this repository: `claude plugin marketplace add thekindofme/milkrice`, then `claude plugin install milkrice@milkrice`
- `milkrice --version`, `milkrice skill`
- Automatic update checks (asks before installing)
- Homebrew cask: `brew install --cask thekindofme/tap/milkrice`
- Pre-release builds were called Margin. On first launch Milkrice moves the old `Application Support/Margin` data folder to `Milkrice` (database and log renamed), and Settings, Agents replaces the old `margin` command and MCP registration
