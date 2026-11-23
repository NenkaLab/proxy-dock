# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · [日本語](README.ja.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [Deutsch](README.de.md) · **Français** · [Español](README.es.md)

Une application de bureau qui fait tourner plusieurs proxys et tunnels côte à côte. Sur un canevas, on relie des blocs
**entrants** (comment le trafic arrive, p. ex. un port SOCKS5 local) à des blocs **sortants** (comment il sort,
p. ex. un serveur Shadowsocks), et chaque carte s'allume ou s'éteint.

<p align="center">
  <img src="images/canvas.png" width="98%" alt="Canevas">
</p>

## Fonctions

- **Entrée** : SOCKS5, SOCKS4, HTTP, HTTPS, serveur Shadowsocks, serveur Hysteria2, TUN (toutes les applications du PC)
- **Sortie** : Direct, proxy HTTP / HTTPS / SOCKS, Shadowsocks, Hysteria2, WireGuard, VLESS (REALITY), VMess,
  Trojan, TUIC, abonnements (liens de partage ou YAML Clash)
- **Blocs** : chaînes (une sortie à travers une autre), routeurs (règles par domaine, IP, port, programme, GeoIP/Geosite),
  groupes (le plus rapide ou basculement), Bloquer
- **Interface réseau** : faire sortir une sortie par Tailscale, une application VPN ou le Wi-Fi sans modifier les routes du PC
- **DNS** : DNS du PC, DoH / DoT / DoQ, par carte, au besoin par le tunnel ; Fake-IP pour le TUN
- **Bloc PC** : utiliser une entrée HTTP comme proxy système Windows pendant qu'elle tourne
- Liens de partage (`ss://`, `vless://`, `vmess://`, `trojan://`, `tuic://`, `hysteria2://`), vérification et test de connexion,
  journaux et statistiques en direct, 8 langues, zone de notification, démarrage à l'ouverture de session

Les proxys tournent sur [sing-box](https://github.com/SagerNet/sing-box) (un processus par carte, inclus dans la version publiée).
Un serveur Hysteria2 ouvert aux autres PC tourne sur le cœur officiel [Hysteria](https://github.com/apernet/hysteria) ;
le moteur Python intégré sert de secours.

## Lancer

Téléchargez une version sur [Releases](https://github.com/NenkaLab/proxy-dock/releases), ou lancez depuis les sources
(Python 3.14) :

```bash
python -m venv .venv
.venv\Scripts\activate            # macOS / Linux : source .venv/bin/activate
pip install -r requirements.txt
python tools/fetch_singbox.py     # sing-box et Hysteria pour ce PC
python proxydock.py
```

Les réglages sont dans `%APPDATA%\ProxyDock` (macOS : `~/Library/Application Support/ProxyDock`,
Linux : `~/.config/ProxyDock`). La page **Mode d'emploi** de l'application explique le reste.

## Compiler

`tools\build_win.bat` → `dist/win/ProxyDock.exe` (Linux : `tools/build_linux.sh`, macOS : `tools/build_mac.sh`).
La compilation télécharge sing-box et Hysteria (SHA-256 vérifié) et les place dans l'application.
Tests : `python tests/run_tests.py`. Publication : `tools\release.bat <version>` crée le tag et pousse ; GitHub Actions compile.

## Limites

- Routeurs, groupes, WireGuard, VLESS / VMess / Trojan / TUIC, TUN, choix d'interface et DNS personnalisé nécessitent le moteur sing-box.
- Pas de répartition de charge entre serveurs ; un serveur Hysteria2 ne peut pas écouter seul sur une plage de ports.
- L'épinglage de certificat tourne sur le moteur Python jusqu'à sing-box 1.15.
- Le TUN et Fake-IP n'ont pas encore été essayés en conditions réelles, macOS pas sur un vrai Mac.
- SOCKS4 n'a pas de mot de passe : ne l'utilisez que sur ce PC.

## Licence

[GPL-3.0](../LICENSE). Les composants inclus (sing-box GPL-3.0, Hysteria MIT, Qt / PySide6 LGPL-3.0, polices OFL-1.1, ...)
et leurs licences sont dans [`licenses/`](../licenses/).

Copyright (C) 2026 NenkaLab
