pub mod drives;
pub mod iso;
pub mod platform;
#[cfg(target_os = "linux")]
pub mod udisks;
pub mod writer;

use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter};
use writer::{Job, Progress};

static BUSY: AtomicBool = AtomicBool::new(false);

#[tauri::command]
fn list_drives() -> Result<Vec<drives::Drive>, String> {
    drives::list().map_err(|e| e.to_string())
}

#[tauri::command]
fn inspect_iso(path: String) -> Result<iso::IsoInfo, String> {
    iso::inspect(std::path::Path::new(&path)).map_err(|e| e.to_string())
}

#[tauri::command]
fn platform_name() -> &'static str {
    std::env::consts::OS
}

#[tauri::command]
fn is_busy() -> bool {
    BUSY.load(Ordering::SeqCst)
}

#[tauri::command]
fn start_burn(app: AppHandle, job: Job) -> Result<(), String> {
    if BUSY.swap(true, Ordering::SeqCst) {
        return Err("A burn is already running.".into());
    }
    std::thread::spawn(move || {
        let emit = |p: Progress| {
            let _ = app.emit("burn-progress", p);
        };
        let result = run_job(&job, &emit);
        if let Err(e) = result {
            emit(Progress::new("error", e.to_string(), 0, 0));
        }
        BUSY.store(false, Ordering::SeqCst);
    });
    Ok(())
}

/// Linux: runs in-process as the normal user — UDisks2 asks for permission when needed.
/// Windows: the whole app already runs as Administrator (see the app manifest).
fn run_job(job: &Job, emit: &dyn Fn(Progress)) -> anyhow::Result<()> {
    let mut f = |p: Progress| emit(p);
    writer::burn(job, &mut f)
}

/// Command-line mode: `burn-it --helper '<job json>'` burns without the GUI and prints progress lines.
pub fn run_helper(job_json: &str) -> i32 {
    use std::io::Write;
    let print = |p: &Progress| {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "BURNIT:{}", serde_json::to_string(p).unwrap_or_default());
        let _ = out.flush();
    };
    let job: Job = match serde_json::from_str(job_json) {
        Ok(j) => j,
        Err(e) => {
            print(&Progress::new("error", format!("Bad job: {}", e), 0, 0));
            return 2;
        }
    };
    // throttle progress output a little
    let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
    let mut f = |p: Progress| {
        let important = p.stage != "write" && p.stage != "copy" && p.stage != "verify" && p.stage != "split";
        if important || last.elapsed().as_millis() >= 150 || p.done == p.total {
            print(&p);
            last = std::time::Instant::now();
        }
    };
    match writer::burn(&job, &mut f) {
        Ok(()) => 0,
        Err(e) => {
            print(&Progress::new("error", e.to_string(), 0, 0));
            1
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![list_drives, inspect_iso, platform_name, is_busy, start_burn])
        .run(tauri::generate_context!())
        .expect("error while running Burn It!");
}
