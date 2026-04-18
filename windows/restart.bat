@echo off
setlocal enabledelayedexpansion
set "SERVICE_NAME=vmcontrol"
set "SCRIPT_DIR=%~dp0"

net session >nul 2>&1
if %errorlevel% neq 0 (
    echo [ERR] Run as Administrator.
    pause
    exit /b 1
)

where nssm >nul 2>&1
if %errorlevel% equ 0 (
    nssm status %SERVICE_NAME% >nul 2>&1
    if !errorlevel! equ 0 (
        nssm restart %SERVICE_NAME%
        echo [OK] NSSM service restarted.
        timeout /t 2 >nul
        echo [INFO] Web UI: http://localhost:8080
        pause
        exit /b 0
    )
)

:: Fallback: stop + start via other mechanisms
call "%SCRIPT_DIR%stop.bat"
timeout /t 2 >nul
call "%SCRIPT_DIR%start.bat"
