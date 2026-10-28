//! Linux disk access through UDisks2 (the system disk service used by GNOME Disks).
//!
//! Burn It! never runs as root on Linux. Every privileged step — opening the USB
//! drive for raw writing, creating the partition table, formatting FAT32, attaching
//! the ISO as a loop device and mounting — is a D-Bus request to UDisks2, which asks
//! the user for permission through the desktop's normal polkit password prompt.
//! This is also what lets Burn It! run as a strictly confined snap.

use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const BUS: &str = "org.freedesktop.UDisks2";
const ROOT: &str = "/org/freedesktop/UDisks2";
const MANAGER: &str = "/org/freedesktop/UDisks2/Manager";

pub const I_BLOCK: &str = "org.freedesktop.UDisks2.Block";
pub const I_DRIVE: &str = "org.freedesktop.UDisks2.Drive";
pub const I_FS: &str = "org.freedesktop.UDisks2.Filesystem";
pub const I_PART: &str = "org.freedesktop.UDisks2.Partition";
pub const I_PTABLE: &str = "org.freedesktop.UDisks2.PartitionTable";
pub const I_LOOP: &str = "org.freedesktop.UDisks2.Loop";

pub type Props = HashMap<String, OwnedValue>;
pub type Objects = HashMap<OwnedObjectPath, HashMap<String, Props>>;
type Opts<'a> = HashMap<&'a str, Value<'a>>;

pub struct UDisks {
    conn: Connection,
}

// ------------------------------------------------------------ property helpers

pub fn get_bool(p: &Props, k: &str) -> bool {
    matches!(p.get(k).map(|v| &**v), Some(Value::Bool(true)))
}

pub fn get_u64(p: &Props, k: &str) -> u64 {
    match p.get(k).map(|v| &**v) {
        Some(Value::U64(n)) => *n,
        Some(Value::U32(n)) => *n as u64,
        _ => 0,
    }
}

pub fn get_str(p: &Props, k: &str) -> String {
    match p.get(k).map(|v| &**v) {
        Some(Value::Str(s)) => s.as_str().trim().to_string(),
        _ => String::new(),
    }
}

pub fn get_path(p: &Props, k: &str) -> String {
    match p.get(k).map(|v| &**v) {
        Some(Value::ObjectPath(o)) => o.as_str().to_string(),
        _ => "/".to_string(),
    }
}

fn bytes_of(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    if let Value::Array(a) = v {
        for x in a.iter() {
            if let Value::U8(b) = x {
                out.push(*b);
            }
        }
    }
    while out.last() == Some(&0) {
        out.pop();
    }
    out
}

/// A NUL-terminated byte-string property (`ay`), e.g. Block.Device.
pub fn get_bytestr(p: &Props, k: &str) -> String {
    p.get(k).map(|v| String::from_utf8_lossy(&bytes_of(v)).to_string()).unwrap_or_default()
}

/// An array of byte strings (`aay`), e.g. Filesystem.MountPoints.
pub fn get_bytestr_list(p: &Props, k: &str) -> Vec<String> {
    match p.get(k).map(|v| &**v) {
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| String::from_utf8_lossy(&bytes_of(x)).to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => vec![],
    }
}

fn friendly(e: zbus::Error) -> anyhow::Error {
    let s = e.to_string();
    if s.contains("NotAuthorized") || s.contains("not authorized") {
        anyhow::anyhow!("Permission was not granted. Nothing was written.")
    } else if s.contains("ServiceUnknown") || s.contains("org.freedesktop.UDisks2 was not provided") {
        anyhow::anyhow!("The UDisks2 disk service is not available. Install the 'udisks2' package and try again.")
    } else if s.to_lowercase().contains("apparmor") {
        anyhow::anyhow!("Burn It! is not allowed to use the disk service. Run: sudo snap connect burn-it:udisks2")
    } else {
        anyhow::anyhow!("Disk service error: {}", s)
    }
}

impl UDisks {
    pub fn connect() -> anyhow::Result<Self> {
        // Password prompts and formatting can take a while; don't time out after 25 s.
        let conn = zbus::blocking::connection::Builder::system()
            .map_err(friendly)?
            .method_timeout(Duration::from_secs(15 * 60))
            .build()
            .map_err(friendly)?;
        Ok(UDisks { conn })
    }

    fn proxy<'a>(&'a self, path: &'a str, iface: &'a str) -> anyhow::Result<Proxy<'a>> {
        Proxy::new(&self.conn, BUS, path, iface).map_err(friendly)
    }

    pub fn objects(&self) -> anyhow::Result<Objects> {
        let p = self.proxy(ROOT, "org.freedesktop.DBus.ObjectManager")?;
        p.call("GetManagedObjects", &()).map_err(friendly)
    }

    /// Finds the block-device object for a device node such as /dev/sdb.
    pub fn block_for_device(&self, objs: &Objects, device: &str) -> Option<String> {
        objs.iter().find_map(|(path, ifs)| {
            let b = ifs.get(I_BLOCK)?;
            (get_bytestr(b, "Device") == device || get_bytestr(b, "PreferredDevice") == device)
                .then(|| path.as_str().to_string())
        })
    }

    /// The disk itself plus every partition that lives on it.
    pub fn disk_and_partitions(objs: &Objects, disk: &str) -> Vec<String> {
        let mut out = vec![disk.to_string()];
        for (path, ifs) in objs {
            if let Some(p) = ifs.get(I_PART) {
                if get_path(p, "Table") == disk {
                    out.push(path.as_str().to_string());
                }
            }
        }
        out
    }

    /// Unmounts every mounted file system on the disk and its partitions.
    pub fn unmount_all(&self, disk: &str) -> anyhow::Result<()> {
        let objs = self.objects()?;
        for path in Self::disk_and_partitions(&objs, disk) {
            let Some(ifs) = objs.iter().find(|(p, _)| p.as_str() == path).map(|(_, i)| i) else { continue };
            if let Some(fs) = ifs.get(I_FS) {
                if !get_bytestr_list(fs, "MountPoints").is_empty() {
                    let mut o: Opts = HashMap::new();
                    o.insert("force", Value::Bool(true));
                    self.proxy(&path, I_FS)?
                        .call::<_, _, ()>("Unmount", &(o,))
                        .map_err(|e| anyhow::anyhow!("Could not unmount the USB drive ({}). Close any open files on it and try again.", friendly(e)))?;
                }
            }
        }
        Ok(())
    }

    /// Opens the whole disk for raw reading and writing. UDisks asks the user for permission.
    pub fn open_device(&self, disk: &str) -> anyhow::Result<File> {
        let mut o: Opts = HashMap::new();
        o.insert("flags", Value::I32(libc::O_EXCL | libc::O_CLOEXEC));
        let fd: zbus::zvariant::OwnedFd = self.proxy(disk, I_BLOCK)?.call("OpenDevice", &("rw", o)).map_err(friendly)?;
        let fd: std::os::fd::OwnedFd = fd.into();
        Ok(File::from(fd))
    }

    pub fn rescan(&self, block: &str) {
        let o: Opts = HashMap::new();
        if let Ok(p) = self.proxy(block, I_BLOCK) {
            let _ = p.call::<_, _, ()>("Rescan", &(o,));
        }
    }

    /// Writes a fresh, empty MBR ("dos") partition table to the disk.
    pub fn format_mbr(&self, disk: &str) -> anyhow::Result<()> {
        let mut o: Opts = HashMap::new();
        o.insert("tear-down", Value::Bool(true));
        self.proxy(disk, I_BLOCK)?.call::<_, _, ()>("Format", &("dos", o)).map_err(friendly)
    }

    /// Creates one FAT32 partition spanning the disk (from 1 MiB) and returns its object path.
    pub fn create_fat32(&self, disk: &str, label: &str) -> anyhow::Result<String> {
        let call = |extra_args: bool| -> zbus::Result<OwnedObjectPath> {
            let o: Opts = HashMap::new();
            let mut f: Opts = HashMap::new();
            f.insert("label", Value::from(label));
            if extra_args {
                // force FAT32 even on small sticks (udisks >= 2.10)
                f.insert("mkfs-args", Value::from(vec!["-F", "32"]));
            }
            self.proxy(disk, I_PTABLE)
                .map_err(|e| zbus::Error::Failure(e.to_string()))?
                .call("CreatePartitionAndFormat", &(1024u64 * 1024, 0u64, "0x0c", "", o, "vfat", f))
        };
        let path = match call(true) {
            Ok(p) => p,
            Err(e) if e.to_string().contains("mkfs-args") || e.to_string().contains("Unknown option") => {
                call(false).map_err(friendly)?
            }
            Err(e) => return Err(friendly(e)),
        };
        let part = path.as_str().to_string();
        // mark it active/bootable (0x80) for old BIOS boot menus; harmless for UEFI
        let o: Opts = HashMap::new();
        let _ = self.proxy(&part, I_PART).map(|p| p.call::<_, _, ()>("SetFlags", &(0x80u64, o)));
        Ok(part)
    }

    /// Waits until the object has a Filesystem interface (after formatting or loop setup).
    pub fn wait_for_fs(&self, path: &str, secs: u64) -> anyhow::Result<()> {
        let start = Instant::now();
        loop {
            let objs = self.objects()?;
            if objs.iter().any(|(p, i)| p.as_str() == path && i.contains_key(I_FS)) {
                return Ok(());
            }
            if start.elapsed() > Duration::from_secs(secs) {
                anyhow::bail!("No readable file system was found (timed out waiting for {}).", path);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    pub fn mount(&self, path: &str, readonly: bool) -> anyhow::Result<PathBuf> {
        let mut o: Opts = HashMap::new();
        if readonly {
            o.insert("options", Value::from("ro"));
        }
        let mp: String = self.proxy(path, I_FS)?.call("Mount", &(o,)).map_err(friendly)?;
        Ok(PathBuf::from(mp))
    }

    pub fn unmount(&self, path: &str) -> anyhow::Result<()> {
        let o: Opts = HashMap::new();
        self.proxy(path, I_FS)?.call::<_, _, ()>("Unmount", &(o,)).map_err(friendly)
    }

    /// Attaches an ISO file as a read-only loop device; returns the loop block object path.
    pub fn loop_setup(&self, iso: &File) -> anyhow::Result<String> {
        use std::os::fd::AsFd;
        let mut o: Opts = HashMap::new();
        o.insert("read-only", Value::Bool(true));
        let fd = zbus::zvariant::Fd::from(iso.as_fd());
        let p: OwnedObjectPath = self.proxy(MANAGER, "org.freedesktop.UDisks2.Manager")?
            .call("LoopSetup", &(fd, o))
            .map_err(friendly)?;
        Ok(p.as_str().to_string())
    }

    pub fn loop_delete(&self, path: &str) {
        let o: Opts = HashMap::new();
        if let Ok(p) = self.proxy(path, I_LOOP) {
            let _ = p.call::<_, _, ()>("Delete", &(o,));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Formats BURNIT_TEST_DEV (a loop device) as MBR + FAT32 through UDisks2.
    #[test]
    #[ignore]
    fn format_loop_device() {
        let dev = std::env::var("BURNIT_TEST_DEV").unwrap();
        let ud = UDisks::connect().unwrap();
        let objs = ud.objects().unwrap();
        let disk = ud.block_for_device(&objs, &dev).unwrap();
        ud.unmount_all(&disk).unwrap();
        ud.format_mbr(&disk).unwrap();
        let part = ud.create_fat32(&disk, "BURNIT").unwrap();
        ud.wait_for_fs(&part, 20).unwrap();
        let objs = ud.objects().unwrap();
        let b = &objs.iter().find(|(p, _)| p.as_str() == part).unwrap().1[I_BLOCK];
        println!("partition={} device={} type={} label={} version={}", part, get_bytestr(b, "Device"), get_str(b, "IdType"), get_str(b, "IdLabel"), get_str(b, "IdVersion"));
        assert_eq!(get_str(b, "IdType"), "vfat");
        assert_eq!(get_str(b, "IdVersion"), "FAT32");
    }
}
