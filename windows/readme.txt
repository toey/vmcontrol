================================================================
  vmcontrol -- Windows Installation Guide
================================================================

PREREQUISITES
-------------

1. Rust Toolchain
   - Install from: https://rustup.rs
   - After installing, run these commands in PowerShell:

     rustup toolchain install stable-x86_64-pc-windows-gnu --force-non-host
     rustup default stable-x86_64-pc-windows-gnu --force-non-host

2. QEMU
   - Install from: https://qemu.weilnetz.de/w64/
   - Default path: C:\Program Files\qemu\

3. websockify (optional, for VNC proxy)
   - Install: pip install websockify


INSTALLATION
------------

1. Open PowerShell or Command Prompt as Administrator
2. Navigate to the windows folder
3. Run: install.bat
4. The installer will:
   - Build vm_ctl.exe from source
   - Create directories under C:\vmcontrol\
   - Install as a Windows service (via NSSM or Scheduled Task)
   - Add a firewall rule for port 8080
5. Access the Web UI at: http://localhost:8080


TROUBLESHOOTING
---------------

"link.exe not found"
  Rust defaults to the MSVC toolchain which requires Visual Studio
  Build Tools. To avoid installing Visual Studio, switch to the
  GNU toolchain instead:

    rustup toolchain install stable-x86_64-pc-windows-gnu --force-non-host
    rustup default stable-x86_64-pc-windows-gnu --force-non-host

"OS error 87" / "failed to remove temporary directory"
  This happens when building on a shared or network filesystem
  (e.g. Parallels shared folders). The installer already sets
  CARGO_TARGET_DIR=C:\vmcontrol\_build to work around this.
  If you still see this error, run this before building:

    set CARGO_TARGET_DIR=C:\vmcontrol\_build

"cargo not found"
  Install the Rust toolchain from https://rustup.rs and restart
  your terminal.


INSTALLED PATHS
---------------

  Binary:    C:\vmcontrol\bin\vm_ctl.exe
  Config:    C:\vmcontrol\bin\config.yaml
  Static:    C:\vmcontrol\bin\static\
  Disks:     C:\vmcontrol\disks\
  ISOs:      C:\vmcontrol\iso\
  Backups:   C:\vmcontrol\backups\
  Logs:      C:\vmcontrol\logs\
  Database:  C:\vmcontrol\vmcontrol.db (auto-created)


SERVICE COMMANDS
----------------

  With NSSM:
    nssm status vmcontrol
    nssm stop vmcontrol
    nssm start vmcontrol
    nssm restart vmcontrol

  With Scheduled Task:
    schtasks /query /tn vmcontrol
    schtasks /run /tn vmcontrol
    schtasks /end /tn vmcontrol
