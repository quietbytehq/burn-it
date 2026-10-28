//! Lists removable USB drives. Internal disks and the disk the OS runs from are never shown.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Drive {
    /// Linux: /dev/sdX   Windows: \\.\PhysicalDriveN
    pub device: String,
    /// Windows disk number (unused on Linux)
    pub number: Option<u32>,
    pub name: String,
    pub size: u64,
    pub bus: String,
    pub mountpoints: Vec<String>,
}

#[cfg(target_os = "linux")]
pub fn list() -> anyhow::Result<Vec<Drive>> {
    use crate::udisks::*;
    let ud = UDisks::connect()?;
    let objs = ud.objects()?;
    Ok(linux_from_objects(&objs))
}

/// Whole, writable, removable/USB disks that hold no system mounts, from UDisks2's object list.
#[cfg(target_os = "linux")]
pub(crate) fn linux_from_objects(objs: &crate::udisks::Objects) -> Vec<Drive> {
    use crate::udisks::*;
    // Debug builds only: lets the test-suite use loop devices as stand-in USB drives.
    let allow_loop = cfg!(debug_assertions) && std::env::var("BURNIT_TEST_ANY_DEVICE").as_deref() == Ok("1");
    const SYSTEM: &[&str] = &["/", "/boot", "/boot/efi", "/efi", "/home", "/usr", "/var"];
    let mut drives = Vec::new();
    for (path, ifs) in objs {
        let Some(block) = ifs.get(I_BLOCK) else { continue };
        if ifs.contains_key(I_PART) || get_bool(block, "ReadOnly") || get_u64(block, "Size") == 0 {
            continue; // partitions, read-only and empty card readers are never targets
        }
        let drive_path = get_path(block, "Drive");
        let drive = objs.iter().find(|(p, _)| p.as_str() == drive_path).and_then(|(_, i)| i.get(I_DRIVE));
        let (name, bus) = match drive {
            Some(d) => {
                let bus = get_str(d, "ConnectionBus").to_lowercase();
                let removable = bus == "usb" || get_bool(d, "Removable") || get_bool(d, "MediaRemovable");
                if !removable || get_bool(block, "HintSystem") && bus != "usb" {
                    continue;
                }
                let name = format!("{} {}", get_str(d, "Vendor"), get_str(d, "Model")).trim().to_string();
                (if name.is_empty() { "USB drive".to_string() } else { name }, if bus.is_empty() { "removable".into() } else { bus })
            }
            None if allow_loop && ifs.contains_key(I_LOOP) => ("Test loop device".to_string(), "loop".to_string()),
            None => continue,
        };
        let mut mounts = Vec::new();
        for part in crate::udisks::UDisks::disk_and_partitions(objs, path.as_str()) {
            if let Some(fs) = objs.iter().find(|(p, _)| p.as_str() == part).and_then(|(_, i)| i.get(I_FS)) {
                mounts.extend(get_bytestr_list(fs, "MountPoints"));
            }
        }
        if mounts.iter().any(|m| SYSTEM.contains(&m.as_str())) {
            continue;
        }
        drives.push(Drive {
            device: get_bytestr(block, "Device"),
            number: None,
            name,
            size: get_u64(block, "Size"),
            bus,
            mountpoints: mounts,
        });
    }
    drives.sort_by(|a, b| a.device.cmp(&b.device));
    drives
}

#[cfg(target_os = "windows")]
pub fn list() -> anyhow::Result<Vec<Drive>> {
    let script = r#"
$ErrorActionPreference='Stop'
$d = @(Get-Disk | Where-Object { ($_.BusType -eq 'USB' -or $_.BusType -eq 'SD' -or $_.BusType -eq 'MMC') -and -not $_.IsBoot -and -not $_.IsSystem -and $_.Size -gt 0 } | ForEach-Object {
  $n = $_.Number
  $letters = @(Get-Partition -DiskNumber $n -ErrorAction SilentlyContinue | Where-Object DriveLetter | ForEach-Object { "$($_.DriveLetter):" })
  [pscustomobject]@{ number=[int]$n; name=[string]$_.FriendlyName; size=[uint64]$_.Size; bus=[string]$_.BusType; mounts=$letters }
})
ConvertTo-Json -InputObject $d -Depth 3 -Compress
"#;
    let out = crate::platform::powershell(script)?;
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap_or(serde_json::Value::Array(vec![]));
    let arr = match v {
        serde_json::Value::Array(a) => a,
        serde_json::Value::Object(_) => vec![v],
        _ => vec![],
    };
    Ok(arr
        .into_iter()
        .filter_map(|d| {
            let n = d["number"].as_u64()? as u32;
            Some(Drive {
                device: format!(r"\\.\PhysicalDrive{}", n),
                number: Some(n),
                name: d["name"].as_str().unwrap_or("USB drive").trim().to_string(),
                size: d["size"].as_u64().unwrap_or(0),
                bus: d["bus"].as_str().unwrap_or("USB").to_string(),
                mountpoints: d["mounts"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_default(),
            })
        })
        .collect())
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn list() -> anyhow::Result<Vec<Drive>> {
    anyhow::bail!("This operating system is not supported yet.")
}

