@echo off
rem Block inbound connections from other PCs to ProxyDock.exe (TCP and UDP). Run as administrator.
rem   firewall.bat          add the block rule
rem   firewall.bat remove   remove it (e.g. to allow LAN use with host 0.0.0.0)
rem Loopback (127.0.0.1) traffic is not filtered by Windows Firewall, so local use is unaffected.
rem The app itself has the same buttons: Settings > Security, or Help > step 3.

net session >nul 2>&1
if errorlevel 1 (
  echo Please run as administrator.
  pause
  exit /b 1
)

set "EXE=%~dp0dist\win\ProxyDock.exe"
set "RULE=ProxyDock - Block External"

rem clean up old port-based rules from earlier versions
for %%N in ("SOCKS5 Proxy - Block External" "Block SOCKS5 Proxy External" "Allow SOCKS5 Proxy Local" "Block SOCKS5 Proxy External UDP" "Allow SOCKS5 Proxy Local UDP") do (
  netsh advfirewall firewall delete rule name=%%N >nul 2>&1
)
netsh advfirewall firewall delete rule name="%RULE%" >nul 2>&1

if /i "%~1"=="remove" (
  echo Rule removed.
  pause
  exit /b 0
)

netsh advfirewall firewall add rule name="%RULE%" dir=in action=block program="%EXE%" enable=yes
echo Block rule added for: %EXE%
pause
