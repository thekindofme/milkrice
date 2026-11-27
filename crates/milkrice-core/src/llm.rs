//! AI notes through whatever the user already has: the Claude Code CLI (Haiku), the Codex CLI
//! (Luna) using their existing subscriptions, or a local Ollama server. No API keys, no accounts.
use crate::store::Meeting;
use crate::types::{AudioSource, Segment};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
    Ollama,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    pub id: Provider,
    pub name: &'static str,
    pub available: bool,
    pub signed_in: bool,
    pub model: String,
    pub detail: String,
}

/// The PATH a new terminal would have, exactly as the user's login shell reports it (no extras
/// added). Used to tell whether `milkrice` will be found when the user or an agent runs it.
pub fn login_shell_path() -> Option<&'static str> {
    static P: OnceLock<Option<String>> = OnceLock::new();
    P.get_or_init(|| {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let out = Command::new(&shell)
            .args(["-ilc", "printf '__MILKRICE_PATH__%s__END__' \"$PATH\""])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout).to_string();
        let a = s.find("__MILKRICE_PATH__")? + "__MILKRICE_PATH__".len();
        let b = s[a..].find("__END__")? + a;
        Some(s[a..b].to_string())
    })
    .as_deref()
}

/// PATH from the user's login shell. GUI apps on macOS start with a bare PATH, which would hide
/// CLIs installed via Homebrew, npm, fnm, etc.
pub fn shell_path() -> &'static str {
    static P: OnceLock<String> = OnceLock::new();
    P.get_or_init(|| {
        let mut path = login_shell_path().unwrap_or_default().to_string();
        let home = dirs::home_dir().unwrap_or_default();
        for extra in [
            home.join(".local/bin"),
            home.join(".claude/local"),
            home.join(".npm-global/bin"),
            home.join(".bun/bin"),
            "/opt/homebrew/bin".into(),
            "/usr/local/bin".into(),
            "/usr/bin".into(),
            "/bin".into(),
        ] {
            let e = extra.to_string_lossy().to_string();
            if !path.split(':').any(|p| p == e) {
                if !path.is_empty() {
                    path.push(':');
                }
                path.push_str(&e);
            }
        }
        path
    })
}

fn find_bin(name: &str) -> Option<PathBuf> {
    which::which_in(name, Some(shell_path()), std::env::temp_dir()).ok()
}

fn scratch_dir() -> PathBuf {
    let d = std::env::temp_dir().join("milkrice-llm");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Run a command with stdin, a timeout and a clean working dir.
fn run(mut cmd: Command, stdin: &str, timeout: Duration) -> Result<String> {
    cmd.env("PATH", shell_path())
        .current_dir(scratch_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().context("starting AI CLI")?;
    {
        let mut si = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let input = stdin.to_string();
        std::thread::spawn(move || {
            let _ = si.write_all(input.as_bytes());
        });
    }
    let mut so = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let mut se = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;
    let out_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = so.read_to_string(&mut s);
        s
    });
    let err_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = se.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            bail!("AI request timed out after {}s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let out = out_t.join().unwrap_or_default();
    let err = err_t.join().unwrap_or_default();
    if !status.success() {
        let msg = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
        bail!("AI CLI failed ({status}): {}", if msg.is_empty() { out.trim().to_string() } else { msg });
    }
    Ok(out)
}

pub fn codex_default_model() -> String {
    codex_model()
}

fn codex_model() -> String {
    // Prefer the newest "luna" model the user's Codex knows about.
    let cache = dirs::home_dir().unwrap_or_default().join(".codex/models_cache.json");
    let text = std::fs::read_to_string(cache).unwrap_or_default();
    let mut lunas: Vec<String> = text
        .split("\"slug\"")
        .skip(1)
        .filter_map(|s| s.split('"').nth(1).map(|x| x.to_string()))
        .filter(|s| s.contains("luna"))
        .collect();
    lunas.sort_by(|a, b| b.cmp(a));
    lunas.into_iter().next().unwrap_or_else(|| "gpt-6-luna".into())
}

pub fn detect() -> Vec<ProviderInfo> {
    let mut v = Vec::new();

    let claude = find_bin("claude");
    let mut claude_in = false;
    let mut claude_detail = "Install Claude Code to use your Claude subscription".to_string();
    if let Some(bin) = &claude {
        let mut c = Command::new(bin);
        c.args(["auth", "status"]);
        match run(c, "", Duration::from_secs(15)) {
            Ok(out) => {
                claude_in = out.contains("\"loggedIn\": true") || out.contains("\"loggedIn\":true");
                claude_detail = if claude_in {
                    "Signed in with your Claude subscription".into()
                } else {
                    "Run `claude` once in a terminal to sign in".into()
                };
            }
            Err(_) => claude_detail = "Claude Code found".into(),
        }
    }
    v.push(ProviderInfo {
        id: Provider::Claude,
        name: "Claude Haiku",
        available: claude.is_some(),
        signed_in: claude_in,
        model: "haiku".into(),
        detail: claude_detail,
    });

    let codex = find_bin("codex");
    let mut codex_in = false;
    let mut codex_detail = "Install the Codex CLI to use your ChatGPT subscription".to_string();
    if let Some(bin) = &codex {
        let mut c = Command::new(bin);
        c.args(["login", "status"]);
        let out = run(c, "", Duration::from_secs(15)).unwrap_or_default();
        codex_in = out.to_lowercase().contains("logged in");
        codex_detail = if codex_in { out.trim().to_string() } else { "Run `codex login` to sign in".into() };
    }
    v.push(ProviderInfo {
        id: Provider::Codex,
        name: "Codex Luna",
        available: codex.is_some(),
        signed_in: codex_in,
        model: codex_model(),
        detail: codex_detail,
    });

    let (ollama_ok, ollama_model) = ollama_probe();
    v.push(ProviderInfo {
        id: Provider::Ollama,
        name: "Ollama (on-device)",
        available: ollama_ok,
        signed_in: ollama_ok && ollama_model.is_some(),
        model: ollama_model.clone().unwrap_or_default(),
        detail: match (ollama_ok, ollama_model) {
            (true, Some(m)) => format!("Running locally with {m}"),
            (true, None) => "Ollama is running but has no local models pulled".into(),
            _ => "Fully offline option: install Ollama and pull a model".into(),
        },
    });
    v
}

fn ollama_probe() -> (bool, Option<String>) {
    let client = match reqwest::blocking::Client::builder().timeout(Duration::from_millis(800)).build() {
        Ok(c) => c,
        Err(_) => return (false, None),
    };
    let Ok(r) = client.get("http://127.0.0.1:11434/api/tags").send() else { return (false, None) };
    let v: serde_json::Value = r.json().unwrap_or_default();
    (true, first_local_model(&v))
}

/// Ollama "cloud" models (e.g. `gpt-oss:120b-cloud`) are listed next to local ones but run on
/// ollama.com, so the transcript would leave the Mac. Never pick one automatically.
fn is_cloud_model(m: &serde_json::Value) -> bool {
    let name = m["name"].as_str().or_else(|| m["model"].as_str()).unwrap_or("");
    name.ends_with("-cloud") || name.ends_with(":cloud") || m.get("remote_host").is_some_and(|h| !h.is_null())
}

/// The first model from `/api/tags` that runs on this Mac.
fn first_local_model(tags: &serde_json::Value) -> Option<String> {
    tags["models"]
        .as_array()?
        .iter()
        .filter(|m| !is_cloud_model(m))
        .find_map(|m| m["name"].as_str().map(String::from))
}

/// One-shot completion. `model` empty = provider default.
pub fn complete(provider: Provider, model: &str, system: &str, prompt: &str) -> Result<String> {
    let timeout = Duration::from_secs(240);
    let out = match provider {
        Provider::Claude => {
            let bin = find_bin("claude").ok_or_else(|| anyhow!("Claude Code CLI not found"))?;
            let mut c = Command::new(bin);
            c.args([
                "-p",
                "--model",
                if model.is_empty() { "haiku" } else { model },
                "--output-format",
                "text",
                "--tools",
                "",
                "--no-session-persistence",
                "--setting-sources",
                "",
                "--strict-mcp-config",
                "--system-prompt",
                system,
            ]);
            run(c, prompt, timeout)?
        }
        Provider::Codex => {
            let bin = find_bin("codex").ok_or_else(|| anyhow!("Codex CLI not found"))?;
            let out_file = scratch_dir().join(format!("codex-{}.txt", uuid::Uuid::new_v4()));
            let m = if model.is_empty() { codex_model() } else { model.to_string() };
            let mut c = Command::new(bin);
            c.args(["exec", "--skip-git-repo-check", "--ephemeral", "--sandbox", "read-only", "-m", &m])
                .args(["-c", "model_reasoning_effort=low", "-c", "notify=[]", "--color", "never", "-o"])
                .arg(&out_file)
                .arg("-");
            let full = format!("{system}\n\n---\n\n{prompt}\n\nRespond with only the requested output. Do not run commands or read files.");
            let res = run(c, &full, timeout);
            let text = std::fs::read_to_string(&out_file).unwrap_or_default();
            // Remove it on failure or timeout too: it can hold part of the notes.
            let _ = std::fs::remove_file(&out_file);
            res?;
            text
        }
        Provider::Ollama => {
            let client = reqwest::blocking::Client::builder().timeout(timeout).build()?;
            let m = if model.is_empty() { ollama_probe().1.ok_or_else(|| anyhow!("no Ollama model"))? } else { model.into() };
            let body = serde_json::json!({
                "model": m, "stream": false, "system": system, "prompt": prompt,
                "options": {"temperature": 0.2}
            });
            let v: serde_json::Value = client.post("http://127.0.0.1:11434/api/generate").json(&body).send()?.json()?;
            v["response"].as_str().unwrap_or_default().to_string()
        }
    };
    let out = out.trim().to_string();
    if out.is_empty() {
        bail!("the AI returned an empty response");
    }
    Ok(out)
}

// ---------------------------------------------------------------- prompts

pub const TEMPLATES: &[(&str, &str, &str)] = &[
    ("auto", "Auto", "Pick the structure that best fits the conversation."),
    ("one_on_one", "1:1", "Sections: Updates, Wins, Blockers & support needed, Feedback, Action items."),
    ("standup", "Stand-up", "One section per person (what they did, what's next, blockers), then Action items."),
    ("customer", "Customer call", "Sections: Context & company, Their pains/needs, Current tools, Objections & concerns, Budget/timeline/decision makers, Next steps."),
    ("interview", "Interview", "Sections: Candidate background, Strengths, Concerns, Notable answers (with brief quotes), Recommendation signals, Follow-ups."),
    ("team", "Team meeting", "Sections: Key decisions, Discussion by topic, Open questions, Action items (owner — task — due)."),
    ("brainstorm", "Brainstorm", "Sections: Problem framing, Ideas (grouped by theme), Promising directions, Next experiments."),
];

fn fmt_ts(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}", s / 60, s % 60)
}

pub fn speaker_name(key: &str, names: &BTreeMap<String, String>) -> String {
    if let Some(n) = names.get(key).filter(|n| !n.trim().is_empty()) {
        return n.clone();
    }
    match key {
        "me" => "Me".into(),
        "them" => "Them".into(),
        k if k.starts_with('s') => format!("Speaker {}", &k[1..]),
        k => k.into(),
    }
}

/// Compact transcript: consecutive lines by the same speaker are merged.
pub fn transcript_text(segments: &[Segment], names: &BTreeMap<String, String>, max_chars: usize) -> String {
    let mut out = String::new();
    let mut last: Option<&str> = None;
    for s in segments {
        if s.text.trim().is_empty() {
            continue;
        }
        if last == Some(s.speaker.as_str()) {
            out.push(' ');
            out.push_str(s.text.trim());
        } else {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("[{}] {}: {}", fmt_ts(s.start_ms), speaker_name(&s.speaker, names), s.text.trim()));
            last = Some(&s.speaker);
        }
    }
    if out.len() > max_chars {
        // Keep the start and the end; meetings tend to conclude with decisions.
        let head = max_chars * 2 / 3;
        let tail = max_chars - head;
        let mut h = head;
        while !out.is_char_boundary(h) {
            h -= 1;
        }
        let mut t = out.len() - tail;
        while !out.is_char_boundary(t) {
            t += 1;
        }
        out = format!("{}\n[… middle of transcript omitted for length …]\n{}", &out[..h], &out[t..]);
    }
    out
}

fn meeting_header(m: &Meeting) -> String {
    let mut h = format!("Meeting: {}\nDate: {}\n", if m.title.is_empty() { "Untitled" } else { &m.title }, m.created_at.format("%A %e %B %Y"));
    if !m.attendees.is_empty() {
        let names: Vec<String> = m
            .attendees
            .iter()
            .map(|a| if a.is_self { format!("{} (me)", a.name) } else { a.name.clone() })
            .collect();
        h.push_str(&format!("Invited: {}\n", names.join(", ")));
    }
    h
}

const NOTES_SYSTEM: &str = "You are Milkrice, a meticulous meeting-notes writer. You turn a raw transcript plus the user's own rough notes into crisp, skimmable notes in Markdown. Rules: The user's notes signal what they care about — expand every one of them with detail from the transcript, keep their wording where possible. Be specific: names, numbers, dates, decisions. Never invent facts that are not in the transcript or notes. Use '### ' headings, short bullets, and **bold** sparingly for key decisions. End with '### Action items' as '- [ ] Owner — task (due date if mentioned)' when there are any. No preamble, no closing remarks, no title line.";

pub fn enhance_prompt(m: &Meeting, segments: &[Segment]) -> (String, String) {
    let tpl = TEMPLATES.iter().find(|t| t.0 == m.template).unwrap_or(&TEMPLATES[0]);
    let mut p = meeting_header(m);
    p.push_str(&format!("Template: {} — {}\n\n", tpl.1, tpl.2));
    p.push_str("## My notes\n");
    p.push_str(if m.notes.trim().is_empty() { "(none)" } else { m.notes.trim() });
    p.push_str("\n\n## Transcript ('Me' is the user)\n");
    p.push_str(&transcript_text(segments, &m.speakers, 60_000));
    p.push_str("\n\nWrite the enhanced notes now.");
    (NOTES_SYSTEM.to_string(), p)
}

pub fn ask_prompt(m: &Meeting, segments: &[Segment], history: &[(String, String)], question: &str) -> (String, String) {
    let system = "You answer questions about a single meeting using only its transcript and notes. Be direct and brief (a few sentences or bullets). Quote short phrases when useful. If the answer is not in the meeting, say so. You may draft follow-up emails or messages when asked.";
    let mut p = meeting_header(m);
    if !m.enhanced.trim().is_empty() {
        p.push_str("\n## Notes\n");
        p.push_str(m.enhanced.trim());
    } else if !m.notes.trim().is_empty() {
        p.push_str("\n## Notes\n");
        p.push_str(m.notes.trim());
    }
    p.push_str("\n\n## Transcript\n");
    p.push_str(&transcript_text(segments, &m.speakers, 50_000));
    for (q, a) in history.iter().rev().take(6).rev() {
        p.push_str(&format!("\n\nUser: {q}\nYou: {a}"));
    }
    p.push_str(&format!("\n\nUser: {question}\nYou:"));
    (system.into(), p)
}

/// Questions across many meetings ("what did we decide about pricing this month?").
pub fn ask_all_prompt(meetings: &[(Meeting, Vec<Segment>)], question: &str) -> (String, String) {
    let system = "You answer questions across the user's recent meetings using only the notes and transcripts provided. Cite meetings by title and date in parentheses. Be concise. If nothing relevant is there, say so.";
    let mut p = String::new();
    let budget = 70_000 / meetings.len().max(1);
    for (m, segs) in meetings {
        p.push_str(&format!("\n\n=== {} — {} ===\n", if m.title.is_empty() { "Untitled" } else { &m.title }, m.created_at.format("%a %e %b %Y")));
        if !m.enhanced.trim().is_empty() {
            p.push_str(m.enhanced.trim());
        } else {
            if !m.notes.trim().is_empty() {
                p.push_str(m.notes.trim());
                p.push('\n');
            }
            p.push_str(&transcript_text(segs, &m.speakers, budget));
        }
    }
    p.push_str(&format!("\n\nQuestion: {question}"));
    (system.into(), p)
}

pub fn title_prompt(m: &Meeting, segments: &[Segment]) -> (String, String) {
    let system = "Write a short, specific meeting title (3-7 words, no quotes, no trailing period, no date).";
    let mut p = meeting_header(m);
    p.push_str("\nTranscript excerpt:\n");
    p.push_str(&transcript_text(segments, &m.speakers, 6_000));
    (system.into(), p)
}

/// Ask the model to map anonymous speaker keys to invited attendees' names using
/// conversational cues ("thanks, Priya"). Returns only confident matches.
pub fn speaker_names_prompt(m: &Meeting, segments: &[Segment]) -> Option<(String, String)> {
    let keys: Vec<&str> = {
        let mut k: Vec<&str> = segments
            .iter()
            .filter(|s| s.source == AudioSource::System)
            .map(|s| s.speaker.as_str())
            .collect();
        k.sort();
        k.dedup();
        k
    };
    if keys.is_empty() {
        return None;
    }
    let system = "You identify who is speaking in a meeting transcript. Output ONLY a JSON object mapping speaker labels to real names, e.g. {\"s2\": \"Priya Shah\"}. Assign a name ONLY with direct evidence in the transcript: the speaker introduces themselves (\"this is Priya\"), or another speaker addresses someone by name and that label answers right after (\"Rishi, can you own this?\" followed by s2 saying \"Yes, I will...\"), or someone is greeted by name and replies. Prefer the full name from the invite list when it matches the first name used. Being on the invite list is NOT evidence by itself. One exception: if exactly one likely person is listed below and one label did nearly all of the other side's talking, that label is that person. A speaker who addresses someone by name is NOT that person. Omit every label you are not sure about; an empty object {} is a fine answer. Never label 'me'.";
    let mut p = meeting_header(m);
    let user = m.speakers.get("me").map(String::as_str).unwrap_or("");
    let cands = crate::names::candidates(&m.title, &m.attendees, user);
    if !cands.is_empty() {
        p.push_str(&format!("\nLikely people (from the invite and title): {}\n", cands.join(", ")));
    }
    let mut time: BTreeMap<&str, u64> = BTreeMap::new();
    for s in segments.iter().filter(|s| s.source == AudioSource::System) {
        *time.entry(s.speaker.as_str()).or_default() += s.end_ms.saturating_sub(s.start_ms);
    }
    let total = time.values().sum::<u64>().max(1);
    let shares: Vec<String> = time.iter().map(|(k, t)| format!("{k} {}%", t * 100 / total)).collect();
    p.push_str(&format!("Share of the other side's talking: {}\n", shares.join(", ")));
    p.push_str(&format!("\nLabels to identify: {}\n\nTranscript (labels shown in brackets):\n", keys.join(", ")));
    let mut raw = String::new();
    for s in segments {
        raw.push_str(&format!("[{}] {}\n", s.speaker, s.text));
        if raw.len() > 30_000 {
            break;
        }
    }
    p.push_str(&raw);
    Some((system.into(), p))
}

pub fn parse_speaker_map(text: &str) -> BTreeMap<String, String> {
    let start = text.find('{');
    let end = text.rfind('}');
    let (Some(a), Some(b)) = (start, end) else { return BTreeMap::new() };
    serde_json::from_str::<BTreeMap<String, String>>(&text[a..=b])
        .unwrap_or_default()
        .into_iter()
        .filter(|(k, v)| k != "me" && !v.trim().is_empty() && v.len() < 80)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_merges_runs() {
        let seg = |sp: &str, t: &str, ms| Segment { id: 0, start_ms: ms, end_ms: ms + 1, source: AudioSource::System, speaker: sp.into(), text: t.into() };
        let segs = vec![seg("s1", "hello", 0), seg("s1", "there", 1000), seg("me", "hi", 2000)];
        let t = transcript_text(&segs, &BTreeMap::new(), 10_000);
        assert_eq!(t, "[00:00] Speaker 1: hello there\n[00:02] Me: hi");
    }

    #[test]
    fn ollama_skips_cloud_models() {
        let tags = serde_json::json!({"models": [
            {"name": "gpt-oss:120b-cloud", "remote_host": "https://ollama.com:443"},
            {"name": "kimi:cloud"},
            {"name": "remote-thing:latest", "remote_host": "https://example.com"},
            {"name": "llama3.2:3b"},
        ]});
        assert_eq!(first_local_model(&tags).as_deref(), Some("llama3.2:3b"));
        let only_cloud = serde_json::json!({"models": [{"name": "gpt-oss:20b-cloud"}]});
        assert_eq!(first_local_model(&only_cloud), None);
    }

    #[test]
    fn parses_speaker_json() {
        let m = parse_speaker_map("Sure: {\"s1\": \"Priya\", \"me\": \"x\", \"s2\": \"\"}");
        assert_eq!(m.len(), 1);
        assert_eq!(m["s1"], "Priya");
    }
}
