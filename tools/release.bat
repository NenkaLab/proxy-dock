@echo off
rem Cut a release: tools\release.bat 2.1.0   (see tools\release.py; add --dry-run to rehearse)
setlocal
cd /d "%~dp0.."
py -3.14 tools\release.py %*
exit /b %ERRORLEVEL%
