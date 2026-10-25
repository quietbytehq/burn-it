pub mod drives;
pub mod iso;
pub mod platform;
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

/// Linux: the GUI runs as the normal user, so the actual burn runs in a
/// privileged copy of this program started through pkexec (password prompt).
#[cfg(target_os = "linux")]
fn run_job(job: &Job, emit: &dyn Fn(Progress)) -> anyhow::Result<()> {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    if unsafe { libc::geteuid() } == 0 {
        let mut f = |p: Progress| emit(p);
        return writer::burn(job, &mut f);
    }
    let exe = std::env::current_exe()?;
    if std::env::var_os("APPIMAGE").is_some() {
        anyhow::bail!("The AppImage build cannot ask for admin rights yet. Please install the .deb or .rpm package.");
    }
    emit(Progress::new("prepare", "Waiting for your password…", 0, 1));
    let mut child = Command::new("pkexec")
        .arg(&exe)
        .arg("--helper")
        .arg(serde_json::to_string(job)?)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Could not start pkexec ({}). Install polkit / pkexec.", e))?;

    let stdout = child.stdout.take().unwrap();
    let mut finished = false;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(json) = line.strip_prefix("BURNIT:") {
            if let Ok(p) = serde_json::from_str::<Progress>(json) {
                if p.stage == "done" || p.stage == "error" {
                    finished = true;
                }
                emit(p);
            }
        }
    }
    let status = child.wait()?;
    if !finished {
        match status.code() {
            Some(126) | Some(127) => anyhow::bail!("Admin permission was not granted. Nothing was written."),
            _ => {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    use std::io::Read;
                    let _ = e.read_to_string(&mut err);
                }
                anyhow::bail!("The helper stopped unexpectedly. {}", err.trim());
            }
        }
    }
    Ok(())
}

/// Windows: the whole app already runs as Administrator (see the app manifest).
#[cfg(not(target_os = "linux"))]
fn run_job(job: &Job, emit: &dyn Fn(Progress)) -> anyhow::Result<()> {
    let mut f = |p: Progress| emit(p);
    writer::burn(job, &mut f)
}

/// Entry point for `burn-it --helper '<job json>'` (runs as root, prints progress lines).
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
