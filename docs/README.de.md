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

- **Blöcke auf einer Arbeitsfläche verbinden**: Eingänge (SOCKS5, HTTP, Shadowsocks- / Hysteria2- / WireGuard-Server,
  TUN, ...) mit Ausgängen (Direkt, vorgelagerter Proxy, Shadowsocks, Hysteria2, WireGuard, VLESS, VMess, Trojan, TUIC,
  Abonnements)
- **Routing**: Verkehr nach Domain, IP, Programm oder Land aufteilen; ein IF-Schalter dient als Kill-Switch; Gruppen
  nehmen den schnellsten Server oder wechseln bei Ausfall
- **Ketten**: einen Ausgang über einen anderen schicken, oder über Tailscale, eine VPN-App oder WLAN
- **Systemproxy**: eine Karte, solange sie läuft, als Windows-Systemproxy verwenden
- **Alltagswerkzeuge**: Teilen-Links, Verbindungstests, Protokolle und Live-Statistik, 8 Sprachen, Tray

Die Proxys laufen auf [sing-box](https://github.com/SagerNet/sing-box), das im Release enthalten ist.

## Starten

Unter Windows installiert `ProxyDock-<Version>-windows-x64-setup.exe` für Ihr Benutzerkonto (keine Administratorrechte nötig); die `.zip` ist dieselbe App als Ordner. Unter macOS die `.dmg` öffnen und ProxyDock in „Programme“ ziehen. Unter Linux die `.AppImage` ausführbar machen und starten.

Einen Build von [Releases](https://github.com/NenkaLab/proxy-dock/releases) laden.

Die Einstellungen liegen in `%APPDATA%\ProxyDock` (macOS: `~/Library/Application Support/ProxyDock`,
Linux: `~/.config/ProxyDock`). Den Rest erklärt die Seite **Anleitung** in der App.

## Bauen

Aus dem Quellcode starten, Tests, Builds, Installer und Releases: [Build.md](Build.md) (Englisch).

## Einschränkungen

- Router, Gruppen, WireGuard, VLESS / VMess / Trojan / TUIC und TUN brauchen die sing-box-Engine (enthalten).
- TUN und macOS wurden noch nicht in echten Umgebungen ausprobiert.
- SOCKS4 hat kein Passwort: nur auf diesem PC verwenden.

## Lizenz

[GPL-3.0](../LICENSE). Enthaltene Komponenten (sing-box GPL-3.0, Hysteria MIT, Qt / PySide6 LGPL-3.0, Schriften OFL-1.1, ...)
und ihre Lizenzen stehen in [`licenses/`](../licenses/).

Copyright (C) 2026 NenkaLab
