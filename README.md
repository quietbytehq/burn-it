<p align="center"><img src="assets/icon-1024.png" width="128" alt="Burn It! logo"></p>

<h1 align="center">Burn It!</h1>
<p align="center">Make bootable USB drives from <b>Linux</b> and <b>Windows</b> ISOs, on <b>Linux</b> and <b>Windows</b>.</p>

---

Burn It! is a small, friendly USB image writer. Pick an ISO, pick a USB drive, click **Burn It!**

- **Linux ISOs** (Ubuntu, Fedora, Mint, Debian, Arch…) and other hybrid images are copied byte-for-byte, then verified.
- **Windows 10/11 installer ISOs** are written the way Windows expects: the USB is formatted FAT32, the files are copied, and the large `install.wim` (over 4 GB) is split automatically. The result boots on any UEFI PC, with Secure Boot on or off.
- **Safe by design.** Only removable USB drives are listed. Internal disks and the disk your system runs from are never shown, and the privileged part re-checks the target before it writes anything.
- Detects the ISO type automatically. You can override it under *Advanced options*.

## Download

Get the latest installer from [Releases](../../releases):

| System | File |
|---|---|
| Ubuntu / Debian / Mint | `burn-it_x.y.z_amd64.deb` |
| Fedora / openSUSE | `burn-it-x.y.z-1.x86_64.rpm` |
| Windows 10 / 11 | `Burn It!_x.y.z_x64-setup.exe` |

On Linux, Burn It! never runs as root: all disk work goes through the system's UDisks2 service (the one GNOME Disks uses), which asks for your password with the normal system prompt when it's about to write. On Windows it asks for administrator rights when it starts.

**Snap:** `sudo snap install burn-it`. If it can't reach the disk service, run `sudo snap connect burn-it:udisks2`.

### Linux requirements
Installed automatically with the package: `udisks2`, `dosfstools`, `wimtools`/`wimlib-utils`. Opening Windows ISOs needs the kernel's `udf` file system (standard on desktop distros).

## How it works

| Image | What Burn It! does |
|---|---|
| Hybrid (Linux, most others) | Raw block copy (device opened through UDisks2) → flush → read-back verification |
| Windows installer | MBR + one FAT32 partition (`BURNIT`), created and mounted by UDisks2 on Linux → copy files → split `install.wim` into `.swm` parts under 4 GB (`wimlib` on Linux, built-in `DISM` on Windows) |

On Windows, FAT32 partitions are limited to 32 GB (a Windows formatting limit); the rest of a bigger stick stays unused.

## Build from source

```bash
# Linux build deps (Debian/Ubuntu)
sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf
cargo install tauri-cli --version "^2" --locked

cd src-tauri
cargo test
cargo tauri dev            # run
cargo tauri build          # .deb / .rpm (Linux) or setup .exe (Windows)
```

Built with [Tauri 2](https://tauri.app) (Rust + a plain HTML/CSS/JS interface, no Node toolchain needed).

## Roadmap
- v0.2: Windows 11 options (skip TPM/Secure Boot check, local account), NTFS mode for legacy BIOS, cancel button, AppImage
- Checksum (SHA-256) check of the ISO before writing
- Multi-language UI

## License
Apache-2.0. Free for everyone to use, share and build on, including commercially.
