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
    use serde_json::Value;
    let out = std::process::Command::new("lsblk")
        .args(["-J", "-b", "-o", "NAME,PATH,SIZE,MODEL,VENDOR,TRAN,RM,HOTPLUG,TYPE,MOUNTPOINT,RO"])
        .output()?;
    if !out.status.success() {
        anyhow::bail!("lsblk failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let v: Value = serde_json::from_slice(&out.stdout)?;
    let mut drives = Vec::new();
    for d in v["blockdevices"].as_array().cloned().unwrap_or_default() {
        if let Some(drive) = linux_candidate(&d) {
            drives.push(drive);
        }
    }
    Ok(drives)
}

#[cfg(target_os = "linux")]
fn as_bool(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::String(s) => s == "1" || s == "true",
        serde_json::Value::Number(n) => n.as_u64() == Some(1),
        _ => false,
    }
}

#[cfg(target_os = "linux")]
fn as_u64(v: &serde_json::Value) -> u64 {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn collect_mounts(d: &serde_json::Value, out: &mut Vec<String>) {
    if let Some(m) = d["mountpoint"].as_str() {
        out.push(m.to_string());
    }
    if let Some(m) = d["mountpoints"].as_array() {
        for x in m.iter().filter_map(|x| x.as_str()) {
            out.push(x.to_string());
        }
    }
    for c in d["children"].as_array().cloned().unwrap_or_default() {
        collect_mounts(&c, out);
    }
}

/// Returns a drive only if it is a whole, writable, removable/USB disk that holds no system mounts.
#[cfg(target_os = "linux")]
pub(crate) fn linux_candidate(d: &serde_json::Value) -> Option<Drive> {
    if d["type"].as_str() != Some("disk") || as_bool(&d["ro"]) {
        return None;
    }
    let tran = d["tran"].as_str().unwrap_or("").to_lowercase();
    let removable = tran == "usb" || as_bool(&d["rm"]) || as_bool(&d["hotplug"]);
    let size = as_u64(&d["size"]);
    if !removable || size == 0 {
        return None;
    }
    let mut mounts = Vec::new();
    collect_mounts(d, &mut mounts);
    const SYSTEM: &[&str] = &["/", "/boot", "/boot/efi", "/efi", "/home", "/usr", "/var", "[SWAP]"];
    if mounts.iter().any(|m| SYSTEM.contains(&m.as_str())) {
        return None;
    }
    let vendor = d["vendor"].as_str().unwrap_or("").trim();
    let model = d["model"].as_str().unwrap_or("").trim();
    let name = format!("{} {}", vendor, model).trim().to_string();
    Some(Drive {
        device: d["path"].as_str().map(String::from).unwrap_or_else(|| format!("/dev/{}", d["name"].as_str().unwrap_or("?"))),
        number: None,
        name: if name.is_empty() { "USB drive".into() } else { name },
        size,
        bus: if tran.is_empty() { "removable".into() } else { tran },
        mountpoints: mounts,
    })
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn filters_system_and_internal_disks() {
        let usb: serde_json::Value = serde_json::json!({"name":"sdb","path":"/dev/sdb","size":16008609792u64,"model":"Ultra","vendor":"SanDisk","tran":"usb","rm":true,"hotplug":true,"type":"disk","ro":false,"mountpoint":null,
            "children":[{"name":"sdb1","mountpoint":"/media/x"}]});
        let nvme = serde_json::json!({"name":"nvme0n1","path":"/dev/nvme0n1","size":512110190592u64,"tran":"nvme","rm":false,"hotplug":false,"type":"disk","ro":false,
            "children":[{"name":"nvme0n1p2","mountpoint":"/"}]});
        let usb_root = serde_json::json!({"name":"sdc","path":"/dev/sdc","size":1u64<<34,"tran":"usb","rm":false,"hotplug":true,"type":"disk","ro":false,
            "children":[{"name":"sdc1","mountpoint":"/"}]});
        assert!(linux_candidate(&usb).is_some());
        assert!(linux_candidate(&nvme).is_none());
        assert!(linux_candidate(&usb_root).is_none());
    }
}
