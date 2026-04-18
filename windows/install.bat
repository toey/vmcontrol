@echo off
setlocal enabledelayedexpansion

:: ================================================================
::  vmcontrol Installer for Windows
:: ================================================================

set "VERSION=0.2.0"

:: --- Path constants (must match windows\src\config.rs) ---
set "QEMU_PATH=C:\Program Files\qemu\qemu-system-x86_64.exe"
set "QEMU_AARCH64_PATH=C:\Program Files\qemu\qemu-system-aarch64.exe"
set "QEMU_IMG_PATH=C:\Program Files\qemu\qemu-img.exe"
set "EDK2_AARCH64_BIOS=C:\Program Files\qemu\share\edk2-aarch64-code.fd"
set "EDK2_X86_SECURE_CODE=C:\Program Files\qemu\share\edk2-x86_64-secure-code.fd"
set "EDK2_X86_VARS=C:\Program Files\qemu\share\edk2-i386-vars.fd"
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

:: Skip Rust check when a pre-built vm_ctl.exe ships next to install.bat
:: (release ZIPs from GitHub Actions put the binary here, no build needed).
set "PREBUILT_CHECK=%~dp0vm_ctl.exe"
if exist "%PREBUILT_CHECK%" (
    echo [OK]   Pre-built vm_ctl.exe found -- skipping Rust toolchain check
) else (
    where cargo >nul 2>&1
    if !errorlevel! neq 0 (
        echo [INFO] Rust toolchain not found. Installing rustup directly...
        :: Download rustup-init.exe from rust-lang.org (avoids winget / msstore issues)
        set "RUSTUP_INIT=%TEMP%\rustup-init.exe"
        if /I "%ARCH%"=="ARM64" (
            set "RUSTUP_URL=https://static.rust-lang.org/rustup/dist/aarch64-pc-windows-msvc/rustup-init.exe"
            set "DEFAULT_HOST=aarch64-pc-windows-msvc"
        ) else (
            set "RUSTUP_URL=https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe"
            set "DEFAULT_HOST=x86_64-pc-windows-msvc"
        )
        echo [INFO] Downloading !RUSTUP_URL!
        powershell -NoProfile -Command "try { [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -Uri '!RUSTUP_URL!' -OutFile '!RUSTUP_INIT!' -UseBasicParsing; exit 0 } catch { Write-Host $_.Exception.Message; exit 1 }"
        if !errorlevel! neq 0 (
            echo [ERR] Failed to download rustup-init.exe.
            echo       Install Rust manually from: https://rustup.rs then re-run.
            pause
            exit /b 1
        )
        echo [INFO] Running rustup-init ^(silent, stable-msvc, default-host=!DEFAULT_HOST!^)...
        "!RUSTUP_INIT!" -y --default-toolchain stable-msvc --default-host !DEFAULT_HOST! --no-modify-path
        if !errorlevel! neq 0 (
            echo [ERR] rustup-init failed. Install Rust manually from: https://rustup.rs
            pause
            exit /b 1
        )
        :: rustup installs to %USERPROFILE%\.cargo\bin — add to this session's PATH.
        set "PATH=%USERPROFILE%\.cargo\bin;!PATH!"
        where cargo >nul 2>&1
        if !errorlevel! neq 0 (
            echo [ERR] Rust installed but cargo still not on PATH.
            echo       Close and re-open the terminal, then re-run install.bat.
            pause
            exit /b 1
        )
        echo [OK]   Rust installed ^(cargo on PATH for this session^)
    ) else (
        echo [OK]   cargo found
    )
)

:: --- Step 2a: Auto-install QEMU if missing ---
set "QEMU_MISSING=0"
if /I "%ARCH%"=="ARM64" (
    if not exist "%QEMU_AARCH64_PATH%" if not exist "%QEMU_PATH%" set "QEMU_MISSING=1"
) else (
    if not exist "%QEMU_PATH%" set "QEMU_MISSING=1"
)

if "!QEMU_MISSING!"=="1" (
    echo.
    echo [INFO] QEMU not found. Downloading latest installer from qemu.weilnetz.de...
    if /I "%ARCH%"=="ARM64" (
        set "QEMU_BASE_URL=https://qemu.weilnetz.de/aarch64/"
    ) else (
        set "QEMU_BASE_URL=https://qemu.weilnetz.de/w64/"
    )
    powershell -NoProfile -Command "[Net.ServicePointManager]::SecurityProtocol='Tls12'; $u='!QEMU_BASE_URL!'; $p=Invoke-WebRequest $u -UseBasicParsing; $f=$p.Links.href | Where-Object {$_ -match '^qemu-.*-setup-\d{8}\.exe$'} | Sort-Object -Descending | Select-Object -First 1; if(-not $f){Write-Host '[ERR] No installer URL found on '+$u; exit 2}; $setup=Join-Path $env:TEMP $f; Write-Host ('[INFO] Downloading '+$u+$f); Invoke-WebRequest ($u+$f) -OutFile $setup -UseBasicParsing; Write-Host '[INFO] Running silent install (/S)...'; $p2=Start-Process $setup -ArgumentList '/S' -Wait -PassThru; exit $p2.ExitCode"
    if !errorlevel! neq 0 (
        echo [ERR] QEMU auto-install failed. Download manually from:
        echo       !QEMU_BASE_URL!
        pause
        exit /b 1
    )
    echo [OK]   QEMU installed to C:\Program Files\qemu
    echo.
)

:: --- Step 2a.5: Auto-install TAP-Windows driver for VM-to-VM networking ---
:: QEMU on Windows has no built-in L2 networking for multi-VM; TAP-Windows
:: gives virtual NICs that vm_ctl attaches per switch adapter at VM start.
set "TAP_CTL=C:\Program Files\TAP-Windows\bin\tapinstall.exe"
if not exist "%TAP_CTL%" (
    echo [INFO] TAP-Windows driver not found. Downloading...
    set "TAP_URL=https://build.openvpn.net/downloads/releases/tap-windows-9.24.2-I601-Win10.exe"
    set "TAP_SETUP=%TEMP%\tap-windows-setup.exe"
    powershell -NoProfile -Command "try { [Net.ServicePointManager]::SecurityProtocol='Tls12'; Invoke-WebRequest '!TAP_URL!' -OutFile '!TAP_SETUP!' -UseBasicParsing; exit 0 } catch { Write-Host $_.Exception.Message; exit 1 }"
    if !errorlevel! neq 0 (
        echo [WARN] Failed to download TAP-Windows. Switch adapters will fall back to NAT.
    ) else (
        echo [INFO] Running silent install...
        "!TAP_SETUP!" /S
        if !errorlevel! neq 0 if !errorlevel! neq 3010 (
            echo [WARN] TAP-Windows installer exited with code !errorlevel!.
        ) else (
            echo [OK]   TAP-Windows driver installed
        )
    )
) else (
    echo [OK]   TAP-Windows driver already present
)

:: Ensure at least one TAP adapter exists so QEMU can attach to it.
:: tapinstall lists installed adapters; create one if none match tap0901.
if exist "%TAP_CTL%" (
    "%TAP_CTL%" hwids tap0901 2>nul | findstr /c:"TAP0901" >nul 2>&1
    if !errorlevel! neq 0 (
        echo [INFO] Creating initial TAP adapter ^(vmctl-tap0^)...
        "%TAP_CTL%" install "C:\Program Files\TAP-Windows\driver\OemVista.inf" tap0901 >nul 2>&1
        :: Rename newest "Ethernet N" / "Local Area Connection N" created by TAP driver.
        powershell -NoProfile -Command "$a = Get-NetAdapter | Where-Object { $_.InterfaceDescription -match 'TAP-Windows' } | Sort-Object -Property MediaConnectionState, ifIndex | Select-Object -First 1; if ($a) { Rename-NetAdapter -Name $a.Name -NewName 'vmctl-tap0' -ErrorAction SilentlyContinue; Enable-NetAdapter -Name 'vmctl-tap0' -Confirm:$false -ErrorAction SilentlyContinue }"
        echo [OK]   TAP adapter vmctl-tap0 provisioned
    )
)

echo.

:: --- Step 2a.8: Auto-install swtpm via MSYS2 (for Windows 11 guest TPM 2.0) ---
set "SWTPM_PATH=C:\msys64\mingw64\bin\swtpm.exe"
set "MKISOFS_PATH=C:\msys64\mingw64\bin\mkisofs.exe"
if not exist "%SWTPM_PATH%" (
    if not exist "C:\msys64\usr\bin\bash.exe" (
        echo [INFO] MSYS2 not installed. Downloading latest installer...
        set "MSYS_SETUP=%TEMP%\msys2-setup.exe"
        powershell -NoProfile -Command "try { [Net.ServicePointManager]::SecurityProtocol='Tls12'; $r = Invoke-RestMethod 'https://api.github.com/repos/msys2/msys2-installer/releases/latest' -Headers @{'User-Agent'='vmcontrol-installer'}; $a = $r.assets | Where-Object { $_.name -match '^msys2-x86_64-\d{8}\.exe$' } | Select-Object -First 1; if (-not $a) { exit 2 }; Write-Host ('[INFO] Downloading ' + $a.browser_download_url); Invoke-WebRequest $a.browser_download_url -OutFile '!MSYS_SETUP!' -UseBasicParsing; exit 0 } catch { Write-Host $_.Exception.Message; exit 1 }"
        if !errorlevel! neq 0 (
            echo [WARN] Failed to download MSYS2. TPM 2.0 won't be available.
            echo        Windows 11 guests need the Win11 Bypass button in the VNC UI.
            goto :after_swtpm
        )
        echo [INFO] Silent installing MSYS2 to C:\msys64 ^(takes several minutes^)...
        "!MSYS_SETUP!" in --confirm-command --accept-messages --root C:\msys64
        if !errorlevel! neq 0 if !errorlevel! neq 3010 (
            echo [WARN] MSYS2 installer exited with code !errorlevel!. TPM 2.0 may not work.
            goto :after_swtpm
        )
    )
    echo [INFO] Installing swtpm + cdrtools via pacman...
    C:\msys64\usr\bin\bash.exe -lc "pacman -Sy --noconfirm --needed mingw-w64-x86_64-swtpm mingw-w64-x86_64-cdrtools"
    if !errorlevel! neq 0 (
        echo [WARN] pacman install swtpm failed. Windows 11 guests will need Win11 Bypass.
        goto :after_swtpm
    )
    if exist "%SWTPM_PATH%" (
        echo [OK]   swtpm installed at %SWTPM_PATH%
    ) else (
        echo [WARN] swtpm.exe not found at expected path after install: %SWTPM_PATH%
    )
) else (
    echo [OK]   swtpm already present at %SWTPM_PATH%
)
:after_swtpm

echo.

:: On ARM64, check for qemu-system-aarch64 as primary binary
if /I "%ARCH%"=="ARM64" (
    if exist "%QEMU_AARCH64_PATH%" (
        echo [OK]   qemu-system-aarch64 found
    ) else (
        echo [WARN] qemu-system-aarch64 not found at %QEMU_AARCH64_PATH%
        echo        ARM64 native VMs will not be available
    )
    :: Also check if qemu-system-x86_64 exists (for x86_64 guest emulation)
    if exist "%QEMU_PATH%" (
        echo [OK]   qemu-system-x86_64 found ^(for x86_64 guest emulation^)
        echo [INFO] Verifying QEMU is ARM64 native build...
        "%QEMU_PATH%" --version >nul 2>&1
        if !errorlevel! neq 0 (
            echo [WARN] qemu-system-x86_64 appears to be an x86_64 build
            echo        It may crash on ARM64 due to JIT-inside-JIT emulation
            echo        Download ARM64 native build from: %QEMU_DOWNLOAD_URL%
        ) else (
            echo [OK]   QEMU verified working on ARM64
        )
    ) else (
        echo [WARN] qemu-system-x86_64 not found at %QEMU_PATH%
    )
    :: At least one QEMU binary must exist
    if not exist "%QEMU_AARCH64_PATH%" (
        if not exist "%QEMU_PATH%" (
            echo [ERR] No QEMU binary found. Install ARM64 QEMU from:
            echo       %QEMU_DOWNLOAD_URL%
            pause
            exit /b 1
        )
    )
    :: Check for EDK2 UEFI firmware
    if exist "%EDK2_AARCH64_BIOS%" (
        echo [OK]   EDK2 aarch64 UEFI firmware found
    ) else (
        :: Also check common alternative paths
        set "EDK2_ALT1=C:\Program Files\qemu\share\edk2-aarch64-code.fd"
        set "EDK2_ALT2=C:\Program Files\qemu\edk2-aarch64-code.fd"
        if exist "!EDK2_ALT2!" (
            set "EDK2_AARCH64_BIOS=!EDK2_ALT2!"
            echo [OK]   EDK2 aarch64 UEFI firmware found at !EDK2_ALT2!
        ) else (
            echo [WARN] EDK2 aarch64 UEFI firmware not found
            echo        aarch64 VMs require edk2-aarch64-code.fd
            echo        Set edk2_aarch64_bios in config.yaml after install
        )
    )
) else (
    if not exist "%QEMU_PATH%" (
        echo [ERR] QEMU not found at %QEMU_PATH%
        echo       Install from: %QEMU_DOWNLOAD_URL%
        pause
        exit /b 1
    )
    echo [OK]   qemu-system-x86_64 found
)

if not exist "%QEMU_IMG_PATH%" (
    echo [WARN] qemu-img not found at %QEMU_IMG_PATH%
)

echo.

:: --- Step 2b: Stop old processes before build ---
echo [INFO] Checking for running vm_ctl processes...
where nssm >nul 2>&1
if %errorlevel% equ 0 (
    nssm status %SERVICE_NAME% >nul 2>&1
    if !errorlevel! equ 0 (
        echo [INFO] Stopping existing NSSM service...
        nssm stop %SERVICE_NAME% >nul 2>&1
        echo [OK]   Service stopped
    )
)
:: Kill any stray vm_ctl.exe processes
tasklist /fi "imagename eq vm_ctl.exe" 2>nul | find /i "vm_ctl.exe" >nul 2>&1
if %errorlevel% equ 0 (
    echo [INFO] Killing stray vm_ctl.exe processes...
    taskkill /f /im vm_ctl.exe >nul 2>&1
    timeout /t 2 /nobreak >nul
    echo [OK]   Stray processes killed
) else (
    echo [OK]   No running vm_ctl processes found
)

echo.

:: --- Step 3: Build or locate binary ---
set "SCRIPT_DIR=%~dp0"
cd /d "%SCRIPT_DIR%"

:: Check for pre-built binary first (cross-compiled or local build)
set "PREBUILT_LOCAL=%SCRIPT_DIR%vm_ctl.exe"
if /I "%ARCH%"=="ARM64" (
    set "PREBUILT_TARGET=%SCRIPT_DIR%target\aarch64-pc-windows-msvc\release\vm_ctl.exe"
) else (
    set "PREBUILT_TARGET=%SCRIPT_DIR%target\x86_64-pc-windows-gnu\release\vm_ctl.exe"
)
:: Also check MSVC target path
set "PREBUILT_MSVC=%SCRIPT_DIR%target\release\vm_ctl.exe"

if exist "%PREBUILT_LOCAL%" (
    set "BINARY=%PREBUILT_LOCAL%"
    echo [OK]   Found pre-built binary at %PREBUILT_LOCAL%
) else if exist "%PREBUILT_TARGET%" (
    set "BINARY=%PREBUILT_TARGET%"
    echo [OK]   Found pre-built binary at %PREBUILT_TARGET%
) else if exist "%PREBUILT_MSVC%" (
    set "BINARY=%PREBUILT_MSVC%"
    echo [OK]   Found pre-built binary at %PREBUILT_MSVC%
) else (
    echo [INFO] No pre-built binary found. Building from source...

    :: Use a local target directory to avoid OS error 87 on shared/network filesystems
    set "CARGO_TARGET_DIR=C:\vmcontrol\_build"

    :: --- Auto-detect best Rust toolchain ---
    echo [INFO] Detecting Rust toolchain...
    set "USE_TOOLCHAIN="

    :: Check current default toolchain
    for /f "tokens=1" %%T in ('rustup default 2^>nul') do set "CURRENT_TC=%%T"
    echo [INFO] Current toolchain: !CURRENT_TC!

    :: Check if MSVC linker (cl.exe) is available via vswhere
    set "MSVC_OK=0"
    where cl.exe >nul 2>&1 && set "MSVC_OK=1"
    if "!MSVC_OK!"=="0" (
        :: Try to find Visual Studio Build Tools via vswhere
        set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
        if exist "!VSWHERE!" (
            for /f "delims=" %%P in ('"!VSWHERE!" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2^>nul') do (
                if exist "%%P\VC\Auxiliary\Build\vcvars64.bat" (
                    set "MSVC_OK=1"
                    echo [INFO] Visual Studio Build Tools found at: %%P
                )
            )
        )
    )

    :: Decide which toolchain to use
    if /I "%ARCH%"=="ARM64" (
        :: ARM64: MSVC is the only supported toolchain (no GNU for aarch64-windows)
        if "!MSVC_OK!"=="1" (
            echo [INFO] MSVC Build Tools detected -- using MSVC toolchain ^(ARM64^)
            echo !CURRENT_TC! | findstr /i "msvc" >nul 2>&1
            if !errorlevel! neq 0 (
                echo [INFO] Switching to stable-msvc toolchain...
                rustup toolchain install stable-msvc >nul 2>&1
                rustup default stable-msvc >nul 2>&1
                echo [OK]   Switched to stable-msvc ^(aarch64-pc-windows-msvc^)
            ) else (
                echo [OK]   Already using MSVC toolchain
            )
            set "USE_TOOLCHAIN=msvc"
        ) else (
            echo [WARN] No MSVC Build Tools detected
            echo [INFO] ARM64 Windows requires MSVC toolchain ^(GNU is not available^)
            echo.
            :: Download VS Build Tools bootstrapper directly (avoids winget / msstore issues)
            set "VSBT_URL=https://aka.ms/vs/17/release/vs_BuildTools.exe"
            set "VSBT_EXE=%TEMP%\vs_BuildTools.exe"
            echo [INFO] Downloading Visual Studio Build Tools bootstrapper...
            powershell -NoProfile -Command "try { [Net.ServicePointManager]::SecurityProtocol='Tls12'; Invoke-WebRequest '!VSBT_URL!' -OutFile '!VSBT_EXE!' -UseBasicParsing; exit 0 } catch { Write-Host $_.Exception.Message; exit 1 }"
            if !errorlevel! neq 0 (
                echo [ERR] Failed to download VS Build Tools. Install manually:
                echo       https://visualstudio.microsoft.com/visual-cpp-build-tools/
                pause
                exit /b 1
            )
            echo [INFO] Installing VS Build Tools ^(ARM64 + Clang^) -- this may take several minutes...
            "!VSBT_EXE!" --quiet --wait --norestart --nocache --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.VC.Tools.ARM64 --add Microsoft.VisualStudio.Component.VC.Llvm.Clang --includeRecommended
            :: Exit codes 3010 (restart required) and 0 are both success; others = failure
            if !errorlevel! neq 0 if !errorlevel! neq 3010 (
                echo [ERR] VS Build Tools installer exited with code !errorlevel!.
                echo       Install manually from: https://visualstudio.microsoft.com/visual-cpp-build-tools/
                pause
                exit /b 1
            )
            echo [OK]   Visual Studio Build Tools installed ^(ARM64 + Clang^)
            echo [INFO] Switching to stable-msvc toolchain...
            rustup toolchain install stable-msvc >nul 2>&1
            rustup default stable-msvc >nul 2>&1
            echo [OK]   Switched to stable-msvc ^(aarch64-pc-windows-msvc^)
            set "USE_TOOLCHAIN=msvc"
        )
    ) else (
        :: x86_64: Try MSVC first, then GNU
        :: Check if MinGW dlltool is available (needed for GNU toolchain)
        set "GNU_OK=0"
        where dlltool.exe >nul 2>&1 && set "GNU_OK=1"

        if "!MSVC_OK!"=="1" (
            echo [INFO] MSVC Build Tools detected -- using MSVC toolchain
            echo !CURRENT_TC! | findstr /i "msvc" >nul 2>&1
            if !errorlevel! neq 0 (
                echo [INFO] Switching to stable-msvc toolchain...
                rustup toolchain install stable-msvc >nul 2>&1
                rustup default stable-msvc >nul 2>&1
                echo [OK]   Switched to stable-msvc
            ) else (
                echo [OK]   Already using MSVC toolchain
            )
            set "USE_TOOLCHAIN=msvc"
        ) else if "!GNU_OK!"=="1" (
            echo [INFO] MinGW-w64 detected -- using GNU toolchain
            echo !CURRENT_TC! | findstr /i "gnu" >nul 2>&1
            if !errorlevel! neq 0 (
                echo [INFO] Switching to stable-gnu toolchain...
                rustup toolchain install stable-x86_64-pc-windows-gnu >nul 2>&1
                rustup default stable-x86_64-pc-windows-gnu >nul 2>&1
                echo [OK]   Switched to stable-x86_64-pc-windows-gnu
            ) else (
                echo [OK]   Already using GNU toolchain
            )
            set "USE_TOOLCHAIN=gnu"
        ) else (
            echo [WARN] No C linker detected ^(no cl.exe / no dlltool.exe^)
            echo.
            :: Download VS Build Tools bootstrapper directly (avoids winget / msstore issues)
            set "VSBT_URL=https://aka.ms/vs/17/release/vs_BuildTools.exe"
            set "VSBT_EXE=%TEMP%\vs_BuildTools.exe"
            echo [INFO] Downloading Visual Studio Build Tools bootstrapper...
            powershell -NoProfile -Command "try { [Net.ServicePointManager]::SecurityProtocol='Tls12'; Invoke-WebRequest '!VSBT_URL!' -OutFile '!VSBT_EXE!' -UseBasicParsing; exit 0 } catch { Write-Host $_.Exception.Message; exit 1 }"
            if !errorlevel! neq 0 (
                echo [WARN] Failed to download VS Build Tools. Attempting build with current toolchain...
                set "USE_TOOLCHAIN=unknown"
            ) else (
                echo [INFO] Installing VS Build Tools -- this may take several minutes...
                "!VSBT_EXE!" --quiet --wait --norestart --nocache --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended
                if !errorlevel! neq 0 if !errorlevel! neq 3010 (
                    echo [WARN] VS Build Tools exited with code !errorlevel!. Attempting build anyway...
                    set "USE_TOOLCHAIN=unknown"
                ) else (
                    echo [OK]   Visual Studio Build Tools installed
                    echo [INFO] Switching to stable-msvc toolchain...
                    rustup toolchain install stable-msvc >nul 2>&1
                    rustup default stable-msvc >nul 2>&1
                    echo [OK]   Switched to stable-msvc
                    set "USE_TOOLCHAIN=msvc"
                )
            )
        )
    )

    :: --- Load MSVC environment if needed (so link.exe is in PATH) ---
    where link.exe >nul 2>&1
    if !errorlevel! neq 0 (
        echo [INFO] link.exe not in PATH -- loading MSVC environment...
        set "VSWHERE_EXE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
        if exist "!VSWHERE_EXE!" (
            for /f "delims=" %%P in ('"!VSWHERE_EXE!" -latest -products * -property installationPath 2^>nul') do set "VS_INSTALL_PATH=%%P"
            if defined VS_INSTALL_PATH (
                echo [INFO] VS Install: !VS_INSTALL_PATH!
                set "VCVARSALL=!VS_INSTALL_PATH!\VC\Auxiliary\Build\vcvarsall.bat"
                if exist "!VCVARSALL!" (
                    if /I "%ARCH%"=="ARM64" (
                        echo [INFO] Running: vcvarsall.bat arm64
                        call "!VCVARSALL!" arm64
                    ) else (
                        echo [INFO] Running: vcvarsall.bat amd64
                        call "!VCVARSALL!" amd64
                    )
                    :: Verify link.exe is now available
                    where link.exe >nul 2>&1
                    if !errorlevel! equ 0 (
                        echo [OK]   MSVC environment loaded -- link.exe found
                    ) else (
                        echo [WARN] vcvarsall.bat ran but link.exe still not found
                        if /I "%ARCH%"=="ARM64" (
                            echo [INFO] ARM64 native build tools missing. Adding component via VS Installer...
                            set "VS_SETUP=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\setup.exe"
                            if exist "!VS_SETUP!" (
                                echo [INFO] Running: setup.exe modify --add VC.Tools.ARM64 + Windows SDK
                                "!VS_SETUP!" modify --installPath "!VS_INSTALL_PATH!" --add Microsoft.VisualStudio.Component.VC.Tools.ARM64 --add Microsoft.VisualStudio.Component.VC.Llvm.Clang --add Microsoft.VisualStudio.Component.Windows11SDK.26100 --quiet
                                if !errorlevel! neq 0 (
                                    echo [WARN] setup.exe modify returned error, trying with Windows10 SDK...
                                    "!VS_SETUP!" modify --installPath "!VS_INSTALL_PATH!" --add Microsoft.VisualStudio.Component.VC.Tools.ARM64 --add Microsoft.VisualStudio.Component.VC.Llvm.Clang --add Microsoft.VisualStudio.Component.Windows10SDK.20348 --quiet
                                )
                                echo [INFO] Retrying vcvarsall.bat arm64...
                                call "!VCVARSALL!" arm64
                                where link.exe >nul 2>&1
                                if !errorlevel! equ 0 (
                                    echo [OK]   ARM64 tools installed -- link.exe found
                                ) else (
                                    echo [ERR] link.exe still not found.
                                    echo       Open "Visual Studio Installer" ^> Modify ^> Individual Components
                                    echo       Check these items:
                                    echo         [x] MSVC v143 ARM64/ARM64EC build tools ^(Latest^)
                                    echo         [x] Windows 11 SDK ^(any version^)
                                    echo       Click Modify, then re-run install.bat
                                    pause
                                    exit /b 1
                                )
                            ) else (
                                echo [ERR] VS Installer setup.exe not found.
                                echo       Open "Visual Studio Installer" manually and add ARM64 tools.
                                pause
                                exit /b 1
                            )
                        )
                    )
                ) else (
                    echo [WARN] vcvarsall.bat not found at !VCVARSALL!
                )
            ) else (
                echo [WARN] vswhere found no VS installations
            )
        ) else (
            echo [WARN] vswhere.exe not found
        )
    ) else (
        echo [OK]   link.exe already in PATH
    )

    :: --- Ensure clang is in PATH (needed by ring crate on ARM64) ---
    where clang.exe >nul 2>&1
    if !errorlevel! neq 0 (
        echo [INFO] clang.exe not in PATH -- searching VS LLVM directory...
        :: Ensure VS_INSTALL_PATH is set (may not be if link.exe was already in PATH)
        if not defined VS_INSTALL_PATH (
            set "VSWHERE_CLK=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
            if exist "!VSWHERE_CLK!" (
                for /f "delims=" %%P in ('"!VSWHERE_CLK!" -latest -products * -property installationPath 2^>nul') do set "VS_INSTALL_PATH=%%P"
            )
        )
        if defined VS_INSTALL_PATH (
            :: Check if LLVM/Clang is already installed but not in PATH
            set "LLVM_BIN=!VS_INSTALL_PATH!\VC\Tools\Llvm\ARM64\bin"
            if not exist "!LLVM_BIN!\clang.exe" set "LLVM_BIN=!VS_INSTALL_PATH!\VC\Tools\Llvm\x64\bin"
            if not exist "!LLVM_BIN!\clang.exe" set "LLVM_BIN=!VS_INSTALL_PATH!\VC\Tools\Llvm\bin"
            if exist "!LLVM_BIN!\clang.exe" (
                set "PATH=!LLVM_BIN!;!PATH!"
                set "LIBCLANG_PATH=!LLVM_BIN!"
                echo [OK]   Added LLVM to PATH: !LLVM_BIN!
            ) else (
                :: Clang not installed -- auto-install via VS Installer
                echo [INFO] Clang/LLVM not installed. Installing via VS Installer...
                set "VS_SETUP=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\setup.exe"
                if exist "!VS_SETUP!" (
                    "!VS_SETUP!" modify --installPath "!VS_INSTALL_PATH!" --add Microsoft.VisualStudio.Component.VC.Llvm.Clang --quiet
                    echo [INFO] Clang component installed. Searching for clang.exe...
                    :: Re-check all possible paths
                    set "LLVM_BIN=!VS_INSTALL_PATH!\VC\Tools\Llvm\ARM64\bin"
                    if not exist "!LLVM_BIN!\clang.exe" set "LLVM_BIN=!VS_INSTALL_PATH!\VC\Tools\Llvm\x64\bin"
                    if not exist "!LLVM_BIN!\clang.exe" set "LLVM_BIN=!VS_INSTALL_PATH!\VC\Tools\Llvm\bin"
                    if exist "!LLVM_BIN!\clang.exe" (
                        set "PATH=!LLVM_BIN!;!PATH!"
                        set "LIBCLANG_PATH=!LLVM_BIN!"
                        echo [OK]   Added LLVM to PATH: !LLVM_BIN!
                    ) else (
                        echo [WARN] clang.exe still not found after install
                        echo        Install manually: Visual Studio Installer ^> Individual Components ^> "C++ Clang Compiler"
                    )
                ) else (
                    echo [WARN] VS Installer setup.exe not found -- cannot auto-install Clang
                    echo        Install Clang via: Visual Studio Installer ^> Individual Components ^> "C++ Clang Compiler"
                )
            )
        ) else (
            echo [WARN] VS installation not found -- cannot locate clang
        )
    ) else (
        echo [OK]   clang.exe found in PATH
    )

    echo [INFO] Building vm_ctl from source ^(release mode^)...
    cargo build --release 2>"%TEMP%\vmcontrol_build.log"
    if !errorlevel! neq 0 (
        echo.
        type "%TEMP%\vmcontrol_build.log" 2>nul | findstr /i "error" 2>nul
        echo.
        echo [ERR] Build failed.
        echo.
        if "!USE_TOOLCHAIN!"=="unknown" (
            echo       No C/C++ linker was found.
            echo.
            echo       Install Visual Studio Build Tools:
            echo         1. Download from: https://visualstudio.microsoft.com/visual-cpp-build-tools/
            echo         2. Select "Desktop development with C++"
            if /I not "%ARCH%"=="ARM64" (
                echo         3. Also select "ARM64/ARM64EC build tools" if on ARM
            )
            echo         3. Re-run install.bat ^(toolchain will be auto-configured^)
            if /I not "%ARCH%"=="ARM64" (
                echo.
                echo       Alternative -- MinGW-w64 ^(x86_64 only, not available for ARM64^):
                echo         1. Run:  winget install -e --id MSYS2.MSYS2
                echo         2. Open MSYS2 and run:  pacman -S mingw-w64-x86_64-toolchain
                echo         3. Add C:\msys64\mingw64\bin to your PATH
                echo         4. Re-run install.bat
            )
        ) else (
            echo       See build log: %TEMP%\vmcontrol_build.log
        )
        echo.
        echo       Alternatively, cross-compile from Mac:
        echo         cd windows
        if /I "%ARCH%"=="ARM64" (
            echo         cargo build --release --target aarch64-pc-windows-msvc
        ) else (
            echo         cargo build --release --target x86_64-pc-windows-gnu
        )
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

:: --- Step 4b: Download virtio-win ISO (for Windows guest VMs) ---
set "VIRTIO_ISO=virtio-win-0.1.285.iso"
set "VIRTIO_URL=https://fedorapeople.org/groups/virt/virtio-win/direct-downloads/archive-virtio/virtio-win-0.1.285-1/%VIRTIO_ISO%"
set "VIRTIO_DEST=%ISO_PATH%\%VIRTIO_ISO%"

if exist "%VIRTIO_DEST%" (
    echo [OK]   virtio-win ISO already exists: %VIRTIO_ISO%
) else (
    echo [INFO] Downloading %VIRTIO_ISO% ^(needed for Windows guest VMs^)...
    powershell -NoProfile -Command "try { [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -Uri '%VIRTIO_URL%' -OutFile '%VIRTIO_DEST%' -UseBasicParsing; Write-Host '[OK]   Downloaded %VIRTIO_ISO%' } catch { Write-Host '[WARN] Failed to download virtio-win ISO:' $_.Exception.Message }"
    if not exist "%VIRTIO_DEST%" (
        echo [WARN] You can download it manually later:
        echo        %VIRTIO_URL%
        echo        Place it in: %ISO_PATH%\
    )
)

echo.

:: --- Step 5: Remove existing service (already stopped in Step 2b) ---
where nssm >nul 2>&1
if %errorlevel% equ 0 (
    nssm status %SERVICE_NAME% >nul 2>&1
    if !errorlevel! equ 0 (
        nssm remove %SERVICE_NAME% confirm >nul 2>&1
    )
)

:: --- Step 6: Copy binary and static files ---
echo [INFO] Installing binary and static files...
copy /y "%BINARY%" "%CTL_BIN%\vm_ctl.exe" >nul
:: Prefer root static\ (single source of truth), fall back to platform static\
if exist "%SCRIPT_DIR%..\static\app.js" (
    xcopy /s /y /i /q "%SCRIPT_DIR%..\static\*" "%STATIC_DIR%\" >nul
    echo [OK]   Static files installed from repo root
) else if exist "%SCRIPT_DIR%static\app.js" (
    xcopy /s /y /i /q "%SCRIPT_DIR%static\*" "%STATIC_DIR%\" >nul
    echo [OK]   Static files installed from platform dir
) else (
    echo [ERR]  Static files not found!
    exit /b 1
)
echo [OK]   Binary installed to %CTL_BIN%\vm_ctl.exe
echo [OK]   Static files installed to %STATIC_DIR%\

:: Service management scripts -- ship copies alongside the binary so users can
:: manage the service even after the install source directory is removed.
for %%S in (start.bat stop.bat restart.bat status.bat) do (
    if exist "%SCRIPT_DIR%%%S" (
        copy /y "%SCRIPT_DIR%%%S" "%CTL_BIN%\%%S" >nul
    )
)
echo [OK]   Service scripts installed: start.bat, stop.bat, restart.bat, status.bat

:: --- Step 7: Generate config.yaml ---
if not exist "%CONFIG_YAML%" (
    echo [INFO] Generating config.yaml with detected paths...
    (
        echo qemu_path: %QEMU_PATH%
        echo qemu_img_path: %QEMU_IMG_PATH%
        echo qemu_aarch64_path: %QEMU_AARCH64_PATH%
        echo edk2_aarch64_bios: %EDK2_AARCH64_BIOS%
        echo edk2_x86_secure_code: %EDK2_X86_SECURE_CODE%
        echo edk2_x86_vars: %EDK2_X86_VARS%
        echo swtpm_path: %SWTPM_PATH%
        echo mkisofs_path: %MKISOFS_PATH%
        :: TCG tuning: qemu64 avoids the Haswell-v4 CPUID features TCG doesn't
        :: emulate (pcid/invpcid/tsc-deadline/spec-ctrl). thread=multi gives each
        :: vCPU its own host core. TCG is safe on Parallels (no nested virt);
        :: switch qemu_accel to "whpx" on bare-metal Windows with Hyper-V
        :: Platform enabled for hardware acceleration.
        echo qemu_cpu_x86: qemu64
        echo qemu_accel: tcg,thread=multi
        :: Bind VNC on all interfaces so browsers on other machines (e.g. the
        :: host when Windows is a Parallels guest) can reach the WebSocket.
        echo vnc_bind_host: 0.0.0.0
        echo ctl_bin_path: C:\vmcontrol\bin
        echo pctl_path: C:\vmcontrol
        echo disk_path: C:\vmcontrol\disks
        echo iso_path: C:\vmcontrol\iso
        echo live_path: C:\vmcontrol\backups
        echo gzip_path: gzip
        echo vs_up_script: vs-up.bat
        echo vs_down_script: vs-down.bat
        echo pctl_script: pctl.bat
        echo domain: localhost
    ) > "%CONFIG_YAML%"
    echo [OK]   config.yaml created
    if /I "%ARCH%"=="ARM64" (
        echo [OK]   ARM64 paths included: qemu_aarch64_path, edk2_aarch64_bios
    )
) else (
    echo [WARN] config.yaml already exists -- skipping ^(preserving your customizations^)
    :: Migrate: older installs don't have edk2_x86_secure_code / edk2_x86_vars,
    :: so Windows VMs fall back to the Mac default path and Secure Boot stays off.
    findstr /c:"edk2_x86_secure_code" "%CONFIG_YAML%" >nul 2>&1
    if !errorlevel! neq 0 (
        echo edk2_x86_secure_code: %EDK2_X86_SECURE_CODE%>> "%CONFIG_YAML%"
        echo edk2_x86_vars: %EDK2_X86_VARS%>> "%CONFIG_YAML%"
        echo [INFO] Added edk2_x86_secure_code / edk2_x86_vars to existing config.yaml
    )
    findstr /c:"swtpm_path" "%CONFIG_YAML%" >nul 2>&1
    if !errorlevel! neq 0 (
        echo swtpm_path: %SWTPM_PATH%>> "%CONFIG_YAML%"
        echo [INFO] Added swtpm_path to existing config.yaml
    )
    findstr /c:"mkisofs_path" "%CONFIG_YAML%" >nul 2>&1
    if !errorlevel! neq 0 (
        echo mkisofs_path: %MKISOFS_PATH%>> "%CONFIG_YAML%"
        echo [INFO] Added mkisofs_path to existing config.yaml
    )
    findstr /c:"qemu_cpu_x86" "%CONFIG_YAML%" >nul 2>&1
    if !errorlevel! neq 0 (
        echo qemu_cpu_x86: qemu64>> "%CONFIG_YAML%"
        echo [INFO] Added qemu_cpu_x86: qemu64 to existing config.yaml
    )
    findstr /c:"qemu_accel" "%CONFIG_YAML%" >nul 2>&1
    if !errorlevel! neq 0 (
        echo qemu_accel: tcg,thread=multi>> "%CONFIG_YAML%"
        echo [INFO] Added qemu_accel: tcg,thread=multi to existing config.yaml
    )
    findstr /c:"vnc_bind_host" "%CONFIG_YAML%" >nul 2>&1
    if !errorlevel! neq 0 (
        echo vnc_bind_host: 0.0.0.0>> "%CONFIG_YAML%"
        echo [INFO] Added vnc_bind_host: 0.0.0.0 to existing config.yaml
    )
)

echo.

:: --- Step 8: Leave API key unconfigured (server starts unauthenticated) ---
:: Generate one later from the web UI (Generate New Key) if you want auth.
set "API_KEY_FILE=%PCTL_PATH%\.api_key"
set "API_KEY="
if exist "%API_KEY_FILE%" (
    del /f /q "%API_KEY_FILE%" >nul 2>&1
    echo [INFO] Removed stale .api_key from previous install
)
:: Clear any system-wide VMCONTROL_API_KEY set by older install.bat versions.
setx VMCONTROL_API_KEY "" /M >nul 2>&1
reg delete "HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment" /v VMCONTROL_API_KEY /f >nul 2>&1
set "VMCONTROL_API_KEY="
echo [OK]   API key auth disabled (generate one later from the web UI if needed)

echo.

:: --- Step 9: Set up Windows Service ---
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
    :: Don't bake VMCONTROL_API_KEY into the service env -- leave auth off.
    nssm set %SERVICE_NAME% AppEnvironmentExtra "" >nul 2>&1
    nssm start %SERVICE_NAME%
    echo [OK]   Service installed and started via NSSM
) else (
    echo [WARN] NSSM not found. Using Scheduled Task as fallback...
    echo        Download NSSM: https://nssm.cc/download
    echo.
    :: Create a launcher script that sets working directory before running.
    :: No VMCONTROL_API_KEY baked in -- server runs unauthenticated by default.
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

:: --- Step 10: Firewall rule ---
echo.
echo [INFO] Adding firewall rule for port 8080...
netsh advfirewall firewall delete rule name="vmcontrol" >nul 2>&1
netsh advfirewall firewall delete rule name="vmcontrol-vnc" >nul 2>&1
netsh advfirewall firewall add rule name="vmcontrol" dir=in action=allow protocol=tcp localport=8080 enable=yes >nul
:: VNC WebSocket range (12001-13000) — QEMU binds one port per VM for the noVNC console.
netsh advfirewall firewall add rule name="vmcontrol-vnc" dir=in action=allow protocol=tcp localport=12001-13000 enable=yes >nul
echo [OK]   Firewall rule added

echo.

:: --- Step 11: Create Desktop shortcut (Admin PowerShell) ---
echo [INFO] Creating desktop shortcut...
set "DESKTOP=%USERPROFILE%\Desktop"
set "SHORTCUT=%DESKTOP%\vmcontrol.lnk"

powershell -NoProfile -Command "$ws = New-Object -ComObject WScript.Shell; $sc = $ws.CreateShortcut('%SHORTCUT%'); $sc.TargetPath = 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe'; $sc.Arguments = '-NoExit -Command cd C:\vmcontrol; C:\vmcontrol\bin\vm_ctl.exe server 0.0.0.0:8080'; $sc.WorkingDirectory = 'C:\vmcontrol'; $sc.Description = 'vmcontrol Server (Admin)'; $sc.Save()"

:: Set shortcut to Run as Administrator
powershell -NoProfile -Command "$bytes = [System.IO.File]::ReadAllBytes('%SHORTCUT%'); $bytes[0x15] = $bytes[0x15] -bor 0x20; [System.IO.File]::WriteAllBytes('%SHORTCUT%', $bytes)"

echo [OK]   Desktop shortcut created: %SHORTCUT%
echo.

:: --- Step 12: Summary ---
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
echo   API Key:     (not set — open in browser directly; use "Generate New Key" in UI to enable auth)
echo.
where nssm >nul 2>&1
if %errorlevel% equ 0 (
    echo   Service:     %SERVICE_NAME% ^(NSSM^)
) else (
    echo   Service:     %SERVICE_NAME% ^(Scheduled Task^)
)
echo.
echo   Service scripts ^(Run as administrator^):
echo     %CTL_BIN%\start.bat     -- start the service
echo     %CTL_BIN%\stop.bat      -- stop the service ^& kill stray vm_ctl.exe
echo     %CTL_BIN%\restart.bat   -- restart the service
echo     %CTL_BIN%\status.bat    -- show service state, listener, log tail
echo.
echo ================================================================
echo.
pause
