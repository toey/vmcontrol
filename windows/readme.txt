================================================================
  vmcontrol -- Windows Installation Guide
================================================================

PREREQUISITES
-------------

1. Rust Toolchain
   - Install from: https://rustup.rs
   - You need EITHER the MSVC or GNU toolchain:

     Option A -- MSVC (recommended):
       Install "Visual Studio Build Tools" from:
       https://visualstudio.microsoft.com/visual-cpp-build-tools/
       Select "Desktop development with C++" workload.
       Then run:
         rustup default stable-msvc

     Option B -- GNU (no Visual Studio required):
       Requires MinGW-w64 which provides gcc, dlltool, etc.
       Install via: winget install -e --id MSYS2.MSYS2
       Or download from: https://github.com/niXman/mingw-builds-binaries/releases
       Add the MinGW bin folder to your PATH, then run:
         rustup toolchain install stable-x86_64-pc-windows-gnu
         rustup default stable-x86_64-pc-windows-gnu

2. QEMU
   - x86_64: https://qemu.weilnetz.de/w64/
   - ARM64:  https://qemu.weilnetz.de/aarch64/
   - Default path: C:\Program Files\qemu\


INSTALLATION
------------

1. Open PowerShell or Command Prompt as Administrator
2. Navigate to the windows folder
3. Run: install.bat
4. The installer will:
   - Build vm_ctl.exe from source (or use pre-built binary)
   - Create directories under C:\vmcontrol\
   - Install as a Windows service (via NSSM or Scheduled Task)
   - Add a firewall rule for port 8080
5. Access the Web UI at: http://localhost:8080


CROSS-COMPILE FROM MAC (alternative)
-------------------------------------

If building on Windows is problematic, you can cross-compile
from macOS:

  cd windows
  cargo build --release --target x86_64-pc-windows-gnu

Then copy target/x86_64-pc-windows-gnu/release/vm_ctl.exe to
the windows folder on the Windows machine and re-run install.bat.
It will detect the pre-built binary and skip building.


TROUBLESHOOTING
---------------

"dlltool.exe not found" / "program not found"
  The GNU toolchain requires MinGW-w64. Either:
  - Install MinGW-w64 (see Prerequisites Option B above)
  - Or switch to MSVC toolchain (see Prerequisites Option A above)

"link.exe not found"
  The MSVC toolchain requires Visual Studio Build Tools.
  Either install Build Tools or switch to the GNU toolchain:

    rustup toolchain install stable-x86_64-pc-windows-gnu
    rustup default stable-x86_64-pc-windows-gnu

  Note: GNU toolchain also requires MinGW-w64.

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
