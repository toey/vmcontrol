# vmcontrol

Cross-platform QEMU/KVM virtual machine management system written in Rust. Provides a web-based control panel and REST API for managing VMs, disks, ISOs, backups, networking, and VNC access.

## Features

- **Web UI** -- Control panel at `http://localhost:8080` for managing VMs
- **REST API** -- Full programmatic control over VM lifecycle with optional API key authentication
- **Multi-Architecture** -- x86_64 and aarch64 (ARM64) guest support
- **Multi-OS** -- Windows, macOS, Linux host platforms
- **VM Groups** -- Organize VMs into logical groups (production, staging, dev, etc.)
- **Cloud-Init** -- NoCloud metadata service with per-VM configuration
- **VNC Console** -- Built-in QEMU WebSocket VNC with bundled noVNC viewer
- **Disk Management** -- Create, clone, resize, delete QCOW2 disks with SQLite tracking
- **Disk Export** -- Download disks as qcow2, or convert to raw/vmdk/vdi/vhdx on-the-fly
- **Image Import** -- Upload vmdk/vdi/vhdx/raw images with auto-conversion to qcow2
- **ISO Mount** -- Upload and boot VMs from ISO images (up to 4 GB)
- **Virtual Switches** -- Inter-VM Layer 2 networking with VLAN segmentation
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
| **OVS** (optional) | -- | `brew install openvswitch` | `apt install openvswitch-switch` |

> **Note:** websockify and Python are no longer required. VNC proxying is now handled natively by QEMU's built-in WebSocket support.

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
2. Build `vm_ctl.exe` from source using Cargo (or use pre-built binary)
3. Create directory structure at `C:\vmcontrol\`
4. Generate `config.yaml` with detected paths
5. Install as Windows Service (NSSM) or Scheduled Task
6. Add firewall rule for port 8080

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

> **Rust toolchain:** You need either MSVC (Visual Studio Build Tools) or GNU (MinGW-w64) toolchain. See `windows/readme.txt` for detailed setup instructions.

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
1. Check prerequisites (cargo, QEMU, genisoimage, openvswitch-switch)
2. Build `vm_ctl` from source
3. Create directory structure
4. Generate `config.yaml`
5. Install as systemd service
6. Configure firewall rules

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
  vs-up.sh              # OVS TAP up script
  vs-down.sh            # OVS TAP down script

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
qemu_aarch64_path: /usr/bin/qemu-system-aarch64
edk2_aarch64_bios: /usr/share/qemu/edk2-aarch64-code.fd
ctl_bin_path: /opt/ctl/bin
pctl_path: /tmp/vmcontrol
disk_path: /tmp/vmcontrol/disks
iso_path: /tmp/vmcontrol/iso
live_path: /tmp/vmcontrol/backups
gzip_path: /usr/bin/gzip
db_path: /tmp/vmcontrol/vmcontrol.db
mds_config_path: /tmp/vmcontrol/mds.json
domain: localhost
qemu_accel: hvf:tcg
qemu_machine: pc
ovs_vsctl_path: /usr/bin/ovs-vsctl
```

The installer generates this file automatically with detected paths. Edit manually to customize.

### aarch64 (ARM64) Guest Support

To run ARM64 VMs, ensure:
- `qemu-system-aarch64` is installed
- EDK2 UEFI firmware is available (`edk2-aarch64-code.fd`)
- Set `arch: aarch64` when creating a VM

The system uses QEMU `virt` machine type with `virtio-gpu-pci` display and `-cpu max` for aarch64 guests.

---

## API Authentication

Set the `VMCONTROL_API_KEY` environment variable to enable API key authentication:

```bash
export VMCONTROL_API_KEY="your-secret-key"
```

All `/api/*` endpoints then require the `X-API-Key` header:

```bash
curl -H "X-API-Key: your-secret-key" http://localhost:8080/api/vm/list
```

If `VMCONTROL_API_KEY` is not set, all requests are allowed without authentication. Static files and EC2 metadata endpoints are always accessible.

---

## API Endpoints

**Base URL:** `http://localhost:8080`

### VM Management

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/vm/list` | List all VMs (grouped) |
| `GET` | `/api/vm/get/{smac}` | Get VM details |
| `POST` | `/api/vm/create-config` | Create VM |
| `POST` | `/api/vm/update-config` | Update VM config |
| `POST` | `/api/vm/start` | Start VM |
| `POST` | `/api/vm/stop` | Stop VM (force halt) |
| `POST` | `/api/vm/powerdown` | Graceful ACPI shutdown |
| `POST` | `/api/vm/reset` | Reset VM |
| `POST` | `/api/vm/delete` | Delete VM |
| `POST` | `/api/vm/set-group` | Set VM group (`smac`, `group_name`) |

### VM Groups

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/group/list` | List all group names |
| `POST` | `/api/vm/set-group` | Assign VM to a group |

Groups are implicit -- they are created when a VM is assigned to a group name and disappear when no VMs belong to them. VMs without a group appear under "(Ungrouped)".

### Disk Management

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/disk/list` | List disks |
| `POST` | `/api/disk/create` | Create disk (`name`, `size`) |
| `POST` | `/api/disk/delete` | Delete disk (`name`) |
| `POST` | `/api/disk/clone` | Clone disk (`source`, `name`) |
| `POST` | `/api/disk/resize` | Resize disk (`name`, `size`) |
| `GET` | `/api/disk/export/{name}` | Export/download disk (see below) |

### Disk Export

Export a disk image with optional format conversion. Disk export is only available when the VM is stopped.

```bash
# Download as qcow2 (default -- no conversion, fastest)
curl -O -H "X-API-Key: KEY" http://localhost:8080/api/disk/export/mydisk

# Convert and download as VMDK (VMware)
curl -O -H "X-API-Key: KEY" "http://localhost:8080/api/disk/export/mydisk?format=vmdk"

# Convert and download as VDI (VirtualBox)
curl -O -H "X-API-Key: KEY" "http://localhost:8080/api/disk/export/mydisk?format=vdi"

# Convert and download as VHDX (Hyper-V)
curl -O -H "X-API-Key: KEY" "http://localhost:8080/api/disk/export/mydisk?format=vhdx"

# Convert and download as raw image
curl -O -H "X-API-Key: KEY" "http://localhost:8080/api/disk/export/mydisk?format=raw"
```

Supported formats: `qcow2` (default), `raw`, `vmdk`, `vdi`, `vhdx`

### Image Import

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/image/list` | List disk images |
| `POST` | `/api/image/upload` | Upload image (auto-converts to qcow2) |
| `POST` | `/api/image/delete` | Delete image |

Upload supports: qcow2, vmdk, vdi, vhdx, raw, img -- non-qcow2 formats are auto-converted via `qemu-img convert`.

### ISO Management

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/iso/list` | List ISOs |
| `POST` | `/api/iso/upload` | Upload ISO (max 4 GB) |
| `POST` | `/api/iso/delete` | Delete ISO |
| `POST` | `/api/vm/mountiso` | Mount ISO to VM |
| `POST` | `/api/vm/unmountiso` | Unmount ISO |

### VNC Console

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/api/vnc/start` | Start VNC session (`smac`, `novncport`) |
| `POST` | `/api/vnc/stop` | Stop VNC session |

VNC uses QEMU's built-in WebSocket support with automatic port assignment (range 12001-13000). The bundled noVNC viewer is accessible from the Web UI.

### Backup & Migration

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/backup/list` | List backups |
| `POST` | `/api/backup/delete` | Delete backup |
| `POST` | `/api/vm/backup` | Create VM backup (gzip snapshot) |
| `POST` | `/api/vm/livemigrate` | Live migrate VM to another host |

### Virtual Network Switches

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/switch/list` | List virtual switches |
| `POST` | `/api/switch/create` | Create switch (`name`) |
| `POST` | `/api/switch/delete` | Delete switch (`id`) |
| `POST` | `/api/switch/rename` | Rename switch (`id`, `name`) |

### Metadata Service (MDS)

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/vm/{smac}/mds` | Get per-VM metadata config |
| `POST` | `/api/vm/{smac}/mds` | Save per-VM metadata config |

---

## Networking

### Network Modes

Each VM network adapter supports:

- **NAT** (default) -- VM gets internet access through the host's NAT. Simple, no extra setup.
- **Switch** -- VM connects to a virtual switch for Layer 2 inter-VM communication.

### Virtual Switches

Virtual switches enable direct Layer 2 connectivity between VMs on the same switch. VMs must configure their own IP addresses (no DHCP).

**How it works by platform:**

| Platform | Technology | Details |
|----------|-----------|---------|
| **Linux** | OVS + TAP | Open vSwitch bridge with TAP interfaces per VM. Supports VLAN tagging. |
| **macOS** | QEMU socket multicast | UDP multicast for VM-to-VM traffic. Each switch gets a unique multicast port. |
| **Windows** | QEMU socket multicast | Same as macOS. |

### VLAN Segmentation

VMs on the same switch can be isolated into VLANs. Set the VLAN ID (0-4094) per network adapter:

- VLAN `0` = untagged (default, communicates with all VMs on the switch)
- VLAN `1`-`4094` = tagged (only communicates with VMs on the same VLAN)

On Linux, VLAN tagging is enforced by OVS. On macOS/Windows, VLAN IDs map to different multicast ports for isolation.

---

## OS Templates

The Create VM form includes OS templates with recommended defaults:

| Template | CPU | Memory | Disk | Notes |
|----------|-----|--------|------|-------|
| Ubuntu Server | 1s/2c/1t | 2 GB | 40 GB | Minimal, headless |
| Ubuntu Desktop | 1s/2c/2t | 4 GB | 60 GB | With GUI |
| Debian | 1s/2c/1t | 2 GB | 40 GB | Stable, minimal |
| CentOS / Rocky | 1s/2c/1t | 2 GB | 40 GB | Enterprise Linux |
| Windows 10/11 | 1s/2c/2t | 4 GB | 64 GB | `is_windows=1` |
| Windows Server | 1s/4c/2t | 8 GB | 80 GB | `is_windows=1` |
| macOS | 1s/4c/2t | 8 GB | 80 GB | Requires compatible host |
| Minimal Linux | 1s/1c/1t | 512 MB | 10 GB | Alpine, Tiny Core, etc. |

Templates pre-fill the form -- all values can be customized before creating.

---

## Build from Source

```bash
# Build for current platform
cd linux   # or mac / windows
cargo build --release

# Run directly (development)
cargo run -- server 0.0.0.0:8080

# Run with config file
./target/release/vm_ctl server 0.0.0.0:8080
```

---

## Project Structure

```
vmcontrol/
├── src/                    # Root / development code (macOS)
│   ├── main.rs             # CLI entry + server launcher
│   ├── server.rs           # Actix-web API routes & handlers
│   ├── operations.rs       # VM/disk QEMU operations
│   ├── db.rs               # SQLite database layer
│   ├── config.rs           # YAML config loader
│   ├── models.rs           # Data structures
│   ├── mds.rs              # EC2-compatible metadata service + cloud-init
│   ├── api_helpers.rs      # QEMU monitor interaction
│   └── ssh.rs              # Command execution
├── static/                  # Web UI (single source of truth)
│   ├── index.html           # Control panel
│   ├── vnc.html             # VNC viewer (noVNC)
│   ├── app.js               # Frontend application
│   ├── style.css            # Styling
│   └── vendor/novnc/        # Bundled noVNC library
├── linux/                   # Linux platform
│   ├── install.sh           # systemd installer
│   ├── src/                 # Linux-specific config (OVS paths, etc.)
│   └── static/              # Synced from root static/
├── mac/                     # macOS platform
│   ├── install.sh           # launchd installer
│   ├── src/                 # macOS-specific config (Homebrew paths)
│   └── static/              # Synced from root static/
├── windows/                 # Windows platform
│   ├── install.bat          # NSSM service installer
│   ├── src/                 # Windows-specific config
│   └── static/              # Synced from root static/
├── Cargo.toml               # Root package (v0.3.0)
├── config.yaml              # Development config
└── README.md
```

### Tech Stack

| Component | Technology |
|-----------|-----------|
| Backend | Rust, Actix-web 4, Tokio |
| Database | SQLite (rusqlite, WAL mode) |
| Frontend | Vanilla JS, HTML, CSS |
| VNC | noVNC + QEMU WebSocket |
| VM Engine | QEMU/KVM |
| Networking | OVS+TAP (Linux), QEMU socket multicast (macOS/Windows) |
| Cloud-Init | NoCloud seed ISO generation |
| Config | YAML (serde_yaml_ng) |

---

## License

MIT
