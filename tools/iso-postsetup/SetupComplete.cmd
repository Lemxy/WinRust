@echo off
rem WinRust post-setup, machine phase. Runs as SYSTEM after OOBE, before the first logon.
rem Do not reboot here: Microsoft advises against it from SetupComplete.cmd.
set "LOG=C:\WinRust\autopost-setupcomplete.log"
if not exist "C:\WinRust" mkdir "C:\WinRust"
echo [%DATE% %TIME%] SetupComplete starting >> "%LOG%"
if not exist "C:\WinRust\tools\autopost.ps1" (
    echo [%DATE% %TIME%] ERROR: C:\WinRust\tools\autopost.ps1 is missing >> "%LOG%"
    exit /b 0
)
powershell -NoProfile -ExecutionPolicy Bypass -File "C:\WinRust\tools\autopost.ps1" -Phase Machine >> "%LOG%" 2>&1
echo [%DATE% %TIME%] SetupComplete done, exit %ERRORLEVEL% >> "%LOG%"
exit /b 0
