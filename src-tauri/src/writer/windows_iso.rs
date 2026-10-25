//! Windows installer USBs: MBR + one FAT32 partition, files copied, install.wim split.
//! FAT32 + MBR boots on every UEFI PC (Secure Boot on or off) without extra tricks.

use super::copy;
use super::{Job, Progress, Reporter};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const LABEL: &str = "BURNIT";

/// Splits an over-sized .wim into .swm parts of < 4 GB, reporting progress.
fn split_wim(src: &Path, dst_dir: &Path, rel: &Path, size: u64, base: u64, total: u64, report: Reporter) -> anyhow::Result<()> {
    let ext = rel.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext != "wim" {
        anyhow::bail!(
            "{} is larger than 4 GB and cannot be split (only .wim files can). Try a different Windows ISO.",
            rel.display()
        );
    }
    let swm = dst_dir.join(rel).with_extension("swm");
    if let Some(p) = swm.parent() {
        std::fs::create_dir_all(p)?;
    }
    let src_s = src.to_string_lossy().to_string();
    let swm_s = swm.to_string_lossy().to_string();

    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = crate::platform::command("wimlib-imagex");
        c.args(["split", &src_s, &swm_s, "3800"]);
        c
    };
    #[cfg(windows)]
    let mut cmd = {
        let mut c = crate::platform::command("dism.exe");
        c.arg("/Split-Image")
            .arg(format!("/ImageFile:{}", src_s))
            .arg(format!("/SWMFile:{}", swm_s))
            .arg("/FileSize:3800");
        c
    };

    report(Progress::new("split", "Splitting the Windows image (it is bigger than 4 GB)…", base, total));
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Cannot run the WIM splitter: {}. On Linux install 'wimtools' (Debian/Ubuntu), 'wimlib' (Arch) or 'wimlib-utils' (Fedora).", e))?;
    let mut out = child.stdout.take().unwrap();
    let mut buf = [0u8; 512];
    let mut line = Vec::new();
    loop {
        let n = out.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            if b == b'\r' || b == b'\n' {
                if let Some(pct) = parse_percent(&String::from_utf8_lossy(&line)) {
                    let done = base + (size as f64 * pct / 100.0) as u64;
                    report(Progress::new("split", "Splitting the Windows image (it is bigger than 4 GB)…", done, total));
                }
                line.clear();
            } else {
                line.push(b);
            }
        }
    }
    let status = child.wait()?;
    if !status.success() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        anyhow::bail!("Splitting the Windows image failed: {}", err.trim());
    }
    // A single file inside the WIM can't be split; make sure every part fits FAT32.
    if let (Some(dir), Some(stem)) = (swm.parent(), swm.file_stem()) {
        let stem = stem.to_string_lossy().to_lowercase();
        for e in std::fs::read_dir(dir)?.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.starts_with(&stem) && name.ends_with(".swm") && e.metadata()?.len() > copy::FAT32_MAX {
                anyhow::bail!("This Windows image contains a single file bigger than 4 GB, so it cannot be put on a FAT32 drive.");
            }
        }
    }
    Ok(())
}

/// Finds the last "NN%" / "NN.N%" in a line of tool output.
pub fn parse_percent(s: &str) -> Option<f64> {
    let idx = s.rfind('%')?;
    let head = &s[..idx];
    let start = head
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_ascii_digit() || *c == '.')
        .last()
        .map(|(i, _)| i)?;
    head[start..].parse::<f64>().ok().filter(|v| (0.0..=100.0).contains(v))
}

fn looks_like_windows(root: &Path) -> bool {
    let has = |name: &str| {
        std::fs::read_dir(root)
            .map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name)))
            .unwrap_or(false)
    };
    has("sources") && (has("efi") || has("bootmgr"))
}

/// Copies the mounted ISO at `src` onto the formatted, mounted USB at `dst`.
fn populate(src: &Path, dst: &Path, report: Reporter) -> anyhow::Result<()> {
    if !looks_like_windows(src) {
        anyhow::bail!("This ISO does not look like a Windows installer (no 'sources' and 'efi' folders). Try the 'Linux / raw' mode instead.");
    }
    if !std::fs::read_dir(src)?.flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("efi")) {
        anyhow::bail!("This Windows ISO has no UEFI boot files; Burn It! only makes UEFI-bootable Windows USBs.");
    }
    let plan = copy::plan(src)?;
    let free = free_space(dst).unwrap_or(u64::MAX);
    if plan.total > free {
        anyhow::bail!("Not enough space on the USB drive.");
    }
    let mut done = copy::copy_all(src, dst, &plan, report)?;
    for (rel, size) in &plan.too_big {
        split_wim(&src.join(rel), dst, rel, *size, done, plan.total, report)?;
        done += size;
    }
    report(Progress::new("flush", "Finishing write (flushing to the drive)…", done, plan.total));
    Ok(())
}

// ---------------------------------------------------------------- Linux

#[cfg(target_os = "linux")]
fn free_space(p: &Path) -> Option<u64> {
    use std::ffi::CString;
    let c = CString::new(p.to_string_lossy().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    Some(s.f_bavail as u64 * s.f_frsize as u64)
}

#[cfg(target_os = "linux")]
pub fn partition_path(dev: &str, n: u32) -> String {
    if dev.chars().last().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        format!("{}p{}", dev, n)
    } else {
        format!("{}{}", dev, n)
    }
}

/// Unmounts every mounted file system on `dev` and its partitions.
#[cfg(target_os = "linux")]
pub fn unmount_all(dev: &str) -> anyhow::Result<()> {
    let out = crate::platform::run("lsblk", &["-nrpo", "NAME,MOUNTPOINT", dev])?;
    for l in String::from_utf8_lossy(&out.stdout).lines() {
        let mut it = l.splitn(2, ' ');
        let (name, mp) = (it.next().unwrap_or(""), it.next().unwrap_or("").trim());
        if mp.is_empty() {
            continue;
        }
        if mp == "[SWAP]" {
            let _ = crate::platform::run("swapoff", &[name]);
            continue;
        }
        let mp = mp.replace("\\x20", " ");
        if crate::platform::run("umount", &[&mp]).is_err() {
            crate::platform::run("umount", &["-l", &mp])
                .map_err(|_| anyhow::anyhow!("Could not unmount {}. Close any open files on the USB drive and try again.", mp))?;
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
struct EmptyDirs(Vec<PathBuf>);

#[cfg(target_os = "linux")]
impl Drop for EmptyDirs {
    fn drop(&mut self) {
        for d in &self.0 {
            let _ = std::fs::remove_dir(d); // only ever removes empty folders
        }
    }
}

#[cfg(target_os = "linux")]
struct Mounted(PathBuf);

#[cfg(target_os = "linux")]
impl Drop for Mounted {
    fn drop(&mut self) {
        let p = self.0.to_string_lossy().to_string();
        if crate::platform::run("umount", &[&p]).is_err() {
            let _ = crate::platform::run("umount", &["-l", &p]);
        }
        let _ = std::fs::remove_dir(&self.0);
    }
}

#[cfg(target_os = "linux")]
pub fn write(job: &Job, report: Reporter) -> anyhow::Result<()> {
    use crate::platform::run;
    let dev = job.device.as_str();

    report(Progress::new("prepare", "Unmounting the USB drive…", 0, 1));
    unmount_all(dev)?;

    report(Progress::new("format", "Creating partition table…", 0, 1));
    run("wipefs", &["-a", dev])?;
    run("parted", &["-s", "-a", "optimal", dev, "mklabel", "msdos", "mkpart", "primary", "fat32", "1MiB", "100%", "set", "1", "boot", "on"])?;
    let _ = run("partprobe", &[dev]);
    let _ = run("udevadm", &["settle"]);
    let part = partition_path(dev, 1);
    let mut tries = 0;
    while !Path::new(&part).exists() {
        tries += 1;
        if tries > 50 {
            anyhow::bail!("The new partition {} did not appear.", part);
        }
        let _ = run("blockdev", &["--rereadpt", dev]);
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    // desktop auto-mounters can grab the new partition; make sure it is free
    let _ = unmount_all(dev);
    report(Progress::new("format", "Formatting FAT32…", 0, 1));
    run("mkfs.vfat", &["-F", "32", "-n", LABEL, &part])?;

    let base = PathBuf::from(format!("/run/burn-it-{}", std::process::id()));
    let iso_mp = base.join("iso");
    let usb_mp = base.join("usb");
    std::fs::create_dir_all(&iso_mp)?;
    std::fs::create_dir_all(&usb_mp)?;
    // dropped last: removes the (by then unmounted, empty) work folders even on errors
    let _dirs = EmptyDirs(vec![iso_mp.clone(), usb_mp.clone(), base.clone()]);

    report(Progress::new("prepare", "Opening the ISO…", 0, 1));
    run("mount", &["-o", "loop,ro", &job.iso, &iso_mp.to_string_lossy()])
        .map_err(|e| anyhow::anyhow!("Could not open the ISO (is the 'udf' kernel module available?): {}", e))?;
    let iso_guard = Mounted(iso_mp.clone());
    run("mount", &["-t", "vfat", "-o", "flush", &part, &usb_mp.to_string_lossy()])?;
    let usb_guard = Mounted(usb_mp.clone());

    populate(&iso_mp, &usb_mp, report)?;

    let _ = run("sync", &[]);
    drop(usb_guard);
    drop(iso_guard);
    Ok(())
}

// ---------------------------------------------------------------- Windows

#[cfg(windows)]
fn free_space(p: &Path) -> Option<u64> {
    let letter = p.to_string_lossy().chars().next()?;
    let out = crate::platform::powershell(&format!("(Get-Volume -DriveLetter {}).SizeRemaining", letter)).ok()?;
    out.trim().parse().ok()
}

#[cfg(windows)]
fn free_letter() -> anyhow::Result<char> {
    for c in ('D'..='Z').rev() {
        if !Path::new(&format!("{}:\\", c)).exists() {
            return Ok(c);
        }
    }
    anyhow::bail!("No free drive letter is available.")
}

#[cfg(windows)]
struct MountedIso(String);

#[cfg(windows)]
impl Drop for MountedIso {
    fn drop(&mut self) {
        let _ = crate::platform::powershell(&format!(
            "Dismount-DiskImage -ImagePath {} | Out-Null",
            crate::platform::ps_quote(&self.0)
        ));
    }
}

#[cfg(windows)]
pub fn write(job: &Job, report: Reporter) -> anyhow::Result<()> {
    use crate::platform::{diskpart, powershell, ps_quote};
    let n = job.number.ok_or_else(|| anyhow::anyhow!("Missing disk number"))?;

    report(Progress::new("prepare", "Opening the ISO…", 0, 1));
    let iso_path = std::fs::canonicalize(&job.iso)?.to_string_lossy().trim_start_matches(r"\\?\").to_string();
    let letter = powershell(&format!(
        "$i = Mount-DiskImage -ImagePath {} -PassThru; ($i | Get-Volume).DriveLetter",
        ps_quote(&iso_path)
    ))?;
    let _iso_guard = MountedIso(iso_path.clone());
    let iso_letter = letter.trim().chars().next().ok_or_else(|| anyhow::anyhow!("Windows did not give the mounted ISO a drive letter."))?;

    report(Progress::new("format", "Formatting the USB drive (FAT32)…", 0, 1));
    let size_out = powershell(&format!("(Get-Disk -Number {}).Size", n))?;
    let size: u64 = size_out.trim().parse().unwrap_or(0);
    let usb_letter = free_letter()?;
    // Windows' own formatter refuses FAT32 above 32 GB, so cap the partition there.
    let create = if size > 32 * 1000 * 1000 * 1000 {
        "create partition primary size=32000".to_string()
    } else {
        "create partition primary".to_string()
    };
    diskpart(&[
        format!("select disk {}", n),
        "attributes disk clear readonly noerr".into(),
        "clean".into(),
        "convert mbr".into(),
        create,
        format!("format fs=fat32 quick label={}", LABEL),
        "active".into(),
        format!("assign letter={}", usb_letter),
    ])?;
    std::thread::sleep(std::time::Duration::from_secs(2));

    let src = PathBuf::from(format!("{}:\\", iso_letter));
    let dst = PathBuf::from(format!("{}:\\", usb_letter));
    populate(&src, &dst, report)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Copies a Windows-style tree (BURNIT_TEST_SRC) into BURNIT_TEST_DST, splitting big WIMs.
    #[test]
    #[ignore]
    fn populate_tree() {
        let src = std::env::var("BURNIT_TEST_SRC").unwrap();
        let dst = std::env::var("BURNIT_TEST_DST").unwrap();
        let mut last = Progress::default();
        let mut stages = std::collections::BTreeSet::new();
        let mut f = |p: Progress| {
            stages.insert(p.stage.clone());
            last = p;
        };
        populate(Path::new(&src), Path::new(&dst), &mut f).unwrap();
        println!("stages={:?} last={:?}", stages, last);
        assert!(stages.contains("split"));
    }

    #[test]
    fn percent_parsing() {
        assert_eq!(parse_percent("Splitting WIM: 100 MiB of 5000 MiB (2%) written, part 1 of 2"), Some(2.0));
        assert_eq!(parse_percent("[==========                 18.5%                          ]"), Some(18.5));
        assert_eq!(parse_percent("no number here"), None);
    }
}
