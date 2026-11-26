## Grundlagen

- **Eingangs**blöcke nehmen Verbindungen auf diesem PC an, zum Beispiel ein SOCKS5-Port.
- **Ausgangs**blöcke sind der Weg hinaus: Direkt, ein anderer Proxy oder ein Tunnel.
- Auf der **Fläche** ziehst du vom Ausgang eines Blocks zum Eingang eines anderen, um sie zu verbinden. Doppelklick bearbeitet einen Block.
- Starte einen Eingang mit **Starten** auf seinem Block oder im Tray-Menü.

## In einer App nutzen

- Rechtsklick auf einen Eingang → **Adresse/Link für Clients kopieren** und als Proxy der App eintragen.
- Ganzer PC: Rechtsklick auf einen HTTP-Eingang → **Als PC-Proxy verwenden** (Windows-Systemproxy, solange er läuft).
- Handys und andere PCs: den Link eines Shadowsocks-, Hysteria2- oder WireGuard-Server-Eingangs kopieren.

## Blöcke dazwischen

- **Router**: Regeln nach Domain, IP, Programm oder Land, jede mit eigenem Ausgang.
- **IF**: eine Bedingung (dann / sonst) oder ein manueller Ein/Aus-Schalter wie ein Kill-Switch.
- **Gruppe**: mehrere Ausgänge als einer: der schnellste, der erste, der geht, oder ein gewählter.
- **via**: die eigene Verbindung eines Ausgangs läuft über einen anderen Ausgang (Kette).
- **Durchleitung** und **Funkpaar**: ordnen nur die Linien.

## Tasten auf der Fläche

| Taste | Aktion |
|---|---|
| Strg+Z / Strg+Y | Rückgängig / Wiederholen |
| F2 · Strg+D · Entf | Umbenennen · Duplizieren · Löschen |
| ← → | Block davor / danach entlang der Linien |
| ↑ ↓ | Block darüber / darunter (nach ← → die andere Wahl, wenn es mehrere gibt) |

## Andere PCs

Setze die Adresse des Eingangs auf `0.0.0.0` und schalte ein Passwort ein. Unter Windows steht die Firewall-Regel in den **Einstellungen**.

## Schließen

X blendet nur das Fenster aus; die Proxys laufen im Tray weiter. Beenden über das Tray-Menü.
