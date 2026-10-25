//! Byte-for-byte image writing (Linux ISOs and other hybrid images).

use super::{human, Job, Progress, Reporter};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};

const CHUNK: usize = 4 * 1024 * 1024;
const SECTOR: usize = 512;

pub fn write(job: &Job, report: Reporter) -> anyhow::Result<()> {
    let mut iso = File::open(&job.iso)?;
    let total = iso.metadata()?.len();

    report(Progress::new("prepare", "Preparing the USB drive…", 0, 1));
    let mut dev = open_device(job)?;

    // Windows mounts a volume as soon as it sees a partition table appear. Writing the
    // first MiB (which holds the partition table) last keeps Windows out of the way.
    let head_len = (1024 * 1024u64).min(total);
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    let mut since_sync = 0u64;

    report(Progress::new("write", format!("Writing {}…", human(total)), 0, total));
    iso.seek(SeekFrom::Start(head_len))?;
    dev.seek(SeekFrom::Start(head_len))?;
    loop {
        let n = read_full(&mut iso, &mut buf)?;
        if n == 0 {
            break;
        }
        let padded = pad_len(n);
        buf[n..padded].fill(0);
        dev.write_all(&buf[..padded])?;
        done += n as u64;
        since_sync += n as u64;
        if since_sync >= 64 * 1024 * 1024 {
            // keep the progress bar honest instead of filling the page cache
            dev.sync_data()?;
            since_sync = 0;
        }
        report(Progress::new("write", "Writing image…", done, total));
    }
    // now the head
    iso.seek(SeekFrom::Start(0))?;
    dev.seek(SeekFrom::Start(0))?;
    let mut head = vec![0u8; pad_len(head_len as usize)];
    read_full(&mut iso, &mut head[..head_len as usize])?;
    dev.write_all(&head)?;
    done += head_len;
    report(Progress::new("write", "Writing image…", done, total));

    report(Progress::new("flush", "Finishing write (flushing to the drive)…", 0, 1));
    dev.sync_all()?;
    drop_cache(&dev);

    if job.verify {
        verify(job, total, report)?;
    }

    finish_device(job)?;
    Ok(())
}

fn verify(job: &Job, total: u64, report: Reporter) -> anyhow::Result<()> {
    let mut iso = File::open(&job.iso)?;
    let mut dev = OpenOptions::new().read(true).open(&job.device)?;
    let mut a = vec![0u8; CHUNK];
    let mut b = vec![0u8; CHUNK];
    let mut done = 0u64;
    report(Progress::new("verify", "Verifying…", 0, total));
    loop {
        let n = read_full(&mut iso, &mut a)?;
        if n == 0 {
            break;
        }
        let padded = pad_len(n);
        read_full(&mut dev, &mut b[..padded])?;
        if a[..n] != b[..n] {
            anyhow::bail!("Verification failed at {} — the USB drive may be faulty or fake-capacity.", human(done));
        }
        done += n as u64;
        report(Progress::new("verify", "Verifying…", done, total));
    }
    Ok(())
}

fn pad_len(n: usize) -> usize {
    n.div_ceil(SECTOR) * SECTOR
}

fn read_full(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

#[cfg(target_os = "linux")]
fn open_device(job: &Job) -> anyhow::Result<File> {
    super::windows_iso::unmount_all(&job.device)?;
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&job.device)
        .map_err(|e| anyhow::anyhow!("Cannot open {} for writing: {}", job.device, e))?;
    Ok(f)
}

#[cfg(windows)]
fn open_device(job: &Job) -> anyhow::Result<File> {
    let n = job.number.ok_or_else(|| anyhow::anyhow!("Missing disk number"))?;
    crate::platform::diskpart(&[format!("select disk {}", n), "attributes disk clear readonly noerr".into(), "clean".into(), "rescan".into()])?;
    std::thread::sleep(std::time::Duration::from_secs(2));
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&job.device)
        .map_err(|e| anyhow::anyhow!("Cannot open disk {} for writing: {}", n, e))?;
    Ok(f)
}

#[cfg(unix)]
fn drop_cache(f: &File) {
    use std::os::unix::io::AsRawFd;
    unsafe {
        libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
    }
}

#[cfg(not(unix))]
fn drop_cache(_f: &File) {}

#[cfg(target_os = "linux")]
fn finish_device(job: &Job) -> anyhow::Result<()> {
    let _ = crate::platform::run("blockdev", &["--rereadpt", &job.device]);
    Ok(())
}

#[cfg(windows)]
fn finish_device(job: &Job) -> anyhow::Result<()> {
    if let Some(n) = job.number {
        let _ = crate::platform::diskpart(&[format!("select disk {}", n), "rescan".into()]);
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", windows)))]
fn finish_device(_job: &Job) -> anyhow::Result<()> {
    Ok(())
}
