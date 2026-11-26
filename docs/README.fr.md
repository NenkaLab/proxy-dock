# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · [日本語](README.ja.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [Deutsch](README.de.md) · **Français** · [Español](README.es.md)

Une application de bureau qui fait tourner plusieurs proxys et tunnels côte à côte. Sur un canevas, on relie des blocs
**entrants** (comment le trafic arrive, p. ex. un port SOCKS5 local) à des blocs **sortants** (comment il sort,
p. ex. un serveur Shadowsocks), et chaque carte s'allume ou s'éteint.

<sub>Conçu pour mon propre usage et partagé tel quel.</sub>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="images/canvas-dark.png">
    <img src="images/canvas-light.png" width="98%" alt="Canevas">
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

## Fonctions

- **Relier des blocs sur un canevas** : entrées (SOCKS5, HTTP, serveur Shadowsocks / Hysteria2 / WireGuard, TUN, ...)
  vers sorties (Direct, proxy amont, Shadowsocks, Hysteria2, WireGuard, VLESS, VMess, Trojan, TUIC, abonnements)
- **Routage** : répartir le trafic par domaine, IP, programme ou pays ; un interrupteur IF sert de kill switch ; les
  groupes prennent le serveur le plus rapide ou basculent en cas de panne
- **Chaînes** : faire passer une sortie par une autre, ou par Tailscale, une appli VPN ou le Wi-Fi
- **Proxy système** : utiliser une carte comme proxy système Windows tant qu'elle tourne
- **Outils du quotidien** : liens de partage, tests de connexion, journaux et statistiques en direct, 8 langues,
  zone de notification

Les proxys tournent sur [sing-box](https://github.com/SagerNet/sing-box), fourni avec la version publiée.

## Lancer

Windows 10 / 11 : `ProxyDock-<version>-windows-x64-setup.exe` l'installe pour votre compte (sans droits administrateur) ;
le `.zip` contient la même application sous forme de dossier. Téléchargez-la sur [Releases](https://github.com/NenkaLab/proxy-dock/releases).

Les réglages sont dans `%APPDATA%\ProxyDock` ; la version 3 reprend les réglages de la 2.x. La page **Mode d'emploi** de
l'application explique le reste.

ProxyDock 3 est une réécriture en Rust et ne fonctionne pour l'instant que sous Windows. Sous macOS et Linux, utilisez la
2.x (l'application Python, sur la branche [`old`](https://github.com/NenkaLab/proxy-dock/tree/old) et dans les versions 2.x).

## Compiler

Lancer depuis les sources, tests, compilation, installateurs et versions : [Build.md](Build.md) (en anglais).
Vérifications avant une version : [TESTING.md](../TESTING.md) (en coréen).

## Limites

- Windows uniquement (macOS / Linux : 2.x).
- TUN n'a pas encore été essayé en conditions réelles.
- SOCKS4 n'a pas de mot de passe : ne l'utilisez que sur ce PC.

## Licence

[GPL-3.0](../LICENSE). Les composants inclus (sing-box GPL-3.0, Hysteria MIT, Slint GPL-3.0, crates Rust, polices OFL-1.1, ...)
et leurs licences sont dans [`licenses/`](../licenses/).

Copyright (C) 2026 NenkaLab
