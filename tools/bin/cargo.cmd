@echo off
rem Skiff cargo PATH wrapper: shared by cmd and PowerShell.
rem Passes every argument through to tools\cargo-gate.ps1; the gate decides.
setlocal
pwsh -NoProfile -ExecutionPolicy Bypass -File "%~dp0..\cargo-gate.ps1" %*
exit /b %ERRORLEVEL%
