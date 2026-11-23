# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · [日本語](README.ja.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · **Deutsch** · [Français](README.fr.md) · [Español](README.es.md)

Eine Desktop-App, die mehrere Proxys und Tunnel nebeneinander betreibt. Auf einer Arbeitsfläche werden
**Eingangs**blöcke (wie Verkehr hereinkommt, z. B. ein lokaler SOCKS5-Port) mit **Ausgangs**blöcken (wie er hinausgeht,
z. B. ein Shadowsocks-Server) verbunden; jede Karte lässt sich ein- und ausschalten.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="images/canvas-dark.png">
    <img src="images/canvas-light.png" width="98%" alt="Arbeitsfläche">
  </picture>
</p>
<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="images/proxies-dark.png">
    <img src="images/proxies-light.png" width="49%" alt="Proxies">
  </picture>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="images/router-dark.png">
    <img src="images/router-light.png" width="49%" alt="Router">
  </picture>
</p>

## Funktionen

- **Eingang**: SOCKS5, SOCKS4, HTTP, HTTPS, Shadowsocks-Server, Hysteria2-Server, WireGuard-Server (Handys und PCs verbinden sich mit der WireGuard-App), Portweiterleitung, TUN (alle Apps dieses PCs)
- **Ausgang**: Direct, HTTP- / HTTPS- / SOCKS-Proxy, Shadowsocks, Hysteria2, WireGuard, VLESS (REALITY), VMess,
  Trojan, TUIC, Abonnements (Freigabelinks oder Clash-YAML)
- **Blöcke**: Ketten (ein Ausgang über einen anderen), Router (Regeln nach Domain, IP, Port, Programm - aus laufenden Programmen wählbar -, GeoIP/Geosite; einfache und erweiterte Ansicht),
  IF (eine Bedingung oder ein manueller Ein/Aus-Schalter, z. B. ein Kill-Switch), Gruppen (schnellster, Failover oder manuell), Block
- **Netzwerkschnittstelle**: einen Ausgang über Tailscale, eine VPN-App oder WLAN senden, ohne die Routen des PCs zu ändern
- **DNS**: DNS des PCs, DoH / DoT / DoQ, pro Karte, auf Wunsch durch den Tunnel; Fake-IP für TUN
- **Karten**: Starten, Start mit der App und automatischer Neustart (wenn sie stoppt oder nicht startet) direkt am Block
- **PC-Block**: einen HTTP-Eingang, solange er läuft, als Windows-Systemproxy nutzen
- Freigabelinks (`ss://`, `vless://`, `vmess://`, `trojan://`, `tuic://`, `hysteria2://`), Verbindungsprüfung und -test,
  Protokolle und Live-Statistik, 8 Sprachen, Infobereich, Start bei der Anmeldung

Die Proxys laufen auf [sing-box](https://github.com/SagerNet/sing-box) (ein Prozess pro Karte, im Release enthalten).
Ein Hysteria2-Server für andere PCs läuft auf dem offiziellen [Hysteria](https://github.com/apernet/hysteria)-Kern;
die eingebaute Python-Engine dient als Ersatz.

## Starten

Unter Windows installiert `ProxyDock-<Version>-windows-x64-setup.exe` für Ihr Benutzerkonto (keine Administratorrechte nötig); die `.zip` ist dieselbe App als Ordner. Unter macOS die `.dmg` öffnen und ProxyDock in „Programme“ ziehen. Unter Linux die `.AppImage` ausführbar machen und starten.

Einen Build von [Releases](https://github.com/NenkaLab/proxy-dock/releases) laden oder aus dem Quellcode starten
(Python 3.14):

```bash
python -m venv .venv
.venv\Scripts\activate            # macOS / Linux: source .venv/bin/activate
pip install -r requirements.txt
python tools/fetch_singbox.py     # sing-box und Hysteria für diesen PC
python proxydock.py
```

Die Einstellungen liegen in `%APPDATA%\ProxyDock` (macOS: `~/Library/Application Support/ProxyDock`,
Linux: `~/.config/ProxyDock`). Den Rest erklärt die Seite **Anleitung** in der App.

## Bauen

`tools\build_win.bat` → `dist/win/ProxyDock/ProxyDock.exe` (Linux: `tools/build_linux.sh`, macOS: `tools/build_mac.sh`).
Der Build lädt sing-box und Hysteria herunter (SHA-256 geprüft) und packt sie in die App.
Tests: `python tests/run_tests.py`. Releases: `tools\release.bat <Version>` setzt ein Tag und pusht; GitHub Actions baut.
Installer: `python tools/build.py installer` (Inno Setup 6). macOS: `python tools/build.py dmg`, Linux: `python tools/build.py appimage`.

## Einschränkungen

- Router, Gruppen, WireGuard, VLESS / VMess / Trojan / TUIC, TUN, Schnittstellenwahl und eigener DNS brauchen die sing-box-Engine.
- Keine Lastverteilung über mehrere Server; Hysteria2-Server können nicht allein auf einem Portbereich lauschen.
- Zertifikat-Pinning läuft bis sing-box 1.15 auf der Python-Engine.
- TUN und Fake-IP wurden noch nicht in echten Umgebungen getestet, macOS nicht auf einem echten Mac.
- SOCKS4 hat kein Passwort, daher nur auf diesem PC nutzen.

## Lizenz

[GPL-3.0](../LICENSE). Enthaltene Komponenten (sing-box GPL-3.0, Hysteria MIT, Qt / PySide6 LGPL-3.0, Schriften OFL-1.1, ...)
und ihre Lizenzen stehen in [`licenses/`](../licenses/).

Copyright (C) 2026 NenkaLab
