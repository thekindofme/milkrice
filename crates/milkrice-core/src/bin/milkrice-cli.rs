//! Headless companion for Milkrice: manage models, transcribe files, and run the full meeting
//! pipeline (VAD → STT → speakers → echo removal → AI notes) on recorded audio.
use anyhow::{anyhow, bail, Context, Result};
use milkrice_core::diarize::SpeakerEmbedder;
use milkrice_core::llm::{self, Provider};
use milkrice_core::session::{Session, SessionConfig, SessionSink};
use milkrice_core::store::Meeting;
use milkrice_core::stt::Transcriber;
use milkrice_core::{catalog, models, AudioFrame, AudioSource, Attendee, Segment};
use parking_lot::Mutex;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

fn read_wav16k(path: &Path) -> Result<Vec<f32>> {
    let mut r = hound::WavReader::open(path).with_context(|| format!("open {}", path.display()))?;
    let spec = r.spec();
    if spec.sample_rate != 16_000 || spec.channels != 1 {
        bail!("{} must be 16 kHz mono (convert with: ffmpeg -i in -ar 16000 -ac 1 out.wav)", path.display());
    }
    Ok(match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().filter_map(|s| s.ok()).collect(),
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>().filter_map(|s| s.ok()).map(|s| s as f32 / scale).collect()
        }
    })
}

fn download(id: &str) -> Result<()> {
    let spec = catalog::find(id).ok_or_else(|| anyhow!("unknown model {id}"))?;
    let cancel = AtomicBool::new(false);
    let mut last = 0u64;
    models::download(spec, &cancel, |d, t| {
        let pct = (d * 100).checked_div(t).unwrap_or(0);
        if pct >= last + 10 || d == t {
            eprintln!("  {id}: {pct}% ({} / {} MB)", d >> 20, t >> 20);
            last = pct;
        }
    })?;
    println!("{id} installed at {}", models::local_path(spec).display());
    Ok(())
}

struct PrintSink {
    segs: Mutex<Vec<Segment>>,
    t0: Instant,
}

impl SessionSink for PrintSink {
    fn segment(&self, seg: Segment) {
        println!(
            "  [{:>6.1}s +{:>4.1}s] {:<5} {}",
            seg.start_ms as f32 / 1000.0,
            self.t0.elapsed().as_secs_f32(),
            seg.speaker,
            seg.text
        );
        self.segs.lock().push(seg);
    }
    fn levels(&self, _: f32, _: f32) {}
    fn warning(&self, msg: &str) {
        eprintln!("  warning: {msg}");
    }
}

fn simulate(model: &str, mic: &Path, system: &Path, provider: Option<Provider>) -> Result<()> {
    let spec = catalog::find(model).ok_or_else(|| anyhow!("unknown model {model}"))?;
    if !models::is_downloaded(spec) {
        download(model)?;
    }
    let t = Instant::now();
    let engine = Arc::new(Mutex::new(Transcriber::load(spec)?));
    println!("loaded {} in {:.2}s", spec.name, t.elapsed().as_secs_f32());
    let cancel = AtomicBool::new(false);
    let spk = models::ensure_speaker_model(&cancel, |_, _| {})?;
    let embedder = Some(Arc::new(Mutex::new(SpeakerEmbedder::load(&spk)?)));

    let mic_s = read_wav16k(mic)?;
    let sys_s = read_wav16k(system)?;
    let audio_secs = mic_s.len().max(sys_s.len()) as f32 / 16_000.0;
    let (tx, rx) = crossbeam_channel::bounded::<AudioFrame>(64);
    let sink = Arc::new(PrintSink { segs: Mutex::new(vec![]), t0: Instant::now() });
    let session = Session::from_frames(SessionConfig::default(), rx, engine, embedder, sink.clone())?;
    // Interleave 100 ms blocks of both streams, as live capture would.
    let n = mic_s.len().max(sys_s.len());
    let t = Instant::now();
    for i in (0..n).step_by(1600) {
        for (src, buf) in [(AudioSource::Mic, &mic_s), (AudioSource::System, &sys_s)] {
            if i < buf.len() {
                let end = (i + 1600).min(buf.len());
                tx.send(AudioFrame { source: src, samples: buf[i..end].to_vec(), hires: Vec::new() })?;
            }
        }
    }
    drop(tx);
    let res = session.stop();
    let took = t.elapsed().as_secs_f32();
    println!(
        "\npipeline: {audio_secs:.1}s of audio processed in {took:.1}s ({:.0}x real time), {} speakers on the system stream",
        audio_secs / took,
        res.clusterer.centroids.len()
    );
    println!("end-of-meeting tidy-up: {:?}", res.relabel);

    let segs = sink.segs.lock().clone();
    let Some(p) = provider else { return Ok(()) };
    let mut m = Meeting {
        id: "sim".into(),
        title: String::new(),
        created_at: chrono::Utc::now(),
        template: "team".into(),
        notes: "beta date\nwho owns release notes?".into(),
        attendees: vec![
            Attendee { name: "Priya Shah".into(), email: None, is_self: false },
            Attendee { name: "Daniel Brooks".into(), email: None, is_self: false },
            Attendee { name: "Me".into(), email: None, is_self: true },
        ],
        ..Default::default()
    };
    let t = Instant::now();
    if let Some((sys, prompt)) = llm::speaker_names_prompt(&m, &segs) {
        let map = llm::parse_speaker_map(&llm::complete(p, "", &sys, &prompt)?);
        println!("\nspeakers ({:?}, {:.1}s): {map:?}", p, t.elapsed().as_secs_f32());
        m.speakers.extend(map);
    }
    let t = Instant::now();
    let (sys, prompt) = llm::title_prompt(&m, &segs);
    m.title = llm::complete(p, "", &sys, &prompt)?;
    println!("title ({:.1}s): {}", t.elapsed().as_secs_f32(), m.title);
    let t = Instant::now();
    let (sys, prompt) = llm::enhance_prompt(&m, &segs);
    let notes = llm::complete(p, "", &sys, &prompt)?;
    println!("\nenhanced notes ({:.1}s):\n{notes}", t.elapsed().as_secs_f32());
    let t = Instant::now();
    let (sys, prompt) = llm::ask_prompt(&m, &segs, &[], "Who owns the release notes and when is the beta?");
    println!("\nask ({:.1}s): {}", t.elapsed().as_secs_f32(), llm::complete(p, "", &sys, &prompt)?);
    Ok(())
}

const HELP: &str = "milkrice — read your local Milkrice meetings (agent-friendly, read-only)

  milkrice ls [-q TEXT] [--since 7d] [-f FOLDER] [-n 20]  list meetings: id date len title · people [notes ai tx:N in:Folder]
  milkrice folders                                  folders and how many meetings each holds
  milkrice show ID [notes,ai,transcript|all]      one meeting; default ai,notes (labelled by source)
  milkrice tx ID [--from mm:ss] [--to mm:ss] [--speaker NAME] [--grep TEXT] [-n 300]
  milkrice search TEXT [--in notes|ai|transcript] [-n 20]
  milkrice mcp                                    MCP server on stdio (5 tools: list_meetings, list_folders, get_meeting, get_transcript, search)
  add --json to ls/folders/show/tx/search for JSON.  ID = 8-char prefix from ls.

Connect agents:
  milkrice agents status [--json]                 what's installed for each agent, and any problems
  milkrice agents install AGENT [--dry-run]       AGENT: claude, codex, cursor, opencode, vscode, skill, cli or all
  milkrice agents uninstall AGENT [--dry-run]     undo it (only Milkrice's own entries are removed)
  milkrice skill                                  print the skill (SKILL.md) for manual install
  milkrice --version

Sources: NOTES = typed by the user · AI SUMMARY = LLM-written (named) · TRANSCRIPT = local speech-to-text ('Me' = user)
Dev: milkrice dev models|download|transcribe|simulate|providers|calendar|embed";

/// `milkrice agents status|install|uninstall …`
fn agents_cmd(args: &[String]) -> Result<()> {
    use milkrice_core::agents_install::{self as ai, Env};
    let json = args.iter().any(|a| a == "--json");
    let pos: Vec<&str> = args.iter().map(String::as_str).filter(|a| !a.starts_with('-')).collect();
    // The binary to link ~/.local/bin/milkrice to: this one, resolved through any symlink.
    let me = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok());
    let mut env = Env::from_system(me);
    env.dry_run = args.iter().any(|a| a == "--dry-run");
    match pos.first().copied().unwrap_or("status") {
        "status" => {
            let st = ai::status(&env);
            if json {
                println!("{}", serde_json::to_string_pretty(&st)?);
                return Ok(());
            }
            match (&st.command, &st.command_source) {
                (Some(c), Some(ai::CliSource::App)) => println!("milkrice command   not installed (agents would use {c}); run `milkrice agents install cli`"),
                (Some(c), Some(src)) => println!("milkrice command   {c} ({})", serde_json::to_value(src)?.as_str().unwrap_or("")),
                _ => println!("milkrice command   not found"),
            }
            if let Some(h) = &st.path_hint {
                println!("                   ! {h}");
            }
            if let Some(id) = &st.claude_plugin {
                println!("Claude Code plugin {id} (provides MCP + skill)");
            }
            for a in &st.agents {
                let state = if a.connected {
                    "connected"
                } else if a.mcp.is_some() || !a.skills.is_empty() {
                    "partly installed"
                } else if a.detected {
                    "not connected"
                } else {
                    "not detected"
                };
                println!("\n{:<18} {state}", a.name);
                if let Some(m) = &a.mcp {
                    println!("  mcp    {} → {}{}", m.file, m.command.as_deref().unwrap_or("?"), if m.works { "" } else { " (missing)" });
                }
                for s in &a.skills {
                    println!("  skill  {s}");
                }
                for w in &a.warnings {
                    println!("  ! {w}");
                }
            }
            Ok(())
        }
        verb @ ("install" | "uninstall") => {
            let which = pos.get(1).ok_or_else(|| anyhow!("usage: milkrice agents {verb} <claude|codex|cursor|opencode|vscode|skill|cli|all>"))?;
            let installing = verb == "install";
            let mut failed = false;
            let mut hints = std::collections::BTreeSet::new();
            for t in ai::expand(&env, which, installing)? {
                let res = if installing { ai::install(&env, t) } else { ai::uninstall(&env, t) };
                match res {
                    Ok(log) => {
                        println!("{}:", t.label());
                        for l in &log.changed {
                            println!("  {l}");
                        }
                        for l in &log.unchanged {
                            println!("  = {l}");
                        }
                        for w in log.warnings {
                            // The PATH hint repeats for each target; print it once at the end.
                            if w.contains("isn't on your shell PATH") {
                                hints.insert(w);
                            } else {
                                println!("  ! {w}");
                            }
                        }
                    }
                    Err(e) => {
                        failed = true;
                        eprintln!("{}: error: {e}", t.label());
                    }
                }
            }
            for h in hints {
                println!("\n! {h}");
            }
            if installing && !env.dry_run && !failed {
                println!("\nRestart the agent (or start a new session) to pick it up.");
            }
            if failed {
                bail!("some targets failed");
            }
            Ok(())
        }
        other => bail!("unknown agents command '{other}' (expected status, install or uninstall)"),
    }
}

fn flag<'a>(args: &'a [String], names: &[&str]) -> Option<&'a str> {
    args.iter().position(|a| names.contains(&a.as_str())).and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// Positional args with flags (and their values) removed.
fn positional(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if a == "--json" {
            continue;
        }
        if a.starts_with('-') {
            skip = true;
            continue;
        }
        out.push(a.as_str());
    }
    out
}

fn agent_cmd(args: &[String]) -> Result<Option<String>> {
    use milkrice_core::agent;
    let json = args.iter().any(|a| a == "--json");
    let pos = positional(args);
    let limit = |d: usize| flag(args, &["-n", "--limit"]).and_then(|v| v.parse().ok()).unwrap_or(d);
    let cmd = pos.first().copied().unwrap_or("");
    if !matches!(cmd, "ls" | "list" | "folders" | "show" | "tx" | "transcript" | "search" | "mcp") {
        return Ok(None);
    }
    let db = milkrice_core::paths::db_path();
    if cmd == "mcp" {
        milkrice_core::mcp::serve(&mut milkrice_core::mcp::LazyStore::new(db))?;
        return Ok(Some(String::new()));
    }
    anyhow::ensure!(db.is_file(), "no Milkrice data yet ({}) — record a meeting first", db.display());
    let store = milkrice_core::store::Store::open_readonly(&db)?;
    let out = match cmd {
        "ls" | "list" => {
            let since = flag(args, &["--since"]).map(agent::parse_since).transpose()?;
            agent::list(&store, flag(args, &["-q", "--query"]), since, flag(args, &["-f", "--folder"]), limit(20), json)?
        }
        "folders" => agent::folders(&store, json)?,
        "show" => {
            let id = pos.get(1).ok_or_else(|| anyhow!("usage: milkrice show ID [notes,ai,transcript|all]"))?;
            let secs = agent::parse_sections(pos.get(2).copied().unwrap_or("ai,notes"))?;
            agent::show(&store, id, &secs, json)?
        }
        "tx" | "transcript" => {
            let id = pos.get(1).ok_or_else(|| anyhow!("usage: milkrice tx ID [--from mm:ss] [--to mm:ss] [--speaker NAME] [--grep TEXT]"))?;
            agent::transcript(
                &store,
                id,
                flag(args, &["--from"]).map(agent::parse_ts).transpose()?,
                flag(args, &["--to"]).map(agent::parse_ts).transpose()?,
                flag(args, &["--speaker"]),
                flag(args, &["--grep"]),
                limit(300),
                json,
            )?
        }
        "search" => {
            let q = pos[1..].join(" ");
            let scope = match flag(args, &["--in"]).unwrap_or("all") {
                "notes" => agent::Scope::Notes,
                "ai" => agent::Scope::Ai,
                "transcript" | "tx" => agent::Scope::Transcript,
                _ => agent::Scope::All,
            };
            agent::search(&store, &q, scope, limit(20), json)?
        }
        _ => unreachable!(),
    };
    Ok(Some(out))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || matches!(args[0].as_str(), "-h" | "--help" | "help") {
        println!("{HELP}");
        return;
    }
    match args[0].as_str() {
        "-V" | "--version" | "version" => {
            println!("milkrice {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        "skill" => {
            print!("{}", milkrice_core::agents_install::SKILL_MD);
            return;
        }
        "agents" => {
            if let Err(e) = agents_cmd(&args[1..]) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
            return;
        }
        _ => {}
    }
    match agent_cmd(&args) {
        Ok(Some(out)) => {
            if !out.is_empty() {
                println!("{out}");
            }
            return;
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
    let dev_args: Vec<String> = if args[0] == "dev" { args[1..].to_vec() } else { args };
    if let Err(e) = dev_main(dev_args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn dev_main(args: Vec<String>) -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    if let Ok(a) = std::env::var("ACCEL") {
        milkrice_core::transcribe_rs::set_ort_accelerator(match a.as_str() {
            "coreml" => milkrice_core::transcribe_rs::OrtAccelerator::CoreMl,
            "auto" => milkrice_core::transcribe_rs::OrtAccelerator::Auto,
            _ => milkrice_core::transcribe_rs::OrtAccelerator::CpuOnly,
        });
    }
    let a = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
    match a(0) {
        "models" => {
            for m in models::statuses() {
                println!("{:<20} {:<26} {:>5} MB  {:<14} {}", m.spec.id, m.spec.name, m.spec.size_mb, m.language_label, if m.downloaded { "downloaded" } else { "" });
            }
        }
        "download" => download(a(1))?,
        "transcribe" => {
            let spec = catalog::find(a(1)).ok_or_else(|| anyhow!("unknown model"))?;
            let mut t = Transcriber::load(spec)?;
            let s = read_wav16k(Path::new(a(2)))?;
            let reps: usize = std::env::var("REPEAT").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
            for _ in 0..reps {
                let t0 = Instant::now();
                let text = t.transcribe(&s)?;
                println!("{text}\n({:.1}s audio in {:.2}s)", s.len() as f32 / 16000.0, t0.elapsed().as_secs_f32());
            }
        }
        "simulate" => {
            let provider = match a(4) {
                "claude" => Some(Provider::Claude),
                "codex" => Some(Provider::Codex),
                "ollama" => Some(Provider::Ollama),
                _ => None,
            };
            simulate(a(1), Path::new(a(2)), Path::new(a(3)), provider)?
        }
        "embed" => embed_matrix(&args[1..])?,
        "ics" => {
            let text = milkrice_core::ics::fetch(a(1))?;
            let now = chrono::Utc::now();
            for e in milkrice_core::ics::parse(&text, "feed", None, now, now + chrono::Duration::days(90)) {
                println!("{} {} | {} people | {:?}", e.start.format("%Y-%m-%d %H:%M"), e.title, e.attendees.len(), e.meeting_url);
            }
        }
        "providers" => {
            for p in llm::detect() {
                println!("{:<20} available={} signed_in={} model={} — {}", p.name, p.available, p.signed_in, p.model, p.detail);
            }
        }
        "calendar" => {
            println!("auth: {:?}", milkrice_core::calendar::auth_status());
            let now = chrono::Utc::now();
            for e in milkrice_core::calendar::events_between(now, now + chrono::Duration::days(7))? {
                println!("{} {} ({} attendees) {:?}", e.start, e.title, e.attendees.len(), e.meeting_url);
            }
        }
        _ => {
            eprintln!("{HELP}");
        }
    }
    Ok(())
}

/// Debug helper: cosine matrix of speaker embeddings for a list of WAVs.
#[allow(dead_code)]
pub fn embed_matrix(files: &[String]) -> Result<()> {
    let cancel = AtomicBool::new(false);
    let path = std::env::var("SPK").map(std::path::PathBuf::from).unwrap_or(models::ensure_speaker_model(&cancel, |_, _| {})?);
    let mut e = SpeakerEmbedder::load(&path)?;
    let embs: Vec<Vec<f32>> = files.iter().map(|f| e.embed(&read_wav16k(Path::new(f)).unwrap())).collect::<Result<_>>()?;
    for (i, a) in embs.iter().enumerate() {
        let row: Vec<String> = embs.iter().map(|b| format!("{:5.2}", milkrice_core::diarize::cosine(a, b))).collect();
        println!("{:<10} {}", Path::new(&files[i]).file_name().unwrap().to_string_lossy(), row.join(" "));
    }
    Ok(())
}
