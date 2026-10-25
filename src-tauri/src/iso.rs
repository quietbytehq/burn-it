//! Looks inside an ISO file to decide how it should be written.
//!
//! * Linux (and most modern) ISOs are "hybrid": they carry an MBR/GPT so they can be
//!   copied byte-for-byte onto a USB stick ("raw" mode).
//! * Windows ISOs are not hybrid (the first 32 KiB are blank) and keep their real
//!   content in a UDF file system. They need the "Windows" mode: format the stick
//!   FAT32, copy the files and split the big install.wim.

use serde::Serialize;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WriteMode {
    /// Copy the image byte-for-byte (Linux and other hybrid images).
    Raw,
    /// FAT32 + file copy + split install.wim (Windows installers).
    Windows,
}

#[derive(Debug, Clone, Serialize)]
pub struct IsoInfo {
    pub path: String,
    pub file_name: String,
    pub size: u64,
    pub volume_label: String,
    /// "linux", "windows" or "unknown"
    pub kind: String,
    pub hybrid: bool,
    pub udf: bool,
    pub suggested_mode: WriteMode,
    pub note: String,
}

const SECTOR: u64 = 2048;

const WINDOWS_LABEL_HINTS: &[&str] = &[
    "CCCOMA", "CPBA", "CENA", "CCSA", "CPRA", "CSSA", "SSS_", "ESD-ISO", "WIN", "J_CC", "GRMC", "IR5_", "IR4_",
];

pub fn inspect(path: &Path) -> anyhow::Result<IsoInfo> {
    let mut f = File::open(path)?;
    let size = f.metadata()?.len();
    if size < 64 * 1024 {
        anyhow::bail!("This file is too small to be a bootable ISO image.");
    }

    // 1. Boot signature in the first sector => hybrid image (bootable when written raw).
    let mut mbr = [0u8; 512];
    f.read_exact(&mut mbr)?;
    let hybrid = mbr[510] == 0x55 && mbr[511] == 0xAA;

    // 2. Walk the ISO 9660 / UDF volume descriptors starting at sector 16.
    let mut volume_label = String::new();
    let mut is_iso9660 = false;
    let mut udf = false;
    let mut buf = [0u8; SECTOR as usize];
    for i in 16..64u64 {
        if f.seek(SeekFrom::Start(i * SECTOR)).is_err() || f.read_exact(&mut buf).is_err() {
            break;
        }
        let id = &buf[1..6];
        match id {
            b"CD001" => {
                is_iso9660 = true;
                if buf[0] == 1 && volume_label.is_empty() {
                    volume_label = String::from_utf8_lossy(&buf[40..72]).trim().to_string();
                }
                if buf[0] == 255 {
                    // terminator for ISO 9660; UDF descriptors may follow, keep scanning
                }
            }
            b"NSR02" | b"NSR03" => udf = true,
            b"BEA01" | b"TEA01" | b"BOOT2" => {}
            _ => {
                if udf || i > 40 {
                    break;
                }
            }
        }
    }

    if !is_iso9660 && !udf && !hybrid {
        anyhow::bail!("This does not look like an ISO or disk image.");
    }

    let upper = volume_label.to_uppercase();
    let label_says_windows = WINDOWS_LABEL_HINTS.iter().any(|h| upper.starts_with(h) || upper.contains("WINDOWS"));

    let (kind, mode, note) = if hybrid {
        (
            "linux",
            WriteMode::Raw,
            "Bootable image — it will be copied byte-for-byte to the USB drive.".to_string(),
        )
    } else if udf || label_says_windows {
        (
            "windows",
            WriteMode::Windows,
            "Windows installer — the USB will be formatted FAT32 (UEFI boot) and large files split automatically."
                .to_string(),
        )
    } else {
        (
            "unknown",
            WriteMode::Raw,
            "This image is not hybrid; it may not boot from USB when copied raw.".to_string(),
        )
    };

    Ok(IsoInfo {
        path: path.to_string_lossy().to_string(),
        file_name: path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        size,
        volume_label,
        kind: kind.to_string(),
        hybrid,
        udf,
        suggested_mode: mode,
        note,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore]
    fn inspect_env_isos() {
        for (var, kind) in [("BURNIT_TEST_LINUX_ISO", "linux"), ("BURNIT_TEST_WIN_ISO", "windows")] {
            let p = std::env::var(var).unwrap();
            let info = super::inspect(std::path::Path::new(&p)).unwrap();
            println!("{:?}", info);
            assert_eq!(info.kind, kind);
        }
    }
}
