# Changelog

All notable changes to Milkrice are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## Unreleased

- Fix: the other side of iPhone calls on the Mac (Continuity) and FaceTime calls was not captured, so it was missing from transcripts and kept recordings. Call audio now comes from a Core Audio process tap, which also hears system services such as `avconferenced`; ScreenCaptureKit remains the fallback. macOS may ask once for System Audio Recording permission
- Kept recordings are now 48 kHz at 128 kbps (about 60 MB an hour, was 16 kHz at 64 kbps) so they sound like the call, not a phone line. Transcription still uses its own 16 kHz stream and is unaffected when recordings are off
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
