//! Recursive file copy with progress, used for Windows installer USBs.

use super::{Progress, Reporter};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const FAT32_MAX: u64 = 4 * 1024 * 1024 * 1024 - 1;

pub struct Plan {
    pub files: Vec<(PathBuf, u64)>,
    pub dirs: Vec<PathBuf>,
    /// Files that are too big for FAT32 and must be split (relative paths).
    pub too_big: Vec<(PathBuf, u64)>,
    pub total: u64,
}

pub fn plan(src: &Path) -> anyhow::Result<Plan> {
    let mut p = Plan { files: vec![], dirs: vec![], too_big: vec![], total: 0 };
    walk(src, src, &mut p)?;
    Ok(p)
}

fn walk(root: &Path, dir: &Path, p: &mut Plan) -> anyhow::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let rel = path.strip_prefix(root)?.to_path_buf();
        let md = e.metadata()?;
        if md.is_dir() {
            p.dirs.push(rel);
            walk(root, &path, p)?;
        } else if md.is_file() {
            p.total += md.len();
            if md.len() > FAT32_MAX {
                p.too_big.push((rel, md.len()));
            } else {
                p.files.push((rel, md.len()));
            }
        }
    }
    Ok(())
}

/// Copies every planned file (except too-big ones). `done_base` lets the caller
/// account for bytes handled elsewhere (e.g. the split step).
pub fn copy_all(src: &Path, dst: &Path, plan: &Plan, report: Reporter) -> anyhow::Result<u64> {
    for d in &plan.dirs {
        fs::create_dir_all(dst.join(d))?;
    }
    let mut buf = vec![0u8; 4 * 1024 * 1024];
    let mut done = 0u64;
    for (rel, _len) in &plan.files {
        let mut from = File::open(src.join(rel))?;
        let target = dst.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut to = File::create(&target)
            .map_err(|e| anyhow::anyhow!("Cannot create {}: {}", target.display(), e))?;
        let name = rel.to_string_lossy().replace('\\', "/");
        let mut since_sync = 0usize;
        loop {
            let n = from.read(&mut buf)?;
            if n == 0 {
                break;
            }
            to.write_all(&buf[..n])?;
            done += n as u64;
            since_sync += n;
            if since_sync >= 64 * 1024 * 1024 {
                to.sync_data()?;
                since_sync = 0;
            }
            report(Progress::new("copy", format!("Copying {}", name), done, plan.total));
        }
        // flush each file so the progress bar matches what is really on the stick
        to.sync_all()?;
    }
    Ok(done)
}
