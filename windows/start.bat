@echo off
setlocal enabledelayedexpansion
set "SERVICE_NAME=vmcontrol"
set "NSSM=C:\vmcontrol\bin\nssm.exe"

net session >nul 2>&1
if %errorlevel% neq 0 (
    echo [ERR] Run as Administrator.
    pause
    exit /b 1
)

if exist "%NSSM%" (
    "%NSSM%" status %SERVICE_NAME% >nul 2>&1
    if !errorlevel! equ 0 (
        "%NSSM%" start %SERVICE_NAME%
        echo [OK] Service started via NSSM.
        goto :end
    )
)

schtasks /query /tn "%SERVICE_NAME%" >nul 2>&1
if %errorlevel% equ 0 (
    schtasks /run /tn "%SERVICE_NAME%" >nul
    echo [OK] Scheduled Task triggered.
    goto :end
)

echo [ERR] %SERVICE_NAME% not installed. Run install.bat first.
exit /b 1

:end
timeout /t 2 >nul
echo [INFO] Web UI: http://localhost:8080
pause
