# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · [日本語](README.ja.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · **Deutsch** · [Français](README.fr.md) · [Español](README.es.md)

Ein Desktop-„Dock" für Proxys: mehrere Proxys anlegen und jeden einzeln ein- oder ausschalten. Für den persönlichen Gebrauch entstanden.

Jeder Proxy kombiniert einen **Eingang** (wie er Verbindungen an einem Port dieses PCs annimmt) und einen **Ausgang**
(wie er das Ziel erreicht). So ist z. B. „Annahme über lokales SOCKS5 → verschlüsselter Tunnel zu einem entfernten
Shadowsocks-Server" eine einzige Karte.

<p align="center">
  <img src="images/canvas.png" width="98%" alt="Fläche">
</p>
<p align="center">
  <img src="images/proxies.png" width="49%" alt="Proxy-Liste">
  <img src="images/editor.png" width="49%" alt="Eingang bearbeiten">
</p>

## Funktionen

| | Unterstützt |
|---|---|
| Eingang | SOCKS5 · SOCKS4/4a · HTTP · HTTPS (TLS) · Shadowsocks-Server · Hysteria2-Server · TUN (alle Apps) |
| Ausgang | Direct · Upstream-HTTP / HTTPS / SOCKS4 / SOCKS5-Proxy · Shadowsocks-Tunnel · Hysteria2-Tunnel (QUIC) · WireGuard · VLESS (TLS / REALITY) · VMess · Trojan · TUIC · Abonnement · Gruppe |
| UDP | Eingang SOCKS5 · Shadowsocks · Hysteria2, Ausgang Direct · Shadowsocks · Hysteria2 · WireGuard · VLESS · VMess · Trojan · TUIC |

- **Fläche**: Editor im Blueprint-Stil - Linien von Eingangsblöcken zu Ausgangsblöcken ziehen; mehrere Eingänge können einen Ausgang teilen, ein mit einem anderen Eingang verbundener Eingang leitet über diesen weiter, und Direct kann eine gewählte Quell-IP nutzen
- **PC-Block**: einen HTTP-Eingang verbinden, um ihn als Windows-Systemproxy zu nutzen, solange er läuft; die vorherigen Einstellungen werden gesichert und beim Stoppen oder Beenden wiederhergestellt (Knöpfe Vorherige wiederherstellen / Aus)
- **Verkettung**: der 'via'-Ausgang eines Ausgangsblocks führt dessen eigene Serververbindung über einen anderen Ausgang (z. B. Shadowsocks durch den Firmen-HTTP-Proxy, HTTPS-Upstream durch einen Hysteria2-Tunnel), beliebig viele Stufen
- **Router**: Ein Router-Block zwischen Eingang und Ausgängen gibt Regeln für Domains, IP-Bereiche, Ports, Programme
  (chrome.exe, ...), GeoIP-Länder und Geosite-Kategorien je einen eigenen Ausgang; nicht passender Verkehr nimmt den
  Standard. Regeln können zu jedem Ausgang, zum **Block**-Ausgang (ablehnen) oder zu einem anderen Router führen
- **Netzwerkschnittstelle**: Jeder Ausgang kann über eine gewählte Netzwerkkarte hinausgehen - Tailscale, WireGuard, eine VPN-App, WLAN - ohne die Routen des PCs zu ändern; nur der dorthin verbundene Verkehr nutzt sie (sing-box-Engine)
- **WireGuard**: ein WireGuard-Tunnelblock, der in ProxyDock läuft - keine VPN-App, daher können mehrere Länder gleichzeitig für verschiedene Apps laufen; `.conf` eines Anbieters importieren; ein WireGuard-Block 'über' einen anderen ergibt ein Doppel-VPN (sing-box-Engine)
- **TUN**: eine virtuelle Netzwerkkarte, die alle Apps auf diesem PC erfasst, auch ohne Proxy-Einstellung - mit einem Router verbunden gelten Programmregeln (game.exe, chrome.exe) für alle Apps. Fragt beim Start nach Administratorrechten (UAC), außer ProxyDock läuft schon als Administrator (sing-box-Engine)
- **VLESS / VMess / Trojan / TUIC**: VLESS mit TLS oder REALITY (Vision-Flow), Trojan, TUIC v5, über TCP, WebSocket oder gRPC. Den `vless://`-, `vmess://`-, `trojan://`-, `tuic://`-, `ss://`- oder `hysteria2://`-Link eines Anbieters kopieren und **Ausgang hinzufügen → Aus einem Freigabelink** wählen (oder **Freigabelink einfügen** im Editor). Cloudflare WARP: ein WARP-WireGuard-Profil (z. B. mit wgcf erstellt) in einen WireGuard-Block importieren (sing-box-Engine)
- **Abonnements**: Die Abo-URL eines Anbieters wird zu einem Block mit allen Servern - einen wählen oder automatisch den schnellsten nehmen (misst alle paar Minuten und meidet ausgefallene Server). Aktualisiert sich nach Zeitplan; die Liste wird verschlüsselt gespeichert und startet auch offline (sing-box-Engine)
- **Gruppen**: Ein Gruppenblock nutzt mehrere Ausgangsblöcke wie einen - den schnellsten oder den ersten funktionierenden in der festgelegten Reihenfolge (Failover). Die Mitglieder über den Ausgang „Mitglieder“ verbinden; ausgefallene Server werden automatisch übersprungen (sing-box-Engine)
- **DNS**: den DNS-Server wählen, den ProxyDock nutzt, wo es Namen selbst auflöst (Direct, IP-Regeln, TUN) - DNS des PCs oder UDP/TCP, DoT (tls://), DoH (https://), DoQ (quic://), HTTP/3 (h3://). Global oder pro Karte; auf Wunsch durch den Tunnel der Karte. TUN-Karten können Fake-IP nutzen, sodass Namen am Ausgang statt im lokalen Netz aufgelöst werden (sing-box-Engine)
- **Engine**: sing-box (Standard) führt jeden Proxy in einem eigenen Prozess aus - mehrere Kerne, UDP über Ketten, Hysteria2 über andere Ausgänge. Die integrierte Python-Engine ist der Ersatz (Einstellungen → Proxy-Engine)
- **Shadowsocks**: AEAD 2018 (aes-128/256-gcm, chacha20-ietf-poly1305) und 2022 (SIP022, blake3), TCP+UDP, `ss://`-Links kopieren
- **Hysteria2**: in beide Richtungen mit der offiziellen Implementierung kompatibel (TCP und UDP), Zertifikats-Pinning, `hysteria2://`-Links kopieren
- **Einstellungen**: globale Standardwerte + lokale Einstellungen pro Proxy (lokal hat Vorrang) - zugelassene Domains, Blockieren privater Netze, Leerlauf-Timeout, Protokoll
- **Verbindung prüfen** im Editor: testet vor dem Speichern den Ausgang (Upstream/Tunnel, Passwörter) und den Eingangsport
- **Verbindungstest**: verbindet sich über einen laufenden Proxy per HTTPS mit 21 Websites in vielen Ländern und misst die Antwortzeiten
- **Protokoll / Statistik** pro Proxy, Live-Durchsatzgraph, optionale Protokolldatei
- **Sicherheit**: Passwörter per Windows DPAPI gespeichert (auf anderen Systemen 0600-Dateien), Verbindungslimit pro IP,
  vorübergehende Sperre nach wiederholten Anmeldefehlern, Erkennung von Schleifen in Proxy-Ketten, Windows-Firewall-Sperre per Knopfdruck
- **Sprachen**: English (Standard), 한국어, 日本語, 简体中文, 繁體中文, Deutsch, Français, Español (Einstellungen → Anzeige → Sprache)
- Infobereich, Start bei Anmeldung, helles/dunkles Design, integrierte Schriften (Pretendard / Pretendard JP / Noto Sans SC·TC / JetBrains Mono)

## Ausführen

Nutzen Sie einen Build aus den [Releases](https://github.com/NenkaLab/proxy-dock/releases), falls vorhanden, oder starten Sie aus dem Quellcode:

```bash
python -m venv .venv
# Windows: .venv\Scripts\activate   /   macOS, Linux: source .venv/bin/activate
pip install -r requirements.txt
python tools/fetch_singbox.py   # sing-box-Engine (ohne sie nutzen Proxys die Python-Engine)
python proxydock.py            # --hidden : ohne Fenster im Infobereich starten
```

Entwickelt und getestet mit Python 3.14.

Beim ersten Start gibt es keine Proxys. Wählen Sie unter **Neuer Proxy** eine Vorlage (Lokaler SOCKS5, Lokaler HTTP,
Shadowsocks-Server, Lokal → Shadowsocks-Tunnel, ...) und starten Sie sie mit dem Schalter auf der Karte.
Details stehen auf der Seite **Anleitung** in der App. Die Oberfläche ist standardmäßig englisch; die Sprache lässt sich
unter Einstellungen → Anzeige → Sprache ändern (nach Neustart wirksam).

Speicherort der Einstellungen

| System | Pfad |
|---|---|
| Windows | `%APPDATA%\ProxyDock` |
| macOS | `~/Library/Application Support/ProxyDock` |
| Linux | `$XDG_CONFIG_HOME/ProxyDock` (Standard `~/.config/ProxyDock`) |

## Bauen

Alles zum Bauen liegt in [`tools/`](../tools/). PyInstaller erzeugt eine einzelne ausführbare Datei (unter macOS eine `.app`).
Die Skripte legen pro System eine virtuelle Umgebung an, installieren nur die Pakete, die von `requirements.txt` abweichen,
und bauen mit `tools/ProxyDock.spec`. Sie lassen sich aus jedem Ordner starten.

| Befehl | Ergebnis |
|---|---|
| `tools\build_win.bat` | `dist/win/ProxyDock.exe` (unter Windows) |
| `tools\build_linux.bat` / `tools/build_linux.sh` | `dist/linux/ProxyDock` (nativ unter Linux, per Docker unter Windows/macOS) |
| `tools/build_mac.sh` | `dist/mac/ProxyDock.app` (nur unter macOS) |
| `tools\build.bat` / `tools/build.sh` | alles, was dieser Computer bauen kann, mit Zusammenfassung |

Der macOS-Build wurde noch nicht auf einem echten Mac getestet.

## Releases und CI

- Jeder Push und Pull Request führt die Tests unter Windows, Linux und macOS aus ([CI](../.github/workflows/ci.yml),
  lokal `python tests/run_tests.py` - ohne Internet).
- `tools\release.bat 2.1.0` (oder `python tools/release.py 2.1.0`) prüft den Arbeitsstand, führt die Tests aus, erhöht
  `__version__`, committet, setzt das Tag `v2.1.0` und pusht. Das Tag startet den [Release-Workflow](../.github/workflows/release.yml),
  der Windows / Linux / macOS baut und ein GitHub-Release mit den Archiven und `SHA256SUMS.txt` veröffentlicht.
  Mit `--dry-run` wird nur geprobt.
- `python tools/build.py package win|linux|mac` erzeugt dieselben Archive lokal in `dist/release/`.

## Bekannte Einschränkungen

- Abonnements lesen Listen von Freigabelinks (unverändert oder base64), kein Clash-YAML. 'Automatisch der schnellste' misst die Server alle paar Minuten mit einer kleinen Testseite (gstatic.com generate_204).
- Gruppen verteilen den Verkehr nicht auf mehrere Server (Lastverteilung; sing-box 1.14 hat dafür keinen Ausgang). Nach einem Failover bleibt die Gruppe beim neuen Mitglied, bis auch dieses ausfällt.
- Tunnel und Upstream-Proxys lösen Domains weiterhin auf ihrem Server auf; die DNS-Einstellung betrifft Abfragen auf diesem PC. Fake-IP wurde noch nicht in echten Umgebungen getestet (mit TUN). DNS-Regeln pro Domain (ein anderer Server für bestimmte Namen) gibt es nicht.
- TUN wurde noch nicht in echten Umgebungen getestet. Nur eine TUN-Karte gleichzeitig; DNS der erfassten Apps beantwortet
  ProxyDock auf diesem PC; strikte Route kann unter Windows IPv6 stören (standardmäßig aus). Unter macOS/Linux muss ProxyDock selbst als Root laufen.
- Router- und Block-Blöcke brauchen die sing-box-Engine. IP- und GeoIP-Regeln lösen die Domain zuerst auf diesem PC auf,
  ein Tunnel dahinter verbindet sich also mit der IP statt die Domain entfernt aufzulösen. Programmregeln sehen nur Verbindungen von diesem PC.
- sing-box-Engine: Ein für andere PCs offener Hysteria2-Server läuft mit dem offiziellen Hysteria2-Kern, der ProxyDock bei jeder
  Anmeldung fragt (sing-box meldet falsche Passwörter nicht, IP-Sperren würden also nicht greifen); der Verkehr läuft dann über
  sing-box. Zertifikat-Pinning läuft weiter mit der Python-Engine (braucht sing-box 1.15). Das Limit pro IP trennt überzählige
  Verbindungen nachträglich; TCP-Leerlaufzeit und 'Details zum Verbindungsende' werden einmal pro Sekunde geprüft, sehr kurze Verbindungen bekommen daher keine Endzeile.
- Python-Engine: Hysteria2 kann nicht *über* andere Ausgänge laufen, UDP wird über verkettete Ausgänge nicht weitergeleitet, und
  Hysteria2 (aioquic) schafft etwa 100 Mbit/s. Hysteria2-Bandbreite (Brutal), Salamander und Port-Hopping brauchen die sing-box-Engine; ein Server auf einem Portbereich braucht dort eine Portweiterleitung (richtet ProxyDock nicht ein).
- Der offizielle Hysteria2-Server (und andere Server, die den Namen prüfen) lehnt eine SNI ab, die nicht zum Zertifikat passt. Nutzt der Ausgang eine IP, neben dem SNI-Feld auf **Vom Server erkennen** klicken, um den Namen aus dem Zertifikat zu lesen.
- Der HTTP-Eingang bearbeitet pro Verbindung nur eine normale (Nicht-CONNECT-)Anfrage.
- SOCKS4 hat keine Passwort-Anmeldung, nutzen Sie es daher nur auf diesem PC.

## Aufbau

```
proxydock.py            Einstiegspunkt
proxydock/
  common.py             gemeinsame Konstanten, Protokoll, Adressbehandlung, Sperren
  config.py             Einstellungsformat (v2), Prüfung, Zusammenführung global/lokal, Passwortspeicher, Migration
  graph.py              Blockgraph (Eingangs- / Ausgangs- / PC-Blöcke) -> Laufzeit-Proxys
  runtime.py            ProxyInstance / ProxyManager (viele Proxys in einem asyncio-Thread)
  relay.py              TCP/UDP-Relay-Kern
  inbound.py            Eingangsprotokolle
  outbound.py           Ausgangsprotokolle
  ss.py                 Shadowsocks-2018/2022-Verschlüsselung
  hysteria2.py          Hysteria2-Server und -Client (aioquic)
  singbox/              sing-box-Engine: Konfigurationsumwandlung, Prozessstart, Sperren / Limit pro IP
  tls.py                selbstsignierte Zertifikate, TLS-Kontexte
  probe.py              Verbindungstest / Verbindungsprüfung
  platform_utils.py     Start bei Anmeldung, Firewall, Portprüfung
  i18n.py, locales/     Übersetzungen (englische Ausgangstexte -> <Sprache>.json)
  ui/                   Oberfläche mit PySide6 + Fluent Widgets
    canvas.py           Seite Fläche (NodeGraphQt)
tools/                  Build-Skripte, PyInstaller-Spec, Dockerfile für den Linux-Build
licenses/               Lizenzen der enthaltenen Drittkomponenten
docs/                   übersetzte READMEs, Screenshots
```

### Übersetzungen

Die Texte der Oberfläche stehen englisch im Code (`_t("...")`) und werden über `proxydock/locales/<Sprache>.json` übersetzt.
Fehlende Einträge erscheinen auf Englisch.

## Lizenz

[GPL-3.0](../LICENSE). Die UI-Bibliothek [PySide6-Fluent-Widgets](https://github.com/zhiyiYo/PyQt-Fluent-Widgets) steht unter
GPL v3, daher wird auch dieses Programm unter GPL v3 verteilt; wer gebaute Programmdateien weitergibt, muss die Bedingungen der
GPL v3 einhalten (Quellcode bereitstellen usw.).

Die Lizenzen der enthaltenen Schriften und Bibliotheken liegen im Ordner [`licenses/`](../licenses/).

| Komponente | Lizenz |
|---|---|
| Pretendard, Pretendard JP, Noto Sans SC/TC, JetBrains Mono (integrierte Schriften) | SIL OFL 1.1 |
| PySide6-Fluent-Widgets | GPL v3 |
| sing-box | GPL v3 |
| Hysteria2 | MIT |
| Qt 6 / PySide6 / Shiboken6, PySideSix-Frameless-Window | LGPL v3 |
| aioquic, pylsqpack, darkdetect | BSD 3-Clause |
| cryptography | Apache 2.0 / BSD 3-Clause |
| pyOpenSSL | Apache 2.0 |
| service-identity, attrs, NodeGraphQt, Qt.py | MIT |
| certifi | MPL 2.0 |
| Python-Laufzeit | PSF License |
| PyInstaller-Bootloader | GPL v2+ (Bootloader-Ausnahme) |

Copyright (C) 2026 NenkaLab
