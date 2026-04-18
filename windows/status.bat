@echo off
setlocal enabledelayedexpansion
set "SERVICE_NAME=vmcontrol"
set "LOG_DIR=C:\vmcontrol\logs"

echo ================================================================
echo   vmcontrol service status
echo ================================================================

set "FOUND=0"

where nssm >nul 2>&1
if %errorlevel% equ 0 (
    nssm status %SERVICE_NAME% >nul 2>&1
    if !errorlevel! equ 0 (
        echo.
        echo [NSSM] Service state:
        nssm status %SERVICE_NAME%
        set "FOUND=1"
    )
)

schtasks /query /tn "%SERVICE_NAME%" >nul 2>&1
if %errorlevel% equ 0 (
    echo.
    echo [Scheduled Task] State:
    schtasks /query /tn "%SERVICE_NAME%" /fo list | findstr /i "Status Next"
    set "FOUND=1"
)

if "!FOUND!"=="0" (
    echo.
    echo [WARN] %SERVICE_NAME% is not installed via NSSM or Scheduled Task.
)

echo.
echo [Processes]
tasklist /fi "imagename eq vm_ctl.exe" 2>nul | find /i "vm_ctl.exe"
if %errorlevel% neq 0 echo   (no vm_ctl.exe running)

echo.
echo [Port 8080]
netstat -ano | findstr /c:":8080" | findstr /c:"LISTENING"
if %errorlevel% neq 0 echo   (nothing listening on :8080)

echo.
echo [Recent log tail -- last 20 lines]
if exist "%LOG_DIR%\vm_ctl.stderr.log" (
    powershell -NoProfile -Command "Get-Content -Path '%LOG_DIR%\vm_ctl.stderr.log' -Tail 20"
) else (
    echo   (no log file at %LOG_DIR%\vm_ctl.stderr.log)
)
echo.
pause
