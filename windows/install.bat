@echo off
setlocal enabledelayedexpansion

:: ================================================================
::  vmcontrol Installer for Windows
:: ================================================================

set "VERSION=0.2.0"

:: --- Path constants (must match windows\src\config.rs) ---
set "QEMU_PATH=C:\Program Files\qemu\qemu-system-x86_64.exe"
set "QEMU_IMG_PATH=C:\Program Files\qemu\qemu-img.exe"
set "CTL_BIN=C:\vmcontrol\bin"
set "CONFIG_YAML=C:\vmcontrol\bin\config.yaml"
set "PCTL_PATH=C:\vmcontrol"
set "DISK_PATH=C:\vmcontrol\disks"
set "ISO_PATH=C:\vmcontrol\iso"
set "LIVE_PATH=C:\vmcontrol\backups"
set "STATIC_DIR=C:\vmcontrol\bin\static"
set "LOG_DIR=C:\vmcontrol\logs"
set "SERVICE_NAME=vmcontrol"

echo.
echo ================================================================
echo   vmcontrol v%VERSION% -- Windows Installer
echo ================================================================
echo.

:: --- Step 1: Administrator check ---
net session >nul 2>&1
if %errorlevel% neq 0 (
    echo [ERR] This script must be run as Administrator.
    echo       Right-click the script and select "Run as administrator".
    pause
    exit /b 1
)
echo [OK]   Running as Administrator

:: --- Step 1b: Architecture detection ---
echo.
echo [INFO] Detecting CPU architecture...
set "ARCH=%PROCESSOR_ARCHITECTURE%"
echo [OK]   Architecture: %ARCH%

if /I "%ARCH%"=="ARM64" (
    set "QEMU_DOWNLOAD_URL=https://qemu.weilnetz.de/aarch64/"
    echo [INFO] ARM64 detected -- QEMU ARM64 native build required
    echo [INFO] x86_64 QEMU will CRASH on ARM64 due to JIT-inside-JIT emulation
) else (
    set "QEMU_DOWNLOAD_URL=https://qemu.weilnetz.de/w64/"
    echo [INFO] x86_64 detected -- standard QEMU build OK
)

:: --- Step 2: Prerequisites ---
echo.
echo [INFO] Checking prerequisites...

where cargo >nul 2>&1
if %errorlevel% neq 0 (
    echo [ERR] Rust toolchain not found.
    echo       Install from: https://rustup.rs
    pause
    exit /b 1
)
echo [OK]   cargo found

if not exist "%QEMU_PATH%" (
    echo [ERR] QEMU not found at %QEMU_PATH%
    echo       Install from: %QEMU_DOWNLOAD_URL%
    pause
    exit /b 1
)
echo [OK]   qemu-system-x86_64 found

:: Check if installed QEMU matches host architecture
if /I "%ARCH%"=="ARM64" (
    echo [INFO] Verifying QEMU is ARM64 native build...
    "%QEMU_PATH%" --version >nul 2>&1
    if !errorlevel! neq 0 (
        echo.
        echo [ERR] ============================================================
        echo [ERR]  QEMU binary is NOT compatible with ARM64!
        echo [ERR]  The installed QEMU is an x86_64 build which will crash
        echo [ERR]  on Windows ARM due to JIT-inside-JIT emulation.
        echo [ERR]
        echo [ERR]  Please download the ARM64 native build from:
        echo [ERR]    %QEMU_DOWNLOAD_URL%
        echo [ERR]
        echo [ERR]  Uninstall current QEMU, then install the ARM64 version.
        echo [ERR] ============================================================
        echo.
        pause
        exit /b 1
    )
    echo [OK]   QEMU verified working on ARM64
)

if not exist "%QEMU_IMG_PATH%" (
    echo [WARN] qemu-img not found at %QEMU_IMG_PATH%
)

:: --- Detect real Python path ---
echo [INFO] Searching for Python installation...
set "PYTHON_EXE="
set "WEBSOCKIFY_EXE="

:: Search %LOCALAPPDATA%\Python\*\python.exe (e.g. pythoncore-3.14-64)
for /d %%D in ("%LOCALAPPDATA%\Python\*") do (
    if exist "%%D\python.exe" (
        if not defined PYTHON_EXE (
            set "PYTHON_EXE=%%D\python.exe"
        )
    )
    if exist "%%D\Scripts\websockify.exe" (
        if not defined WEBSOCKIFY_EXE (
            set "WEBSOCKIFY_EXE=%%D\Scripts\websockify.exe"
        )
    )
)
:: Search %LOCALAPPDATA%\Programs\Python\*\python.exe
for /d %%D in ("%LOCALAPPDATA%\Programs\Python\*") do (
    if exist "%%D\python.exe" (
        if not defined PYTHON_EXE (
            set "PYTHON_EXE=%%D\python.exe"
        )
    )
    if exist "%%D\Scripts\websockify.exe" (
        if not defined WEBSOCKIFY_EXE (
            set "WEBSOCKIFY_EXE=%%D\Scripts\websockify.exe"
        )
    )
)

if defined PYTHON_EXE (
    echo [OK]   Python found: %PYTHON_EXE%
) else (
    echo [WARN] Python not found. VNC proxy will not work.
    echo        Install Python from: https://www.python.org/downloads/
)

:: Install websockify if python found but websockify missing
if defined PYTHON_EXE (
    if not defined WEBSOCKIFY_EXE (
        echo [INFO] websockify not found. Installing via pip...
        "%PYTHON_EXE%" -m pip install websockify >nul 2>&1
        if !errorlevel! neq 0 (
            echo [WARN] Failed to install websockify. Install manually: pip install websockify
        ) else (
            echo [OK]   websockify installed
            :: Re-search for websockify.exe after install
            for /d %%D in ("%LOCALAPPDATA%\Python\*") do (
                if exist "%%D\Scripts\websockify.exe" (
                    if not defined WEBSOCKIFY_EXE set "WEBSOCKIFY_EXE=%%D\Scripts\websockify.exe"
                )
            )
            for /d %%D in ("%LOCALAPPDATA%\Programs\Python\*") do (
                if exist "%%D\Scripts\websockify.exe" (
                    if not defined WEBSOCKIFY_EXE set "WEBSOCKIFY_EXE=%%D\Scripts\websockify.exe"
                )
            )
        )
    ) else (
        echo [OK]   websockify found: %WEBSOCKIFY_EXE%
    )
)

echo.

:: --- Step 3: Build or locate binary ---
set "SCRIPT_DIR=%~dp0"
cd /d "%SCRIPT_DIR%"

:: Check for pre-built binary first (cross-compiled from Mac/Linux)
set "PREBUILT=%SCRIPT_DIR%target\x86_64-pc-windows-gnu\release\vm_ctl.exe"
set "PREBUILT2=%SCRIPT_DIR%vm_ctl.exe"

if exist "%PREBUILT%" (
    set "BINARY=%PREBUILT%"
    echo [OK]   Found pre-built binary at %PREBUILT%
) else if exist "%PREBUILT2%" (
    set "BINARY=%PREBUILT2%"
    echo [OK]   Found pre-built binary at %PREBUILT2%
) else (
    echo [INFO] No pre-built binary found. Building from source...

    :: Use a local target directory to avoid OS error 87 on shared/network filesystems
    set "CARGO_TARGET_DIR=C:\vmcontrol\_build"

    echo [INFO] Building vm_ctl from source ^(release mode^)...
    cargo build --release
    if !errorlevel! neq 0 (
        echo [ERR] Build failed.
        echo       Alternatively, cross-compile from Mac: cargo build --release --target x86_64-pc-windows-gnu
        echo       Then place vm_ctl.exe in this folder and re-run install.bat
        pause
        exit /b 1
    )
    set "BINARY=!CARGO_TARGET_DIR!\release\vm_ctl.exe"
)

if not exist "%BINARY%" (
    echo [ERR] Binary not found at %BINARY%
    pause
    exit /b 1
)
echo [OK]   Binary ready
echo.

:: --- Step 4: Create directories ---
echo [INFO] Creating directories...
if not exist "%CTL_BIN%" mkdir "%CTL_BIN%"
if not exist "%PCTL_PATH%" mkdir "%PCTL_PATH%"
if not exist "%DISK_PATH%" mkdir "%DISK_PATH%"
if not exist "%ISO_PATH%" mkdir "%ISO_PATH%"
if not exist "%LIVE_PATH%" mkdir "%LIVE_PATH%"
if not exist "%STATIC_DIR%" mkdir "%STATIC_DIR%"
if not exist "%LOG_DIR%" mkdir "%LOG_DIR%"
echo [OK]   Directories created

:: --- Step 5: Stop existing service ---
where nssm >nul 2>&1
if %errorlevel% equ 0 (
    nssm status %SERVICE_NAME% >nul 2>&1
    if !errorlevel! equ 0 (
        echo [INFO] Stopping existing service...
        nssm stop %SERVICE_NAME% >nul 2>&1
        nssm remove %SERVICE_NAME% confirm >nul 2>&1
    )
)

:: --- Step 6: Copy binary and static files ---
echo [INFO] Installing binary and static files...
copy /y "%BINARY%" "%CTL_BIN%\vm_ctl.exe" >nul
xcopy /s /y /i /q "%SCRIPT_DIR%static\*" "%STATIC_DIR%\" >nul
echo [OK]   Binary installed to %CTL_BIN%\vm_ctl.exe
echo [OK]   Static files installed to %STATIC_DIR%\

:: --- Step 7: Generate config.yaml ---
:: Set detected paths for config
set "CONF_PYTHON=python3"
if defined PYTHON_EXE set "CONF_PYTHON=%PYTHON_EXE%"
set "CONF_WEBSOCKIFY=websockify"
if defined WEBSOCKIFY_EXE set "CONF_WEBSOCKIFY=%WEBSOCKIFY_EXE%"

if not exist "%CONFIG_YAML%" (
    echo [INFO] Generating config.yaml with detected paths...
    (
        echo qemu_path: %QEMU_PATH%
        echo qemu_img_path: %QEMU_IMG_PATH%
        echo ctl_bin_path: C:\vmcontrol\bin
        echo pctl_path: C:\vmcontrol
        echo disk_path: C:\vmcontrol\disks
        echo iso_path: C:\vmcontrol\iso
        echo live_path: C:\vmcontrol\backups
        echo gzip_path: gzip
        echo python_path: %CONF_PYTHON%
        echo websockify_path: %CONF_WEBSOCKIFY%
        echo vs_up_script: vs-up.bat
        echo vs_down_script: vs-down.bat
        echo pctl_script: pctl.bat
        echo domain: localhost
    ) > "%CONFIG_YAML%"
    echo [OK]   config.yaml created
    echo [OK]   python_path: %CONF_PYTHON%
    echo [OK]   websockify_path: %CONF_WEBSOCKIFY%
) else (
    echo [WARN] config.yaml already exists -- skipping ^(preserving your customizations^)
    echo [INFO] To update python/websockify paths, edit %CONFIG_YAML%
    echo [INFO]   python_path: %CONF_PYTHON%
    echo [INFO]   websockify_path: %CONF_WEBSOCKIFY%
)

echo.

:: --- Step 8: Set up Windows Service ---
echo [INFO] Setting up Windows service...

where nssm >nul 2>&1
if %errorlevel% equ 0 (
    echo [INFO] Installing service via NSSM...
    nssm install %SERVICE_NAME% "%CTL_BIN%\vm_ctl.exe" server 0.0.0.0:8080
    nssm set %SERVICE_NAME% AppDirectory "%CTL_BIN%"
    nssm set %SERVICE_NAME% DisplayName "vmcontrol VM Management Server"
    nssm set %SERVICE_NAME% Description "QEMU VM management control panel"
    nssm set %SERVICE_NAME% Start SERVICE_AUTO_START
    nssm set %SERVICE_NAME% AppStdout "%LOG_DIR%\vm_ctl.stdout.log"
    nssm set %SERVICE_NAME% AppStderr "%LOG_DIR%\vm_ctl.stderr.log"
    nssm set %SERVICE_NAME% AppRotateFiles 1
    nssm set %SERVICE_NAME% AppRotateBytes 10485760
    nssm start %SERVICE_NAME%
    echo [OK]   Service installed and started via NSSM
) else (
    echo [WARN] NSSM not found. Using Scheduled Task as fallback...
    echo        Download NSSM: https://nssm.cc/download
    echo.
    :: Create a launcher script that sets working directory before running
    (
        echo @echo off
        echo cd /d "%CTL_BIN%"
        echo "%CTL_BIN%\vm_ctl.exe" server 0.0.0.0:8080
    ) > "%CTL_BIN%\start_vmcontrol.bat"
    schtasks /delete /tn "%SERVICE_NAME%" /f >nul 2>&1
    schtasks /create /tn "%SERVICE_NAME%" /tr "\"%CTL_BIN%\start_vmcontrol.bat\"" /sc onstart /ru SYSTEM /f >nul
    if !errorlevel! equ 0 (
        echo [OK]   Scheduled Task created ^(starts on boot^)
        echo [INFO] Starting vmcontrol now...
        schtasks /run /tn "%SERVICE_NAME%" >nul 2>&1
    ) else (
        echo [WARN] Failed to create Scheduled Task.
        echo        You can start manually: "%CTL_BIN%\vm_ctl.exe" server
    )
)

:: --- Step 9: Firewall rule ---
echo.
echo [INFO] Adding firewall rule for port 8080...
netsh advfirewall firewall delete rule name="vmcontrol" >nul 2>&1
netsh advfirewall firewall add rule name="vmcontrol" dir=in action=allow protocol=tcp localport=8080 enable=yes >nul
echo [OK]   Firewall rule added

echo.

:: --- Step 10: Create Desktop shortcut (Admin PowerShell) ---
echo [INFO] Creating desktop shortcut...
set "DESKTOP=%USERPROFILE%\Desktop"
set "SHORTCUT=%DESKTOP%\vmcontrol.lnk"

powershell -NoProfile -Command "$ws = New-Object -ComObject WScript.Shell; $sc = $ws.CreateShortcut('%SHORTCUT%'); $sc.TargetPath = 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe'; $sc.Arguments = '-NoExit -Command cd C:\vmcontrol; C:\vmcontrol\bin\vm_ctl.exe server 0.0.0.0:8080'; $sc.WorkingDirectory = 'C:\vmcontrol'; $sc.Description = 'vmcontrol Server (Admin)'; $sc.Save()"

:: Set shortcut to Run as Administrator
powershell -NoProfile -Command "$bytes = [System.IO.File]::ReadAllBytes('%SHORTCUT%'); $bytes[0x15] = $bytes[0x15] -bor 0x20; [System.IO.File]::WriteAllBytes('%SHORTCUT%', $bytes)"

echo [OK]   Desktop shortcut created: %SHORTCUT%
echo.

:: --- Step 11: Summary ---
echo ================================================================
echo   vmcontrol v%VERSION% installed successfully!
echo ================================================================
echo.
echo   Binary:      %CTL_BIN%\vm_ctl.exe
echo   Static:      %STATIC_DIR%\
echo   Config:      %CONFIG_YAML%
echo   Data:        %PCTL_PATH%\
echo   Disks:       %DISK_PATH%\
echo   ISOs:        %ISO_PATH%\
echo   Backups:     %LIVE_PATH%\
echo   Logs:        %LOG_DIR%\
echo   DB:          %PCTL_PATH%\vmcontrol.db ^(auto-created^)
echo.
echo   Web UI:      http://localhost:8080
echo.
where nssm >nul 2>&1
if %errorlevel% equ 0 (
    echo   Service:     %SERVICE_NAME% ^(NSSM^)
    echo.
    echo   Commands:
    echo     nssm status %SERVICE_NAME%
    echo     nssm stop %SERVICE_NAME%
    echo     nssm start %SERVICE_NAME%
    echo     nssm restart %SERVICE_NAME%
) else (
    echo   Service:     %SERVICE_NAME% ^(Scheduled Task^)
    echo.
    echo   Commands:
    echo     schtasks /query /tn %SERVICE_NAME%
    echo     schtasks /run /tn %SERVICE_NAME%
    echo     schtasks /end /tn %SERVICE_NAME%
)
echo.
echo ================================================================
echo.
pause
