// Milkrice UI — vanilla JS, no build step.
"use strict";

const T = window.__TAURI__;
const invoke = (cmd, args) => T.core.invoke(cmd, args);
const listen = (ev, fn) => T.event.listen(ev, (e) => fn(e.payload));

const $ = (s, el = document) => el.querySelector(s);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const ICON = {
  globe: '<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="6"/><path d="M2 8h12M8 2c2 2 2 10 0 12M8 2c-2 2-2 10 0 12"/></svg>',
  live: '<svg viewBox="0 0 16 16"><path d="M3 6v4M5.5 4v8M8 2v12M10.5 4v8M13 6v4"/></svg>',
  translate: '<svg viewBox="0 0 16 16"><path d="M2 4h7M5.5 2v2M3.5 4c.5 2.5 2.5 4.5 5 5M7.5 4c-.5 2.5-2.5 4.5-5 5M9 14l2.5-6 2.5 6M9.8 12h3.4"/></svg>',
  disk: '<svg viewBox="0 0 16 16"><rect x="2" y="9" width="12" height="4.5" rx="1.2"/><path d="M3 9l1.5-5h7L13 9M11 11.3h.01"/></svg>',
  down: '<svg viewBox="0 0 16 16"><path d="M8 2v8M4.5 6.5 8 10l3.5-3.5M2.5 13.5h11"/></svg>',
  trash: '<svg viewBox="0 0 16 16"><path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 9h5.8l.6-9"/></svg>',
  check: '<svg viewBox="0 0 16 16"><path d="m3.5 8.5 3 3 6-7"/></svg>',
  stop: '<svg viewBox="0 0 16 16"><rect x="4" y="4" width="8" height="8" rx="1.5" fill="currentColor" stroke="none"/></svg>',
  pause: '<svg viewBox="0 0 16 16"><path d="M5.5 3.5v9M10.5 3.5v9" stroke-width="2.2"/></svg>',
  play: '<svg viewBox="0 0 16 16"><path d="M5 3.5v9l7.5-4.5z" fill="currentColor"/></svg>',
  transcript: '<svg viewBox="0 0 16 16"><path d="M3 4h10M3 7h7M3 10h10M3 13h5"/></svg>',
  spark: '<svg viewBox="0 0 16 16"><path d="M8 1.5 9.4 6.6 14.5 8 9.4 9.4 8 14.5 6.6 9.4 1.5 8l5.1-1.4z"/></svg>',
  send: '<svg viewBox="0 0 16 16"><path d="M2.5 8h10M8.5 4l4 4-4 4"/></svg>',
  dots: '<svg viewBox="0 0 16 16"><circle cx="3.5" cy="8" r=".8" fill="currentColor"/><circle cx="8" cy="8" r=".8" fill="currentColor"/><circle cx="12.5" cy="8" r=".8" fill="currentColor"/></svg>',
  copy: '<svg viewBox="0 0 16 16"><rect x="5" y="5" width="8.5" height="8.5" rx="1.5"/><path d="M3 10.5V3.8C3 3.3 3.3 3 3.8 3h6.7"/></svg>',
  people: '<svg viewBox="0 0 16 16"><circle cx="6" cy="5.5" r="2.3"/><path d="M2 13c.4-2.4 2-3.6 4-3.6s3.6 1.2 4 3.6M11 3.5a2.2 2.2 0 0 1 0 4.2M12.2 9.6c1.1.4 1.9 1.6 2 3.4"/></svg>',
  cal: '<svg viewBox="0 0 16 16"><rect x="2.5" y="3.5" width="11" height="10" rx="1.5"/><path d="M2.5 6.5h11M5.5 2v3M10.5 2v3"/></svg>',
  folder: '<svg viewBox="0 0 16 16"><path d="M2 4.5c0-.6.4-1 1-1h3.2l1.5 1.6H13c.6 0 1 .4 1 1v6.4c0 .6-.4 1-1 1H3c-.6 0-1-.4-1-1z"/></svg>',
  phone: '<svg viewBox="0 0 16 16"><rect x="4.5" y="1.5" width="7" height="13" rx="1.6"/><path d="M7 12.3h2"/></svg>',
  link: '<svg viewBox="0 0 16 16"><path d="M6.8 9.2a2.8 2.8 0 0 0 4 0l2-2a2.8 2.8 0 0 0-4-4l-.8.8M9.2 6.8a2.8 2.8 0 0 0-4 0l-2 2a2.8 2.8 0 0 0 4 4l.8-.8"/></svg>',
};

// ------------------------------------------------------------------ state
const S = {
  view: "home", // home | note | settings | onboarding
  meetingId: null,
  detail: null, // {meeting, segments, chats, busy}
  meetings: [],
  events: [],
  settings: null,
  rec: { recording: false },
  prompt: null,
  tab: "notes", // notes | transcript | ask
  showMine: false,
  query: "",
  folders: [], // [{id, name, count}]
  folder: null, // folder id the sidebar is filtered to
  allFolders: false, // show every folder, not just the first few
  models: null,
  progress: {},
  providers: null,
  templates: [],
  perms: null,
  levels: { mic: 0, system: 0 },
  asking: false,
  langFilter: "all",
};

function toast(msg, ms = 2600) {
  const t = $("#toast");
  t.textContent = msg;
  t.classList.add("show");
  clearTimeout(toast._t);
  toast._t = setTimeout(() => t.classList.remove("show"), ms);
}
const fail = (e) => toast(String(e?.message ?? e));

// ------------------------------------------------------------------ formatting
const fmtTime = (d) => new Date(d).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
const fmtDur = (ms) => {
  const s = Math.floor(ms / 1000), h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), x = s % 60;
  return h ? `${h}:${String(m).padStart(2, "0")}:${String(x).padStart(2, "0")}` : `${m}:${String(x).padStart(2, "0")}`;
};
const fmtTs = (ms) => { const s = Math.floor(ms / 1000); return `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`; };
function dayLabel(d) {
  const x = new Date(d), now = new Date();
  const days = Math.round((new Date(now.toDateString()) - new Date(x.toDateString())) / 864e5);
  if (days === 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days < 7) return x.toLocaleDateString([], { weekday: "long" });
  return x.toLocaleDateString([], { month: "long", day: "numeric", year: x.getFullYear() === now.getFullYear() ? undefined : "numeric" });
}
function eventDay(d) {
  const x = new Date(d), now = new Date();
  const days = Math.round((new Date(x.toDateString()) - new Date(now.toDateString())) / 864e5);
  if (days === 0) return "Today";
  if (days === 1) return "Tomorrow";
  return x.toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" });
}
const initials = (n) => (n || "?").split(/[\s@._-]+/).filter(Boolean).slice(0, 2).map((w) => w[0].toUpperCase()).join("");
const SP_COLORS = ["var(--sp1)", "var(--sp2)", "var(--sp3)", "var(--sp4)", "var(--sp5)", "var(--sp6)"];
function speakerColor(key) {
  if (key === "me") return "var(--ink)";
  const n = parseInt(String(key).replace(/\D/g, ""), 10);
  return SP_COLORS[((isNaN(n) ? 1 : n) - 1) % SP_COLORS.length];
}
function speakerName(key, m) {
  const n = m?.speakers?.[key];
  if (n) return n;
  if (key === "me") return S.settings?.user_name || "Me";
  if (key === "them") return "Them";
  return `Speaker ${String(key).replace(/^s/, "")}`;
}

// Small, safe Markdown renderer (headings, lists, tasks, bold, italics, code, quotes).
// With { tasks: true } checkboxes are live and carry their source line (data-line).
const TASK_RE = /^(\s*[-*]\s+\[)( |x|X)(\]\s+)(.*)/;
function md(src, opts = {}) {
  const inline = (t) => esc(t)
    .replace(/`([^`]+)`/g, "<code>$1</code>")
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
    .replace(/(^|[^*])\*([^*\s][^*]*)\*/g, "$1<em>$2</em>");
  const out = [];
  let list = false;
  const close = () => { if (list) { out.push("</ul>"); list = false; } };
  const lines = String(src || "").split("\n");
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i].trimEnd();
    let m;
    if (!line.trim()) { close(); continue; }
    if ((m = line.match(/^#{1,6}\s+(.*)/))) { close(); out.push(`<h3>${inline(m[1])}</h3>`); continue; }
    if ((m = line.match(TASK_RE))) {
      if (!list) { out.push("<ul>"); list = true; }
      const done = m[2] !== " ";
      out.push(`<li class="task ${done ? "done" : ""}"><label><input type="checkbox" ${done ? "checked" : ""} ${opts.tasks ? `data-line="${i}"` : "disabled"}><span>${inline(m[4])}</span></label></li>`);
      continue;
    }
    if ((m = line.match(/^\s*(?:[-*•]|\d+[.)])\s+(.*)/))) {
      if (!list) { out.push("<ul>"); list = true; }
      out.push(`<li>${inline(m[1])}</li>`);
      continue;
    }
    if ((m = line.match(/^>\s?(.*)/))) { close(); out.push(`<blockquote>${inline(m[1])}</blockquote>`); continue; }
    close();
    out.push(`<p>${inline(line)}</p>`);
  }
  close();
  return out.join("");
}

// ------------------------------------------------------------------ data
async function loadMeetings() {
  S.meetings = await invoke("list_meetings", { query: S.query || null });
  renderSidebar();
}
async function loadFolders() {
  S.folders = await invoke("list_folders").catch(() => []);
  if (S.folder && !S.folders.some((f) => f.id === S.folder)) S.folder = null;
  renderSidebar();
}
async function loadEvents() {
  S.events = await invoke("upcoming_events");
  renderSidebar();
  if (S.view === "home") render();
}
// Leaving a note that never got anything in it removes it (e.g. an upcoming event you peeked at).
async function leaveNote(nextId) {
  const prev = S.view === "note" ? S.meetingId : null;
  if (!prev || prev === nextId) return;
  const n = await invoke("discard_if_empty", { keep: nextId || null }).catch(() => 0);
  if (n) await loadMeetings();
}
async function goHome() {
  closeFolderPicker();
  await leaveNote(null);
  S.view = "home"; S.meetingId = null; S.detail = null;
  render(); renderSidebar();
}
async function openMeeting(id, tab) {
  await leaveNote(id);
  S.meetingId = id;
  S.view = "note";
  if (tab) S.tab = tab;
  S.detail = await invoke("get_meeting", { id });
  S.detail.audio = await invoke("meeting_audio", { id }).catch(() => []);
  if (!tab) S.tab = S.rec.recording && S.rec.meeting_id === id ? "notes" : S.detail.meeting.enhanced ? "notes" : (S.detail.segments.length && S.settings?.provider ? "transcript" : "notes");
  S.showMine = false;
  S.editingAi = null;
  S.confirmRegen = null;
  render();
  renderSidebar();
}
async function refreshDetail() {
  if (S.view !== "note" || !S.meetingId) return;
  const ed = $("#notesEditor");
  const typed = ed ? ed.value : null;
  S.detail = await invoke("get_meeting", { id: S.meetingId });
  if (typed !== null) S.detail.meeting.notes = typed; // a save may still be in flight
  S.detail.audio = await invoke("meeting_audio", { id: S.meetingId }).catch(() => []);
  render(true);
}

// ------------------------------------------------------------------ sidebar
function renderSidebar() {
  const el = $("#lists");
  const now = Date.now();
  const upcoming = S.events
    .filter((x) => new Date(x.event.end) > now && (x.real || !S.settings?.only_real_meetings))
    .slice(0, 2);
  let h = "";
  // Two quiet blocks above the notes (next meetings, folders), each one line per row; the
  // full schedule lives on Home.
  if (upcoming.length && !S.query) {
    h += `<section class="side-sec"><div class="group-label">Coming up</div>`;
    for (const x of upcoming) {
      const e = x.event, live = new Date(e.start) <= now;
      const today = new Date(e.start).toDateString() === new Date().toDateString();
      h += `<button class="item event-item" data-event="${esc(e.id)}" title="${esc(e.title || "")}${e.attendees.length ? ` · ${e.attendees.length} people` : ""}">
        <span class="t">${esc(e.title || "Untitled event")}</span>
        <span class="when">${live ? "Now" : today ? fmtTime(e.start) : `${new Date(e.start).toLocaleDateString([], { weekday: "short" })} ${fmtTime(e.start)}`}</span>
      </button>`;
    }
    h += `</section>`;
  }
  h += foldersSidebarHtml();
  if (h) h += `<div class="side-rule"></div>`;
  let last = null;
  const list = S.folder ? S.meetings.filter((m) => (m.folders || []).includes(S.folder)) : S.meetings;
  if (!list.length && S.query) h += `<div class="group-label">No matches</div>`;
  else if (!list.length && S.folder) h += `<p class="folder-empty">Nothing here yet. Drag notes in, or use <b>Add to folder</b> on a note.</p>`;
  for (const m of list) {
    const d = dayLabel(m.created_at);
    if (d !== last) { h += `<div class="group-label">${esc(d)}</div>`; last = d; }
    const rec = S.rec.recording && S.rec.meeting_id === m.id;
    const who = m.attendees.filter((a) => !a.is_self).map((a) => a.name.split(" ")[0]).slice(0, 3).join(", ");
    h += `<button class="item ${S.meetingId === m.id && S.view === "note" ? "active" : ""}" data-meeting="${esc(m.id)}" draggable="true">
      <span class="t">${esc(m.title || "Untitled")}</span>
      <span class="s">${m.from_iphone ? `<i class="from-phone" title="Recorded on iPhone">${ICON.phone}</i>` : ""}${fmtTime(m.created_at)}${m.duration_ms ? ` · ${fmtDur(m.duration_ms)}` : ""}${who ? ` · ${esc(who)}` : ""}</span>
      ${rec ? '<span class="rec-dot"></span>' : ""}
    </button>`;
  }
  el.innerHTML = h;
}

// Folders: a quiet list that appears once the first folder exists. Click to filter the notes
// below; drag a note onto a folder to file it.
const FOLDERS_SHOWN = 5;
const FOLDER_COLORS = ["ember", "sage", "blue", "amber", "violet", "teal", "rose", "slate"];
function folderColor(f) {
  if (f.color) return f.color;
  let h = 0;
  for (const c of f.id) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return FOLDER_COLORS[h % FOLDER_COLORS.length];
}
function foldersSidebarHtml() {
  if (!S.folders.length || S.query) return "";
  let shown = S.folders;
  if (!S.allFolders && shown.length > FOLDERS_SHOWN + 1) {
    shown = shown.slice(0, FOLDERS_SHOWN);
    const act = S.folders.find((f) => f.id === S.folder);
    if (act && !shown.includes(act)) shown.push(act);
  }
  let h = `<section class="side-sec"><div class="group-label">Folders</div>`;
  for (const f of shown) {
    const on = S.folder === f.id;
    h += `<div class="item folder-item fc-${folderColor(f)} ${on ? "active" : ""}" data-folder="${esc(f.id)}" role="button" tabindex="0">
      ${ICON.folder}<span class="t">${esc(f.name)}</span>
      ${on ? `<button class="icon-btn folder-more" data-folder-menu="${esc(f.id)}" title="Rename or delete">${ICON.dots}</button>` : `<span class="n">${f.count || ""}</span>`}
    </div>`;
  }
  if (shown.length < S.folders.length) h += `<button class="link folder-all" data-folders-all>Show all ${S.folders.length}</button>`;
  return h + `</section>`;
}

function closeFolderMenu() { $("#folderMenu")?.remove(); document.removeEventListener("mousedown", folderMenuOutside, true); }
function folderMenuOutside(e) { if (!e.target.closest("#folderMenu")) closeFolderMenu(); }
function openFolderMenu(id, anchor) {
  closeFolderMenu();
  const f = S.folders.find((x) => x.id === id);
  if (!f) return;
  const pop = document.createElement("div");
  pop.id = "folderMenu";
  pop.className = "menu-pop";
  const r = anchor.getBoundingClientRect();
  pop.style.left = `${r.right - 160}px`;
  pop.style.top = `${r.bottom + 4}px`;
  pop.innerHTML = `<div class="swatches">${FOLDER_COLORS.map((c) => `<button class="sw fc-${c} ${folderColor(f) === c ? "on" : ""}" data-color="${c}" title="${c}" aria-label="${c}"></button>`).join("")}</div>
    <button data-act="rename">Rename</button><button data-act="delete" class="danger">Delete folder</button>`;
  pop.querySelectorAll("[data-color]").forEach((b) => (b.onclick = async () => {
    await invoke("set_folder_color", { id: f.id, color: b.dataset.color }).catch(fail);
    closeFolderMenu();
    await afterFolderChange();
  }));
  document.body.appendChild(pop);
  pop.querySelector('[data-act="rename"]').onclick = () => { closeFolderMenu(); renameFolder(f); };
  pop.querySelector('[data-act="delete"]').onclick = () => { closeFolderMenu(); deleteFolder(f); };
  setTimeout(() => document.addEventListener("mousedown", folderMenuOutside, true), 0);
}
function renameFolder(f) {
  dialog(`<form><h4>Rename folder</h4>
    <input class="field" name="n" value="${esc(f.name)}" maxlength="60" autofocus />
    <div class="actions"><button type="button" class="btn" data-cancel>Cancel</button><button class="btn primary">Save</button></div></form>`,
    async (dl) => {
      try { await invoke("rename_folder", { id: f.id, name: dl.querySelector("[name=n]").value }); } catch (e) { return fail(e); }
      await afterFolderChange();
    });
}
function deleteFolder(f) {
  dialog(`<form><h4>Delete “${esc(f.name)}”?</h4>
    <p class="hint" style="margin:0 0 12px">The folder goes away. Its ${f.count === 1 ? "note stays" : "notes stay"} in Milkrice.</p>
    <div class="actions"><button type="button" class="btn" data-cancel>Cancel</button><button class="btn primary">Delete folder</button></div></form>`,
    async () => {
      await invoke("delete_folder", { id: f.id }).catch(fail);
      if (S.folder === f.id) S.folder = null;
      await afterFolderChange();
    });
}
async function afterFolderChange() {
  await Promise.all([loadFolders(), loadMeetings()]);
  if (S.view === "note" && S.meetingId) await refreshDetail();
}
async function fileNote(meetingId, folderId, on) {
  try { await invoke("set_meeting_folder", { meetingId, folderId, on }); } catch (e) { return fail(e); }
  await afterFolderChange();
}

$("#lists").addEventListener("dragstart", (e) => {
  const m = e.target.closest("[data-meeting]");
  if (m) { e.dataTransfer.setData("text/x-milkrice-note", m.dataset.meeting); e.dataTransfer.effectAllowed = "link"; }
});
$("#lists").addEventListener("dragover", (e) => {
  const f = e.target.closest("[data-folder]");
  if (f && e.dataTransfer.types.includes("text/x-milkrice-note")) { e.preventDefault(); f.classList.add("drop"); }
});
$("#lists").addEventListener("dragleave", (e) => e.target.closest("[data-folder]")?.classList.remove("drop"));
$("#lists").addEventListener("drop", (e) => {
  const f = e.target.closest("[data-folder]");
  const id = e.dataTransfer.getData("text/x-milkrice-note");
  if (!f || !id) return;
  e.preventDefault();
  const name = S.folders.find((x) => x.id === f.dataset.folder)?.name;
  fileNote(id, f.dataset.folder, true).then(() => toast(`Added to ${name}`));
});

$("#lists").addEventListener("click", (e) => {
  const fm = e.target.closest("[data-folder-menu]");
  if (fm) { e.stopPropagation(); return openFolderMenu(fm.dataset.folderMenu, fm); }
  if (e.target.closest("[data-folders-all]")) { S.allFolders = true; return renderSidebar(); }
  const fo = e.target.closest("[data-folder]");
  if (fo) { S.folder = S.folder === fo.dataset.folder ? null : fo.dataset.folder; return renderSidebar(); }
  const m = e.target.closest("[data-meeting]");
  if (m) return openMeeting(m.dataset.meeting).catch(fail);
  const ev = e.target.closest("[data-event]");
  if (ev) {
    const x = S.events.find((y) => y.event.id === ev.dataset.event);
    if (x) openEvent(x.event, false);
  }
});
async function openEvent(event, record) {
  try {
    const id = await invoke("start_event", { event, record });
    await loadMeetings();
    await openMeeting(id, "notes");
  } catch (e) { fail(e); }
}

// ------------------------------------------------------------------ banner (meeting starting)
// Work out *why* the mic is silent before telling the user anything.
async function diagnoseMic(kind) {
  await refreshPerms();
  if (S.perms?.microphone !== "granted") { S.sysWarn = "mic-perm"; renderSysWarn(); return; }
  await loadDevices();
  S.micDiag = { kind, probing: true };
  S.sysWarn = "mic";
  renderSysWarn();
  try {
    const probes = await invoke("probe_mics");
    const active = S.devices?.active;
    const better = probes
      .filter((p) => !p.error && !p.digital_silence && p.peak > 0.03 && p.name !== active)
      .sort((a, b) => b.peak - a.peak)[0];
    S.micDiag = { kind, probing: false, better: better?.name || null, probes };
  } catch (e) { S.micDiag = { kind, probing: false, better: null }; }
  renderSysWarn();
}

function renderSysWarn() {
  let el = $("#sysWarn");
  const show = S.sysWarn && S.rec.recording && (S.sysWarn !== "system" || S.perms?.system_audio !== "granted");
  if (!show) { if (el) el.remove(); return; }
  if (!el) { el = document.createElement("div"); el.id = "sysWarn"; $("#main").appendChild(el); }
  const active = shortDevice(S.devices?.active);
  const d = S.micDiag || {};
  let text, action;
  if (S.sysWarn === "mic-perm") {
    text = "macOS isn't letting Milkrice use the microphone.";
    action = `<button class="btn small ember" id="sysFix">Allow microphone</button>`;
  } else if (S.sysWarn === "mic") {
    const what = d.kind === "silent" ? `${esc(active)} isn't sending any sound` : `Milkrice can barely hear you on ${esc(active)}`;
    if (d.probing) { text = `${what}. Checking your other microphones…`; action = `<span class="spinner"></span>`; }
    else if (d.better) { text = `${what} — but <b>${esc(d.better)}</b> can hear you.`; action = `<button class="btn small ember" id="sysSwitch">Switch to ${esc(shortDevice(d.better))}</button><button class="btn small" id="sysDismiss">Ignore</button>`; }
    else { text = `${what}. Is it muted, or is another mic in use?`; action = `<button class="btn small ember" id="sysPick">Choose mic</button><button class="btn small" id="sysDismiss">Ignore</button>`; }
  } else {
    text = "Only your microphone is being transcribed. Allow Milkrice to hear the other people on the call.";
    action = `<button class="btn small ember" id="sysFix">Fix in 10 seconds</button>`;
  }
  el.innerHTML = `<div class="syswarn"><span>${text}</span>${action}</div>`;
  const sw = $("#sysSwitch");
  if (sw) sw.onclick = async () => { const dev = S.micDiag.better; S.sysWarn = false; renderSysWarn(); try { await invoke("set_mic", { device: dev }); S.settings.mic_device = dev; toast(`Switched to ${dev}`); loadDevices(); } catch (e) { fail(e); } };
  const pk = $("#sysPick");
  if (pk) pk.onclick = () => { S.sysWarn = false; renderSysWarn(); openDevicePicker(); };
  const dm = $("#sysDismiss");
  if (dm) dm.onclick = () => { S.sysWarn = false; renderSysWarn(); };
  const fx = $("#sysFix");
  if (fx) fx.onclick = () => openPermsSheet(S.sysWarn === "mic-perm" ? "Milkrice needs your microphone" : "Hear everyone on the call");
}
function autoBannerHtml() {
  const m = S.meetings.find((x) => x.id === S.rec.meeting_id);
  const title = S.autoTitle || m?.title || "your meeting";
  return `<div class="banner auto">
    <span class="rec-badge"><span class="rec-dot"></span>REC</span>
    <div class="b-text">
      <div class="b-title">Recording automatically: ${esc(title)}</div>
      <div class="b-sub">${S.settings?.calendar_mode === "auto" ? "Auto-record started this from your calendar." : "Auto-record is now off; this recording keeps going until you stop it."}</div>
    </div>
    <button class="btn ember small" id="bStop">${ICON.stop} Stop</button>
    ${S.settings?.calendar_mode === "auto" ? `<button class="btn small" id="bAutoOff">Turn off auto-record</button>` : ""}
    <button class="icon-btn" id="bHide" title="Hide">×</button>
  </div>`;
}

function renderBanner(force) {
  const b = $("#banner");
  const p = S.prompt;
  const auto = S.rec.recording && S.rec.auto && S.autoHidden !== S.rec.meeting_id;
  const key = auto ? `auto|${S.rec.meeting_id}|${S.settings?.calendar_mode}|${S.autoTitle || ""}` : p && !S.rec.recording ? `${p.kind}|${p.title}|${p.at}` : "";
  if (b.dataset.key === key && !force) return; // unchanged: don't replay the entrance animation
  b.dataset.key = key;
  if (!key) { b.innerHTML = ""; return; }
  if (auto) {
    b.innerHTML = autoBannerHtml();
    $("#bStop").onclick = () => invoke("stop_recording").catch(fail);
    const off = $("#bAutoOff");
    if (off) off.onclick = turnOffAutoRecord;
    $("#bHide").onclick = () => { S.autoHidden = S.rec.meeting_id; renderBanner(); };
    return;
  }
  const who = p.event ? p.event.attendees.filter((a) => !a.is_self).map((a) => a.name).slice(0, 4).join(", ") : "";
  b.innerHTML = `<div class="banner">
    <span class="rec-dot"></span>
    <div class="b-text">
      <div class="b-title">${esc(p.kind === "call" ? `${p.app} call in progress` : `${p.title} is starting`)}</div>
      <div class="b-sub">${p.kind === "call" ? "No calendar event — take notes anyway?" : who ? `With ${esc(who)}` : "From your calendar"}</div>
    </div>
    <button class="btn ember small" id="bStart">Start notes</button>
    <button class="btn small" id="bDismiss">Not now</button>
  </div>`;
  $("#bStart").onclick = async () => {
    try {
      if (p.event) await openEvent(p.event, true);
      else {
        const m = await invoke("create_meeting", { title: p.title });
        await invoke("start_recording", { id: m.id });
        await loadMeetings();
        await openMeeting(m.id, "notes");
      }
    } catch (e) { fail(e); }
  };
  $("#bDismiss").onclick = () => invoke("dismiss_prompt");
}

// ------------------------------------------------------------------ views
function render(soft) {
  renderBanner();
  if (S.view !== "note" && player.meeting) stopPlayer();
  $("#homeBtn").hidden = S.view === "home" || S.view === "onboarding";
  const v = $("#view");
  if (S.view === "onboarding") return renderOnboarding(v);
  if (S.view === "settings") return renderSettings(v);
  if (S.view === "note" && S.detail) return renderNote(v, soft);
  return renderHome(v);
}

function renderHome(v) {
  const hr = new Date().getHours();
  const greet = hr < 12 ? "Good morning" : hr < 18 ? "Good afternoon" : "Good evening";
  const name = S.settings?.user_name ? `, ${S.settings.user_name.split(" ")[0]}` : "";
  const now = Date.now();
  const evs = S.events.filter((x) => new Date(x.event.end) > now && (x.real || !S.settings?.only_real_meetings)).slice(0, 6);
  const calOk = S.perms?.calendar === "granted";
  let h = `<div class="page"><h1 class="hello">${greet}${esc(name)}</h1>
    <p class="hello-sub">${S.rec.recording ? "Milkrice is listening." : evs.length ? `${evs.length} meeting${evs.length > 1 ? "s" : ""} coming up.` : "Your notes, kept on this Mac."}</p>`;
  const missing = ["microphone", "system_audio", "calendar"].filter((k) => S.perms && S.perms[k] !== "granted");
  if (missing.length) h += `<div class="callout setup">${ICON.spark}<div style="flex:1">Milkrice can't hear everything yet — ${missing.length} permission${missing.length > 1 ? "s" : ""} to go.</div><button class="btn small ember" id="setupPerms">Finish setup</button></div><div style="height:14px"></div>`;
  h += `<label class="ask-all">${ICON.spark}<input id="askAll" placeholder="Ask your recent meetings…" value="${esc(S.homeQ || "")}" /></label>`;
  if (S.homeAsking) h += `<div class="answer"><span class="busy"><span class="spinner"></span>Reading your meetings…</span></div>`;
  else if (S.homeAnswer) h += `<div class="answer md">${md(S.homeAnswer)}</div>`;
  h += `<div class="section-h"><h3>Coming up</h3>${calOk ? "" : '<button class="link" id="connectCal">Connect calendar</button>'}</div>`;
  if (evs.length) {
    h += `<div class="cards">`;
    for (const x of evs) {
      const e = x.event, live = new Date(e.start) <= now;
      const who = e.attendees.filter((a) => !a.is_self).map((a) => a.name).slice(0, 3).join(", ");
      h += `<div class="event-card ${live ? "now" : ""}">
        <div class="time"><b>${live ? "Now" : fmtTime(e.start)}</b>${live ? `until ${fmtTime(e.end)}` : eventDay(e.start)}</div>
        <div style="min-width:0"><div class="et">${esc(e.title || "Untitled event")}</div>
          <div class="ea">${who ? esc(who) : esc(e.calendar)}${e.meeting_url ? " · call link" : ""}</div></div>
        <div style="display:flex;gap:6px">
          ${x.meeting_id ? `<button class="btn small" data-open="${esc(x.meeting_id)}">Open</button>` : ""}
          <button class="btn small ${live ? "ember" : ""}" data-ev="${esc(e.id)}" data-rec="${live ? 1 : 0}">${live ? "Start notes" : "Prep notes"}</button>
        </div></div>`;
    }
    h += `</div>`;
  } else {
    h += `<div class="callout">${ICON.cal}<div>${calOk ? "Nothing else on your calendar today." : "Connect your calendar and Milkrice will nudge you as meetings start — and stop when they end. It reads the calendars already on this Mac; no sign-in."}</div></div>`;
  }
  h += `<div class="section-h"><h3>Recent notes</h3></div>`;
  if (S.meetings.length) {
    for (const m of S.meetings.slice(0, 8)) {
      h += `<button class="note-row" data-open="${esc(m.id)}"><span class="nd">${esc(dayLabel(m.created_at).slice(0, 9))}</span>
        <span style="min-width:0"><div class="nt">${esc(m.title || "Untitled")}</div><div class="ns">${esc((m.snippet || "").replace(/[#*\-\[\]]/g, "").trim() || "No notes yet")}</div></span></button>`;
    }
  } else {
    h += `<div class="empty"><img class="illus" src="img/empty-state.jpg" alt=""><p>No notes yet. Start one with <b>New note</b>, or let Milkrice catch your next meeting.</p></div>`;
  }
  h += `</div>`;
  v.innerHTML = h + dockHtml();
  bindDock();
  v.querySelectorAll("[data-open]").forEach((b) => (b.onclick = () => openMeeting(b.dataset.open).catch(fail)));
  v.querySelectorAll("[data-ev]").forEach((b) => (b.onclick = () => {
    const x = S.events.find((y) => y.event.id === b.dataset.ev);
    if (x) openEvent(x.event, b.dataset.rec === "1");
  }));
  const cc = $("#connectCal");
  if (cc) cc.onclick = connectCalendar;
  const sp = $("#setupPerms");
  if (sp) sp.onclick = () => openPermsSheet();
  const aa = $("#askAll");
  if (aa) aa.onkeydown = async (e) => {
    if (e.key !== "Enter" || !aa.value.trim()) return;
    S.homeQ = aa.value.trim(); S.homeAsking = true; S.homeAnswer = null; renderHome(v);
    try { S.homeAnswer = await invoke("ask_all", { question: S.homeQ }); } catch (err) { fail(err); }
    S.homeAsking = false; if (S.view === "home") renderHome(v);
  };
}

async function connectCalendar() { await grantPerm("calendar"); }

// Honest privacy badge: "On this Mac" only when notes are written locally (or not at all).
function renderBadge() {
  const b = document.querySelector(".local-badge");
  if (!b || !S.settings) return;
  // Only the provider the user chose counts: with none chosen, AI notes don't run at all.
  const p = S.settings.provider;
  const local = p === "ollama";
  const name = { claude: "Claude", codex: "Codex", ollama: "Ollama" }[p];
  if (local || !p) { b.textContent = "● On this Mac"; b.title = p ? "Audio, transcripts and notes stay on this Mac" : "Audio, transcripts and notes stay on this Mac. AI notes are off until you choose who writes them."; }
  else { b.textContent = `● Audio on this Mac · notes via ${name}`; b.title = `Audio and transcripts stay on this Mac. When notes are written, the transcript text is sent to ${name}.`; }
  renderAutoMarker();
  // With the auto-record marker beside it, keep the badge short (the full text stays in the tooltip).
  if (S.settings.calendar_mode === "auto") b.textContent = local || !p ? "● On this Mac" : `● Notes: ${name}`;
}

// Auto-record must never be forgotten: a persistent marker in the sidebar footer.
function renderAutoMarker() {
  const foot = document.querySelector(".sidebar-foot");
  if (!foot || !S.settings) return;
  let mk = $("#autoMark");
  const on = S.settings.calendar_mode === "auto";
  if (!on) { if (mk) mk.remove(); closeAutoPop(); return; }
  if (!mk) {
    mk = document.createElement("button");
    mk.id = "autoMark";
    mk.className = "auto-mark";
    mk.title = "Auto-record is on: Milkrice starts recording calendar meetings by itself";
    mk.innerHTML = `<span class="rec-dot"></span>Auto-record`;
    mk.onclick = toggleAutoPop;
    foot.insertBefore(mk, foot.querySelector(".local-badge"));
  }
}
function closeAutoPop() { $("#autoPop")?.remove(); document.removeEventListener("mousedown", autoOutside, true); }
function autoOutside(e) { if (!e.target.closest("#autoPop") && !e.target.closest("#autoMark")) closeAutoPop(); }
function toggleAutoPop() {
  if ($("#autoPop")) return closeAutoPop();
  const pop = document.createElement("div");
  pop.id = "autoPop";
  pop.className = "auto-pop";
  pop.innerHTML = `<b>Auto-record is on</b>
    <p>Milkrice starts recording calendar meetings with people in them a minute before they begin, and tells you when it does.</p>
    <button class="btn small primary" id="autoOff">Turn off auto-record</button>
    <p class="hint">You'll get a notification instead, and choose each time.</p>`;
  document.querySelector(".sidebar").appendChild(pop);
  $("#autoOff").onclick = turnOffAutoRecord;
  setTimeout(() => document.addEventListener("mousedown", autoOutside, true), 0);
}
async function turnOffAutoRecord() {
  try {
    S.settings = await invoke("turn_off_auto_record");
    closeAutoPop();
    renderBadge();
    renderBanner(true);
    if (S.view === "settings") render();
    toast("Auto-record is off. Milkrice will notify you instead.");
  } catch (e) { fail(e); }
}

// ------------------------------------------------------------------ permissions
const PERMS = [
  ["microphone", "Microphone", "Hears your side of the conversation."],
  ["system_audio", "Other participants", "Hears everyone else on the call (Screen & System Audio Recording). Milkrice never records your screen."],
  ["calendar", "Calendar", "Knows when meetings start and who's in them. Reads calendars already on this Mac."],
  ["notifications", "Notifications", "Taps you on the shoulder when a meeting starts or notes are ready."],
];
const PERM_PANE = { microphone: "Microphone", system_audio: "Screen & System Audio Recording", calendar: "Calendars", notifications: "Notifications" };
S.permHelp = {}; // kind -> "settings" | "restart"

async function refreshPerms() {
  const before = S.perms;
  S.perms = await invoke("permissions");
  // Screen capture switches on only after a relaunch; offer it once Settings was visited.
  if (before && JSON.stringify(before) !== JSON.stringify(S.perms)) renderPermsInPlace();
  return S.perms;
}

async function grantPerm(kind) {
  try {
    const r = await invoke("grant_permission", { kind });
    if (r.opened_settings) S.permHelp[kind] = kind === "system_audio" ? "restart" : "settings";
    await refreshPerms();
    renderPermsInPlace();
    if (kind === "calendar" && S.perms.calendar === "granted") { toast("Calendar connected"); setTimeout(loadEvents, 1200); }
  } catch (e) { fail(e); }
}

function permsHtml() {
  const p = S.perms || {};
  return `<div class="perms">${PERMS.map(([k, name, why]) => {
    const st = p[k] || "not_determined";
    const ok = st === "granted";
    const help = S.permHelp[k];
    let action;
    if (ok) action = `<span class="badge sage">${ICON.check.replace("<svg", '<svg style="width:11px;height:11px;stroke-width:2.4"')} Allowed</span>`;
    else if (help === "restart") action = `<button class="btn small" data-perm-open="${k}">Open Settings</button><button class="btn small primary" id="permRestart">Restart Milkrice</button>`;
    else action = `<button class="btn small ${k === "microphone" || k === "system_audio" ? "primary" : ""}" data-perm="${k}">${st === "denied" ? "Open Settings" : "Allow"}</button>`;
    const hint = !ok && help
      ? `<div class="perm-hint">${k === "system_audio"
          ? `System Settings is open at <b>Privacy &amp; Security → Screen &amp; System Audio Recording</b>. Switch <b>Milkrice</b> on — if it isn't listed, click <b>+</b> and pick Milkrice from the Finder window that just opened. macOS only applies this after a restart, so then press <b>Restart Milkrice</b>.`
          : st === "denied"
            ? `macOS remembers an earlier “Don't Allow”. In <b>Privacy &amp; Security → ${PERM_PANE[k]}</b> (now open), switch <b>Milkrice</b> on.`
            : `macOS didn't show its prompt. Click <b>Allow</b> again; if nothing appears, quit and reopen Milkrice and try once more.`}</div>`
      : "";
    return `<div class="perm ${ok ? "ok" : ""}"><span class="status ${ok ? "ok" : ""}"></span>
      <div class="pi"><b>${name}</b><span>${why}</span></div><div class="perm-act">${action}</div>${hint}</div>`;
  }).join("")}</div>`;
}

function bindPerms(root) {
  root.querySelectorAll("[data-perm]").forEach((b) => (b.onclick = async () => {
    b.disabled = true; b.textContent = "Waiting for macOS…";
    await grantPerm(b.dataset.perm);
  }));
  root.querySelectorAll("[data-perm-open]").forEach((b) => (b.onclick = () => invoke("open_permission_settings", { kind: b.dataset.permOpen })));
  const rs = root.querySelector("#permRestart");
  if (rs) rs.onclick = () => invoke("restart_app");
}

function renderPermsInPlace() {
  document.querySelectorAll(".perms-slot").forEach((el) => { el.innerHTML = permsHtml(); bindPerms(el); });
}

// Poll while a permissions panel is on screen so ticks appear as soon as the user flips a switch.
// System Settings usually covers Milkrice while the user flips a switch, so keep polling even
// when the window is occluded, and re-check the moment Milkrice is focused again.
setInterval(() => { if (document.querySelector(".perms-slot")) refreshPerms(); }, 1500);
window.addEventListener("focus", () => refreshPerms().then(() => { renderPermsInPlace(); renderSysWarn(); }));

function openPermsSheet(title) {
  const dlg = dialog(`<form><h4>${esc(title || "Let Milkrice listen")}</h4>
    <p class="hint" style="margin:0 0 10px">Audio is processed on this Mac and never uploaded. Each switch takes a second.</p>
    <div class="perms-slot">${permsHtml()}</div>
    <div class="actions"><button type="button" class="btn" data-cancel>Done</button></div></form>`, () => {});
  dlg.style.width = "520px";
  bindPerms(dlg);
  refreshPerms();
}

// ------------------------------------------------------------------ note view
function speakersInMeeting(d) {
  const keys = [...new Set(d.segments.map((s) => s.speaker))];
  return keys.sort((a, b) => (a === "me" ? -1 : b === "me" ? 1 : a.localeCompare(b, undefined, { numeric: true })));
}

function renderNote(v, soft) {
  const d = S.detail, m = d.meeting;
  const isRec = S.rec.recording && S.rec.meeting_id === m.id;
  const editorFocused = document.activeElement && document.activeElement.id === "notesEditor";
  const askFocused = document.activeElement && document.activeElement.id === "askInput";
  // Soft refreshes while typing only touch the transcript/chat to keep the caret steady.
  // Never throw away an in-progress edit of the AI notes on a background refresh.
  const aiEditing = S.editingAi === m.id && $("#aiEditor");
  // ...except when AI notes have just arrived: they replace the editor, so show them now.
  const aiArrived = S.tab === "notes" && m.enhanced && !S.showMine && !$("#aiNotes") && !aiEditing;
  if (soft && !aiArrived && (editorFocused || askFocused || aiEditing || document.activeElement?.id === "titleInput")) {
    const tr = $("#transcriptBox");
    if (tr) tr.innerHTML = transcriptHtml(d);
    const sp = $("#speakersBox");
    if (sp) sp.innerHTML = speakersHtml(d);
    const bz = $("#busyBox");
    if (bz) bz.innerHTML = busyHtml(d);
    return;
  }
  const att = m.attendees.filter((a) => !a.is_self);
  const tplOpts = (S.templates.length ? S.templates : [{ id: "auto", name: "Auto" }])
    .map((t) => `<option value="${t.id}" ${t.id === m.template ? "selected" : ""}>${esc(t.name)}</option>`).join("");
  let h = `<div class="page"><div class="note-head">
    <input id="titleInput" class="title-input" value="${esc(m.title)}" placeholder="Untitled meeting" spellcheck="false" />
    <div class="meta">
      <span class="chip">${ICON.cal}${esc(new Date(m.created_at).toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" }))} · ${fmtTime(m.created_at)}</span>
      ${m.duration_ms ? `<span class="chip">${fmtDur(m.duration_ms)}</span>` : ""}
      ${folderChipsHtml(d)}
      ${att.slice(0, 5).map((a, i) => `<span class="chip" title="${esc(a.email || "")}"><span class="av" style="background:${SP_COLORS[i % 6]}">${esc(initials(a.name))}</span>${esc(a.name)}</span>`).join("")}
      ${att.length > 5 ? `<span class="chip">+${att.length - 5}</span>` : ""}
      ${m.meeting_url ? `<a class="chip" href="#" id="joinLink">${ICON.link}Join call</a>` : ""}
    </div></div>
    <div class="tabs">
      <button class="tab ${S.tab === "notes" ? "on" : ""}" data-tab="notes">Notes</button>
      <button class="tab ${S.tab === "transcript" ? "on" : ""}" data-tab="transcript">Transcript${d.segments.length ? ` <span class="hint">${d.segments.length}</span>` : ""}</button>
      <button class="tab ${S.tab === "ask" ? "on" : ""}" data-tab="ask">Ask</button>
      <span class="spacer"></span>
      <span id="busyBox">${busyHtml(d)}</span>
      <button class="icon-btn" id="moreBtn" title="More">${ICON.dots}</button>
    </div>`;
  if (S.tab === "notes") {
    if (m.enhanced && !S.showMine && S.editingAi === m.id) {
      h += provenanceHtml(m) + `<div class="sheet"><textarea id="aiEditor" class="editor md-editor" spellcheck="true">${esc(m.enhanced)}</textarea></div>
        <div class="note-actions">
          <button class="btn small primary" id="aiSave">Save changes</button>
          <button class="btn small" id="aiCancel">Cancel</button>
          <span class="hint">Markdown: “- [ ]” makes an action item.</span>
        </div>`;
    } else if (m.enhanced && !S.showMine) {
      h += provenanceHtml(m) + `<div class="sheet"><div class="md" id="aiNotes">${md(m.enhanced, { tasks: true })}</div></div>
        <div class="note-actions" id="aiActions">${S.confirmRegen === m.id ? regenConfirmHtml() : `
          <button class="btn small" id="aiEdit">Edit</button>
          <button class="btn small" id="toggleMine">My notes</button>
          <button class="btn small" id="regen" ${d.busy ? "disabled" : ""}>${ICON.spark} Regenerate</button>
          <button class="btn small" id="copyMd">${ICON.copy} Copy</button>
          ${formatHtml(tplOpts)}`}
        </div>
        ${m.notes.trim() ? `<div class="yours"><b style="color:var(--ink)">Your notes</b>\n${esc(m.notes.trim())}</div>` : ""}`;
    } else {
      const noProvider = !S.settings?.provider && !m.enhanced && !isRec && d.segments.length > 0;
      h += (noProvider ? choosePromptHtml() : "") + `<div class="sheet"><textarea id="notesEditor" class="editor" spellcheck="true" placeholder="${isRec ? "Jot down what matters — Milkrice listens and fills in the rest…" : "Write notes here. Start recording and Milkrice will turn them into full notes."}">${esc(m.notes)}</textarea></div>
        <div class="note-actions">
          ${m.enhanced ? `<button class="btn small" id="toggleMine">Show AI notes</button>` : ""}
          ${!isRec && (d.segments.length || m.notes.trim()) ? `<button class="btn small ${m.enhanced ? "" : "primary"}" id="regen" ${d.busy ? "disabled" : ""}>${ICON.spark} ${m.enhanced ? "Regenerate" : "Write AI notes"}</button>${formatHtml(tplOpts)}` : ""}
        </div>`;
    }
  } else if (S.tab === "transcript") {
    h += `<div class="speakers" id="speakersBox">${speakersHtml(d)}</div>
      <div class="transcript" id="transcriptBox">${transcriptHtml(d)}</div>`;
  } else {
    h += chatHtml(d);
  }
  h += `</div>`;
  v.innerHTML = h + dockHtml();
  bindNote(v);
  bindAudio();
  bindDock();
  const ed = $("#notesEditor");
  if (ed) { autoGrow(ed); if (editorFocused) ed.focus(); }
  if (S.tab === "transcript" && isRec) v.scrollTop = v.scrollHeight;
}

// Folders on a note: one chip per folder. With none, a single quiet "Folder" chip, or the
// suggested folder in its place (one click to accept, × to dismiss for this note).
function folderChipsHtml(d) {
  const fs = d.folders || [];
  if (fs.length) return fs.map((f) => `<button class="chip folder-chip fc-${folderColor(f)}" data-folder-pick title="Change folders">${ICON.folder}${esc(f.name)}</button>`).join("");
  if (d.folder_suggestion) {
    return `<span class="chip folder-chip sug" title="Suggested from your other notes">${ICON.folder}<button data-sug-add>Add to ${esc(d.folder_suggestion.name)}</button><button class="x" data-sug-no title="Not this folder" aria-label="Dismiss suggestion">×</button></span>`;
  }
  return `<button class="chip folder-chip add" data-folder-pick title="Add to folder">${ICON.folder}Folder</button>`;
}

function closeFolderPicker() { $("#folderPop")?.remove(); document.removeEventListener("mousedown", folderPickerOutside, true); }
function folderPickerOutside(e) { if (!e.target.closest("#folderPop") && !e.target.closest("[data-folder-pick]")) closeFolderPicker(); }
function openFolderPicker(anchor) {
  if ($("#folderPop")) return closeFolderPicker();
  const pop = document.createElement("div");
  pop.id = "folderPop";
  pop.className = "menu-pop folder-pop";
  const r = anchor.getBoundingClientRect();
  pop.style.left = `${Math.min(r.left, window.innerWidth - 280)}px`;
  pop.style.top = `${r.bottom + 6}px`;
  document.body.appendChild(pop);
  paintFolderPicker();
  setTimeout(() => document.addEventListener("mousedown", folderPickerOutside, true), 0);
}
function paintFolderPicker() {
  const pop = $("#folderPop");
  if (!pop || !S.detail) return;
  const d = S.detail, m = d.meeting;
  const inIds = (d.folders || []).map((f) => f.id);
  const auto = inIds.length > 0 && inIds.every((id) => (d.series_folders || []).includes(id));
  pop.innerHTML = `${S.folders.map((f) => `<button class="pick fc-${folderColor(f)} ${inIds.includes(f.id) ? "on" : ""}" data-fid="${esc(f.id)}"><span class="box">${ICON.check}</span>${esc(f.name)}</button>`).join("")}
    <form class="new-folder"><input class="field" name="n" placeholder="${S.folders.length ? "New folder…" : "Name your first folder"}" maxlength="60" spellcheck="false" /></form>
    ${d.has_series && inIds.length ? `<label class="auto-add"><input type="checkbox" ${auto ? "checked" : ""} /> Auto-add future “${esc(m.title)}” meetings</label>` : ""}`;
  pop.querySelectorAll("[data-fid]").forEach((b) => (b.onclick = async () => {
    await fileNote(m.id, b.dataset.fid, !b.classList.contains("on"));
    paintFolderPicker();
  }));
  pop.querySelector(".new-folder").onsubmit = async (e) => {
    e.preventDefault();
    const name = e.target.n.value.trim();
    if (!name) return;
    try { await invoke("create_folder", { name, meetingId: m.id }); } catch (err) { return fail(err); }
    await afterFolderChange();
    paintFolderPicker();
  };
  const cb = pop.querySelector(".auto-add input");
  if (cb) cb.onchange = async () => {
    for (const id of inIds) await invoke("set_folder_series", { meetingId: m.id, folderId: id, on: cb.checked }).catch(fail);
    await refreshDetail();
    paintFolderPicker();
    toast(cb.checked ? "Future meetings in this series will be added automatically" : "Future meetings won't be added automatically");
  };
  if (!S.folders.length) pop.querySelector(".new-folder input").focus();
}

// The AI notes' layout (Auto, 1:1, Stand-up…): only matters when AI notes get written, so it
// sits beside those buttons, labelled, instead of in the note's header.
function formatHtml(opts) {
  return `<label class="fmt" title="How the AI lays out the notes. Auto picks from the meeting.">Format <select id="tplSelect">${opts}</select></label>`;
}

// Who wrote what: the AI notes carry their provenance at the top.
function provenanceHtml(m) {
  const tpl = (S.templates.find((t) => t.id === m.template) || {}).name || (m.template === "auto" ? "Auto" : m.template);
  const by = m.enhanced_by ? `<b>${esc(m.enhanced_by)}</b>` : "an AI model";
  const stt = m.stt_model ? ` Transcribed on this Mac by ${m.stt_model}.` : "";
  return `<div class="src-strip" title="AI notes can contain mistakes. Check them against the transcript.${esc(stt)}">${ICON.spark}
    <span>Written by ${by} from your notes and the transcript${tpl === "Auto" ? "" : ` · ${esc(tpl)} format`}${m.enhanced_edited ? ` · <span class="edited">edited by you</span>` : ""}</span></div>`;
}

function regenConfirmHtml() {
  return `<div class="inline-confirm"><span>Regenerating replaces your edits to the AI notes.</span>
    <button class="btn small ember" id="regenYes">Replace my edits</button>
    <button class="btn small" id="regenNo">Keep them</button></div>`;
}

function choosePromptHtml() {
  return `<div class="callout choose">${ICON.spark}<div style="flex:1"><b>Choose who writes your notes</b>
    <span>Milkrice hasn't written AI notes for this meeting because you haven't picked an AI yet. Nothing has left this Mac.</span></div>
    <button class="btn small ember" id="chooseProv">Choose</button></div>`;
}

// A small sheet for picking the AI provider, used wherever AI notes are needed but none is chosen.
async function openProviderChooser(after) {
  if (!S.providers) await loadProviders(false).catch(() => {});
  const avail = (S.providers || []).filter((p) => p.available && p.signed_in);
  const dlg = dialog(`<form><h4>Who writes your notes?</h4>
    <p class="hint" style="margin:0 0 10px">Milkrice sends the transcript text, never audio, to the AI you pick. Ollama keeps it all on this Mac. Milkrice remembers your choice.</p>
    <div class="prov-pick">${avail.length ? avail.map((p) => `<button type="button" class="provider" data-pick="${p.id}"><span class="status ok"></span><div class="pi"><b>${esc(p.name)}</b><span>${esc(p.id === "ollama" ? "On this Mac. Nothing leaves it." : p.detail)}</span></div></button>`).join("")
      : `<p class="hint">No AI found. Sign in to Claude Code (<code>claude</code>) or Codex (<code>codex login</code>), or run Ollama, then open Settings → AI notes.</p>`}</div>
    <div class="actions"><button type="button" class="btn" data-cancel>Not now</button></div></form>`, () => {});
  dlg.style.width = "460px";
  dlg.querySelectorAll("[data-pick]").forEach((b) => (b.onclick = async () => {
    await saveSettings({ provider: b.dataset.pick });
    dlg.close(); dlg.remove();
    toast(`Notes will be written by ${S.providers.find((p) => p.id === b.dataset.pick)?.name}`);
    if (after) after();
  }));
}

function busyHtml(d) {
  return d.busy ? `<span class="busy"><span class="spinner"></span>${esc(d.busy)}…</span>` : "";
}

const GUESS_TIP = "Guessed from the conversation — click to confirm or rename";
const isGuessed = (key, m) => !!m?.speakers?.[key] && (m.speakers_guessed || []).includes(key);

function speakersHtml(d) {
  const keys = speakersInMeeting(d);
  if (!keys.length) return "";
  const hasUnknown = keys.some((k) => k.startsWith("s") && !d.meeting.speakers[k]);
  return keys.map((k) => {
    const g = isGuessed(k, d.meeting);
    return `<button class="chip ${g ? "guessed" : ""}" data-speaker="${esc(k)}" title="${g ? GUESS_TIP : "Rename"}"><i class="av" style="background:${speakerColor(k)};width:9px;height:9px"></i><span class="nm">${esc(speakerName(k, d.meeting))}</span>${g ? '<span class="q" aria-label="guessed">?</span>' : ""}</button>`;
  }).join("")
    + (hasUnknown ? `<button class="link" id="idSpeakers" style="margin-left:6px">${ICON.people} Identify speakers</button>` : "");
}

function audioBarHtml(d) {
  if (!(d.audio || []).length) return "";
  const playing = player.meeting === S.meetingId && !player.el.paused;
  return `<div class="audio-bar">
    <div class="ab-row">
      <button class="icon-btn" id="audioToggle" title="${playing ? "Pause" : "Play"}">${playing ? ICON.pause : ICON.play}</button>
      <span class="ab-time" id="audioPos">${fmtTs(player.meeting === S.meetingId ? playerMs() : 0)}</span>
      <div class="tl" id="tl" tabindex="0" role="slider" aria-label="Recording position" title="Click or drag to jump · ←/→ skip 5 s">
        <canvas id="tlCanvas"></canvas><div class="tl-tip" id="tlTip" hidden></div>
      </div>
      <span class="ab-time dim" id="audioLen">${fmtTs(timelineMs(d))}</span>
    </div>
    <div class="ab-row ab-foot"><span class="tl-legend" id="tlLegend">${timelineLegendHtml(d)}</span>
      <span style="flex:1"></span><button class="link" id="audioDelete">Delete recording</button></div></div>`;
}

// ------------------------------------------------------------------ playback timeline
// Loudness outlines of the kept recordings, per meeting (fetched once, drawn on every render).
const peaksCache = { id: null, n: 0, parts: null };

function loadPeaks(d) {
  const n = (d.audio || []).length;
  if (peaksCache.id === S.meetingId && peaksCache.n === n) return;
  const id = S.meetingId;
  Object.assign(peaksCache, { id, n, parts: null });
  invoke("meeting_audio_peaks", { id }).then((parts) => {
    if (peaksCache.id !== id) return;
    peaksCache.parts = parts || [];
    const len = $("#audioLen");
    if (len && S.detail) len.textContent = fmtTs(timelineMs(S.detail));
    drawTimeline();
  }).catch(() => {});
}

// Meeting length on the timeline: the transcript or the recordings, whichever runs longer.
function timelineMs(d) {
  let end = 0;
  for (const s of d.segments || []) end = Math.max(end, s.end_ms);
  for (const p of (peaksCache.id === S.meetingId && peaksCache.parts) || []) end = Math.max(end, p.offset_ms + p.mic.length * p.step_ms);
  return Math.max(end, 1000);
}

// Where the player is, on the meeting timeline.
function playerMs() {
  const parts = S.detail?.audio || [];
  const part = parts.find((p) => T.core.convertFileSrc(p.path) === player.el.dataset.src);
  return part ? part.offset_ms + player.el.currentTime * 1000 : 0;
}

function speakerAt(d, ms) {
  for (const s of d.segments || []) if (s.start_ms <= ms && ms < s.end_ms) return s.speaker;
  return null;
}

function timelineLegendHtml(d) {
  const keys = [...new Set((d.segments || []).map((s) => s.speaker))];
  return keys.map((k) => `<span class="tl-key"><i style="background:${speakerColor(k)}"></i>${esc(speakerName(k, d.meeting))}</span>`).join("");
}

// CSS colour (incl. var(--x)) -> canvas colour.
function resolveColor(c) {
  const el = document.createElement("i");
  el.style.color = c;
  el.style.display = "none";
  document.body.appendChild(el);
  const v = getComputedStyle(el).color;
  el.remove();
  return v;
}

function drawTimeline() {
  const d = S.detail, cv = $("#tlCanvas");
  if (!d || !cv) return;
  const box = cv.parentElement.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const w = Math.max(1, Math.round(box.width)), h = Math.max(1, Math.round(box.height));
  if (cv.width !== w * dpr || cv.height !== h * dpr) { cv.width = w * dpr; cv.height = h * dpr; }
  const g = cv.getContext("2d");
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.clearRect(0, 0, w, h);
  const total = timelineMs(d);
  const parts = (peaksCache.id === S.meetingId && peaksCache.parts) || [];
  const colors = {};
  const color = (k) => (colors[k] ??= resolveColor(k === null ? "var(--ink-3)" : speakerColor(k)));
  const laneH = 4, waveH = h - laneH - 3, mid = waveH / 2;
  const head = player.meeting === S.meetingId ? playerMs() : -1;
  const BAR = 3, GAP = 1;
  // Speaker lane: who is talking, straight from the transcript.
  for (const s of d.segments || []) {
    const x0 = (s.start_ms / total) * w, x1 = (s.end_ms / total) * w;
    g.fillStyle = color(s.speaker);
    g.globalAlpha = 0.9;
    g.fillRect(x0, h - laneH, Math.max(1, x1 - x0 - 0.5), laneH);
  }
  // Loudness bars, coloured by the speaker at that moment (or by side when the transcript has
  // nothing there), dimmed ahead of the playhead.
  for (let x = 0; x < w; x += BAR + GAP) {
    const t0 = (x / w) * total, t1 = ((x + BAR) / w) * total;
    let mic = 0, call = 0;
    for (const p of parts) {
      if (!p.step_ms) continue;
      const i0 = Math.max(0, Math.floor((t0 - p.offset_ms) / p.step_ms));
      const i1 = Math.min(p.mic.length, Math.ceil((t1 - p.offset_ms) / p.step_ms));
      for (let i = i0; i < i1; i++) { mic = Math.max(mic, p.mic[i]); call = Math.max(call, p.call[i]); }
    }
    const lvl = Math.max(mic, call) / 255;
    if (lvl <= 0.02) continue;
    const who = speakerAt(d, (t0 + t1) / 2) ?? (mic > call ? "me" : null);
    const bh = Math.max(2, lvl * lvl * waveH);
    g.globalAlpha = head >= 0 && t0 <= head ? 1 : 0.42;
    g.fillStyle = color(who);
    g.fillRect(x, mid - bh / 2, BAR, bh);
  }
  if (head >= 0) {
    const x = (head / total) * w;
    g.globalAlpha = 1;
    g.fillStyle = resolveColor("var(--ember)");
    g.fillRect(Math.min(w - 2, x), 0, 2, h);
  }
  g.globalAlpha = 1;
  cv.parentElement.setAttribute("aria-valuemax", String(Math.round(total / 1000)));
  cv.parentElement.setAttribute("aria-valuenow", String(Math.round(Math.max(0, head) / 1000)));
}

function bindTimeline() {
  const tl = $("#tl"), tip = $("#tlTip");
  if (!tl) return;
  loadPeaks(S.detail);
  const msAt = (e) => {
    const r = tl.getBoundingClientRect();
    return Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)) * timelineMs(S.detail);
  };
  let dragging = false;
  tl.onpointerdown = (e) => { dragging = true; tl.setPointerCapture(e.pointerId); seekTo(msAt(e)); };
  tl.onpointermove = (e) => {
    const ms = msAt(e);
    if (dragging) seekTo(ms);
    const who = speakerAt(S.detail, ms);
    tip.hidden = false;
    tip.textContent = `${fmtTs(ms)}${who ? ` · ${speakerName(who, S.detail.meeting)}` : ""}`;
    const r = tl.getBoundingClientRect();
    tip.style.left = `${Math.min(r.width - tip.offsetWidth, Math.max(0, e.clientX - r.left - tip.offsetWidth / 2))}px`;
  };
  tl.onpointerup = tl.onpointercancel = () => (dragging = false);
  tl.onpointerleave = () => { if (!dragging) tip.hidden = true; };
  tl.onkeydown = (e) => {
    const step = e.shiftKey ? 30000 : 5000;
    if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
      e.preventDefault();
      const now = player.meeting === S.meetingId ? playerMs() : 0;
      seekTo(Math.max(0, now + (e.key === "ArrowRight" ? step : -step)));
    } else if (e.key === " ") { e.preventDefault(); $("#audioToggle")?.click(); }
  };
  drawTimeline();
}

// Jump to a point on the timeline; keeps playing if it was, otherwise just moves the playhead.
function seekTo(ms) {
  const wasPlaying = player.meeting === S.meetingId && !player.el.paused;
  playAt(ms, { autoplay: wasPlaying, lead: 0 });
  drawTimeline();
}

window.addEventListener("resize", () => drawTimeline());

// One audio element for the whole app, outside the re-rendered DOM: a re-render used to make a
// fresh <audio> while the old, detached one kept playing, so two recordings could play at once.
const player = { el: new Audio(), meeting: null, token: 0 };
player.el.preload = "metadata";
player.el.onplay = player.el.onpause = () => {
  const tg = $("#audioToggle");
  if (tg && player.meeting === S.meetingId) { tg.innerHTML = player.el.paused ? ICON.play : ICON.pause; tg.title = player.el.paused ? "Play" : "Pause"; }
};
player.el.ontimeupdate = player.el.onseeked = () => {
  if (player.meeting !== S.meetingId) return;
  const el = $("#audioPos");
  if (el) el.textContent = fmtTs(playerMs());
  drawTimeline();
};
// A meeting resumed later has several recordings: carry on into the next one.
player.el.onended = () => {
  const parts = S.detail?.audio || [];
  if (player.meeting !== S.meetingId) return;
  const i = parts.findIndex((p) => T.core.convertFileSrc(p.path) === player.el.dataset.src);
  if (i >= 0 && i + 1 < parts.length) playAt(parts[i + 1].offset_ms, { lead: 0 });
};
// play() rejects with AbortError whenever a newer play/pause/seek interrupts it: not an error.
const playQuietly = () => player.el.play().catch((e) => { if (e?.name !== "AbortError") fail(e); });

function stopPlayer() {
  player.token++;
  player.el.pause();
  if (player.el.dataset.src) { delete player.el.dataset.src; player.el.removeAttribute("src"); player.el.load(); }
  player.meeting = null;
}

// Play the kept recording from a transcript offset (recordings may be split by resumes).
// Only the latest request wins, however fast the clicks come.
function playAt(ms, { autoplay = true, lead = 500 } = {}) {
  const parts = S.detail?.audio || [];
  if (!parts.length) return;
  let part = parts[0];
  for (const p of parts) if (p.offset_ms <= ms) part = p;
  const a = player.el;
  const token = ++player.token;
  player.meeting = S.meetingId;
  const seek = () => {
    if (token !== player.token) return;
    a.currentTime = Math.max(0, (ms - part.offset_ms - lead) / 1000);
    if (autoplay) playQuietly();
    else drawTimeline();
  };
  const src = T.core.convertFileSrc(part.path);
  if (a.dataset.src !== src) {
    a.pause();
    a.dataset.src = src;
    a.src = src;
    a.addEventListener("loadedmetadata", seek, { once: true });
  } else if (a.readyState >= 1) seek();
  else a.addEventListener("loadedmetadata", seek, { once: true });
}

function bindAudio() {
  const tg = $("#audioToggle");
  // Leaving the meeting (or its recording going away) stops playback.
  if (!tg) { if (player.meeting) stopPlayer(); return; }
  if (player.meeting && player.meeting !== S.meetingId) stopPlayer();
  tg.onclick = () => {
    if (player.meeting !== S.meetingId || !player.el.dataset.src) playAt(S.detail.segments[0]?.start_ms || 0);
    else if (player.el.paused) { player.token++; playQuietly(); }
    else { player.token++; player.el.pause(); }
  };
  document.querySelectorAll("[data-play]").forEach((b) => (b.onclick = () => playAt(Number(b.dataset.play))));
  bindTimeline();
  $("#audioDelete").onclick = async () => {
    if (!confirm("Delete this meeting's recording? The transcript and notes stay.")) return;
    stopPlayer();
    await invoke("delete_meeting_audio", { id: S.meetingId }).catch(fail);
    S.detail.audio = []; render();
  };
}

function transcriptHtml(d) {
  const isRec = S.rec.recording && S.rec.meeting_id === d.meeting.id;
  if (!d.segments.length) {
    return isRec
      ? `<div class="live-cue"><img class="illus" src="img/listening.jpg" alt="">Listening… lines appear here as people speak.</div>`
      : `<p class="hint">No transcript yet.</p>`;
  }
  let h = audioBarHtml(d), prev = null;
  const playable = (d.audio || []).length > 0;
  for (const s of d.segments) {
    const same = prev && prev.speaker === s.speaker && s.start_ms - prev.end_ms < 4000;
    h += `<div class="line ${s.speaker === "me" ? "me" : ""}"><div class="ts">${same ? "" : playable ? `<button class="ts-play" data-play="${s.start_ms}" title="Play from here">${fmtTs(s.start_ms)}</button>` : fmtTs(s.start_ms)}</div><div>
      ${same ? "" : `<button class="who ${isGuessed(s.speaker, d.meeting) ? "guessed" : ""}" data-speaker="${esc(s.speaker)}" ${isGuessed(s.speaker, d.meeting) ? `title="${GUESS_TIP}"` : ""}><i style="background:${speakerColor(s.speaker)}"></i><span class="nm">${esc(speakerName(s.speaker, d.meeting))}</span>${isGuessed(s.speaker, d.meeting) ? '<span class="q">?</span>' : ""}</button>`}
      <div class="tx" data-seg="${s.id}">${esc(s.text)}</div></div></div>`;
    prev = s;
  }
  if (isRec) h += `<div class="live-cue"><span class="rec-dot"></span>Listening…</div>`;
  return h;
}

function chatHtml(d) {
  const sugg = ["What were the decisions?", "List action items with owners", "Draft a follow-up email", "What did they push back on?"];
  let h = `<div class="chat">`;
  for (const c of d.chats) h += `<div class="msg ${c.role}">${c.role === "assistant" ? `<div class="md">${md(c.content)}</div>` : esc(c.content)}</div>`;
  if (S.asking) h += `<div class="msg assistant"><span class="busy"><span class="spinner"></span>Thinking…</span></div>`;
  if (!d.chats.length && !S.asking) h += `<p class="hint">Ask anything about this meeting. Answers come from the transcript, using your AI of choice.</p>
    <div class="suggest">${sugg.map((q) => `<button class="btn small" data-q="${esc(q)}">${esc(q)}</button>`).join("")}</div>`;
  return h + `</div>`;
}

function autoGrow(el) { el.style.height = "auto"; el.style.height = Math.max(320, el.scrollHeight) + "px"; }

let saveTimer = null;
function bindNote(v) {
  const d = S.detail, m = d.meeting;
  v.querySelectorAll("[data-folder-pick]").forEach((b) => (b.onclick = () => openFolderPicker(b)));
  const sa = v.querySelector("[data-sug-add]");
  if (sa) sa.onclick = () => fileNote(m.id, d.folder_suggestion.id, true).then(() => toast(`Added to ${d.folder_suggestion.name}`));
  const sn = v.querySelector("[data-sug-no]");
  if (sn) sn.onclick = async () => { await invoke("dismiss_folder_suggestion", { meetingId: m.id }).catch(fail); await refreshDetail(); };
  const title = $("#titleInput");
  title.oninput = () => { clearTimeout(title._t); title._t = setTimeout(() => invoke("update_meeting", { id: m.id, field: "title", value: title.value }).then(loadMeetings), 400); m.title = title.value; };
  const ed = $("#notesEditor");
  if (ed) ed.oninput = () => {
    autoGrow(ed);
    m.notes = ed.value;
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => invoke("update_meeting", { id: m.id, field: "notes", value: ed.value }).catch(fail), 350);
  };
  v.querySelectorAll("[data-tab]").forEach((b) => (b.onclick = () => { S.tab = b.dataset.tab; render(); if (S.tab === "ask") setTimeout(() => $("#askInput")?.focus(), 0); }));
  const tsel = $("#tplSelect");
  if (tsel) tsel.onchange = (e) => { m.template = e.target.value; invoke("update_meeting", { id: m.id, field: "template", value: e.target.value }); };
  const tm = $("#toggleMine");
  if (tm) tm.onclick = () => { S.showMine = !S.showMine; render(); };
  const rg = $("#regen");
  if (rg) rg.onclick = () => {
    if (S.detail?.busy) return;
    if (!S.settings?.provider) return openProviderChooser(enhanceNow);
    // Regenerating over the user's own edits needs an in-page yes (window.confirm is unreliable here).
    if (m.enhanced && m.enhanced_edited && !S.showMine) { S.confirmRegen = m.id; render(); return; }
    enhanceNow();
  };
  const ry = $("#regenYes");
  if (ry) ry.onclick = () => { S.confirmRegen = null; enhanceNow(); };
  const rn = $("#regenNo");
  if (rn) rn.onclick = () => { S.confirmRegen = null; render(); };
  const cp2 = $("#chooseProv");
  if (cp2) cp2.onclick = () => openProviderChooser(() => render());
  // Editing the AI notes: a plain Markdown editor, saved as the user's version.
  const ae = $("#aiEdit");
  if (ae) ae.onclick = () => { S.editingAi = m.id; S.confirmRegen = null; render(); const t = $("#aiEditor"); if (t) { autoGrow(t); t.focus(); } };
  const aed = $("#aiEditor");
  if (aed) { autoGrow(aed); aed.oninput = () => autoGrow(aed); aed.onkeydown = (e) => { if (e.metaKey && e.key === "s") { e.preventDefault(); $("#aiSave").click(); } if (e.key === "Escape") $("#aiCancel").click(); }; }
  const as = $("#aiSave");
  if (as) as.onclick = async () => {
    const val = $("#aiEditor").value;
    try {
      await invoke("update_meeting", { id: m.id, field: "enhanced", value: val });
      m.enhanced = val; m.enhanced_edited = true; S.editingAi = null;
      render(); toast("AI notes saved as your edit");
    } catch (e) { fail(e); }
  };
  const ac = $("#aiCancel");
  if (ac) ac.onclick = () => { S.editingAi = null; render(); };
  // Live action items: tick the box, flip "- [ ]" ↔ "- [x]" in the Markdown and save.
  v.querySelectorAll("#aiNotes input[data-line]").forEach((cb) => (cb.onchange = async () => {
    const lines = m.enhanced.split("\n");
    const i = Number(cb.dataset.line);
    lines[i] = lines[i].replace(TASK_RE, (_, a, _x, b, rest) => `${a}${cb.checked ? "x" : " "}${b}${rest}`);
    m.enhanced = lines.join("\n");
    m.enhanced_edited = true;
    cb.closest("li").classList.toggle("done", cb.checked);
    try { await invoke("update_meeting", { id: m.id, field: "enhanced", value: m.enhanced }); } catch (e) { fail(e); }
    const strip = $(".src-strip span");
    if (strip && !strip.querySelector(".edited")) strip.insertAdjacentHTML("beforeend", ` · <span class="edited">edited by you</span>`);
  }));
  const cp = $("#copyMd");
  if (cp) cp.onclick = async () => { await navigator.clipboard.writeText(`# ${m.title || "Meeting notes"}\n\n${m.enhanced.trim()}\n`); toast("Notes copied (transcript not included)"); };
  const jl = $("#joinLink");
  if (jl) jl.onclick = (e) => { e.preventDefault(); T.opener?.openUrl(m.meeting_url).catch(() => window.open(m.meeting_url)); };
  $("#moreBtn").onclick = moreMenu;
  v.querySelectorAll("[data-speaker]").forEach((b) => (b.onclick = () => renameSpeaker(b.dataset.speaker)));
  const ids = $("#idSpeakers");
  if (ids) ids.onclick = async () => {
    toast("Working out who's who…");
    try { await invoke("identify_speakers", { id: m.id }); await refreshDetail(); toast("Speakers named"); } catch (e) { fail(e); }
  };
  v.querySelectorAll("[data-seg]").forEach((el) => {
    el.ondblclick = () => {
      el.contentEditable = "true"; el.focus();
      el.onblur = () => { el.contentEditable = "false"; invoke("update_segment", { id: Number(el.dataset.seg), text: el.textContent.trim() }); };
    };
  });
  v.querySelectorAll("[data-q]").forEach((b) => (b.onclick = () => askQuestion(b.dataset.q)));
}

async function enhanceNow() {
  const id = S.meetingId;
  S.detail.busy = "Writing notes"; render(true);
  try {
    await invoke("enhance", { id });
    S.showMine = false;
    await refreshDetail();
    toast("Notes enhanced");
  } catch (e) { fail(e); await refreshDetail(); }
}

async function askQuestion(q) {
  q = (q || "").trim();
  if (!q || !S.meetingId) return;
  S.tab = "ask"; S.asking = true;
  S.detail.chats.push({ role: "user", content: q });
  render();
  try { await invoke("ask", { id: S.meetingId, question: q }); } catch (e) { fail(e); }
  S.asking = false;
  await refreshDetail();
  render();
}

function dialog(html, onOk) {
  const dlg = document.createElement("dialog");
  dlg.innerHTML = html;
  document.body.appendChild(dlg);
  dlg.showModal();
  const close = () => { dlg.close(); dlg.remove(); };
  dlg.querySelector("[data-cancel]")?.addEventListener("click", close);
  dlg.querySelector("form")?.addEventListener("submit", async (e) => { e.preventDefault(); await onOk(dlg); close(); });
  dlg.addEventListener("cancel", close);
  return dlg;
}

function renameSpeaker(key) {
  const m = S.detail.meeting;
  const names = [...new Set([...m.attendees.map((a) => a.name), ...Object.values(m.speakers)])].filter(Boolean);
  const guessed = isGuessed(key, m);
  const dlg = dialog(`<form><h4>${guessed ? `Is this ${esc(m.speakers[key])}?` : "Who is this?"}</h4>
    ${guessed ? `<p class="hint" style="margin:0 0 10px">Milkrice guessed this name from the conversation. Confirm it, or type the right name.</p>` : ""}
    <input class="field" name="n" value="${esc(m.speakers[key] || (key === "me" ? S.settings?.user_name || "" : ""))}" placeholder="Name" autofocus />
    ${names.length ? `<div class="sugs">${names.map((n) => `<button type="button" class="btn small" data-n="${esc(n)}">${esc(n)}</button>`).join("")}</div>` : ""}
    <p class="hint">Giving two speakers the same name merges them.</p>
    <div class="actions"><button type="button" class="btn" data-cancel>Cancel</button><button class="btn primary">${guessed ? "Confirm" : "Save"}</button></div></form>`,
    async (dl) => {
      await invoke("rename_speaker", { id: m.id, key, name: dl.querySelector("[name=n]").value }).catch(fail);
      await refreshDetail();
      render();
    });
  dlg.querySelectorAll("[data-n]").forEach((b) => (b.onclick = () => { dlg.querySelector("[name=n]").value = b.dataset.n; }));
}

function moreMenu() {
  const m = S.detail.meeting;
  dialog(`<form><h4>${esc(m.title || "This note")}</h4>
    <div style="display:grid;gap:8px">
      ${S.settings.obsidian ? `<button type="button" class="btn" id="mxObs">Open in Obsidian</button>` : ""}
      <button type="button" class="btn" id="mxSave">Export Markdown to Documents/Milkrice</button>
      <button type="button" class="btn" id="mxCopyAll">Copy everything (Markdown)</button>
      <button type="button" class="btn" id="mxCopy">Copy transcript</button>
      <button type="button" class="btn danger" id="mxDel">Delete note</button>
    </div>
    <div class="actions"><button type="button" class="btn" data-cancel>Close</button></div></form>`, () => {});
  const mo = $("#mxObs");
  if (mo) mo.onclick = () => invoke("open_in_obsidian", { id: m.id }).catch(fail);
  $("#mxSave").onclick = async () => { try { const p = await invoke("export_markdown", { id: m.id, save: true }); toast(`Saved to ${p}`, 4000); } catch (e) { fail(e); } };
  $("#mxCopy").onclick = () => copyText(noteText("transcript"), "Transcript");
  $("#mxCopyAll").onclick = async () => { try { copyText(await invoke("export_markdown", { id: m.id, save: false }), "Note"); } catch (e) { fail(e); } };
  $("#mxDel").onclick = async () => {
    if (!confirm("Delete this note and its transcript? This can't be undone.")) return;
    await invoke("delete_meeting", { id: m.id });
    document.querySelector("dialog")?.remove();
    S.view = "home"; S.meetingId = null; S.detail = null;
    await loadMeetings(); render();
  };
}

// ------------------------------------------------------------------ copy menu
// Right-click anywhere on a note: copy the selection, or one part of the note, or all of it.
// Text fields keep the system menu (cut / copy / paste / spelling) unless text is outside them.
function noteText(kind) {
  const d = S.detail, m = d.meeting;
  if (kind === "mine") return m.notes.trim();
  if (kind === "ai") return m.enhanced.trim();
  if (kind === "transcript") return d.segments.map((s) => `[${fmtTs(s.start_ms)}] ${speakerName(s.speaker, m)}: ${s.text}`).join("\n");
  return "";
}
async function copyText(t, what) {
  try { await navigator.clipboard.writeText(t); toast(`${what} copied`); } catch (e) { fail(e); }
}
function closeCtx() { $("#ctxMenu")?.remove(); document.removeEventListener("mousedown", ctxOutside, true); }
function ctxOutside(e) { if (!e.target.closest("#ctxMenu")) closeCtx(); }
function openCtx(e) {
  closeCtx();
  if (S.view !== "note" || !S.detail) return;
  const field = e.target.closest("input, textarea");
  const sel = field ? field.value.slice(field.selectionStart, field.selectionEnd) : String(window.getSelection() || "");
  if (field) return; // native menu: cut, copy, paste, look up, spelling
  e.preventDefault();
  const d = S.detail, m = d.meeting;
  const items = [];
  if (sel.trim()) items.push(["sel", "Copy", "⌘C"]);
  if (items.length) items.push(["-"]);
  if (m.notes.trim()) items.push(["mine", "Copy my notes"]);
  if (m.enhanced.trim()) items.push(["ai", "Copy AI notes"]);
  if (d.segments.length) items.push(["transcript", "Copy transcript"]);
  items.push(["all", "Copy everything (Markdown)"]);
  const pop = document.createElement("div");
  pop.id = "ctxMenu";
  pop.className = "menu-pop ctx-menu";
  pop.innerHTML = items.map(([k, label, kbd]) => k === "-" ? "<hr>" : `<button data-k="${k}">${label}${kbd ? `<span class="kbd">${kbd}</span>` : ""}</button>`).join("");
  document.body.appendChild(pop);
  const w = pop.offsetWidth, h = pop.offsetHeight;
  pop.style.left = `${Math.min(e.clientX, innerWidth - w - 8)}px`;
  pop.style.top = `${Math.min(e.clientY, innerHeight - h - 8)}px`;
  pop.querySelectorAll("[data-k]").forEach((b) => (b.onclick = async () => {
    closeCtx();
    const k = b.dataset.k;
    if (k === "sel") return copyText(sel, "Selection");
    if (k === "all") {
      try { return copyText(await invoke("export_markdown", { id: m.id, save: false }), "Note"); } catch (err) { return fail(err); }
    }
    copyText(noteText(k), { mine: "Your notes", ai: "AI notes", transcript: "Transcript" }[k]);
  }));
  setTimeout(() => document.addEventListener("mousedown", ctxOutside, true), 0);
}
document.addEventListener("contextmenu", openCtx);
document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeCtx(); });

// ------------------------------------------------------------------ dock
function dockHtml() {
  const r = S.rec;
  const here = S.view === "note" && S.detail;
  const recHere = r.recording && (!here || r.meeting_id === S.meetingId);
  const bars = (n) => Array.from({ length: n }, () => "<b></b>").join("");
  if (r.recording) {
    return `<div class="dock recording">
      <span class="rec-live" title="${r.auto ? "Recording automatically (auto-record)" : "Recording"}"><span class="rec-dot"></span>REC${r.auto ? `<small>auto</small>` : ""}</span>
      <span class="timer" id="dockTimer">${fmtDur(r.elapsed_ms || 0)}</span>
      <button class="rec" id="dockStop" title="Stop recording">${ICON.stop}<span>Stop</span></button>
      ${deviceChipHtml(true)}
      ${recHere && here ? `<label class="ask">${ICON.spark}<input id="askInput" placeholder="Ask about this meeting…" /></label>
      <button class="icon-btn ${S.tab === "transcript" ? "on" : ""}" id="dockTranscript" title="Transcript">${ICON.transcript}</button>`
      : `<span style="flex:1"></span><button class="btn small" id="dockGoto">Back to meeting</button>`}
    </div>`;
  }
  if (here) {
    return `<div class="dock">
      <button class="rec idle" id="dockStart" title="Start recording"><span class="dot"></span><span>${S.detail.segments.length ? "Resume" : "Record"}</span></button>
      ${deviceChipHtml(false)}
      <label class="ask">${ICON.spark}<input id="askInput" placeholder="Ask about this meeting…" /></label>
      <button class="icon-btn ${S.tab === "transcript" ? "on" : ""}" id="dockTranscript" title="Transcript">${ICON.transcript}</button>
    </div>`;
  }
  return "";
}

// ------------------------------------------------------------------ audio devices
S.devices = null;
S.testLevel = 0;
S.testSilent = false;
async function loadDevices() {
  try { S.devices = await invoke("audio_devices"); } catch (_) {}
  const label = $("#chipLabel");
  if (label) label.textContent = shortDevice(S.devices?.active);
  return S.devices;
}
function shortDevice(name) {
  if (!name) return "Microphone";
  const n = name.replace(/\s*Microphone$/i, " mic").replace(/^MacBook (Air|Pro)/, "MacBook");
  return n.length > 22 ? n.slice(0, 21) + "…" : n;
}
const BARS = (n, id, cls) => `<span class="meter ${cls}" ${id ? `id="${id}"` : ""}>${Array.from({ length: n }, () => "<b></b>").join("")}</span>`;
function deviceChipHtml(recording) {
  const sys = S.rec.system_audio;
  return `<button class="dev-chip" id="devChip" title="Choose microphone">
      ${BARS(4, "mMic", "me")}<span id="chipLabel">${esc(shortDevice(S.devices?.active))}</span>
      <svg viewBox="0 0 16 16" style="width:10px;height:10px"><path d="m4 6 4 4 4-4"/></svg>
    </button>${recording ? `<span class="dev-them ${sys ? "" : "off"}" title="${sys ? "Hearing the other participants (system audio)" : "Not hearing other participants"}">${BARS(3, "mSys", "them")}<span>${sys ? "call" : "no call audio"}</span></span>` : ""}`;
}

function closeDevicePicker() {
  const p = $("#devPop");
  if (p) p.remove();
  invoke("mic_test", { device: null, on: false }).catch(() => {});
  document.removeEventListener("mousedown", devOutside, true);
}
function devOutside(e) {
  if (!e.target.closest("#devPop") && !e.target.closest("#devChip")) closeDevicePicker();
}
async function openDevicePicker() {
  if ($("#devPop")) return closeDevicePicker();
  const d = await loadDevices();
  if (!d) return;
  const pop = document.createElement("div");
  pop.id = "devPop";
  pop.className = "dev-pop";
  const row = (value, title, sub) => {
    const on = (S.devices.selected || "") === value;
    return `<button class="dev-row ${on ? "on" : ""}" data-dev="${esc(value)}"><span class="radio"></span>
      <span class="dev-name"><b>${esc(title)}</b>${sub ? `<span>${esc(sub)}</span>` : ""}</span>
      ${on ? BARS(5, "testBars", "me") : ""}</button>`;
  };
  pop.innerHTML = `<div class="dev-h">Microphone</div>
    ${row("", "System default", d.default_input || "Follows macOS")}
    ${d.inputs.filter((n) => n).map((n) => row(n, n, n === d.default_input ? "Current macOS default" : "")).join("")}
    <button class="link dev-find" id="devFind">Which mic can hear me?</button>
    <div class="dev-hint" id="devHint">${S.rec.recording ? "Switching keeps your transcript — Milkrice picks up right where it left off." : "Speak — the bars should move."}</div>
    <div class="dev-h" style="margin-top:6px">Other participants</div>
    <div class="dev-row static"><span class="status ${S.perms?.system_audio === "granted" && d.capture_system ? "ok" : ""}"></span>
      <span class="dev-name"><b>All system audio</b><span>${d.capture_system ? `Whatever plays through ${esc(d.default_output || "your speakers")} — any app, any output.` : "Off in Settings → Audio."}</span></span>
      ${S.perms?.system_audio === "granted" ? "" : `<button class="btn small" id="devSysFix">Allow</button>`}</div>`;
  $("#main").appendChild(pop);
  pop.querySelectorAll("[data-dev]").forEach((b) => (b.onclick = async () => {
    const dev = b.dataset.dev || null;
    try {
      await invoke("set_mic", { device: dev });
      S.settings.mic_device = dev;
      toast(`Using ${dev || "the system default microphone"}`);
    } catch (e) { fail(e); }
    closeDevicePicker();
    await loadDevices();
    openDevicePicker();
  }));
  const fd = $("#devFind");
  if (fd) fd.onclick = async () => {
    fd.textContent = "Say something…";
    try {
      const probes = await invoke("probe_mics");
      const best = probes.filter((p) => !p.error && !p.digital_silence).sort((a, b) => b.peak - a.peak)[0];
      for (const p of probes) {
        const rowEl = [...pop.querySelectorAll("[data-dev]")].find((r) => r.dataset.dev === p.name);
        if (!rowEl) continue;
        const sub = rowEl.querySelector(".dev-name span") || rowEl.querySelector(".dev-name").appendChild(document.createElement("span"));
        sub.textContent = p.error ? "Unavailable" : p.digital_silence ? "No sound (muted or disconnected)" : p.peak > 0.03 ? (p === best ? "Hears you best" : "Hears you") : "Too quiet";
        if (p === best && p.peak > 0.03) rowEl.classList.add("best");
      }
      fd.textContent = best && best.peak > 0.03 ? `Best: ${best.name}` : "Nothing heard — check mute switches";
      if (!S.rec.recording) invoke("mic_test", { device: S.devices.selected || null, on: true }).catch(() => {});
    } catch (e) { fd.textContent = String(e); }
  };
  const fx = $("#devSysFix");
  if (fx) fx.onclick = () => { closeDevicePicker(); openPermsSheet("Hear everyone on the call"); };
  setTimeout(() => document.addEventListener("mousedown", devOutside, true), 0);
  // Live preview while choosing (recording already streams real levels).
  if (!S.rec.recording) invoke("mic_test", { device: S.devices.selected || null, on: true }).catch((e) => { const h = $("#devHint"); if (h) h.textContent = String(e); });
}

function bindDock() {
  const dc = $("#devChip");
  if (dc) dc.onclick = openDevicePicker;
  const st = $("#dockStart");
  if (st) st.onclick = async () => {
    if (S.perms && S.perms.microphone !== "granted") {
      await grantPerm("microphone");
      if (S.perms.microphone !== "granted") { openPermsSheet("Milkrice needs your microphone"); return; }
    }
    st.disabled = true;
    toast("Starting…", 1200);
    try { await invoke("start_recording", { id: S.meetingId }); } catch (e) { fail(e); st.disabled = false; }
  };
  const sp = $("#dockStop");
  if (sp) sp.onclick = () => invoke("stop_recording").catch(fail);
  const tr = $("#dockTranscript");
  if (tr) tr.onclick = () => { S.tab = S.tab === "transcript" ? "notes" : "transcript"; render(); };
  const go = $("#dockGoto");
  if (go) go.onclick = () => openMeeting(S.rec.meeting_id).catch(fail);
  const ai = $("#askInput");
  if (ai) ai.onkeydown = (e) => { if (e.key === "Enter") { const q = ai.value; ai.value = ""; askQuestion(q); } };
}

function paintLevels() {
  const paint = (el, v) => {
    if (!el) return;
    const bars = el.children, n = bars.length;
    const lvl = Math.min(1, Math.sqrt(v) * 1.6);
    for (let i = 0; i < n; i++) {
      const shape = 1 - Math.abs(i - (n - 1) / 2) / n;
      bars[i].style.height = `${3 + Math.round(lvl * shape * 17 * (0.75 + Math.random() * 0.5))}px`;
    }
  };
  const live = S.rec.recording;
  paint($("#mMic"), live ? S.levels.mic : $("#devPop") ? S.testLevel : 0);
  paint($("#mSys"), live ? S.levels.system : 0);
  paint($("#testBars"), live ? S.levels.mic : S.testLevel);
  paint($("#obtBars"), live ? S.levels.mic : 0);
  const hint = $("#devHint");
  if (hint && !live && S.testSilent) hint.innerHTML = `macOS is giving Milkrice silence from this mic. <button class="link" id="devMicFix">Fix</button>`, ($("#devMicFix").onclick = () => { closeDevicePicker(); openPermsSheet("Milkrice needs your microphone"); });
}

// ------------------------------------------------------------------ settings
async function loadModels() { S.models = await invoke("list_models"); }
async function loadProviders(refresh) {
  const r = await invoke("providers", { refresh: !!refresh });
  S.providers = r.providers; S.templates = r.templates;
}

const LANGS = [["all", "All languages"], ["en", "English"], ["es", "Spanish"], ["fr", "French"], ["de", "German"], ["it", "Italian"], ["pt", "Portuguese"], ["nl", "Dutch"], ["zh", "Chinese"], ["ja", "Japanese"], ["ko", "Korean"], ["ru", "Russian"], ["ar", "Arabic"], ["hi", "Hindi"], ["si", "Sinhala"], ["ta", "Tamil"]];

function modelCard(m) {
  const active = S.models.active === m.id;
  const p = S.progress[m.id];
  const dl = S.models.downloading.includes(m.id) || (p && !p.finished);
  const pct = p && p.total ? Math.round((p.done / p.total) * 100) : 0;
  let right;
  if (m.downloaded) right = active ? `<span>${ICON.disk}${m.size_mb >= 1000 ? (m.size_mb / 1024).toFixed(1) + " GB" : m.size_mb + " MB"}</span><button class="link" data-del="${m.id}">${ICON.trash} Delete</button>`
    : `<button class="btn small" data-use="${m.id}">Use</button><button class="link" data-del="${m.id}">${ICON.trash}</button>`;
  else if (dl) right = `<div class="progress"><i style="width:${pct}%"></i></div><span>${pct}%</span><button class="link" data-cancel="${m.id}">Cancel</button>`;
  else right = `<button class="link" data-dl="${m.id}" style="color:var(--ink-2)">${ICON.down} ${m.size_mb >= 1000 ? (m.size_mb / 1024).toFixed(1) + " GB" : m.size_mb + " MB"}</button>`;
  return `<div class="model ${active ? "active" : ""}" data-model="${m.id}">
    <div class="mtop"><div><div class="mname">${esc(m.name)}${active ? `<span class="badge solid">${ICON.check.replace("<svg", '<svg style="width:11px;height:11px;stroke-width:2.4"')} Active</span>` : m.recommended ? '<span class="badge">Recommended</span>' : ""}</div>
      <div class="mtag">${esc(m.tagline)}</div></div>
      <div class="bars"><span>accuracy</span><span class="bar"><i style="width:${m.accuracy * 100}%"></i></span><span>speed</span><span class="bar"><i style="width:${m.speed * 100}%"></i></span></div></div>
    <div class="mfoot"><span>${ICON.globe}${esc(m.language_label)}</span>${m.live ? `<span>${ICON.live}Live</span>` : ""}${m.translate ? `<span>${ICON.translate}Translate</span>` : ""}<span class="grow"></span>${right}</div>
  </div>`;
}

function modelsHtml() {
  if (!S.models) return `<p class="hint">Loading models…</p>`;
  const f = S.langFilter;
  const list = S.models.models.filter((m) => f === "all" || m.languages.includes(f));
  const mine = list.filter((m) => m.downloaded), rest = list.filter((m) => !m.downloaded);
  const rec = rest.filter((m) => m.recommended), more = rest.filter((m) => !m.recommended);
  // A filtered search for a language only the extra models cover opens the disclosure.
  const openMore = S.moreModels || (f !== "all" && !rec.length && more.length);
  return `<div class="models-toolbar"><h3>Downloaded models</h3>
      <select class="field" id="langFilter">${LANGS.map(([k, n]) => `<option value="${k}" ${k === f ? "selected" : ""}>${n}</option>`).join("")}</select></div>
    ${mine.length ? mine.map(modelCard).join("") : `<p class="hint">None yet. Parakeet v2 is the best start for English; Parakeet v3 covers 25 European languages.</p>`}
    ${rec.length ? `<div class="models-toolbar"><h3>Recommended</h3></div>${rec.map(modelCard).join("")}` : ""}
    ${more.length ? `<details class="more-models" id="moreModels" ${openMore ? "open" : ""}><summary>More models <span class="hint">${more.length} for other languages, translation or older Macs</span></summary>${more.map(modelCard).join("")}</details>` : ""}
    <p class="hint">Models are open weights and run entirely on this Mac. Speaker recognition uses a small extra model (${S.models.speaker_model ? "installed" : "downloads with your first model"}).</p>`;
}

function seg(name, value, opts) {
  return `<div class="seg" data-seg-name="${name}">${opts.map(([v, l]) => `<button class="${v === value ? "on" : ""}" data-v="${v}">${l}</button>`).join("")}</div>`;
}
const sw = (name, on) => `<button class="switch ${on ? "on" : ""}" data-switch="${name}" role="switch" aria-checked="${on}"></button>`;

function renderSettings(v) {
  const s = S.settings;
  let h = `<div class="page settings"><h1>Settings</h1><p class="desc">Audio and transcripts stay on this Mac. Only when Milkrice writes notes does the text go to the AI you pick below — choose Ollama to keep even that on-device.</p>

  <h2>Permissions</h2><p class="desc">macOS asks once for each. Milkrice takes you straight to the right switch.</p>
  <div class="perms-slot">${permsHtml()}</div>

  <h2>Transcription</h2><p class="desc">Pick an open-weights speech model. Bigger is more accurate; smaller is faster.</p>
  <div class="row"><div class="rl"><b>Spoken language</b><span>"Auto" lets multilingual models detect it.</span></div>
    <select class="field" id="langSel">${[["auto", "Auto-detect"], ...LANGS.slice(1)].map(([k, n]) => `<option value="${k}" ${k === s.language ? "selected" : ""}>${n}</option>`).join("")}</select></div>
  <div id="modelsBox">${modelsHtml()}</div>

  <h2>AI notes</h2><p class="desc">Milkrice uses an AI you already have — no API keys, no Milkrice account. Your transcript is sent only to the provider you choose.</p>
  <div id="provBox">${providersHtml()}</div>
  <div class="row"><div class="rl"><b>Default template</b><span>How notes are structured; change per meeting too.</span></div>
    <select class="field" id="tplDefault">${S.templates.map((t) => `<option value="${t.id}" ${t.id === s.default_template ? "selected" : ""}>${esc(t.name)}</option>`).join("")}</select></div>
  <div class="row"><div class="rl"><b>Write notes when a meeting ends</b><span>Also names speakers and titles the meeting.</span></div>${sw("auto_enhance", s.auto_enhance)}</div>

  <h2>Calendar &amp; meetings</h2><p class="desc">Milkrice only ever <b>reads</b> calendars. Pick the least access that works for you.</p>
  <div id="calSources"><p class="hint">Checking calendars…</p></div>
  <div class="row"><div class="rl"><b>When a meeting starts</b><span>Notify you, start recording on its own, or stay quiet.</span></div>${seg("calendar_mode", s.calendar_mode, [["notify", "Notify me"], ["auto", "Record automatically"], ["off", "Off"]])}</div>
  ${s.calendar_mode === "auto" ? `<p class="auto-ack"><span class="rec-dot"></span><span>Auto-record is on. Milkrice records meetings with people in them by itself, shows <b>Auto-record</b> in the sidebar and menu bar while it's on, and tells you each time it starts. Let the people you meet know they're being transcribed.</span></p>` : ""}
  <div class="row"><div class="rl"><b>Stop when the meeting ends</b><span>After the event's end time once the room goes quiet.</span></div>${sw("auto_stop", s.auto_stop)}</div>
  <div class="row"><div class="rl"><b>Open at login</b><span>Milkrice waits in the menu bar so it can catch your first meeting of the day.</span></div><button class="switch" id="autostartSw" role="switch" aria-checked="false"></button></div>
  <div class="row"><div class="rl"><b>Notice unscheduled calls</b><span>Zoom, Teams, Webex, Slack, FaceTime… using your mic.</span></div>${sw("detect_calls", s.detect_calls)}</div>
  <div class="row"><div class="rl"><b>Only meetings with people</b><span>Skip focus blocks and reminders without guests or a call link.</span></div>${sw("only_real_meetings", s.only_real_meetings)}</div>

  <h2>Obsidian &amp; second brain</h2><p class="desc">Mirror every meeting into your vault as plain Markdown — summary, your notes and transcript, with attendees as [[links]]. Anything you add outside Milkrice's block is never touched.</p>
  <div id="obsBox"><p class="hint">Looking for vaults…</p></div>

  <h2>iPhone sync</h2><p class="desc">Optional. Meetings you record with Milkrice for iPhone (beta) show up here, and yours show up there. They travel through a folder in your iCloud Drive, end-to-end encrypted with a key that only your paired devices hold.</p>
  <div id="syncBox"><p class="hint">Checking…</p></div>

  <h2>Agents</h2><p class="desc">Let Claude Code, Codex, Cursor, OpenCode, VS Code or any MCP client read your meetings. Your notes, AI summaries and speech transcripts stay clearly labelled so agents never confuse them.</p>
  <div id="agentsBox"><p class="hint">Checking…</p></div>

  <h2>Audio</h2>
  <div class="row"><div class="rl"><b>Microphone</b><span>Your voice is always labelled as you.</span></div><select class="field" id="micSel"><option value="">System default</option></select></div>
  <div class="row"><div class="rl"><b>Capture other participants</b><span>Transcribes everyone else on the call, not just you.</span></div>${sw("capture_system", s.capture_system)}</div>
  <div class="row"><div class="rl"><b>Recognise speakers</b><span>Tells voices apart locally; names come from the invite.</span></div>${sw("diarize", s.diarize)}</div>
  <div class="row"><div class="rl"><b>Keep recordings</b><span>Off by default. When on, each meeting's audio is saved on this Mac (you left, the call right, about 60 MB an hour) so you can replay any moment from the transcript.</span></div>${sw("keep_audio", s.keep_audio)}</div>
  ${s.keep_audio ? `<div class="row"><div class="rl"><b>Delete recordings after</b><span id="audioUsage">Transcripts and notes are always kept.</span></div>${seg("audio_retention_days", String(s.audio_retention_days), [["7", "7 days"], ["30", "30 days"], ["90", "90 days"], ["0", "Never"]])}</div>` : ""}
  <div class="row"><div class="rl"><b>Your name</b><span>Used for your lines in transcripts.</span></div><input class="field" id="userName" value="${esc(s.user_name)}" placeholder="Me" /></div>
  </div>`;
  v.innerHTML = h;
  bindSettings(v);
  invoke("input_devices").then((devs) => {
    const sel = $("#micSel");
    if (!sel) return;
    sel.innerHTML = `<option value="">System default</option>` + devs.map((d) => `<option ${d === s.mic_device ? "selected" : ""}>${esc(d)}</option>`).join("");
  });
}

async function loadObsidian() {
  const box = $("#obsBox");
  if (!box) return;
  const vaults = await invoke("obsidian_vaults").catch(() => []);
  const o = S.settings.obsidian;
  const save = async (patch) => { await saveSettings({ obsidian: patch === null ? null : { ...(S.settings.obsidian || { folder: "Meetings", link_people: true, link_daily_note: true, include_transcript: true }), ...patch } }); loadObsidian(); };
  if (!o) {
    box.innerHTML = vaults.length
      ? vaults.map((v) => `<div class="provider"><span class="status"></span><div class="pi"><b>${esc(v.name)}</b><span>${esc(v.path)}</span></div><button class="btn small" data-vault="${esc(v.path)}">Use this vault</button></div>`).join("") + `<button class="link" id="obsPick">Choose another folder…</button>`
      : `<div class="callout">No Obsidian vaults found. Any folder works too — Markdown is just files. <button class="btn small" id="obsPick">Choose folder…</button></div>`;
  } else {
    const vname = o.root.split("/").pop();
    box.innerHTML = `<div class="provider on"><span class="status ok"></span><div class="pi"><b>${esc(vname)}</b><span>${esc(o.root)}/${esc(o.folder)}</span></div><button class="link" id="obsOff">Turn off</button></div>
      <div class="row"><div class="rl"><b>Folder in the vault</b><span>Notes are named “2026-11-16 Meeting title.md”.</span></div><input class="field" id="obsFolder" value="${esc(o.folder)}" /></div>
      <div class="row"><div class="rl"><b>Link people</b><span>Attendees and named speakers become [[Name]] links.</span></div>${sw("obs_link_people", o.link_people)}</div>
      <div class="row"><div class="rl"><b>Link the daily note</b><span>The date becomes [[YYYY-MM-DD]].</span></div>${sw("obs_link_daily_note", o.link_daily_note)}</div>
      <div class="row"><div class="rl"><b>Include the transcript</b><span>Folded under the notes so the page stays readable.</span></div>${sw("obs_include_transcript", o.include_transcript)}</div>
      <div style="display:flex;gap:10px;align-items:center;margin-top:10px"><button class="btn small" id="obsAll">Export all past meetings</button><span class="hint" id="obsAllRes"></span></div>`;
  }
  box.querySelectorAll("[data-vault]").forEach((b) => (b.onclick = () => save({ root: b.dataset.vault })));
  const pick = $("#obsPick");
  if (pick) pick.onclick = async () => {
    const dir = await T.dialog?.open({ directory: true, multiple: false, title: "Choose your vault or notes folder" }).catch(fail);
    if (dir) save({ root: dir });
  };
  const off = $("#obsOff");
  if (off) off.onclick = () => save(null);
  const fo = $("#obsFolder");
  if (fo) fo.onchange = () => save({ folder: fo.value.trim().replace(/^\/+|\/+$/g, "") || "Meetings" });
  box.querySelectorAll("[data-switch^='obs_']").forEach((b) => (b.onclick = (ev) => { ev.stopPropagation(); const k = b.dataset.switch.slice(4); save({ [k]: !S.settings.obsidian[k] }); }));
  const all = $("#obsAll");
  if (all) all.onclick = async () => {
    all.disabled = true; $("#obsAllRes").textContent = "Writing…";
    try { const n = await invoke("obsidian_export_all"); $("#obsAllRes").textContent = `${n} notes written to your vault.`; } catch (e) { fail(e); $("#obsAllRes").textContent = ""; }
    all.disabled = false;
  };
}

function ago(t) {
  if (!t) return "never";
  const s = Math.max(0, (Date.now() - new Date(t).getTime()) / 1000);
  if (s < 45) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return new Date(t).toLocaleDateString(undefined, { day: "numeric", month: "short" });
}

async function loadSync() {
  const box = $("#syncBox");
  if (!box || S.pairing) return;
  let st;
  try { st = await invoke("sync_status"); } catch (e) { box.innerHTML = `<p class="hint">${esc(e)}</p>`; return; }
  if (S.pairing || !$("#syncBox")) return;
  if (!st.setup) {
    box.innerHTML = (st.icloud_available
      ? `<div class="provider"><span class="status"></span><div class="pi"><b>iCloud Drive</b><span>A “Milkrice Sync” folder that holds only sealed files. Apple can't read them, and there's no Milkrice server.</span></div><button class="btn small primary" id="syncOn">Turn on</button></div>`
      : `<div class="callout">iCloud Drive is off on this Mac. Turn it on in System Settings › Apple Account › iCloud, or choose a folder your iPhone can also see.</div>`)
      + `<button class="link" id="syncPick">Use another folder…</button>`;
    const go = async (folder) => {
      box.querySelectorAll("button").forEach((b) => (b.disabled = true));
      try { await invoke("sync_setup", { folder }); toast("Sync is on. Now pair your iPhone."); loadSync(); } catch (e) { fail(e); loadSync(); }
    };
    const on = $("#syncOn");
    if (on) on.onclick = () => go(null);
    $("#syncPick").onclick = async () => {
      const dir = await T.dialog?.open({ directory: true, multiple: false, title: "Choose a folder your iPhone can also see" }).catch(fail);
      if (dir) go(dir);
    };
    return;
  }
  const where = st.in_icloud ? "iCloud Drive › " + esc(String(st.folder).split("/").pop()) : esc(st.folder);
  const phones = (st.devices || []).filter((d) => !d.this_mac);
  const health = st.error ? `<span class="sync-err">${esc(st.error)}</span>` : `Last synced ${ago(st.last_ok)}${st.meetings_from_others ? ` · ${st.meetings_from_others} meetings from other devices` : ""}`;
  box.innerHTML = `
    <div class="provider on"><span class="status ${st.error ? "" : "ok"}"></span><div class="pi"><b>${where}</b><span>${health}</span></div>
      <button class="btn small" id="syncNow">Sync now</button></div>
    ${phones.map((d) => `<div class="provider on"><span class="dev-ic">${d.kind === "mac" ? ICON.disk : ICON.phone}</span><div class="pi"><b>${esc(d.name)}</b><span>${d.kind === "mac" ? "Mac" : "iPhone"} · last seen ${ago(d.last_seen)}</span></div></div>`).join("")}
    ${phones.length ? "" : `<div class="callout">No iPhone yet. Install Milkrice for iPhone, then pair it here.</div>`}
    <div style="display:flex;gap:10px;align-items:center;margin-top:10px"><button class="btn small ${phones.length ? "" : "primary"}" id="syncPair">Pair an iPhone</button><button class="link" id="syncLeave">Stop syncing on this Mac</button></div>
    <p class="hint" style="margin-top:8px">When your iPhone asks for notes, this Mac writes them with the AI chosen above. Meetings from the iPhone are mirrored to Obsidian too, if that's on.</p>`;
  $("#syncNow").onclick = () => { invoke("sync_now"); toast("Syncing…", 1200); setTimeout(loadSync, 2500); };
  $("#syncPair").onclick = () => startPairing();
  $("#syncLeave").onclick = async () => {
    if (!confirm("Stop syncing on this Mac? Your meetings stay here. The key is removed from this Mac, so you'd need to set up sync again to rejoin.")) return;
    try { await invoke("sync_leave"); loadSync(); } catch (e) { fail(e); }
  };
}

async function startPairing() {
  const box = $("#syncBox");
  let inv;
  try { inv = await invoke("sync_pair_start"); } catch (e) { fail(e); return; }
  const started = Date.now();
  S.pairing = true;
  const stop = async (cancel) => {
    clearInterval(S.pairTimer); S.pairTimer = null; S.pairing = false;
    if (cancel) await invoke("sync_pair_cancel").catch(() => {});
    loadSync();
  };
  box.innerHTML = `<div class="pair">
      <div class="pair-qr">${inv.svg}</div>
      <div class="pair-text"><b>Scan this with your iPhone</b>
        <ol class="steps-mini"><li>Open Milkrice on your iPhone.</li><li>Settings › Sync with Mac › <b>Pair with my Mac</b>.</li><li>Point the camera at this code.</li></ol>
        <p class="hint" id="pairState"><span class="spin"></span> Waiting for your iPhone…</p>
        <details class="pair-paste"><summary class="hint">No camera? Paste a code instead</summary><code>${esc(inv.qr)}</code> <button class="btn small" id="pairCopy">Copy</button></details>
        <button class="link" id="pairCancel">Cancel</button></div></div>`;
  $("#pairCancel").onclick = () => stop(true);
  $("#pairCopy").onclick = () => navigator.clipboard.writeText(inv.qr).then(() => toast("Copied"));
  S.pairTimer = setInterval(async () => {
    if (!$("#syncBox")) { stop(true); return; }
    if (Date.now() - started > 10 * 60 * 1000) { toast("Pairing timed out. Try again."); stop(true); return; }
    let j;
    try { j = await invoke("sync_pair_poll"); } catch (e) { fail(e); stop(true); return; }
    if (!j) return;
    clearInterval(S.pairTimer); S.pairTimer = null;
    const t = $(".pair-text");
    t.innerHTML = `<b>“${esc(j.name)}” wants to join</b>
      <p>Check that your iPhone shows the same code:</p>
      <p class="pair-code">${esc(j.code.slice(0, 3))} ${esc(j.code.slice(3))}</p>
      <div style="display:flex;gap:10px;align-items:center"><button class="btn small primary" id="pairYes">The codes match</button><button class="link" id="pairNo">They don't match</button></div>`;
    $("#pairYes").onclick = async () => {
      try { const name = await invoke("sync_pair_accept"); toast(`Paired with ${name}`, 3500); } catch (e) { fail(e); }
      stop(false);
    };
    $("#pairNo").onclick = () => { toast("Not paired. Nothing was shared."); stop(true); };
  }, 1000);
}

async function loadCalendarSources() {
  const box = $("#calSources");
  if (!box) return;
  let c;
  try { c = await invoke("calendar_sources"); } catch (e) { box.innerHTML = `<p class="hint">${esc(e)}</p>`; return; }
  const macOk = S.perms?.calendar === "granted";
  const feeds = c.feeds.map((f) => `<div class="provider on"><span class="status ok"></span><div class="pi"><b>${esc(f.name)}</b><span>Read-only iCal subscription · ${esc(f.host)}${f.owner_email ? ` · as ${esc(f.owner_email)}` : ""}</span></div><button class="link" data-feed-rm="${esc(f.id)}">Remove</button></div>`).join("");
  box.innerHTML = `
    <div class="provider ${macOk ? "on" : ""}"><span class="status ${macOk ? "ok" : ""}"></span>
      <div class="pi"><b>Calendars on this Mac</b><span>${macOk ? `${c.event_count} upcoming events${c.calendars_with_events.length ? ` from ${esc(c.calendars_with_events.join(", "))}` : ""}. Includes any account added in System Settings → Internet Accounts.` : "Needs Calendar permission (Permissions above)."}</span></div>
      ${macOk ? '<span class="badge sage">Connected</span>' : ""}</div>
    ${feeds}
    <details class="gcal" ${c.feeds.length || (macOk && c.event_count) ? "" : "open"}><summary><b>Add Google Calendar</b> <span class="hint">— two ways, both without giving Milkrice your Google password</span></summary>
      <div class="gcal-body">
        <p><b>Least access (recommended):</b> paste your calendar's <i>secret address in iCal format</i>. Milkrice can only read that one calendar, can't change anything, and you can revoke it any time by resetting the address in Google.</p>
        <ol class="steps-mini">
          <li>Open <a href="#" data-open="https://calendar.google.com/calendar/u/0/r/settings">Google Calendar settings</a> → your calendar under <b>Settings for my calendars</b>.</li>
          <li>Scroll to <b>Integrate calendar</b> and copy <b>Secret address in iCal format</b>.</li>
          <li>Paste it here:</li>
        </ol>
        <div class="gcal-form">
          <input class="field" id="feedUrl" placeholder="https://calendar.google.com/calendar/ical/…/private-…/basic.ics" />
          <input class="field" id="feedEmail" placeholder="Your email on that calendar (optional)" />
          <button class="btn small primary" id="feedAdd">Add</button>
        </div>
        <p class="hint">Your email lets Milkrice skip meetings you've declined. The address is stored only on this Mac.</p>
        <p><b>Or, no new access at all:</b> add your Google account in <a href="#" data-open="x-apple.systempreferences:com.apple.Internet-Accounts-Settings.extension">System Settings → Internet Accounts</a> with Calendars on. Milkrice then sees it through the Calendar permission it already has.</p>
      </div></details>`;
  box.querySelectorAll("[data-open]").forEach((a) => (a.onclick = (e) => { e.preventDefault(); T.opener?.openUrl(a.dataset.open).catch(() => window.open(a.dataset.open)); }));
  box.querySelectorAll("[data-feed-rm]").forEach((b) => (b.onclick = async () => { await invoke("remove_calendar_feed", { id: b.dataset.feedRm }).catch(fail); S.settings = await invoke("get_settings"); loadCalendarSources(); setTimeout(loadEvents, 1500); }));
  const add = $("#feedAdd");
  if (add) add.onclick = async () => {
    const url = $("#feedUrl").value.trim();
    if (!url) return;
    add.disabled = true; add.textContent = "Checking…";
    try {
      const r = await invoke("add_calendar_feed", { name: "Google Calendar", url, ownerEmail: $("#feedEmail").value.trim() || null });
      toast(`Connected — ${r.upcoming} events in the next 30 days`, 3500);
      S.settings = await invoke("get_settings");
      setTimeout(loadEvents, 1500);
      loadCalendarSources();
    } catch (e) { fail(e); add.disabled = false; add.textContent = "Add"; }
  };
}

async function loadAgents() {
  const box = $("#agentsBox");
  if (!box) return;
  let a;
  try { a = await invoke("agents_status"); } catch (e) { box.innerHTML = `<p class="hint">${esc(e)}</p>`; return; }
  const row = (key, name, desc, ok, avail, label) => `<div class="provider ${ok ? "on" : ""}"><span class="status ${ok ? "ok" : ""}"></span>
    <div class="pi"><b>${name}</b><span>${desc}</span></div>
    ${ok ? '<span class="badge sage">Connected</span>' : avail ? `<button class="btn small" data-agent="${key}">${label}</button>` : '<span class="hint">Not installed</span>'}</div>`;
  const agent = (id, name, desc) => {
    const g = a.agents.find((x) => x.target === id) || {};
    const warn = (g.warnings || [])[0];
    return row(id, name, desc + (warn ? ` ${esc(warn)}` : ""), g.connected, g.detected, "Connect");
  };
  const cliWhere = a.cli_installed ? `Installed at ${esc(a.command)}.` : `Installs to ${esc(a.cli_link)}.`;
  const both = "Adds the Milkrice MCP server and the milkrice skill.";
  box.innerHTML =
    row("cli", "<code>milkrice</code> command", `Token-efficient CLI for any agent or script: <code>milkrice ls</code>, <code>show</code>, <code>tx</code>, <code>search</code>. ${cliWhere}`, a.cli_installed, a.cli_bundled, "Install") +
    (a.path_hint ? `<p class="hint">${esc(a.path_hint)}</p>` : "") +
    agent("claude", "Claude Code", a.claude_plugin ? `Connected through the Milkrice plugin (${esc(a.claude_plugin)}).` : both) +
    agent("codex", "Codex", both) +
    agent("cursor", "Cursor", both) +
    agent("opencode", "OpenCode", both) +
    agent("vscode", "VS Code", "Adds the MCP server and skill for GitHub Copilot.") +
    `<p class="hint">Connect also installs the <code>milkrice</code> command. Any other MCP client: run <code>milkrice mcp</code> over stdio. Access is read-only. To undo, run <code>milkrice agents uninstall</code> with the agent's name.</p>`;
  box.querySelectorAll("[data-agent]").forEach((b) => (b.onclick = async () => {
    b.disabled = true; b.textContent = "Connecting…";
    try { toast(await invoke("agent_connect", { target: b.dataset.agent }), 3500); } catch (e) { fail(e); }
    loadAgents();
  }));
}

function providersHtml() {
  if (!S.providers) return `<p class="hint"><span class="spinner" style="display:inline-block;vertical-align:-2px"></span> Looking for Claude Code, Codex and Ollama…</p>`;
  const sel = S.settings.provider;
  const head = sel ? "" : `<div class="callout choose" style="margin-bottom:8px">${ICON.spark}<div><b>No AI chosen. AI notes are off.</b><span>Pick one below and Milkrice will remember it. Until then nothing is sent anywhere.</span></div></div>`;
  return head + S.providers.map((p) => {
    const on = sel === p.id;
    return `<div class="provider ${on ? "on" : ""}"><span class="status ${p.available && p.signed_in ? "ok" : ""}"></span>
      <div class="pi"><b>${esc(p.name)}${p.model ? ` <span class="hint">· ${esc(p.model)}</span>` : ""}</b><span>${esc(p.detail)}</span></div>
      ${p.available && p.signed_in ? (on ? `<span class="badge">Selected</span>` : `<button class="btn small" data-prov="${p.id}">Use</button>`) : ""}</div>`;
  }).join("") + `<div style="display:flex;gap:16px"><button class="link" id="provRefresh">Check again</button>${sel ? `<button class="link" id="provNone" style="color:var(--ink-2)">Don't use AI notes</button>` : ""}</div>`;
}

async function saveSettings(patch) {
  S.settings = await invoke("save_settings", { settings: { ...S.settings, ...patch } });
  renderBadge();
}

function bindSettings(v) {
  v.querySelectorAll("[data-switch]:not([data-switch^='obs_'])").forEach((b) => (b.onclick = async () => {
    const k = b.dataset.switch;
    await saveSettings({ [k]: !S.settings[k] });
    render();
  }));
  v.querySelectorAll("[data-seg-name]").forEach((g) => g.querySelectorAll("button").forEach((b) => (b.onclick = async () => {
    const val = g.dataset.segName === "audio_retention_days" ? Number(b.dataset.v) : b.dataset.v;
    await saveSettings({ [g.dataset.segName]: val });
    render();
  })));
  $("#langSel").onchange = (e) => saveSettings({ language: e.target.value });
  $("#tplDefault").onchange = (e) => saveSettings({ default_template: e.target.value });
  $("#micSel").onchange = (e) => saveSettings({ mic_device: e.target.value || null });
  $("#userName").onchange = (e) => saveSettings({ user_name: e.target.value.trim() });
  const cc = $("#calConnect");
  if (cc) cc.onclick = connectCalendar;
  bindPerms(v);
  const as = $("#autostartSw");
  const paintAs = (on) => { as.classList.toggle("on", on); as.setAttribute("aria-checked", String(on)); };
  invoke("autostart_get").then((on) => paintAs(!!on)).catch(() => {});
  as.onclick = async () => {
    try { paintAs(!!(await invoke("autostart_set", { on: !as.classList.contains("on") }))); } catch (e) { fail(e); }
  };
  loadAgents();
  loadObsidian();
  loadSync();
  loadCalendarSources();
  if (S.settings.keep_audio) invoke("audio_storage").then((u) => { const el = $("#audioUsage"); if (el) el.textContent = `Using ${(u.bytes / 1048576).toFixed(u.bytes > 1e8 ? 0 : 1)} MB. Transcripts and notes are always kept.`; }).catch(() => {});
  bindModels(v);
  bindProviders(v);
}

function bindModels(v) {
  const box = $("#modelsBox", v) || v;
  const lf = $("#langFilter", box);
  if (lf) lf.onchange = (e) => { S.langFilter = e.target.value; rerenderModels(); };
  const mm = $("#moreModels", box);
  if (mm) mm.ontoggle = () => { S.moreModels = mm.open; };
  box.querySelectorAll("[data-dl]").forEach((b) => (b.onclick = async () => {
    S.progress[b.dataset.dl] = { done: 0, total: 0 };
    await invoke("download_model", { id: b.dataset.dl }).catch(fail);
    await loadModels(); rerenderModels();
  }));
  box.querySelectorAll("[data-cancel]").forEach((b) => (b.onclick = () => invoke("cancel_download", { id: b.dataset.cancel })));
  box.querySelectorAll("[data-use]").forEach((b) => (b.onclick = async () => {
    await saveSettings({ model_id: b.dataset.use });
    await loadModels(); rerenderModels(); toast("Loading model…");
  }));
  box.querySelectorAll("[data-del]").forEach((b) => (b.onclick = async () => {
    if (!confirm("Delete this model from disk?")) return;
    try { await invoke("delete_model", { id: b.dataset.del }); S.settings = await invoke("get_settings"); } catch (e) { fail(e); }
    await loadModels(); rerenderModels();
  }));
}

function rerenderModels() {
  const box = $("#modelsBox");
  if (box) { box.innerHTML = modelsHtml(); bindModels(box.parentElement); }
  const ob = $("#obModels");
  if (ob) renderOnboarding($("#view"));
}

function bindProviders(v) {
  v.querySelectorAll("[data-prov]").forEach((b) => (b.onclick = async () => { await saveSettings({ provider: b.dataset.prov }); $("#provBox").innerHTML = providersHtml(); bindProviders(v); }));
  const pn = $("#provNone", v);
  if (pn) pn.onclick = async () => { await saveSettings({ provider: null }); $("#provBox").innerHTML = providersHtml(); bindProviders(v); toast("AI notes are off"); };
  const r = $("#provRefresh", v);
  if (r) r.onclick = async () => { S.providers = null; $("#provBox").innerHTML = providersHtml(); await loadProviders(true); $("#provBox").innerHTML = providersHtml(); bindProviders(v); };
}

// ------------------------------------------------------------------ onboarding
function renderOnboarding(v) {
  if (S.obTest) return renderObTest(v);
  const hasModel = S.models?.models.some((m) => m.downloaded);
  const dling = S.models?.models.find((m) => S.progress[m.id] && !S.progress[m.id].finished);
  const permsOk = ["microphone", "system_audio", "calendar"].every((k) => S.perms?.[k] === "granted");
  const avail = (S.providers || []).filter((p) => p.available && p.signed_in);
  const chosen = S.settings.provider ? avail.find((p) => p.id === S.settings.provider) : null;
  const skipped = !chosen && S.obNoAi;
  const ai = chosen || skipped;
  const pct = dling && S.progress[dling.id].total ? Math.round((S.progress[dling.id].done / S.progress[dling.id].total) * 100) : 0;
  v.innerHTML = `<div class="onboard"><img class="illus" src="img/onboarding.jpg" alt="">
    <h1>Notes that write themselves.<br>Audio never leaves your Mac.</h1>
    <p>Milkrice transcribes your meetings with an open model on this Mac and turns your rough jottings into clean notes. No account, no bots in your calls. To write the notes, the transcript text goes to the AI you choose — or stays here with Ollama.</p>
    <div class="steps" id="obModels">
      <div class="step ${hasModel ? "done" : ""}"><span class="n">${hasModel ? "✓" : 1}</span><div><b>Download a speech model</b><span>${hasModel ? "Ready." : dling ? `Downloading ${esc(dling.name)}… ${pct}%` : "Parakeet v2 (451 MB) — fast and accurate for English."}</span></div>
        ${hasModel ? "" : dling ? `<div class="progress" style="width:120px"><i style="width:${pct}%"></i></div>` : `<div style="display:flex;gap:6px"><button class="btn small" id="obMore">Other models</button><button class="btn small primary" id="obDl">Download</button></div>`}</div>
      <div class="step ${permsOk ? "done" : ""}" style="grid-template-columns:28px 1fr"><span class="n">${permsOk ? "✓" : 2}</span><div><b>Let Milkrice listen</b><span>One click each — Milkrice opens the right switch for you.</span>
        <div class="perms-slot" style="margin-top:10px">${permsHtml()}</div></div></div>
      ${S.vaults?.length ? `<div class="step ${S.settings.obsidian ? "done" : ""}"><span class="n">${S.settings.obsidian ? "✓" : "★"}</span><div><b>Keep notes in Obsidian <span class="hint">(optional)</span></b><span>${S.settings.obsidian ? `Every meeting is mirrored into ${esc(S.settings.obsidian.root.split("/").pop())}/${esc(S.settings.obsidian.folder)}.` : `Found your vault “${esc(S.vaults[0].name)}”. Mirror every meeting there as Markdown.`}</span></div>
        ${S.settings.obsidian ? "" : `<button class="btn small" id="obVault">Use it</button>`}</div>` : ""}
      <div class="step ${ai ? "done" : ""}"><span class="n">${ai ? "✓" : 3}</span><div><b>Who writes your notes?</b><span>${chosen ? `${esc(chosen.name)}: ${chosen.id === "ollama" ? "on this Mac, nothing leaves it." : "the transcript text is sent to it when notes are written. Audio never is."} <button class="link" id="obChange">Change</button>` : skipped ? `No AI for now. Milkrice only transcribes, and nothing leaves this Mac. Pick one later in Settings → AI notes. <button class="link" id="obChange">Choose now</button>` : S.providers ? (avail.length ? "Pick one and Milkrice remembers it. Claude and Codex use your existing subscription; Ollama keeps everything on this Mac." : "Sign in to Claude Code (claude) or Codex (codex login), or run Ollama.") : "Checking…"}</span>
          ${!ai && S.providers ? `<div style="display:flex;gap:6px;margin-top:8px;flex-wrap:wrap">${avail.map((p) => `<button class="btn small" data-ob-prov="${p.id}">${esc(p.name)}</button>`).join("")}<button class="btn small quiet" id="obNoAi">Not now</button></div>` : ""}</div>
        ${ai || !S.providers || avail.length ? "" : `<button class="btn small" id="obAi">Check again</button>`}</div>
    </div>
    <div style="display:flex;gap:10px;align-items:center"><button class="btn primary" id="obDone" ${hasModel && ai ? "" : "disabled"}>Next: test your mic</button>
      <span class="hint">${!hasModel ? "A speech model is required." : !ai ? "Choose who writes your notes, or pick “Not now”." : "A 20-second test, then you're done."}</span></div></div>`;
  const dl = $("#obDl");
  if (dl) dl.onclick = async () => { S.progress["parakeet-v2"] = { done: 0, total: 0 }; await invoke("download_model", { id: "parakeet-v2" }).catch(fail); renderOnboarding(v); };
  const more = $("#obMore");
  if (more) more.onclick = () => { S.view = "settings"; render(); };
  bindPerms(v);
  const aib = $("#obAi");
  if (aib) aib.onclick = async () => { S.providers = null; renderOnboarding(v); await loadProviders(true); renderOnboarding(v); };
  v.querySelectorAll("[data-ob-prov]").forEach((b) => (b.onclick = async () => { S.obNoAi = false; await saveSettings({ provider: b.dataset.obProv }); renderOnboarding(v); renderBadge(); }));
  const na = $("#obNoAi");
  if (na) na.onclick = () => { S.obNoAi = true; renderOnboarding(v); };
  const ch = $("#obChange");
  if (ch) ch.onclick = async () => { S.obNoAi = false; await saveSettings({ provider: null }); renderOnboarding(v); };
  const ov = $("#obVault");
  if (ov) ov.onclick = async () => { await saveSettings({ obsidian: { root: S.vaults[0].path, folder: "Meetings", link_people: true, link_daily_note: true, include_transcript: true } }); renderOnboarding(v); };
  $("#obDone").onclick = () => { S.obTest = { phase: "ready", lines: [] }; renderOnboarding(v); };
}

// ------------------------------------------------------------------ onboarding: 20-second mic test
const OB_TEST_SECS = 20;
function renderObTest(v) {
  const t = S.obTest;
  const lines = t.lines.map((s) => `<div class="ob-line ${s.speaker === "me" ? "me" : ""}"><span class="ts">${fmtTs(s.start_ms)}</span><span>${esc(s.text)}</span></div>`).join("");
  let body, actions;
  if (t.phase === "ready") {
    body = `<p>Milkrice will record into a scratch note for ${OB_TEST_SECS} seconds. Say something out loud, like what you're working on today, and watch the words appear. Everything stays on this Mac.</p>`;
    actions = `<button class="btn ember" id="obtStart"><span class="dot-w"></span>Start the test</button><button class="btn" id="obtSkip">Skip</button>`;
  } else if (t.phase === "recording") {
    body = `<div class="ob-live"><span class="rec-badge"><span class="rec-dot"></span>REC</span><span class="ob-count" id="obtCount">${t.left}s</span>
        <span class="meter me" id="obtBars">${Array.from({ length: 9 }, () => "<b></b>").join("")}</span></div>
      <div class="ob-lines" id="obtLines">${lines || `<p class="hint">Listening… say something.</p>`}</div>`;
    actions = `<button class="btn" id="obtStop">${ICON.stop} Stop early</button>`;
  } else {
    const heard = t.lines.length > 0;
    body = `<div class="ob-lines">${lines || ""}</div>
      ${heard ? `<p>That's Milkrice hearing you, transcribed on this Mac. You're ready for your next meeting.</p>`
        : `<p>Milkrice didn't hear anything. Check the mic is the right one and isn't muted, then try again.</p>`}`;
    actions = heard
      ? `<button class="btn primary" id="obtDone">Looks good</button><button class="link" id="obtKeep">Keep this test note</button>`
      : `<button class="btn primary" id="obtRetry">Try again</button><button class="btn" id="obtMic">Choose mic</button><button class="link" id="obtDoneAnyway">Finish anyway</button>`;
  }
  v.innerHTML = `<div class="onboard ob-test"><h1>${t.phase === "done" ? (t.lines.length ? "Milkrice can hear you." : "Nothing heard yet.") : "Say something."}</h1>
    ${body}<div class="ob-actions">${actions}</div></div>`;
  const on = (id, fn) => { const el = $("#" + id); if (el) el.onclick = fn; };
  on("obtStart", startObTest);
  on("obtSkip", () => finishOnboarding(false));
  on("obtStop", () => stopObTest());
  on("obtDone", () => finishOnboarding(false));
  on("obtDoneAnyway", () => finishOnboarding(false));
  on("obtKeep", () => finishOnboarding(true));
  on("obtRetry", async () => { await discardObTestNote(); S.obTest = { phase: "ready", lines: [] }; startObTest(); });
  on("obtMic", () => openDevicePicker());
  const box = $("#obtLines");
  if (box) box.scrollTop = box.scrollHeight;
}

async function startObTest() {
  const v = $("#view");
  if (S.perms && S.perms.microphone !== "granted") {
    await grantPerm("microphone");
    if (S.perms.microphone !== "granted") { openPermsSheet("Milkrice needs your microphone"); return; }
  }
  try {
    const m = await invoke("create_meeting", { title: "Milkrice mic test" });
    S.obTest = { phase: "recording", lines: [], id: m.id, left: OB_TEST_SECS };
    renderObTest(v);
    await invoke("start_recording", { id: m.id });
    S.obTest.timer = setInterval(() => {
      if (!S.obTest || S.obTest.phase !== "recording") return;
      S.obTest.left -= 1;
      const c = $("#obtCount");
      if (c) c.textContent = `${Math.max(0, S.obTest.left)}s`;
      if (S.obTest.left <= 0) stopObTest();
    }, 1000);
  } catch (e) { fail(e); S.obTest = { phase: "ready", lines: [] }; renderObTest(v); }
}

async function stopObTest() {
  const t = S.obTest;
  if (!t || t.phase !== "recording") return;
  clearInterval(t.timer);
  t.phase = "done";
  // No AI pass for the test: nothing from it goes anywhere.
  await invoke("stop_recording", { ai: false }).catch(fail);
  renderObTest($("#view"));
}

async function discardObTestNote() {
  const id = S.obTest?.id;
  if (id) await invoke("delete_meeting", { id }).catch(() => {});
}

async function finishOnboarding(keepTest) {
  if (S.obTest?.phase === "recording") await stopObTest();
  if (!keepTest) await discardObTestNote();
  const keptId = keepTest ? S.obTest?.id : null;
  S.obTest = null;
  await saveSettings({ onboarded: true });
  // Launch at login is on by default so Milkrice is there for the first meeting of the day.
  invoke("autostart_set", { on: true }).catch(() => {});
  await loadMeetings();
  if (keptId) { await openMeeting(keptId, "transcript"); return; }
  S.view = "home";
  render();
}

// ------------------------------------------------------------------ chrome
$("#newNote").onclick = async () => {
  try {
    const m = await invoke("create_meeting", { title: "" });
    await loadMeetings();
    await openMeeting(m.id, "notes");
    $("#titleInput")?.focus();
  } catch (e) { fail(e); }
};
$("#openSettings").onclick = async () => {
  await leaveNote(null);
  S.view = "settings"; render(); renderSidebar();
  S.perms = await invoke("permissions");
  await Promise.all([loadModels(), S.providers ? null : loadProviders(false)]);
  if (S.view === "settings") render();
};
$("#search").oninput = (e) => { S.query = e.target.value; clearTimeout(e.target._t); e.target._t = setTimeout(loadMeetings, 150); };
document.querySelector(".brand").onclick = goHome;
$("#homeBtn").onclick = goHome;
document.addEventListener("keydown", (e) => {
  if (e.metaKey && e.key === "n") { e.preventDefault(); $("#newNote").click(); }
  if (e.metaKey && e.key === ",") { e.preventDefault(); $("#openSettings").click(); }
  if (e.metaKey && e.key === "k") { e.preventDefault(); $("#search").focus(); }
  if (e.metaKey && (e.key === "[" || (e.shiftKey && e.key.toLowerCase() === "h"))) { e.preventDefault(); goHome(); }
});

// ------------------------------------------------------------------ live events
function wireEvents() {
  listen("segment", (p) => {
    // A transcribed line from the mic proves it isn't blocked.
    if ((S.sysWarn === "mic" || S.sysWarn === "mic-perm") && p.segment.source === "mic") { S.sysWarn = false; renderSysWarn(); }
    if (S.obTest && p.meeting_id === S.obTest.id) {
      S.obTest.lines.push(p.segment);
      S.obTest.lines.sort((a, b) => a.start_ms - b.start_ms);
      const box = $("#obtLines");
      if (box) { box.innerHTML = S.obTest.lines.map((s) => `<div class="ob-line ${s.speaker === "me" ? "me" : ""}"><span class="ts">${fmtTs(s.start_ms)}</span><span>${esc(s.text)}</span></div>`).join(""); box.scrollTop = box.scrollHeight; }
      return;
    }
    if (S.view === "note" && S.detail && p.meeting_id === S.meetingId) {
      // Mic lines are held briefly for echo checks, so arrivals can be out of order.
      const segs = S.detail.segments;
      let i = segs.length;
      while (i > 0 && segs[i - 1].start_ms > p.segment.start_ms) i--;
      segs.splice(i, 0, p.segment);
      if (S.tab === "transcript") {
        const box = $("#transcriptBox");
        if (box) {
          const v = $("#view");
          const atBottom = v.scrollHeight - v.scrollTop - v.clientHeight < 120;
          box.innerHTML = transcriptHtml(S.detail);
          const sp = $("#speakersBox");
          if (sp) { sp.innerHTML = speakersHtml(S.detail); sp.querySelectorAll("[data-speaker]").forEach((b) => (b.onclick = () => renameSpeaker(b.dataset.speaker))); }
          box.querySelectorAll("[data-speaker]").forEach((b) => (b.onclick = () => renameSpeaker(b.dataset.speaker)));
          if (atBottom) v.scrollTop = v.scrollHeight;
        }
      }
    }
  });
  listen("levels", (l) => { S.levels = l; });
  listen("mic-test", (t) => { S.testLevel = t.level; S.testSilent = !!t.digital_silence && (S.testSilent || t.level === 0); });
  listen("rec-state", (r) => {
    const changed = r.recording !== S.rec.recording || r.paused !== S.rec.paused || r.meeting_id !== S.rec.meeting_id || r.system_audio !== S.rec.system_audio || !!r.auto !== !!S.rec.auto;
    if (!r.recording) S.autoTitle = null;
    const finishing = r.finishing;
    S.rec = r;
    if (finishing) toast("Recording stopped");
    if (changed) { render(true); renderSidebar(); renderSysWarn(); if (!r.recording) { S.sysWarn = false; loadMeetings(); } }
    else { const t = $("#dockTimer"); if (t) t.textContent = fmtDur(r.elapsed_ms || 0); }
  });
  listen("prompt", (p) => { S.prompt = p; renderBanner(); });
  listen("auto-recording", (p) => { S.autoTitle = p.title; S.autoHidden = null; renderBanner(); });
  // Changed outside the window (e.g. "Turn off auto-record" in the menu bar).
  listen("settings-changed", async () => { S.settings = await invoke("get_settings"); renderBadge(); renderBanner(true); if (S.view === "settings") render(); });
  listen("open-meeting", (p) => { loadMeetings(); openMeeting(p.id, "notes").catch(fail); });
  listen("meeting-updated", (p) => { loadMeetings(); loadFolders(); if (p.id === S.meetingId) refreshDetail(); });
  listen("busy", (p) => { if (S.detail && p.id === S.meetingId) { S.detail.busy = p.what; const bz = $("#busyBox"); if (bz) bz.innerHTML = busyHtml(S.detail); const rg = $("#regen"); if (rg) rg.disabled = !!p.what; } });
  listen("warning", (m) => {
    if (/participants' audio|Screen Recording/i.test(m)) { S.sysWarn = "system"; renderSysWarn(); return; }
    if (/^MIC_(SILENT|QUIET)/.test(m)) { diagnoseMic(/^MIC_SILENT/.test(m) ? "silent" : "quiet"); return; }
    const lost = /^CAPTURE_LOST:(mic|system)/.exec(m), back = /^CAPTURE_RESTORED:(mic|system)/.exec(m);
    if (lost) { toast(lost[1] === "mic" ? "Lost the microphone. Reconnecting…" : "Lost call audio. Reconnecting…", 8000); return; }
    if (back) { toast(back[1] === "mic" ? "Microphone is back" : "Call audio is back"); return; }
    toast(m, 5000);
  });
  listen("events-updated", () => loadEvents());
  listen("sync-status", () => { if (S.view === "settings") loadSync(); });
  listen("engine", (e) => { if (!e.ready && e.error) toast(e.error, 5000); else if (e.ready) toast("Speech model ready"); });
  listen("download-progress", (p) => {
    S.progress[p.id] = { ...(S.progress[p.id] || {}), ...p };
    if (p.finished) {
      if (p.error && p.error !== "cancelled") toast(`Download failed: ${p.error}`, 5000);
      else if (!p.error && p.id !== "speakers") toast("Model ready");
      delete S.progress[p.id];
      loadModels().then(async () => { S.settings = await invoke("get_settings"); rerenderModels(); });
      return;
    }
    const card = document.querySelector(`[data-model="${p.id}"] .progress i`);
    const pct = p.total ? Math.round((p.done / p.total) * 100) : 0;
    if (card) { card.style.width = pct + "%"; card.parentElement.nextElementSibling.textContent = pct + "%"; }
    else if (S.models && !S.models.downloading.includes(p.id) && p.id !== "speakers") { S.models.downloading.push(p.id); rerenderModels(); }
    if (S.view === "onboarding") { clearTimeout(rerenderModels._t); rerenderModels._t = setTimeout(rerenderModels, 250); }
  });
}

// ------------------------------------------------------------------ boot
(async function boot() {
  try {
    wireEvents();
    [S.settings, S.perms, S.prompt] = await Promise.all([invoke("get_settings"), invoke("permissions"), invoke("current_prompt")]);
    await Promise.all([loadMeetings(), loadModels(), loadEvents(), loadFolders()]);
    if (!S.settings.onboarded) { S.view = "onboarding"; S.vaults = await invoke("obsidian_vaults").catch(() => []); }
    render();
    renderBadge();
    if (S.settings.onboarded && ["microphone", "system_audio", "calendar"].some((k) => S.perms[k] !== "granted")) openPermsSheet();
    invoke("rec_state");
    loadDevices();
    loadProviders(false).then(() => { renderBadge(); if (S.view !== "note") render(); });
    setInterval(paintLevels, 90);
    setInterval(() => { if (S.view === "home" && document.activeElement?.id !== "askAll") renderHome($("#view")); }, 60_000);
  } catch (e) {
    $("#view").innerHTML = `<div class="page"><h1 class="hello">Something went wrong</h1><p>${esc(e)}</p></div>`;
  }
})();
