# vmcontrol

Cross-platform QEMU/KVM virtual machine management system written in Rust. Provides a web-based control panel and REST API for managing VMs, disks, ISOs, backups, and VNC access.

## Features

- **Web UI** -- Control panel at `http://localhost:8080` for managing VMs
- **REST API** -- Full programmatic control over VM lifecycle
- **Multi-OS** -- Windows (x86_64 + ARM64), macOS, Linux
- **Cloud-Init** -- NoCloud metadata service with per-VM configuration
- **VNC Console** -- WebSocket VNC proxy via websockify (noVNC viewer included)
- **Disk Management** -- Create, clone, delete QCOW2 disks with SQLite tracking
- **ISO Mount** -- Upload and boot VMs from ISO images (up to 4 GB)
- **Live Migration** -- Move running VMs between hosts
- **Backup** -- Timestamped VM snapshots with gzip compression

---

## Quick Start

```bash
# Clone
git clone https://github.com/toey/vmcontrol.git
cd vmcontrol

# Install (pick your OS)
# Windows:  run windows\install.bat as Administrator
# macOS:    sudo bash mac/install.sh
# Linux:    sudo bash linux/install.sh

# Access Web UI
open http://localhost:8080
```

---

## Prerequisites

| Requirement | Windows | macOS | Linux |
|-------------|---------|-------|-------|
| **Rust** | [rustup.rs](https://rustup.rs) | [rustup.rs](https://sh.rustup.rs) | [rustup.rs](https://sh.rustup.rs) |
| **QEMU** | [qemu.weilnetz.de](https://qemu.weilnetz.de/w64/) | `brew install qemu` | `apt install qemu-system-x86 qemu-utils` |
| **ISO tool** | Included (oscdimg/mkisofs) | Included (hdiutil) | `apt install genisoimage` |
| **Python 3** | Optional (for VNC proxy) | Optional | Optional |
| **websockify** | `pip install websockify` | `pip3 install websockify` | `pip3 install websockify` |

---

## Installation

### Windows

> Requires **PowerShell as Administrator**

```powershell
cd windows
.\install.bat
```

The installer will:
1. Detect CPU architecture (x86_64 / ARM64) and verify QEMU compatibility
2. Build `vm_ctl.exe` from source using Cargo
3. Auto-detect Python and websockify paths
4. Create directory structure at `C:\vmcontrol\`
5. Generate `config.yaml` with detected paths
6. Install as Windows Service (NSSM) or Scheduled Task
7. Add firewall rule for port 8080

**Service management:**
```powershell
# NSSM
nssm status vmcontrol
nssm stop vmcontrol
nssm start vmcontrol
nssm restart vmcontrol

# Scheduled Task (fallback)
schtasks /query /tn vmcontrol
schtasks /run /tn vmcontrol
schtasks /end /tn vmcontrol
```

**Installed paths:**
```
C:\vmcontrol\
  bin\vm_ctl.exe        # Binary
  bin\config.yaml       # Configuration
  bin\static\           # Web UI files
  disks\                # QCOW2 disk images
  iso\                  # ISO files
  backups\              # VM snapshots
  logs\                 # QEMU + server logs
  vmcontrol.db          # SQLite database
```

> **Windows ARM64 note:** If running on ARM (e.g. Parallels), install the QEMU ARM64 build from [qemu.weilnetz.de/aarch64](https://qemu.weilnetz.de/aarch64/). The installer detects this automatically.

> **Rust toolchain:** If you see `link.exe not found`, switch to the GNU toolchain:
> ```powershell
> rustup toolchain install stable-x86_64-pc-windows-gnu --force-non-host
> rustup default stable-x86_64-pc-windows-gnu --force-non-host
> ```

---

### macOS

```bash
sudo bash mac/install.sh
```

The installer will:
1. Check prerequisites (cargo, QEMU)
2. Build `vm_ctl` from source
3. Create directory structure
4. Generate `config.yaml`
5. Install as launchd daemon

**Service management:**
```bash
sudo launchctl stop com.vmcontrol.vm_ctl
sudo launchctl start com.vmcontrol.vm_ctl

# Reload service
sudo launchctl unload /Library/LaunchDaemons/com.vmcontrol.vm_ctl.plist
sudo launchctl load /Library/LaunchDaemons/com.vmcontrol.vm_ctl.plist
```

**Installed paths:**
```
/opt/ctl/bin/
  vm_ctl                # Binary
  config.yaml           # Configuration
  static/               # Web UI files

/tmp/vmcontrol/
  disks/                # QCOW2 disk images
  iso/                  # ISO files
  backups/              # VM snapshots
  logs/                 # QEMU logs
  vmcontrol.db          # SQLite database
```

---

### Linux

```bash
sudo bash linux/install.sh
```

The installer will:
1. Check prerequisites (cargo, QEMU, genisoimage)
2. Build `vm_ctl` from source
3. Create directory structure
4. Generate `config.yaml`
5. Install as systemd service

**Service management:**
```bash
sudo systemctl status vmcontrol
sudo systemctl start vmcontrol
sudo systemctl stop vmcontrol
sudo systemctl restart vmcontrol

# View logs
sudo journalctl -u vmcontrol -f
```

**Firewall:**
```bash
# UFW
sudo ufw allow 8080/tcp

# firewalld
sudo firewall-cmd --add-port=8080/tcp --permanent
sudo firewall-cmd --reload
```

**Installed paths:**
```
/opt/ctl/bin/
  vm_ctl                # Binary
  config.yaml           # Configuration
  static/               # Web UI files

/tmp/vmcontrol/
  disks/                # QCOW2 disk images
  iso/                  # ISO files
  backups/              # VM snapshots
  logs/                 # QEMU logs
  vmcontrol.db          # SQLite database
```

---

## Configuration

Config file location:
- **Windows:** `C:\vmcontrol\bin\config.yaml`
- **macOS / Linux:** `/opt/ctl/bin/config.yaml`

```yaml
qemu_path: /usr/bin/qemu-system-x86_64
qemu_img_path: /usr/bin/qemu-img
ctl_bin_path: /opt/ctl/bin
pctl_path: /tmp/vmcontrol
disk_path: /tmp/vmcontrol/disks
iso_path: /tmp/vmcontrol/iso
live_path: /tmp/vmcontrol/backups
gzip_path: /usr/bin/gzip
python_path: python3
websockify_path: websockify
domain: localhost
```

The installer generates this file automatically with detected paths. Edit manually to customize.

---

## API Endpoints

**Base URL:** `http://localhost:8080`

### VM Management

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/vm/list` | List all VMs |
| `GET` | `/api/vm/get/{smac}` | Get VM details |
| `POST` | `/api/vm/create-config` | Create VM |
| `POST` | `/api/vm/update-config` | Update VM config |
| `POST` | `/api/vm/start` | Start VM |
| `POST` | `/api/vm/stop` | Stop VM |
| `POST` | `/api/vm/powerdown` | Graceful shutdown |
| `POST` | `/api/vm/reset` | Reset VM |
| `POST` | `/api/vm/delete` | Delete VM |

### Disk & ISO

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/disk/list` | List disks |
| `POST` | `/api/disk/create` | Create disk |
| `POST` | `/api/disk/delete` | Delete disk |
| `POST` | `/api/disk/clone` | Clone disk |
| `GET` | `/api/iso/list` | List ISOs |
| `POST` | `/api/iso/upload` | Upload ISO (max 4 GB) |
| `POST` | `/api/vm/mountiso` | Mount ISO to VM |
| `POST` | `/api/vm/unmountiso` | Unmount ISO |

### VNC & Backup

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/api/vnc/start` | Start VNC proxy |
| `POST` | `/api/vnc/stop` | Stop VNC proxy |
| `GET` | `/api/backup/list` | List backups |
| `POST` | `/api/vm/backup` | Create backup |
| `POST` | `/api/vm/livemigrate` | Live migrate VM |

### Metadata Service (MDS)

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/vm/{smac}/mds` | Get VM metadata config |
| `POST` | `/api/vm/{smac}/mds` | Save VM metadata config |

---

## Build from Source

```bash
# Build for current platform
cd linux   # or mac / windows
cargo build --release

# Cross-compile Windows from macOS
cd windows
cargo build --release --target x86_64-pc-windows-gnu

# Run directly
./target/release/vm_ctl server 0.0.0.0:8080
```

---

## Project Structure

```
vmcontrol/
├── src/                    # Shared cross-platform code
│   ├── main.rs            # CLI entry + server launcher
│   ├── server.rs          # Actix-web API routes
│   ├── operations.rs      # VM/disk operations
│   ├── db.rs              # SQLite database
│   ├── config.rs          # Config loader
│   ├── models.rs          # Data structures
│   ├── mds.rs             # EC2-compatible metadata service
│   ├── api_helpers.rs     # QEMU monitor helpers
│   └── ssh.rs             # Command execution
├── static/                 # Web UI
│   ├── index.html         # Control panel
│   ├── vnc.html           # VNC viewer (noVNC)
│   ├── app.js             # Application logic
│   └── style.css          # Styling
├── windows/                # Windows platform
│   ├── install.bat        # Windows installer
│   └── src/               # Windows-specific code
├── mac/                    # macOS platform
│   ├── install.sh         # macOS installer
│   └── src/               # macOS-specific code
├── linux/                  # Linux platform
│   ├── install.sh         # Linux installer
│   └── src/               # Linux-specific code
└── README.md
```

---

## License

MIT
