# vmcontrol

Cross-platform QEMU/KVM virtual machine management system written in Rust. Provides a web-based control panel and REST API for full VM lifecycle management -- from creating disks and booting VMs to live migration, backups, and VNC console access.

---

## Features

| Category | Highlights |
|----------|-----------|
| **Web UI** | Single-page control panel at `http://localhost:8080` |
| **REST API** | 70+ endpoints with optional API key auth |
| **Multi-Architecture** | x86_64 and aarch64 (ARM64) guests with Secure Boot |
| **Multi-OS Host** | Windows, macOS, Linux with native service integration |
| **VM Groups** | Organize VMs into logical groups |
| **Networking** | NAT, Virtual Switch (VLAN), Bridge/TAP, Port Forwarding, Internal VM-to-VM |
| **Cloud-Init** | EC2-compatible MDS with 12+ configurable options (timezone, locale, DNS, NTP, swap, phone-home, etc.) |
| **SSH Key Store** | Named SSH keys saved in DB, selectable per VM |
| **VNC Console** | QEMU WebSocket VNC with bundled noVNC viewer + file transfer |
| **Send Files to VM** | Transfer files via ISO mount or QEMU Guest Agent direct write |
| **QEMU Guest Agent** | Direct VM communication via virtio-serial (auto-provisioned) |
| **TPM 2.0** | swtpm emulation for Windows 11 Secure Boot |
| **Disk Management** | Create, clone, resize QCOW2 disks with IOPS throttling |
| **Disk Editor** | Mount, browse, read/write files inside QCOW2 disks |
| **Disk Export** | Download as qcow2 / raw / vmdk / vdi / vhdx |
| **Image Import** | Upload vmdk/vdi/vhdx/raw with auto-conversion to qcow2 |
| **VM Export/Import** | Export/import VMs or entire groups as ZIP archives |
| **OS Templates** | Customizable presets with auto-clone disk on selection |
| **ISO Mount** | Upload and hot-mount ISO images (up to 4 GB) |
| **Windows Support** | Auto-mount virtio-win ISO, UEFI NVRAM preservation, VNC toolbar, Win11 bypass |
| **Live Migration** | Move running VMs between hosts |
| **Backup** | Timestamped gzip-compressed snapshots |
| **DHCP Management** | Subnet config, batch IP assignment, static leases |
| **VFIO Passthrough** | PCI device passthrough (Linux) |

---

## Quick Start

```bash
git clone https://github.com/toey/vmcontrol.git
cd vmcontrol

# Install (pick your OS)
# macOS:    sudo bash mac/install.sh
# Linux:    sudo bash linux/install.sh
# Windows:  run windows\install.bat as Administrator

# Open Web UI
open http://localhost:8080
```

---

## Prerequisites

| Requirement | Windows | macOS | Linux |
|-------------|---------|-------|-------|
| **Rust** | [rustup.rs](https://rustup.rs) | [rustup.rs](https://sh.rustup.rs) | [rustup.rs](https://sh.rustup.rs) |
| **QEMU** | [qemu.weilnetz.de](https://qemu.weilnetz.de/w64/) | `brew install qemu` | `apt install qemu-system-x86 qemu-utils` |
| **ISO tool** | Included (oscdimg/mkisofs) | Included (hdiutil) | `apt install genisoimage` |
| **swtpm** (optional) | -- | `brew install swtpm` | `apt install swtpm` |
| **OVS** (optional) | -- | `brew install openvswitch` | `apt install openvswitch-switch` |

> VNC uses QEMU's built-in WebSocket support. No external websockify or Python required.
> swtpm is needed for TPM 2.0 emulation (required by Windows 11).

---

## Installation

### macOS

```bash
sudo bash mac/install.sh
```

Installs as a **launchd** daemon.

```bash
sudo launchctl stop com.vmcontrol.vm_ctl
sudo launchctl start com.vmcontrol.vm_ctl

# Reload service
sudo launchctl unload /Library/LaunchDaemons/com.vmcontrol.vm_ctl.plist
sudo launchctl load /Library/LaunchDaemons/com.vmcontrol.vm_ctl.plist
```

### Linux

```bash
sudo bash linux/install.sh
```

Installs as a **systemd** service.

```bash
sudo systemctl status vmcontrol
sudo systemctl start vmcontrol
sudo systemctl stop vmcontrol
sudo systemctl restart vmcontrol
sudo journalctl -u vmcontrol -f
```

Firewall:

```bash
sudo ufw allow 8080/tcp                                          # UFW
sudo firewall-cmd --add-port=8080/tcp --permanent && sudo firewall-cmd --reload  # firewalld
```

### Windows

> Run as **Administrator**

```powershell
cd windows
.\install.bat
```

Installs as a Windows Service (NSSM) or Scheduled Task.

```powershell
nssm status vmcontrol
nssm stop vmcontrol
nssm start vmcontrol
nssm restart vmcontrol
```

> **ARM64 note:** Install the QEMU ARM64 build from [qemu.weilnetz.de/aarch64](https://qemu.weilnetz.de/aarch64/). The installer detects this automatically.

### Installed Paths

| | macOS / Linux | Windows |
|---|---|---|
| Binary | `/opt/ctl/bin/vm_ctl` | `C:\vmcontrol\bin\vm_ctl.exe` |
| Config | `/opt/ctl/bin/config.yaml` | `C:\vmcontrol\bin\config.yaml` |
| Static | `/opt/ctl/bin/static/` | `C:\vmcontrol\bin\static\` |
| Disks | `/tmp/vmcontrol/disks/` | `C:\vmcontrol\disks\` |
| ISOs | `/tmp/vmcontrol/iso/` | `C:\vmcontrol\iso\` |
| Backups | `/tmp/vmcontrol/backups/` | `C:\vmcontrol\backups\` |
| Database | `/tmp/vmcontrol/vmcontrol.db` | `C:\vmcontrol\vmcontrol.db` |
| Logs | `/tmp/vmcontrol/vm_ctl.*.log` | `C:\vmcontrol\logs\` |

---

## Configuration

```yaml
# config.yaml
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
qemu_accel: hvf:tcg           # macOS: hvf:tcg, Linux: kvm:tcg
qemu_machine: pc
ovs_vsctl_path: /usr/bin/ovs-vsctl
bridge_sudo: true              # Use sudo for bridge mode
bridge_sudo_path: /usr/bin/sudo
internal_mcast_port: 11111     # VM-to-VM multicast port
```

The installer generates this file automatically. Edit to customize.

> The installer also downloads `virtio-win-0.1.285.iso` to the ISO directory automatically (needed for Windows guest VMs).

### aarch64 (ARM64) Guests

Ensure `qemu-system-aarch64` and EDK2 UEFI firmware (`edk2-aarch64-code.fd`) are installed. Select `aarch64` architecture when creating a VM. Uses QEMU `virt` machine type with `virtio-gpu-pci` display.

**Secure Boot (ARM64):** Windows ARM64 VMs automatically use AAVMF Secure Boot firmware (`AAVMF_CODE.secboot.fd` + `AAVMF_VARS.ms.fd`) with Microsoft keys pre-enrolled. Install the firmware files to the path configured in `edk2_aarch64_secure_code` / `edk2_aarch64_secure_vars`. Linux ARM64 VMs use the standard `edk2-aarch64-code.fd`.

---

## API Authentication

```bash
# Enable by setting environment variable
export VMCONTROL_API_KEY="your-secret-key"

# All /api/* endpoints require the header
curl -H "X-API-Key: your-secret-key" http://localhost:8080/api/vm/list
```

If `VMCONTROL_API_KEY` is not set, all requests are allowed without authentication. Static files and EC2 metadata endpoints always bypass auth.

---

## API Endpoints

**Base URL:** `http://localhost:8080`

### VM Lifecycle

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/vm/list` | List all VMs (grouped) |
| `GET` | `/api/vm/get/{smac}` | Get VM details by short MAC |
| `POST` | `/api/vm/create-config` | Create new VM |
| `POST` | `/api/vm/update-config` | Update VM config |
| `POST` | `/api/vm/rename` | Rename VM (`old_name`, `new_name`) |
| `POST` | `/api/vm/start` | Start VM |
| `POST` | `/api/vm/stop` | Force halt VM |
| `POST` | `/api/vm/powerdown` | Graceful ACPI shutdown |
| `POST` | `/api/vm/reset` | Reset/reboot VM |
| `POST` | `/api/vm/delete` | Delete VM and release disks |
| `POST` | `/api/vm/set-group` | Set VM group (`smac`, `group_name`) |
| `GET` | `/api/group/list` | List all group names |
| `GET` | `/api/vm/export/{smac}` | Export VM as ZIP archive (config + disks) |
| `POST` | `/api/vm/import` | Import VM from ZIP archive |
| `GET` | `/api/group/export/{name}` | Export entire VM group as ZIP |
| `POST` | `/api/group/import` | Import VM group from ZIP |

### Disks

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/disk/list` | List all disks with owner info |
| `POST` | `/api/disk/create` | Create QCOW2 disk (`name`, `size`) |
| `POST` | `/api/disk/delete` | Delete disk (`name`) |
| `POST` | `/api/disk/clone` | Clone disk (`source`, `name`) |
| `POST` | `/api/disk/resize` | Resize disk (`name`, `size`) |
| `GET` | `/api/disk/export/{name}` | Export disk (`?format=vmdk\|vdi\|vhdx\|raw`) |
| `GET` | `/api/disk/edit-supported` | Check if disk editing is supported on host |
| `POST` | `/api/disk/mount` | Mount QCOW2 disk for browsing |
| `POST` | `/api/disk/unmount` | Unmount previously mounted disk |
| `GET` | `/api/disk/mounted` | List currently mounted disks |
| `GET` | `/api/disk/browse/{name}` | Browse files in mounted disk (`?path=/`) |
| `GET` | `/api/disk/readfile/{name}` | Read a file from mounted disk |
| `POST` | `/api/disk/writefile/{name}` | Write a file to mounted disk |

### Images

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/image/list` | List uploaded images |
| `POST` | `/api/image/upload` | Upload image (auto-converts to qcow2) |
| `POST` | `/api/image/delete` | Delete image |

Supported upload formats: qcow2, vmdk, vdi, vhdx, raw, img

### ISOs

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/iso/list` | List ISO files |
| `POST` | `/api/iso/upload` | Upload ISO (max 4 GB) |
| `POST` | `/api/iso/delete` | Delete ISO |
| `POST` | `/api/vm/mountiso` | Mount ISO to running VM |
| `POST` | `/api/vm/unmountiso` | Unmount ISO |

### Networking

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/switch/list` | List virtual switches |
| `POST` | `/api/switch/create` | Create switch (`name`) |
| `POST` | `/api/switch/delete` | Delete switch (`id`) |
| `POST` | `/api/switch/rename` | Rename switch (`id`, `name`) |
| `GET` | `/api/vm/{smac}/portforward` | List port forwards |
| `POST` | `/api/vm/{smac}/portforward` | Add port forward (`protocol`, `host_port`, `guest_port`) |
| `POST` | `/api/vm/{smac}/portforward/delete` | Delete port forward |
| `GET` | `/api/internal-network` | List internal VM-to-VM network |
| `GET` | `/api/mac/list` | List MAC addresses |
| `GET` | `/api/ip/list` | List IP pool |

### DHCP

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/dhcp/list` | List DHCP leases |
| `POST` | `/api/dhcp/add` | Add DHCP lease |
| `POST` | `/api/dhcp/delete` | Delete DHCP lease |
| `POST` | `/api/dhcp/sync` | Sync DHCP from VMs |
| `GET` | `/api/dhcp/subnet` | Get subnet config (subnet, gateway, netmask, range) |
| `POST` | `/api/dhcp/subnet` | Save subnet config |
| `POST` | `/api/dhcp/batch-assign` | Auto-assign IPs from range to all VMs |

### VNC Console

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/api/vnc/start` | Start VNC session (`smac`) |
| `POST` | `/api/vnc/stop` | Stop VNC session |
| `POST` | `/api/vnc/token` | Generate time-limited VNC access token |
| `GET` | `/api/vnc/resolve/{token}` | Resolve VNC token to connection info |

Auto-assigns VNC ports from range 12001-13000.

### Send Files to VM

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/api/vm/sendfiles/{smac}` | Upload files → create ISO → auto-mount on CD drive |
| `POST` | `/api/vm/sendfiles-cleanup/{smac}` | Unmount and delete temp ISO |
| `GET` | `/api/vm/guest-agent/{smac}` | Check QEMU Guest Agent availability |
| `POST` | `/api/vm/guestfile/{smac}` | Write file directly to VM filesystem via Guest Agent |
| `GET` | `/api/vm/blockinfo/{smac}` | Query per-drive block device info |

### Devices

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/devices/vfio` | List PCI devices bound to vfio-pci (Linux) |

### OS Templates

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/os-templates` | List custom OS templates |
| `POST` | `/api/os-templates/create` | Create OS template |
| `POST` | `/api/os-templates/update` | Update OS template |
| `POST` | `/api/os-templates/delete` | Delete OS template |

### API Key Management

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/apikey` | Get current API key |
| `POST` | `/api/apikey/generate` | Generate new random API key |

### Backup & Migration

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/backup/list` | List backups |
| `POST` | `/api/backup/delete` | Delete backup |
| `POST` | `/api/vm/backup` | Create gzip snapshot |
| `POST` | `/api/vm/livemigrate` | Live migrate to remote host |

### Metadata Service (MDS)

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/mds/config` | Get global MDS config |
| `POST` | `/api/mds/config` | Save global MDS config |
| `GET` | `/api/vm/{smac}/mds` | Get per-VM metadata |
| `POST` | `/api/vm/{smac}/mds` | Save per-VM metadata |

### SSH Keys

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/sshkey/list` | List saved SSH keys |
| `POST` | `/api/sshkey/create` | Save SSH key (`name`, `pubkey`) |
| `POST` | `/api/sshkey/delete` | Delete SSH key (`id`) |

### Template Images

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/template-images` | List template-to-image mappings |
| `POST` | `/api/template-images/set` | Set mapping (`template_key`, `disk_name`) |

### Utility

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/host/ram` | Get host total/used/available RAM |

### EC2-Compatible Metadata (for VMs)

VMs can query metadata at `http://169.254.169.254` (or the host gateway):

```
/2009-04-04/user-data
/2009-04-04/meta-data/instance-id
/2009-04-04/meta-data/hostname
/2009-04-04/meta-data/local-ipv4
/2009-04-04/meta-data/public-keys/0/openssh-key
/2009-04-04/meta-data/network/interfaces/macs/{mac}/local-ipv4s
```

---

## Networking

### Network Modes

Each VM network adapter supports one of three modes:

| Mode | Description | Host Access |
|------|-------------|-------------|
| **NAT** | SLIRP user-mode networking. VM gets internet via host. | Outbound only (use port forwarding for inbound) |
| **Switch** | Virtual Layer 2 switch for inter-VM communication. | No host access |
| **Bridge** | Bridge/TAP mode. VM gets IP on host network. | Full bidirectional (ping, SSH, etc.) |

### NAT + Port Forwarding

NAT is the default mode. VMs can reach the internet but the host cannot initiate connections to VMs. Use **port forwarding** to expose VM services:

```bash
# Forward host:2222 -> guest:22 (SSH)
curl -X POST http://localhost:8080/api/vm/{smac}/portforward \
  -H "Content-Type: application/json" \
  -d '{"protocol":"tcp","host_port":"2222","guest_port":"22"}'
```

### Internal VM-to-VM Network

VMs on NAT mode get auto-assigned internal IPs from `192.168.100.0/24` for direct VM-to-VM communication via QEMU socket multicast. The Internal Net tab shows the IP pool and assignments.

### Virtual Switches

Switches create Layer 2 segments for VM-to-VM traffic. Implementation varies by platform:

| Platform | Technology | Details |
|----------|-----------|---------|
| **Linux** | OVS + TAP | Open vSwitch bridge with TAP interfaces. Full VLAN support. |
| **macOS** | QEMU socket multicast | UDP multicast per switch. VLAN IDs map to different ports. |
| **Windows** | QEMU socket multicast | Same as macOS. |

### VLAN Segmentation

VMs on the same switch can be isolated with VLANs (0-4094):

- VLAN `0` = untagged (talks to all VMs on the switch)
- VLAN `1`-`4094` = tagged (only talks to same VLAN)

### Bridge Mode

Bridge mode gives VMs an IP address on the host's physical network. The host can directly ping/SSH into VMs.

| Platform | Technology | IP Assignment |
|----------|-----------|---------------|
| **macOS** | vmnet-shared | Automatic DHCP from 192.168.64.0/24 |
| **Linux** | TAP device | Manual or DHCP from host bridge |
| **Windows** | TAP-Windows Adapter V9 | Manual or DHCP |

> Bridge mode requires **sudo** by default. Configure `bridge_sudo: false` in config.yaml to disable.

---

## Send Files to VM

The VNC console includes a **Send Files** button for transferring files into running VMs. Two methods are available:

### ISO Method (CD Drive)

Works on **all VMs** without any guest software. Files are packaged into an ISO image and auto-mounted on an available CD drive (cd0-cd3).

1. Click "Send Files" in the VNC toolbar
2. Drag & drop files (or click to browse)
3. Select "CD Drive (ISO)" method
4. Click Send -- ISO is created and mounted automatically
5. Access files from the CD drive inside the VM

### Guest Agent Method (Direct Write)

Writes files **directly to the VM filesystem** via QEMU Guest Agent. Requires `qemu-guest-agent` installed in the guest:

- **Linux VMs**: Auto-installed via cloud-init. Or manually: `apt install qemu-guest-agent`
- **Windows VMs (x86_64)**: Install from the auto-mounted virtio-win ISO (D: drive): run `virtio-win-guest-tools.exe` or `guest-agent\qemu-ga-x86_64.msi`
- **Windows VMs (ARM64)**: Guest agent is not included in virtio-win ISO for ARM64. Use the ISO method instead

All VMs are provisioned with virtio-serial devices for guest agent communication. The VNC modal auto-detects agent availability and shows the appropriate method.

---

## Windows VM Support

Windows VMs get automatic enhancements:

- **TPM 2.0**: swtpm emulator provides TPM device (required for Windows 11)
- **virtio-win ISO**: Auto-mounted on cd3 for driver and guest agent installation
- **UEFI NVRAM preservation**: Windows Boot Manager entries are preserved when cloning templates. Per-VM `{smac}_efivars.fd` files store UEFI boot configuration
- **VNC toolbar buttons**: Shift+F10 (CMD during install), Alt+Shift (switch language), Win11 Bypass (registry bypass for TPM/SecureBoot/NRO checks), Ctrl+V Paste
- **Secure Boot (ARM64)**: AAVMF firmware with Microsoft keys pre-enrolled

---

## OS Templates

The Create VM form includes templates with recommended defaults:

| Template | vCPUs | Memory | Architecture | Notes |
|----------|-------|--------|-------------|-------|
| Ubuntu Server | 2 | 2 GB | x86_64 | Headless server |
| Ubuntu Desktop | 4 | 4 GB | x86_64 | With GUI |
| Debian | 2 | 1 GB | x86_64 | Stable, minimal |
| CentOS / Rocky | 2 | 2 GB | x86_64 | Enterprise Linux |
| Windows 10/11 | 4 | 4 GB | x86_64 | TPM 2.0, virtio-win |
| Windows Server | 8 | 8 GB | x86_64 | TPM 2.0, virtio-win |
| macOS | 8 | 8 GB | x86_64 | Requires compatible host |
| Minimal Linux | 1 | 512 MB | x86_64 | Alpine, Tiny Core, etc. |

Custom templates can be created, updated, and deleted via the OS Templates API or the Web UI.

### Template Image Mapping + Auto-Clone

Each template can be mapped to a base disk image (persisted in DB). When you select a template:

1. The mapped base image is found
2. A clone is created automatically with name `{vm-name}-disk0`
3. The cloned disk is set as **Disk 0** -- ready to boot

This means each VM gets its own independent disk copy from the base image.

---

## IOPS Throttling

Disk I/O can be throttled per disk with 6 presets:

| Preset | IOPS | Burst Max | Burst Length |
|--------|------|-----------|-------------|
| Low | 3,200 | 3,840 | 60s |
| Standard | 9,600 | 11,520 | 60s |
| High | 19,200 | 23,040 | 60s |
| Ultra | 38,400 | 46,080 | 60s |
| Max | 76,800 | 92,160 | 60s |
| Unlimited | No limit | -- | -- |
| Custom | User-defined | User-defined | User-defined |

---

## Cloud-Init & Metadata Service

Each VM can be configured with cloud-init metadata:

- **Hostname** -- auto-generated or custom
- **SSH Public Key** -- select from saved keys or paste manually
- **Root Password** -- default: `changeme`
- **Custom Userdata** -- additional cloud-init YAML
- **Internal IP** -- auto-assigned from pool for VM-to-VM networking
- **Auto-install packages** -- `qemu-guest-agent` is installed automatically on Linux VMs
- **Timezone** -- e.g. `Asia/Bangkok`
- **Locale** -- e.g. `en_US.UTF-8`
- **DNS Nameservers** -- comma-separated (e.g. `8.8.8.8, 1.1.1.1`)
- **NTP Servers** -- comma-separated
- **Swap Size** -- auto-create swap file (MB)
- **Phone Home URL** -- cloud-init completion callback
- **Power State** -- action after cloud-init completes (poweroff/reboot/none)
- **Disable Root SSH** -- toggle
- **Growpart** -- auto-expand root partition
- **Extra Packages** -- comma-separated additional packages
- **Extra Runcmd** -- additional shell commands (one per line)
- **Write Files** -- JSON array of files to write

The metadata service is EC2-compatible. VMs query it for their configuration during boot via cloud-init's NoCloud datasource.

### Named SSH Keys

SSH public keys can be saved with names in the database. When configuring a VM's cloud-init, select a saved key from the dropdown instead of copy-pasting.

---

## Disk Export

Export stopped VM disks with optional format conversion:

```bash
# Original qcow2 (fastest, no conversion)
curl -O http://localhost:8080/api/disk/export/mydisk

# Convert to other formats
curl -O "http://localhost:8080/api/disk/export/mydisk?format=vmdk"   # VMware
curl -O "http://localhost:8080/api/disk/export/mydisk?format=vdi"    # VirtualBox
curl -O "http://localhost:8080/api/disk/export/mydisk?format=vhdx"   # Hyper-V
curl -O "http://localhost:8080/api/disk/export/mydisk?format=raw"    # Raw image
```

---

## Database

SQLite with WAL mode. Tables:

| Table | Purpose |
|-------|---------|
| `vms` | VM configs, status, group assignments |
| `disks` | Disk inventory with owner tracking |
| `switches` | Virtual switch definitions |
| `dhcp_leases` | DHCP lease records |
| `ssh_keys` | Named SSH public keys |
| `template_images` | OS template to base image mappings |
| `os_templates` | Custom OS template definitions |
| `backups` | Backup metadata |
| `snapshots` | Disk snapshot records |
| `settings` | Key-value app settings (DHCP subnet, etc.) |

---

## Build from Source

```bash
# Pick your platform directory
cd linux   # or mac / windows
cargo build --release

# Run directly (development)
cargo run -- server 0.0.0.0:8080
```

---

## Project Structure

```
vmcontrol/
├── src/                       # Rust source code
│   ├── main.rs                # CLI + server entry point
│   ├── server.rs              # Actix-web API routes & handlers
│   ├── operations.rs          # QEMU VM/disk operations
│   ├── db.rs                  # SQLite database layer
│   ├── config.rs              # YAML config loader
│   ├── models.rs              # Data structures (VmStartConfig, etc.)
│   ├── mds.rs                 # EC2-compatible metadata service
│   ├── api_helpers.rs         # QEMU monitor protocol (QMP)
│   ├── guest_agent.rs         # QEMU Guest Agent (QGA) protocol
│   ├── disk_edit.rs           # QCOW2 disk mount/browse/edit
│   └── ssh.rs                 # Command execution utilities
├── static/                    # Web UI (source of truth)
│   ├── index.html             # Control panel
│   ├── app.js                 # Frontend application
│   ├── style.css              # Styling
│   ├── vnc.html               # noVNC viewer + file transfer
│   └── vendor/novnc/          # Bundled noVNC library
├── mac/                       # macOS platform
│   ├── install.sh             # launchd installer
│   └── src/                   # macOS-specific config defaults
├── linux/                     # Linux platform
│   ├── install.sh             # systemd installer
│   └── src/                   # Linux-specific config defaults
├── windows/                   # Windows platform
│   ├── install.bat            # NSSM service installer
│   └── src/                   # Windows-specific config defaults
├── Cargo.toml                 # Package manifest (v0.3.0)
└── config.yaml                # Development config
```

## Tech Stack

| Component | Technology |
|-----------|-----------|
| Backend | Rust, Actix-web 4, Tokio |
| Database | SQLite (rusqlite, WAL mode) |
| Frontend | Vanilla JS, HTML, CSS |
| VNC | noVNC + QEMU WebSocket |
| VM Engine | QEMU/KVM |
| Networking | OVS+TAP (Linux), vmnet-shared (macOS), TAP-Windows (Windows), QEMU multicast |
| Cloud-Init | NoCloud seed ISO generation |
| Config | YAML (serde_yaml_ng) |

---

## License

MIT
