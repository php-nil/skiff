@echo off
rem Skiff git PATH wrapper: shared by cmd and PowerShell.
rem Passes every argument through to tools\git-gate.ps1; the gate decides.
setlocal
pwsh -NoProfile -ExecutionPolicy Bypass -File "%~dp0..\git-gate.ps1" %*
exit /b %ERRORLEVEL%
