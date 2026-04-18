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
    echo [ERR] git not found on PATH. Install from: https://git-scm.com/download/win
    popd & pause & exit /b 1
)

if not exist ".git" (
    echo [ERR] Not a git clone: %CD%
    echo       Run this only from a cloned vmcontrol repo.
    popd & pause & exit /b 1
)

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
