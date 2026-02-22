#!/usr/bin/env bash
set -euo pipefail

# ================================================================
#  vmcontrol Installer for macOS
# ================================================================

VERSION="0.2.0"

# --- Path constants (must match mac/src/config.rs) ---
QEMU_PATH="/opt/homebrew/bin/qemu-system-x86_64"
QEMU_IMG_PATH="/opt/homebrew/bin/qemu-img"
CTL_BIN="/opt/ctl/bin"
CONFIG_YAML="/opt/ctl/bin/config.yaml"
PCTL_PATH="/tmp/vmcontrol"
DISK_PATH="/tmp/vmcontrol/disks"
ISO_PATH="/tmp/vmcontrol/iso"
LIVE_PATH="/tmp/vmcontrol/backups"
STATIC_DIR="/opt/ctl/bin/static"
LOG_DIR="/tmp/vmcontrol"
SERVICE_LABEL="com.vmcontrol.vm_ctl"
PLIST_PATH="/Library/LaunchDaemons/${SERVICE_LABEL}.plist"

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
echo "  vmcontrol v${VERSION} — macOS Installer"
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
    echo "  Install: brew install qemu"
    exit 1
fi
success "qemu-system-x86_64 found"

# qemu-img
if [[ ! -x "$QEMU_IMG_PATH" ]]; then
    warn "qemu-img not found at $QEMU_IMG_PATH (installed with QEMU)"
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
if launchctl list "$SERVICE_LABEL" &>/dev/null; then
    info "Stopping existing service..."
    launchctl unload "$PLIST_PATH" 2>/dev/null || true
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
qemu_path: /opt/homebrew/bin/qemu-system-x86_64
qemu_img_path: /opt/homebrew/bin/qemu-img
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

# --- Step 6: Set up launchd service ---
info "Setting up launchd service..."

cat > "$PLIST_PATH" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${SERVICE_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>${CTL_BIN}/vm_ctl</string>
        <string>server</string>
        <string>0.0.0.0:8080</string>
    </array>
    <key>WorkingDirectory</key>
    <string>${CTL_BIN}</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>${LOG_DIR}/vm_ctl.stdout.log</string>
    <key>StandardErrorPath</key>
    <string>${LOG_DIR}/vm_ctl.stderr.log</string>
</dict>
</plist>
PLIST

launchctl load "$PLIST_PATH"
success "Service registered and started"

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
echo "  Service:     $SERVICE_LABEL (launchd)"
echo ""
echo "  Web UI:      http://localhost:8080"
echo ""
echo "  Commands:"
echo "    sudo launchctl stop $SERVICE_LABEL"
echo "    sudo launchctl start $SERVICE_LABEL"
echo "    sudo launchctl unload $PLIST_PATH"
echo "    sudo launchctl load $PLIST_PATH"
echo ""
echo "================================================================"
