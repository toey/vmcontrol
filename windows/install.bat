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
    echo       Install from: https://qemu.weilnetz.de/w64/
    pause
    exit /b 1
)
echo [OK]   qemu-system-x86_64 found

if not exist "%QEMU_IMG_PATH%" (
    echo [WARN] qemu-img not found at %QEMU_IMG_PATH%
)

where websockify >nul 2>&1
if %errorlevel% neq 0 (
    echo [WARN] websockify not found ^(optional, needed for VNC proxy^)
    echo        Install: pip install websockify
)

echo.

:: --- Step 3: Build from source ---
set "SCRIPT_DIR=%~dp0"
cd /d "%SCRIPT_DIR%"

echo [INFO] Building vm_ctl from source ^(release mode^)...
cargo build --release
if %errorlevel% neq 0 (
    echo [ERR] Build failed.
    pause
    exit /b 1
)

set "BINARY=%SCRIPT_DIR%target\release\vm_ctl.exe"
if not exist "%BINARY%" (
    echo [ERR] Binary not found at %BINARY%
    pause
    exit /b 1
)
echo [OK]   Binary built successfully
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
if not exist "%CONFIG_YAML%" (
    echo [INFO] Generating default config.yaml...
    (
        echo qemu_path: C:\Program Files\qemu\qemu-system-x86_64.exe
        echo qemu_img_path: C:\Program Files\qemu\qemu-img.exe
        echo ctl_bin_path: C:\vmcontrol\bin
        echo pctl_path: C:\vmcontrol
        echo disk_path: C:\vmcontrol\disks
        echo iso_path: C:\vmcontrol\iso
        echo live_path: C:\vmcontrol\backups
        echo gzip_path: gzip
        echo websockify_path: websockify
        echo vs_up_script: vs-up.bat
        echo vs_down_script: vs-down.bat
        echo pctl_script: pctl.bat
        echo domain: localhost
    ) > "%CONFIG_YAML%"
    echo [OK]   config.yaml created
) else (
    echo [WARN] config.yaml already exists -- skipping ^(preserving your customizations^)
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
    schtasks /delete /tn "%SERVICE_NAME%" /f >nul 2>&1
    schtasks /create /tn "%SERVICE_NAME%" /tr "\"%CTL_BIN%\vm_ctl.exe\" server 0.0.0.0:8080" /sc onstart /ru SYSTEM /f >nul
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

:: --- Step 10: Summary ---
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
