const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const dialog = window.__TAURI__.dialog;

const $ = (id) => document.getElementById(id);
const state = { iso: null, drives: [], drive: null };

function human(bytes) {
  const u = ["B", "KB", "MB", "GB", "TB"];
  let v = bytes, i = 0;
  while (v >= 1000 && i < u.length - 1) { v /= 1000; i++; }
  return i === 0 ? `${bytes} B` : `${v.toFixed(1)} ${u[i]}`;
}

function show(view) {
  for (const id of ["setup", "working", "finished"]) $(id).hidden = id !== view;
}

function effectiveMode() {
  const m = $("mode").value;
  if (m !== "auto") return m;
  return state.iso ? state.iso.suggested_mode : "raw";
}

function updateBurnButton() {
  $("burn").disabled = !(state.iso && state.drive);
  const note = $("iso-note");
  if (state.iso && state.drive && effectiveMode() === "raw" && state.iso.size > state.drive.size) {
    note.hidden = false;
    note.className = "note warn";
    note.textContent = "This USB drive is too small for the selected image.";
    $("burn").disabled = true;
  } else if (state.iso) {
    note.hidden = false;
    note.className = state.iso.kind === "unknown" ? "note warn" : "note";
    note.textContent = state.iso.note;
  }
}

async function pickIso() {
  const path = await dialog.open({
    multiple: false,
    directory: false,
    filters: [{ name: "Disk images", extensions: ["iso", "img", "ISO", "IMG"] }, { name: "All files", extensions: ["*"] }],
  });
  if (!path) return;
  try {
    const info = await invoke("inspect_iso", { path });
    state.iso = info;
    $("iso-name").textContent = info.file_name;
    $("iso-name").classList.add("set");
    $("iso-name").title = info.path;
    $("iso-meta").hidden = false;
    const kind = $("iso-kind");
    kind.className = `badge ${info.kind}`;
    kind.textContent = { linux: "Linux / bootable image", windows: "Windows installer", unknown: "Unknown image" }[info.kind];
    $("iso-size").textContent = human(info.size) + (info.volume_label ? ` · ${info.volume_label}` : "");
  } catch (e) {
    state.iso = null;
    $("iso-name").textContent = "No ISO selected";
    $("iso-name").classList.remove("set");
    $("iso-meta").hidden = true;
    await dialog.message(String(e), { title: "Can't use this file", kind: "error" });
  }
  updateBurnButton();
}

async function refreshDrives() {
  const sel = $("drive-select");
  const previous = state.drive?.device;
  try {
    state.drives = await invoke("list_drives");
  } catch (e) {
    state.drives = [];
    $("drive-hint").textContent = String(e);
  }
  sel.innerHTML = "";
  if (state.drives.length === 0) {
    sel.innerHTML = "<option>No USB drive found — plug one in</option>";
    sel.disabled = true;
    state.drive = null;
  } else {
    for (const d of state.drives) {
      const o = document.createElement("option");
      o.value = d.device;
      const where = d.number != null ? `Disk ${d.number}` : d.device;
      o.textContent = `${d.name} — ${human(d.size)} (${where})`;
      sel.appendChild(o);
    }
    sel.disabled = false;
    const keep = state.drives.find((d) => d.device === previous);
    state.drive = keep || state.drives[0];
    sel.value = state.drive.device;
  }
  updateBurnButton();
}

async function burn() {
  const d = state.drive, iso = state.iso;
  const mode = effectiveMode();
  const ok = await dialog.ask(
    `Everything on "${d.name}" (${human(d.size)}) will be permanently erased.\n\nWrite ${iso.file_name} to it?`,
    { title: "Erase this USB drive?", kind: "warning", okLabel: "Erase and burn", cancelLabel: "Cancel" }
  );
  if (!ok) return;
  show("working");
  setProgress({ stage: "prepare", message: "Starting…", done: 0, total: 0 });
  started = performance.now();
  lastSample = null;
  try {
    await invoke("start_burn", {
      job: { iso: iso.path, device: d.device, number: d.number, mode, verify: $("verify").checked },
    });
  } catch (e) {
    finish(false, String(e));
  }
}

const TITLES = {
  prepare: "Preparing…", format: "Formatting…", write: "Writing…", copy: "Copying files…",
  split: "Splitting Windows image…", verify: "Verifying…", flush: "Finishing up…",
};
let started = 0, lastSample = null;

function setProgress(p) {
  $("stage-title").textContent = TITLES[p.stage] || p.stage;
  $("stage-msg").textContent = p.message;
  const fill = $("bar-fill");
  if (p.total > 1) {
    const pct = Math.min(100, (p.done / p.total) * 100);
    fill.classList.remove("indeterminate");
    fill.style.width = pct.toFixed(1) + "%";
    $("pct").textContent = Math.floor(pct) + "%";
    const now = performance.now();
    if (lastSample && lastSample.stage === p.stage && now - lastSample.t > 800) {
      const rate = (p.done - lastSample.done) / ((now - lastSample.t) / 1000);
      if (rate > 0) {
        const left = (p.total - p.done) / rate;
        $("speed").textContent = `${human(rate)}/s · ${left > 90 ? Math.round(left / 60) + " min" : Math.round(left) + " s"} left`;
      }
      lastSample = { stage: p.stage, done: p.done, t: now };
    } else if (!lastSample || lastSample.stage !== p.stage) {
      lastSample = { stage: p.stage, done: p.done, t: now };
    }
  } else {
    fill.classList.add("indeterminate");
    $("pct").textContent = "";
    $("speed").textContent = "";
  }
}

function finish(ok, msg) {
  show("finished");
  $("result-icon").textContent = ok ? "✓" : "!";
  $("result-icon").className = ok ? "big-icon" : "big-icon err";
  $("result-title").textContent = ok ? "Your USB drive is ready" : "Something went wrong";
  const mins = Math.max(1, Math.round((performance.now() - started) / 60000));
  $("result-msg").textContent = ok ? `${msg}\nTook about ${mins} min.` : msg;
}

listen("burn-progress", (ev) => {
  const p = ev.payload;
  if (p.stage === "done") finish(true, p.message);
  else if (p.stage === "error") finish(false, p.message);
  else setProgress(p);
});

$("pick-iso").addEventListener("click", pickIso);
$("refresh").addEventListener("click", refreshDrives);
$("drive-select").addEventListener("change", (e) => {
  state.drive = state.drives.find((d) => d.device === e.target.value) || null;
  updateBurnButton();
});
$("mode").addEventListener("change", updateBurnButton);
$("burn").addEventListener("click", burn);
$("again").addEventListener("click", () => { show("setup"); refreshDrives(); });

// keep the drive list fresh while the user is on the setup screen
setInterval(() => { if (!$("setup").hidden && document.activeElement !== $("drive-select")) refreshDrives(); }, 4000);
refreshDrives();

// splash: remove from the page once its fade-out finishes (or when clicked)
(() => {
  const sp = $("splash");
  if (!sp) return;
  const hide = () => sp.classList.add("gone");
  sp.addEventListener("animationend", (e) => { if (e.animationName === "sp-out") hide(); });
  sp.addEventListener("click", hide);
  setTimeout(hide, 4000); // safety net
})();
