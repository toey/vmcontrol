@echo off
setlocal enabledelayedexpansion
set "SERVICE_NAME=vmcontrol"

net session >nul 2>&1
if %errorlevel% neq 0 (
    echo [ERR] Run as Administrator.
    pause
    exit /b 1
)

set "STOPPED=0"

where nssm >nul 2>&1
if %errorlevel% equ 0 (
    nssm status %SERVICE_NAME% >nul 2>&1
    if !errorlevel! equ 0 (
        nssm stop %SERVICE_NAME%
        echo [OK] NSSM service stopped.
        set "STOPPED=1"
    )
)

schtasks /query /tn "%SERVICE_NAME%" >nul 2>&1
if %errorlevel% equ 0 (
    schtasks /end /tn "%SERVICE_NAME%" >nul 2>&1
    echo [OK] Scheduled Task ended.
    set "STOPPED=1"
)

:: Kill stray vm_ctl.exe processes not managed by the service
tasklist /fi "imagename eq vm_ctl.exe" 2>nul | find /i "vm_ctl.exe" >nul 2>&1
if %errorlevel% equ 0 (
    taskkill /f /im vm_ctl.exe >nul 2>&1
    echo [OK] Stray vm_ctl.exe processes killed.
    set "STOPPED=1"
)

if "!STOPPED!"=="0" (
    echo [INFO] Nothing to stop -- %SERVICE_NAME% is not running.
)
pause
