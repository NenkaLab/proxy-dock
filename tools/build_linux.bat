@echo off
rem Linux build -> dist/linux/ProxyDock (via Docker on Windows/macOS)
rem Build logic lives in tools\build.py (this file is ASCII-only on purpose).
rem Set NOPAUSE=1 to skip the final pause.
setlocal
cd /d "%~dp0.."
py -3.14 tools\build.py linux
set "RC=%ERRORLEVEL%"
if "%RC%"=="9009" echo Python 3.14 launcher not found. Install Python 3.14 from python.org.
if not "%NOPAUSE%"=="1" pause
exit /b %RC%
