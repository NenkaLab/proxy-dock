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
- **Direktleitung**: ausgewählte Blöcke gehen direkt über eine Netzwerkkarte hinaus, an einem VPN, einem TUN oder
  jeder App vorbei, die das Netzwerk des ganzen PCs übernimmt, ohne deren Split-Tunneling (Windows: eine kleine
  Hyper-V-VM; Linux: ein Netzwerk-Namespace; macOS: eine kleine VM, noch nicht ausprobiert)
- **Startseite**: ein Schalter für die ganze App (aus lehnt jede Verbindung ab, ein Kill-Switch; jede Karte behält
  ihr eigenes An / Aus), der Zustand der Karten, der Verkehr live und die Direktleitungs-VMs

Die Proxys laufen auf [sing-box](https://github.com/SagerNet/sing-box), das im Release enthalten ist.

## Starten

Zu laden unter [Releases](https://github.com/NenkaLab/proxy-dock/releases):

- **Windows 10 / 11**: `ProxyDock-<Version>-windows-x64-setup.exe` installiert für Ihr Benutzerkonto (keine
  Administratorrechte nötig); die `.zip` ist dieselbe App als Ordner.
- **macOS** (Apple Silicon): die `.dmg` öffnen und ProxyDock in „Programme“ ziehen. Sie ist nicht von Apple
  beglaubigt, daher blockiert macOS den ersten Start: Systemeinstellungen → Datenschutz & Sicherheit → **Dennoch öffnen**
  (Rechtsklick → Öffnen geht ab macOS 15 nicht mehr), oder die Download-Markierung im Terminal entfernen:
  ```
  xattr -dr com.apple.quarantine /Applications/ProxyDock.app
  ```
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
- Direktleitung: unter Windows braucht sie Hyper-V (Pro, Enterprise oder Education) und Administratorrechte; jede
  VM erscheint im Router als neues Gerät (eigene MAC- und IP-Adresse), und ein Anbieter, der nur eine Adresse pro
  Anschluss vergibt, gibt ihr womöglich keine. Ihr Verkehr zeigt deine echte IP-Adresse und kein VPN schützt ihn:
  du bist für die Nutzung verantwortlich (die App fragt, bevor sie eine VM erstellt).
- Direktleitung unter Linux: nur in einem Container ausprobiert (ein Netzwerk-Namespace mit macvlan; WLAN nutzt
  ipvlan, wo DHCP womöglich nicht geht: nutze eine feste Adresse). Braucht `ip` (iproute2) und root über pkexec.
- Direktleitung unter macOS (Apple Silicon): geschrieben und typgeprüft, aber **noch auf keinem Mac ausprobiert**
  (Virtualization.framework mit einer überbrückten vmnet-Schnittstelle; der Helfer muss als root laufen).

## Mitmachen

Wenn du etwas behoben oder verbessert hast – einen Fehler, eine noch nicht ausprobierte Plattform (macOS, Linux),
eine Übersetzung –, schick bitte einen Pull Request. Ob selbst geschrieben oder mit einem KI-Werkzeug, beides ist in
Ordnung, aber prüfe es vor dem Senden: bauen, die Tests laufen lassen (`cargo test --workspace` in `native/`) und
die Änderung in der App ausprobieren, wo es darauf ankommt auf einem echten Gerät. Schreib in den Pull Request, was
du wie geprüft hast.

## Lizenz

[GPL-3.0](../LICENSE). Enthaltene Komponenten (sing-box GPL-3.0, Hysteria MIT, Slint GPL-3.0, Rust-Crates, Schriften OFL-1.1, ...)
und ihre Lizenzen stehen in [`licenses/`](../licenses/).

Copyright (C) 2026 NenkaLab
