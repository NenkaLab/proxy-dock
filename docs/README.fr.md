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
- **Ligne directe** : les blocs choisis sortent directement par une carte réseau, en passant outre un VPN, un TUN ou
  toute app qui prend en charge le réseau de tout le PC, sans son split tunneling (Windows : une petite VM Hyper-V ;
  Linux : un espace de noms réseau ; macOS : une petite VM, pas encore essayée)
- **Page d'accueil** : un interrupteur pour toute l'app (désactivé, il refuse toute connexion, un kill switch ; chaque
  carte garde son propre état), l'état des cartes, le trafic en direct et les VM de ligne directe

Les proxys tournent sur [sing-box](https://github.com/SagerNet/sing-box), fourni avec la version publiée.

## Lancer

Téléchargez-la sur [Releases](https://github.com/NenkaLab/proxy-dock/releases) :

- **Windows 10 / 11** : `ProxyDock-<version>-windows-x64-setup.exe` l'installe pour votre compte (sans droits
  administrateur) ; le `.zip` contient la même application sous forme de dossier.
- **macOS** (Apple Silicon) : ouvrez le `.dmg` et faites glisser ProxyDock dans Applications. Elle n'est pas signée par
  Apple : la première fois, clic droit → Ouvrir.
- **Linux** (x64) : l'`.AppImage` se lance telle quelle (d'abord `chmod +x`) ; le `.tar.gz` contient la même
  application sous forme de dossier.

Les réglages sont dans `%APPDATA%\ProxyDock` (Windows), `~/Library/Application Support/ProxyDock` (macOS) ou
`~/.config/ProxyDock` (Linux) ; la version 3 reprend les réglages de la 2.x. La page **Mode d'emploi** de l'application
explique le reste. ProxyDock 3 est une réécriture en Rust ; la 2.x (l'application Python) est sur la branche [`old`](https://github.com/NenkaLab/proxy-dock/tree/old).

## Compiler

Lancer depuis les sources, tests, compilation, installateurs et versions : [Build.md](Build.md) (en anglais).
Vérifications avant une version : [TESTING.md](../TESTING.md) (en coréen).

## Limites

- macOS et Linux sont compilés et testés automatiquement, mais pas encore essayés sur de vraies machines.
- macOS / Linux : pas de règle qui bloque les autres PC (utilisez le pare-feu du système), pas d'« Exécuter en tant
  qu'administrateur » au démarrage (une carte TUN demande le mot de passe en démarrant), le profil colorimétrique de
  l'écran n'est pas lu (sRGB supposé), les mots de passe des réglages ne sont pas chiffrés (comme en 2.x) et les
  boutons précédent / suivant de la souris ne reviennent pas en arrière.
- Linux : le proxy système (bloc PC) n'est réglé que sous GNOME et KDE ; les notifications ont besoin de `notify-send`.
- TUN n'a pas encore été essayé en conditions réelles.
- SOCKS4 n'a pas de mot de passe : ne l'utilisez que sur ce PC.
- Ligne directe : sous Windows elle demande Hyper-V (Pro, Entreprise ou Éducation) et les droits d'administrateur ;
  chaque VM apparaît sur votre routeur comme un nouvel appareil (ses propres adresses MAC et IP), et un fournisseur
  qui ne donne qu'une adresse par ligne risque de ne pas lui en donner. Son trafic montre votre vraie adresse IP et
  aucun VPN ne le protège : vous êtes responsable de son usage (l'app demande votre accord avant de créer une VM).
- Ligne directe sous Linux : essayée en conteneur seulement (un espace de noms réseau avec macvlan ; le Wi-Fi utilise
  ipvlan, où le DHCP peut ne pas marcher : utilisez une adresse fixe). Demande `ip` (iproute2) et root via pkexec.
- Ligne directe sous macOS (Apple Silicon) : écrite et vérifiée au typage, mais **encore jamais essayée sur un Mac**
  (Virtualization.framework avec une interface vmnet pontée ; l'assistant doit tourner en root).

## Contribuer

Si vous avez corrigé ou amélioré quelque chose – un bug, une plateforme pas encore essayée (macOS, Linux), une
traduction –, envoyez une pull request. À la main ou avec un outil d'IA, peu importe, mais vérifiez avant d'envoyer :
compilez, lancez les tests (`cargo test --workspace` dans `native/`) et essayez le changement dans l'app, sur une
vraie machine quand cela compte. Indiquez dans la pull request ce que vous avez vérifié et comment.

## Licence

[GPL-3.0](../LICENSE). Les composants inclus (sing-box GPL-3.0, Hysteria MIT, Slint GPL-3.0, crates Rust, polices OFL-1.1, ...)
et leurs licences sont dans [`licenses/`](../licenses/).

Copyright (C) 2026 NenkaLab
