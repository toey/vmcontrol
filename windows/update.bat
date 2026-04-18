@echo off
setlocal enabledelayedexpansion
set "SCRIPT_DIR=%~dp0"

:: Admin check -- service restart needs elevation
net session >nul 2>&1
if %errorlevel% neq 0 (
    echo [ERR] Run as Administrator.
    pause
    exit /b 1
)

:: Step up to the repo root (update.bat lives in windows\ so parent is the repo)
pushd "%SCRIPT_DIR%.."

where git >nul 2>&1
if %errorlevel% neq 0 (
    :: Git for Windows not installed in this shell -- auto-install silently.
    set "ARCH=%PROCESSOR_ARCHITECTURE%"
    set "GIT_SETUP=%TEMP%\git-for-windows-setup.exe"
    echo [INFO] git not on PATH. Downloading Git for Windows latest release...
    if /I "!ARCH!"=="ARM64" (
        set "GIT_ASSET_PATTERN=arm64\.exe$"
    ) else (
        set "GIT_ASSET_PATTERN=64-bit\.exe$"
    )
    powershell -NoProfile -Command "try { [Net.ServicePointManager]::SecurityProtocol='Tls12'; $r = Invoke-RestMethod 'https://api.github.com/repos/git-for-windows/git/releases/latest' -Headers @{'User-Agent'='vmcontrol-installer'}; $a = $r.assets | Where-Object { $_.name -match '!GIT_ASSET_PATTERN!' -and $_.name -notmatch 'MinGit|Portable' } | Select-Object -First 1; if (-not $a) { Write-Host 'No Git asset matched !GIT_ASSET_PATTERN!'; exit 2 }; Write-Host ('[INFO] Downloading ' + $a.browser_download_url); Invoke-WebRequest $a.browser_download_url -OutFile '!GIT_SETUP!' -UseBasicParsing; exit 0 } catch { Write-Host $_.Exception.Message; exit 1 }"
    if !errorlevel! neq 0 (
        echo [ERR] Failed to download Git installer. Install manually from:
        echo       https://git-scm.com/download/win
        popd & pause & exit /b 1
    )
    echo [INFO] Running silent install ^(this takes a minute or two^)...
    "!GIT_SETUP!" /VERYSILENT /NORESTART /NOCANCEL /SP- /SUPPRESSMSGBOXES /CLOSEAPPLICATIONS /RESTARTAPPLICATIONS /COMPONENTS="icons,ext\reg\shellhere,assoc,assoc_sh"
    if !errorlevel! neq 0 if !errorlevel! neq 3010 (
        echo [ERR] Git installer exited with code !errorlevel!.
        popd & pause & exit /b 1
    )
    :: Add git to this shell's PATH (installer updates the system PATH for new shells only).
    if exist "%ProgramFiles%\Git\cmd\git.exe" (
        set "PATH=%ProgramFiles%\Git\cmd;!PATH!"
    )
    where git >nul 2>&1
    if !errorlevel! neq 0 (
        echo [ERR] Git installed but still not on PATH. Close and re-open this shell, then re-run update.bat.
        popd & pause & exit /b 1
    )
    echo [OK]   Git for Windows installed
)

if not exist ".git" (
    echo [ERR] Not a git clone: %CD%
    echo       Run this only from a cloned vmcontrol repo.
    popd & pause & exit /b 1
)

:: When the repo sits on a shared volume (Parallels psf, SMB, WSL mount, etc.)
:: git refuses to touch it with "detected dubious ownership". Add the current
:: path to safe.directory for this user so fetch/pull succeed.
git config --global --add safe.directory "%CD%" >nul 2>&1
git config --global --add safe.directory "*" >nul 2>&1

echo [INFO] Pulling latest from origin...
git fetch --all --prune
if !errorlevel! neq 0 (
    echo [ERR] git fetch failed.
    popd & pause & exit /b 1
)
git pull --ff-only
if !errorlevel! neq 0 (
    echo [WARN] git pull had conflicts or the branch isn't fast-forwardable.
    echo        Resolve manually and re-run update.bat.
    popd & pause & exit /b 1
)

echo [INFO] Clearing cached pre-built vm_ctl.exe so the new source rebuilds...
if exist "%SCRIPT_DIR%vm_ctl.exe" del /f /q "%SCRIPT_DIR%vm_ctl.exe" >nul 2>&1

popd

echo [INFO] Handing off to install.bat for rebuild + reinstall...
call "%SCRIPT_DIR%install.bat"
