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
PCTL_PATH="/opt/ctl/data"
DISK_PATH="/opt/ctl/data/disks"
ISO_PATH="/opt/ctl/data/iso"
LIVE_PATH="/opt/ctl/data/backups"
STATIC_DIR="/opt/ctl/bin/static"
LOG_DIR="/opt/ctl/data"
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

# Open vSwitch (required for virtual switches)
if ! command -v ovs-vsctl &>/dev/null; then
    warn "Open vSwitch not found (required for virtual switches)"
    echo "       Install: sudo apt install openvswitch-switch"
else
    success "Open vSwitch found"
fi

echo ""

# --- Step 1b: Stop old processes before build ---
info "Checking for running vm_ctl processes..."
if systemctl is-active --quiet "$SERVICE_NAME" 2>/dev/null; then
    info "Stopping existing systemd service..."
    systemctl stop "$SERVICE_NAME" || true
    success "Service stopped"
fi
# Kill any stray vm_ctl processes not managed by systemd
if pgrep -x vm_ctl &>/dev/null; then
    info "Killing stray vm_ctl processes..."
    pkill -x vm_ctl 2>/dev/null || true
    sleep 1
    # Force kill if still alive
    if pgrep -x vm_ctl &>/dev/null; then
        pkill -9 -x vm_ctl 2>/dev/null || true
    fi
    success "Stray processes killed"
else
    success "No running vm_ctl processes found"
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

# --- Step 3b: Migrate data from /tmp/vmcontrol (if exists) ---
OLD_DATA="/tmp/vmcontrol"
if [[ -d "$OLD_DATA" && "$PCTL_PATH" != "$OLD_DATA" ]]; then
    info "Migrating data from $OLD_DATA to $PCTL_PATH..."
    for sub in disks iso backups; do
        if [[ -d "$OLD_DATA/$sub" ]] && ls "$OLD_DATA/$sub"/* &>/dev/null; then
            cp -a "$OLD_DATA/$sub/"* "$PCTL_PATH/$sub/" 2>/dev/null && \
                success "Migrated $sub" || warn "Failed to migrate $sub"
        fi
    done
    for f in vmcontrol.db mds.json .api_key; do
        if [[ -f "$OLD_DATA/$f" ]]; then
            cp -a "$OLD_DATA/$f" "$PCTL_PATH/$f" 2>/dev/null && \
                success "Migrated $f" || warn "Failed to migrate $f"
        fi
    done
    success "Data migration complete"
fi

# --- Step 4: Copy binary & static files ---
info "Installing binary and static files..."

cp "$BINARY" "$CTL_BIN/vm_ctl"
chmod +x "$CTL_BIN/vm_ctl"
# Prefer root static/ (single source of truth), fall back to platform static/
if [[ -d "$SCRIPT_DIR/../static" ]]; then
    cp -r "$SCRIPT_DIR/../static/"* "$STATIC_DIR/"
    success "Static files installed from repo root"
elif [[ -d "$SCRIPT_DIR/static" ]]; then
    cp -r "$SCRIPT_DIR/static/"* "$STATIC_DIR/"
    success "Static files installed from platform dir"
else
    error "Static files not found!"
    exit 1
fi
success "Binary installed to $CTL_BIN/vm_ctl"
success "Static files installed to $STATIC_DIR/"

# --- Step 5: Generate config.yaml ---
if [[ ! -f "$CONFIG_YAML" ]]; then
    info "Generating default config.yaml..."
    cat > "$CONFIG_YAML" << 'YAML'
qemu_path: /usr/bin/qemu-system-x86_64
qemu_img_path: /usr/bin/qemu-img
ctl_bin_path: /opt/ctl/bin
pctl_path: /opt/ctl/data
disk_path: /opt/ctl/data/disks
iso_path: /opt/ctl/data/iso
live_path: /opt/ctl/data/backups
gzip_path: /usr/bin/gzip
websockify_path: websockify
vs_up_script: vs-up.sh
vs_down_script: vs-down.sh
pctl_script: pctl.sh
domain: localhost
YAML
    success "config.yaml created"
else
    # Migrate old /tmp/vmcontrol paths to /opt/ctl/data
    if grep -q '/tmp/vmcontrol' "$CONFIG_YAML" 2>/dev/null; then
        info "Migrating config.yaml paths from /tmp/vmcontrol to /opt/ctl/data..."
        sed -i 's|/tmp/vmcontrol|/opt/ctl/data|g' "$CONFIG_YAML"
        success "config.yaml paths migrated"
    else
        warn "config.yaml already exists — skipping (preserving your customizations)"
    fi
fi

# --- Step 6: Generate API key (always regenerate on install) ---
API_KEY_FILE="${PCTL_PATH}/.api_key"
API_KEY=$(openssl rand -hex 32 2>/dev/null || LC_ALL=C tr -dc 'a-f0-9' < /dev/urandom | head -c 64)
echo "$API_KEY" > "$API_KEY_FILE"
chmod 600 "$API_KEY_FILE"
success "API key generated and saved to $API_KEY_FILE"

# --- Step 7: Set up systemd service ---
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
Environment=VMCONTROL_API_KEY=${API_KEY}
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

# --- Step 8: Summary ---
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
echo -e "  API Key:     ${YELLOW}${API_KEY}${NC}"
echo "  Key File:    $API_KEY_FILE"
echo ""
echo "  Commands:"
echo "    sudo systemctl status $SERVICE_NAME"
echo "    sudo systemctl stop $SERVICE_NAME"
echo "    sudo systemctl start $SERVICE_NAME"
echo "    sudo systemctl restart $SERVICE_NAME"
echo "    sudo journalctl -u $SERVICE_NAME -f"
echo ""
echo "================================================================"
