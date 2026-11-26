# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · [日本語](README.ja.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · **Deutsch** · [Français](README.fr.md) · [Español](README.es.md)

Eine Desktop-App, die mehrere Proxys und Tunnel nebeneinander betreibt. Auf einer Arbeitsfläche werden
**Eingangs**blöcke (wie Verkehr hereinkommt, z. B. ein lokaler SOCKS5-Port) mit **Ausgangs**blöcken (wie er hinausgeht,
z. B. ein Shadowsocks-Server) verbunden; jede Karte lässt sich ein- und ausschalten.

<sub>Für den eigenen Gebrauch gebaut und so geteilt, wie es ist.</sub>

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

Zu laden unter [Releases](https://github.com/NenkaLab/proxy-dock/releases):

- **Windows 10 / 11**: `ProxyDock-<Version>-windows-x64-setup.exe` installiert für Ihr Benutzerkonto (keine
  Administratorrechte nötig); die `.zip` ist dieselbe App als Ordner.
- **macOS** (Apple Silicon): die `.dmg` öffnen und ProxyDock in „Programme“ ziehen. Sie ist nicht von Apple signiert:
  beim ersten Mal mit Rechtsklick → Öffnen starten.
- **Linux** (x64): die `.AppImage` läuft direkt (vorher `chmod +x`); die `.tar.gz` ist dieselbe App als Ordner.

Die Einstellungen liegen in `%APPDATA%\ProxyDock` (Windows), `~/Library/Application Support/ProxyDock` (macOS) oder
`~/.config/ProxyDock` (Linux); Version 3 liest die Einstellungen von 2.x weiter. Den Rest erklärt die Seite **Anleitung**
in der App. ProxyDock 3 ist eine Neufassung in Rust; 2.x (die Python-App) liegt im Zweig [`old`](https://github.com/NenkaLab/proxy-dock/tree/old).

## Bauen

Aus dem Quellcode starten, Tests, Builds, Installer und Releases: [Build.md](Build.md) (Englisch). Prüfungen vor einem
Release: [TESTING.md](../TESTING.md) (Koreanisch).

## Einschränkungen

- macOS und Linux werden automatisch gebaut und getestet, aber noch nicht auf echten Rechnern ausprobiert.
- macOS / Linux: keine Regel, die andere PCs sperrt (die Firewall des Systems verwenden), kein „Als Administrator
  ausführen“ beim Start (eine TUN-Karte fragt beim Start nach dem Passwort), das Farbprofil des Monitors wird nicht
  gelesen (sRGB angenommen), Passwörter in den Einstellungen werden nicht verschlüsselt (wie in 2.x), und die
  Zurück-/Vor-Tasten der Maus navigieren nicht.
- Linux: der Systemproxy (PC-Block) wird nur unter GNOME und KDE gesetzt; Benachrichtigungen brauchen `notify-send`.
- TUN wurde noch nicht in echten Umgebungen ausprobiert.
- SOCKS4 hat kein Passwort: nur auf diesem PC verwenden.

## Lizenz

[GPL-3.0](../LICENSE). Enthaltene Komponenten (sing-box GPL-3.0, Hysteria MIT, Slint GPL-3.0, Rust-Crates, Schriften OFL-1.1, ...)
und ihre Lizenzen stehen in [`licenses/`](../licenses/).

Copyright (C) 2026 NenkaLab
