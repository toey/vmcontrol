#!/usr/bin/env bash
set -euo pipefail

# ================================================================
#  vmcontrol Installer for Linux
# ================================================================

VERSION="0.2.0"

# --- Path constants (must match linux/src/config.rs) ---
QEMU_PATH="/usr/bin/qemu-system-x86_64"
QEMU_IMG_PATH="/usr/bin/qemu-img"
CTL_BIN="/opt/ctl/bin"
CONFIG_YAML="/opt/ctl/bin/config.yaml"
PCTL_PATH="/tmp/vmcontrol"
DISK_PATH="/tmp/vmcontrol/disks"
ISO_PATH="/tmp/vmcontrol/iso"
LIVE_PATH="/tmp/vmcontrol/backups"
STATIC_DIR="/opt/ctl/bin/static"
LOG_DIR="/tmp/vmcontrol"
SERVICE_NAME="vmcontrol"
SYSTEMD_UNIT="/etc/systemd/system/${SERVICE_NAME}.service"

# --- Colors ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

info()    { echo -e "${BLUE}[INFO]${NC} $*"; }
success() { echo -e "${GREEN}[OK]${NC}   $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC} $*"; }
error()   { echo -e "${RED}[ERR]${NC}  $*"; }

# --- Root check ---
if [[ $EUID -ne 0 ]]; then
    echo "This script requires root privileges. Re-running with sudo..."
    exec sudo env "PATH=$PATH" "$0" "$@"
fi

echo ""
echo "================================================================"
echo "  vmcontrol v${VERSION} — Linux Installer"
echo "================================================================"
echo ""

# --- Step 1: Prerequisites ---
info "Checking prerequisites..."

# Rust / cargo
if ! command -v cargo &>/dev/null; then
    error "Rust toolchain not found."
    echo "  Install: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    exit 1
fi
success "cargo $(cargo --version 2>/dev/null | awk '{print $2}')"

# QEMU
if [[ ! -x "$QEMU_PATH" ]]; then
    error "QEMU not found at $QEMU_PATH"
    echo "  Install:"
    echo "    Ubuntu/Debian: sudo apt install qemu-system-x86"
    echo "    CentOS/RHEL:   sudo dnf install qemu-system-x86-core"
    echo "    Arch:           sudo pacman -S qemu-system-x86"
    exit 1
fi
success "qemu-system-x86_64 found"

# qemu-img
if [[ ! -x "$QEMU_IMG_PATH" ]]; then
    warn "qemu-img not found at $QEMU_IMG_PATH"
    echo "       Install: sudo apt install qemu-utils  (or equivalent)"
fi

# genisoimage (for seed ISO)
if ! command -v genisoimage &>/dev/null && ! command -v mkisofs &>/dev/null; then
    warn "genisoimage/mkisofs not found (needed for cloud-init seed ISO)"
    echo "       Install: sudo apt install genisoimage"
fi

# websockify (optional)
if ! command -v websockify &>/dev/null; then
    warn "websockify not found (optional, needed for VNC proxy)"
    echo "       Install: pip3 install websockify"
fi

echo ""

# --- Step 2: Build from source ---
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

info "Building vm_ctl from source (release mode)..."
cargo build --release 2>&1 | tail -5

BINARY="$SCRIPT_DIR/target/release/vm_ctl"
if [[ ! -f "$BINARY" ]]; then
    error "Build failed: binary not found at $BINARY"
    exit 1
fi
success "Binary built: $(du -h "$BINARY" | awk '{print $1}')"
echo ""

# --- Step 3: Create directories ---
info "Creating directories..."
mkdir -p "$CTL_BIN"
mkdir -p "$PCTL_PATH"
mkdir -p "$DISK_PATH"
mkdir -p "$ISO_PATH"
mkdir -p "$LIVE_PATH"
mkdir -p "$STATIC_DIR"
success "Directories created"

# --- Step 4: Copy binary & static files ---
info "Installing binary and static files..."

# Stop existing service before overwriting binary
if systemctl is-active --quiet "$SERVICE_NAME" 2>/dev/null; then
    info "Stopping existing service..."
    systemctl stop "$SERVICE_NAME" || true
fi

cp "$BINARY" "$CTL_BIN/vm_ctl"
chmod +x "$CTL_BIN/vm_ctl"
cp -r "$SCRIPT_DIR/static/"* "$STATIC_DIR/"
success "Binary installed to $CTL_BIN/vm_ctl"
success "Static files installed to $STATIC_DIR/"

# --- Step 5: Generate config.yaml ---
if [[ ! -f "$CONFIG_YAML" ]]; then
    info "Generating default config.yaml..."
    cat > "$CONFIG_YAML" << 'YAML'
qemu_path: /usr/bin/qemu-system-x86_64
qemu_img_path: /usr/bin/qemu-img
ctl_bin_path: /opt/ctl/bin
pctl_path: /tmp/vmcontrol
disk_path: /tmp/vmcontrol/disks
iso_path: /tmp/vmcontrol/iso
live_path: /tmp/vmcontrol/backups
gzip_path: /usr/bin/gzip
websockify_path: websockify
vs_up_script: vs-up.sh
vs_down_script: vs-down.sh
pctl_script: pctl.sh
domain: localhost
YAML
    success "config.yaml created"
else
    warn "config.yaml already exists — skipping (preserving your customizations)"
fi

# --- Step 6: Set up systemd service ---
info "Setting up systemd service..."

cat > "$SYSTEMD_UNIT" << UNIT
[Unit]
Description=vmcontrol VM Management Server
After=network.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=${CTL_BIN}
ExecStart=${CTL_BIN}/vm_ctl server 0.0.0.0:8080
Restart=on-failure
RestartSec=5
StandardOutput=append:${LOG_DIR}/vm_ctl.stdout.log
StandardError=append:${LOG_DIR}/vm_ctl.stderr.log

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl enable "$SERVICE_NAME"
systemctl start "$SERVICE_NAME"
success "Service enabled and started"

echo ""

# --- Firewall hints ---
warn "If you have a firewall enabled, allow port 8080:"
echo "       UFW:       sudo ufw allow 8080/tcp"
echo "       firewalld: sudo firewall-cmd --add-port=8080/tcp --permanent && sudo firewall-cmd --reload"

echo ""

# --- Step 7: Summary ---
echo "================================================================"
echo -e "  ${GREEN}vmcontrol v${VERSION} installed successfully!${NC}"
echo "================================================================"
echo ""
echo "  Binary:      $CTL_BIN/vm_ctl"
echo "  Static:      $STATIC_DIR/"
echo "  Config:      $CONFIG_YAML"
echo "  Data:        $PCTL_PATH/"
echo "  Disks:       $DISK_PATH/"
echo "  ISOs:        $ISO_PATH/"
echo "  Backups:     $LIVE_PATH/"
echo "  DB:          $PCTL_PATH/vmcontrol.db (auto-created)"
echo "  Logs:        $LOG_DIR/vm_ctl.{stdout,stderr}.log"
echo "  Service:     $SERVICE_NAME (systemd)"
echo ""
echo "  Web UI:      http://localhost:8080"
echo ""
echo "  Commands:"
echo "    sudo systemctl status $SERVICE_NAME"
echo "    sudo systemctl stop $SERVICE_NAME"
echo "    sudo systemctl start $SERVICE_NAME"
echo "    sudo systemctl restart $SERVICE_NAME"
echo "    sudo journalctl -u $SERVICE_NAME -f"
echo ""
echo "================================================================"
