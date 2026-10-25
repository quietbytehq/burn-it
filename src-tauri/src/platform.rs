//! Small helpers for running system tools quietly (no console windows on Windows).

use std::process::{Command, Output};

pub fn command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// Runs a command and fails with a readable message if it exits non-zero.
pub fn run(program: &str, args: &[&str]) -> anyhow::Result<Output> {
    let out = command(program)
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!("Could not start '{}': {}. Is it installed?", program, e))?;
    if !out.status.success() {
        anyhow::bail!(
            "'{} {}' failed: {}{}",
            program,
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim(),
            String::from_utf8_lossy(&out.stdout).trim()
        );
    }
    Ok(out)
}

#[cfg(windows)]
pub fn powershell(script: &str) -> anyhow::Result<String> {
    let out = command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .output()?;
    if !out.status.success() {
        anyhow::bail!("PowerShell error: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Quotes a string for use inside a single-quoted PowerShell literal.
#[cfg(windows)]
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[cfg(windows)]
pub fn diskpart(lines: &[String]) -> anyhow::Result<String> {
    let path = std::env::temp_dir().join(format!("burn-it-diskpart-{}.txt", std::process::id()));
    std::fs::write(&path, lines.join("\r\n") + "\r\n")?;
    let out = command("diskpart.exe").arg("/s").arg(&path).output();
    let _ = std::fs::remove_file(&path);
    let out = out?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if !out.status.success() {
        anyhow::bail!("DiskPart reported an error:\n{}", text.trim());
    }
    Ok(text)
}
