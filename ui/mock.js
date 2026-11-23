// Design-preview bridge: only active when the page is opened outside the Tauri app.
(function () {
  if (window.__TAURI__) return;
  const now = Date.now();
  const iso = (ms) => new Date(ms).toISOString();
  const listeners = {};
  const flag = (f) => location.hash.slice(1).split(",").includes(f);
  const emit = (ev, payload) => (listeners[ev] || []).forEach((f) => f({ payload }));
  const people = [
    { name: "Priya Shah", email: "priya@acme.co", is_self: false },
    { name: "Sam Okafor", email: "sam@acme.co", is_self: false },
    { name: "Alex", email: "me@example.com", is_self: true },
  ];
  const segs = [
    [0, "s1", "Okay, let's get going. Main thing today is the beta date."],
    [5200, "me", "Right, I think QA needs at least until Thursday to finish the regression pass."],
    [11800, "s2", "Thursday works if we freeze scope tomorrow. Design is basically done."],
    [18500, "s1", "Then let's say beta ships Friday, and Sam owns the release notes."],
    [24600, "me", "Sounds good. I'll update the onboarding copy before then."],
  ].map(([t, sp, text], i) => ({ id: i + 1, start_ms: t, end_ms: t + 4800, source: sp === "me" ? "mic" : "system", speaker: sp, text }));
  const db = {
    settings: { model_id: "parakeet-v2", language: "auto", provider: flag("noai") || flag("onboarding") ? null : "claude", provider_model: "", capture_system: true, mic_device: null, calendar_mode: flag("auto") ? "auto" : "notify", auto_stop: true, detect_calls: true, only_real_meetings: true, auto_enhance: true, diarize: true, default_template: "auto", user_name: "Alex", onboarded: !flag("onboarding") },
    meetings: [
      { id: "m1", title: "Beta launch sync", created_at: iso(now - 3600e3), started_at: null, ended_at: null, duration_ms: 1860000, event_id: null, attendees: people, notes: "beta date?\nQA timeline\ncopy", enhanced: "### Beta launch\n- Beta ships **Friday**; scope freezes tomorrow\n- QA needs until Thursday for the regression pass\n- Design is effectively complete\n\n### Action items\n- [ ] Sam — write release notes (before Friday)\n- [ ] Alex — update onboarding copy\n- [ ] Priya — confirm scope freeze with eng", template: "team", status: "done", speakers: { s1: "Priya Shah", s2: "Sam Okafor" }, speakers_guessed: ["s2"], enhanced_by: "claude:haiku", stt_model: "parakeet-v2", enhanced_edited: false, clusterer: null, meeting_url: "https://zoom.us/j/123" },
      { id: "m2", title: "1:1 with Dana", created_at: iso(now - 26 * 3600e3), duration_ms: 1500000, attendees: [], notes: "", enhanced: "### Updates\n- Hiring loop for the infra role closes next week", template: "one_on_one", status: "done", speakers: {}, meeting_url: null },
      { id: "m3", title: "Customer call — Northwind", created_at: iso(now - 4 * 86400e3), duration_ms: 2700000, attendees: [], notes: "pricing pushback", enhanced: "", template: "customer", status: "done", speakers: {}, meeting_url: null },
      { id: "m4", title: "Hiring sync", created_at: iso(now - 5 * 86400e3), duration_ms: 900000, attendees: [], notes: "", enhanced: "", template: "auto", status: "done", speakers: {}, meeting_url: null },
    ],
    segments: { m1: segs, m2: [], m3: [], m4: segs.slice(0, 3) },
    chats: { m1: [] },
    // #nofolders starts with none, to preview the clean first-run sidebar.
    folders: flag("nofolders") ? [] : [{ id: "f1", name: "Product", color: "ember" }, { id: "f2", name: "Hiring", color: "sage" }, { id: "f3", name: "Customers", color: "blue" }],
    filed: flag("nofolders") ? [] : [["m1", "f1"], ["m4", "f2"]],
    series: [],
    dismissed: [],
    sync: { setup: flag("sync"), phone: flag("sync") },
  };
  db.meetings[2].from_iphone = true;
  const fcount = (id) => db.filed.filter(([, f]) => f === id).length;
  const foldersOf = (mid) => db.filed.filter(([m]) => m === mid).map(([, f]) => db.folders.find((x) => x.id === f)).filter(Boolean);
  const models = [
    ["parakeet-v2", "Parakeet TDT 0.6B v2", "The sharpest English model for everyday meetings", 451, ["en"], 0.86, 0.86, false, true, true],
    ["parakeet-v3", "Parakeet TDT 0.6B v3", "Fast, accurate notes in 25 European languages", 456, ["en", "de", "fr"], 0.82, 0.86, false, true, true],
    ["canary-180m-flash", "Canary 180M Flash", "Tiny and instant, runs well on any hardware", 146, ["en", "de", "es", "fr"], 0.75, 0.92, true, true, false],
    ["whisper-medium", "Whisper Medium", "Broadest language coverage, a little slower", 469, Array(99).fill("x"), 0.78, 0.45, true, false, false],
    ["moonshine-base", "Moonshine Base", "Small and quick for English on older Macs", 61, ["en"], 0.7, 0.95, false, true, false],
  ].map(([id, name, tagline, size_mb, languages, accuracy, speed, translate, live, recommended], i) => ({ id, name, tagline, size_mb, languages, accuracy, speed, translate, live, recommended, downloaded: i === 0, language_label: languages.length === 1 ? "English only" : `${languages.length} languages` }));
  const events = [
    { event: { id: "e1", title: "Design review", start: iso(now - 2 * 60e3), end: iso(now + 28 * 60e3), attendees: people, location: null, meeting_url: "https://meet.google.com/abc", calendar: "Work", calendar_color: null, all_day: false }, real: true, meeting_id: null },
    { event: { id: "e2", title: "Roadmap planning", start: iso(now + 2 * 3600e3), end: iso(now + 3 * 3600e3), attendees: people.slice(0, 2), location: null, meeting_url: null, calendar: "Work", calendar_color: null, all_day: false }, real: true, meeting_id: null },
    { event: { id: "y1@1", title: "Yoga", start: iso(now + 4 * 3600e3), end: iso(now + 5 * 3600e3), attendees: [], location: "Studio", meeting_url: null, calendar: "Personal", calendar_color: null, all_day: false, recurring: true }, real: true, meeting_id: null },
  ];
  let hidden = [];
  const isHidden = (e) => hidden.some((h) => h.key === (h.series ? e.id.split("@")[0] : e.id));
  const handlers = {
    get_settings: () => db.settings,
    save_settings: ({ settings }) => (db.settings = settings),
    permissions: () => ({ microphone: "granted", system_audio: "granted", calendar: "granted", notifications: "not_determined" }),
    grant_permission: ({ kind }) => ({ status: "not_determined", opened_settings: true }),
    current_prompt: () => (flag("prompt") ? { kind: "calendar", title: "Design review", event: events[0].event, app: null, at: iso(now) } : null),
    list_meetings: ({ query }) => db.meetings.filter((m) => !query || m.title.toLowerCase().includes(query.toLowerCase())).map((m) => ({ ...m, snippet: m.enhanced.slice(0, 200), folders: foldersOf(m.id).map((f) => f.id) })),
    list_folders: () => db.folders.map((f) => ({ ...f, count: fcount(f.id) })).sort((a, b) => a.name.localeCompare(b.name)),
    create_folder: ({ name, meetingId: meeting_id }) => { let f = db.folders.find((x) => x.name.toLowerCase() === name.trim().toLowerCase()); if (!f) { const cs = ["ember", "sage", "blue", "amber", "violet", "teal", "rose", "slate"]; f = { id: "f" + Date.now(), name: name.trim(), color: cs.find((c) => !db.folders.some((x) => x.color === c)) || "slate" }; db.folders.push(f); } if (meeting_id && !db.filed.some(([m, x]) => m === meeting_id && x === f.id)) db.filed.push([meeting_id, f.id]); return { ...f, count: fcount(f.id) }; },
    set_folder_color: ({ id, color }) => { db.folders.find((f) => f.id === id).color = color; },
    discard_if_empty: () => 0,
    rename_folder: ({ id, name }) => { db.folders.find((f) => f.id === id).name = name.trim(); },
    delete_folder: ({ id }) => { db.folders = db.folders.filter((f) => f.id !== id); db.filed = db.filed.filter(([, f]) => f !== id); },
    set_meeting_folder: ({ meetingId: meeting_id, folderId: folder_id, on }) => { db.filed = db.filed.filter(([m, f]) => !(m === meeting_id && f === folder_id)); if (on) db.filed.push([meeting_id, folder_id]); else db.series = db.series.filter((f) => f !== folder_id); },
    set_folder_series: ({ folderId: folder_id, on }) => { db.series = db.series.filter((f) => f !== folder_id); if (on) db.series.push(folder_id); },
    dismiss_folder_suggestion: ({ meetingId: meeting_id }) => { db.dismissed.push(meeting_id); },
    list_models: () => ({ models, active: db.settings.model_id, downloading: [], speaker_model: true }),
    upcoming_events: () => events.filter((x) => !isHidden(x.event)),
    hidden_events: () => [...hidden].reverse(),
    hide_event: ({ eventId, series }) => { const e = events.find((x) => x.event.id === eventId).event; series = series && !!e.recurring; const h = { key: series ? e.id.split("@")[0] : e.id, title: e.title, series, start: series ? null : e.start }; hidden = hidden.filter((x) => x.key !== h.key).concat(h); return h; },
    unhide_event: ({ key }) => { hidden = hidden.filter((x) => x.key !== key); },
    providers: () => ({
      providers: [
        { id: "claude", name: "Claude Haiku", available: true, signed_in: true, model: "haiku", detail: "Signed in with your Claude subscription" },
        { id: "codex", name: "Codex Luna", available: true, signed_in: true, model: "gpt-6-luna", detail: "Logged in using ChatGPT" },
        { id: "ollama", name: "Ollama (on-device)", available: false, signed_in: false, model: "", detail: "Fully offline option: install Ollama and pull a model" },
      ],
      templates: [{ id: "auto", name: "Auto" }, { id: "one_on_one", name: "1:1" }, { id: "team", name: "Team meeting" }, { id: "customer", name: "Customer call" }],
    }),
    get_meeting: ({ id }) => {
      const meeting = db.meetings.find((m) => m.id === id), folders = foldersOf(id);
      // Mock suggestion: Customer call → Product (the real one compares people, titles and notes).
      const sug = !folders.length && !db.dismissed.includes(id) && /customer|beta|launch/i.test(meeting.title) ? db.folders.find((f) => f.name === "Product") : null;
      return { meeting, segments: db.segments[id] || [], chats: db.chats[id] || [], busy: null, folders, folder_suggestion: sug || null, series_folders: db.series, has_series: !!meeting.event_id || id === "m1" };
    },
    create_meeting: ({ title }) => { const m = { id: "n" + Date.now(), title: title || "", created_at: iso(Date.now()), duration_ms: 0, attendees: [], notes: "", enhanced: "", template: "auto", status: "draft", speakers: {}, meeting_url: null }; db.meetings.unshift(m); db.segments[m.id] = []; return m; },
    update_meeting: ({ id, field, value }) => { const m = db.meetings.find((x) => x.id === id); m[field] = value; if (field === "enhanced") m.enhanced_edited = true; },
    delete_meeting: ({ id }) => { db.meetings = db.meetings.filter((m) => m.id !== id); },
    rename_speaker: ({ id, key, name }) => { const m = db.meetings.find((x) => x.id === id); m.speakers_guessed = (m.speakers_guessed || []).filter((k) => k !== key); if (name.trim()) m.speakers[key] = name.trim(); else delete m.speakers[key]; },
    export_markdown: ({ id }) => { const m = db.meetings.find((x) => x.id === id); return `# ${m.title}\n\n${m.enhanced}\n\n## My notes\n${m.notes}`; },
    enhance: async ({ id }) => { await new Promise((r) => setTimeout(r, 700)); const m = db.meetings.find((x) => x.id === id); m.enhanced = "### Summary\n- Regenerated notes\n\n### Action items\n- [ ] Follow up"; m.enhanced_by = "claude:haiku"; m.enhanced_edited = false; return m.enhanced; },
    turn_off_auto_record: () => { db.settings.calendar_mode = "notify"; return db.settings; },
    autostart_get: () => db.autostart ?? false,
    autostart_set: ({ on }) => (db.autostart = on),
    dismiss_prompt: () => null,
    ask_all: async () => { await new Promise(r=>setTimeout(r,600)); return "- The beta ships **Friday** (Beta launch sync, Sat 31 Oct)\n- Sam owns the release notes"; },
    agents_status: () => ({
      cli_bundled: true, cli_installed: false, cli_link: "~/.local/bin/milkrice", command: null, path_hint: null, claude_plugin: null,
      agents: [
        { target: "claude", detected: true, connected: true, warnings: [] },
        { target: "codex", detected: true, connected: false, warnings: [] },
        { target: "cursor", detected: true, connected: false, warnings: [] },
        { target: "opencode", detected: false, connected: false, warnings: [] },
        { target: "vscode", detected: true, connected: false, warnings: [] },
        { target: "skill", detected: true, connected: false, warnings: [] },
      ],
    }),
    agent_connect: () => "Connected",
    // #sync starts with sync on and an iPhone paired; #nosync with iCloud off.
    sync_status: () => db.sync.setup
      ? { setup: true, icloud_available: true, folder: "/Users/alex/Library/Mobile Documents/com~apple~CloudDocs/Milkrice Sync", in_icloud: true, device_name: "Alex's MacBook Air", last_ok: iso(Date.now() - 40e3), error: null, pairing: false,
          devices: [{ id: "a", name: "Alex's MacBook Air", kind: "mac", last_seen: iso(now), this_mac: true }, ...(db.sync.phone ? [{ id: "b", name: "Alex's iPhone", kind: "iphone", last_seen: iso(now - 300e3), this_mac: false }] : [])],
          meetings_from_others: db.sync.phone ? 3 : 0, unreadable: 0 }
      : { setup: false, icloud_available: !flag("nosync"), default_folder: "~/Library/Mobile Documents/com~apple~CloudDocs/Milkrice Sync", default_has_vault: false },
    sync_setup: () => { db.sync.setup = true; return handlers.sync_status(); },
    sync_pair_start: () => { db.sync.polls = 0; let r = ""; for (let y = 0; y < 25; y++) for (let x = 0; x < 25; x++) if ((x * 7 + y * 13 + x * y) % 3 === 0 || ((x < 7 || x > 17) && y < 7) || (x < 7 && y > 17)) r += `<rect x="${x}" y="${y}" width="1" height="1"/>`; return { qr: "milkrice-pair:v1:AbC123preview", svg: `<svg viewBox="-2 -2 29 29" xmlns="http://www.w3.org/2000/svg"><rect x="-2" y="-2" width="29" height="29" fill="#fff"/><g fill="#1d1b16">${r}</g></svg>` }; },
    sync_pair_poll: () => (++db.sync.polls > 3 ? { name: "Alex's iPhone", code: "482915" } : null),
    sync_pair_accept: () => { db.sync.phone = true; return "Alex's iPhone"; },
    sync_pair_cancel: () => null,
    sync_now: () => null,
    sync_leave: () => { db.sync = { setup: false }; },
    audio_devices: () => ({ inputs: ["MacBook Air Microphone", "AirPods Pro", "Shure MV7"], default_input: "MacBook Air Microphone", default_output: "AirPods Pro", selected: null, active: "MacBook Air Microphone", capture_system: true }),
    set_mic: () => null,
    mic_test: () => null,
    probe_mics: async () => { await new Promise(r=>setTimeout(r,1500)); return [{name:"MacBook Air Microphone",peak:0,digital_silence:true,error:null},{name:"AirPods Pro",peak:0.004,digital_silence:false,error:null},{name:"Shure MV7",peak:0.21,digital_silence:false,error:null}]; },
    calendar_sources: () => ({ feeds: [], calendars_with_events: ["Work"], event_count: 2 }),
    meeting_audio: ({id}) => id === "m1" ? [{offset_ms: 0, path: "/x/0.m4a"}] : [],
    audio_storage: () => ({ bytes: 48e6 }),
    obsidian_vaults: () => [{name:"second-brain", path:"~/Notes/second-brain"}],
    obsidian_export_all: () => 12,
    input_devices: () => ["MacBook Air Microphone"],
    rec_state: () => {
      const rec = flag("rec") || flag("auto");
      emit("rec-state", rec ? { recording: true, meeting_id: "m1", paused: false, elapsed_ms: 754000, system_audio: true, auto: flag("auto") } : { recording: false });
      if (flag("auto")) emit("auto-recording", { meeting_id: "m1", title: "Beta launch sync" });
    },
    start_recording: ({ id }) => {
      emit("rec-state", { recording: true, meeting_id: id, paused: false, elapsed_ms: 0, system_audio: true });
      // Fake live transcript lines so the onboarding test and live views have something to show.
      clearTimeout(db.fakeT);
      const lines = ["Testing, testing, one two three.", "Today I'm working on the onboarding flow.", "Milkrice should pick this up on its own."];
      lines.forEach((text, i) => setTimeout(() => {
        const seg = { id: 100 + i, start_ms: 1500 + i * 3500, end_ms: 4500 + i * 3500, source: "mic", speaker: "me", text };
        (db.segments[id] ||= []).push(seg);
        emit("segment", { meeting_id: id, segment: seg });
      }, 1600 + i * 2200));
    },
    stop_recording: () => emit("rec-state", { recording: false }),
    ask: ({ id, question }) => { (db.chats[id] ||= []).push({ role: "user", content: question }, { role: "assistant", content: "- Beta ships **Friday**\n- Scope freezes tomorrow" }); return ""; },
  };
  window.__TAURI__ = {
    core: { convertFileSrc: (p) => p, invoke: async (cmd, args) => (handlers[cmd] ? handlers[cmd](args || {}) : null) },
    event: { listen: async (ev, fn) => { (listeners[ev] ||= []).push(fn); return () => {}; } },
  };
  setInterval(() => { emit("levels", { mic: Math.random() * 0.3, system: Math.random() * 0.25 }); emit("mic-test", { level: Math.random() * 0.3, digital_silence: false }); }, 100);
})();
