# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · [日本語](README.ja.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [Deutsch](README.de.md) · **Français** · [Español](README.es.md)

Un « dock » de proxys pour le bureau : créez plusieurs proxys et activez ou désactivez chacun d'eux. Conçu pour un usage personnel.

Chaque proxy associe une **entrée** (comment il accepte les connexions sur un port de ce PC) et une **sortie**
(comment il atteint la destination). Par exemple, « réception sur un SOCKS5 local → tunnel chiffré vers un serveur
Shadowsocks distant » tient sur une seule carte.

<p align="center">
  <img src="images/canvas.png" width="98%" alt="Canevas">
</p>
<p align="center">
  <img src="images/proxies.png" width="49%" alt="Liste des proxys">
  <img src="images/editor.png" width="49%" alt="Éditeur d'entrée">
</p>

## Fonctionnalités

| | Pris en charge |
|---|---|
| Entrée | SOCKS5 · SOCKS4/4a · HTTP · HTTPS (TLS) · serveur Shadowsocks · serveur Hysteria2 · TUN (toutes les applications) |
| Sortie | Direct · proxy amont HTTP / HTTPS / SOCKS4 / SOCKS5 · tunnel Shadowsocks · tunnel Hysteria2 (QUIC) · WireGuard · VLESS (TLS / REALITY) · Trojan · TUIC |
| UDP | entrée SOCKS5 · Shadowsocks · Hysteria2, sortie Direct · Shadowsocks · Hysteria2 · WireGuard · VLESS · Trojan · TUIC |

- **Canevas** : éditeur façon blueprint - tirez des liens des blocs d'entrée vers les blocs de sortie ; plusieurs entrées peuvent partager une sortie, une entrée reliée à une autre entrée passe par celle-ci, et Direct peut partir d'une IP source choisie
- **Bloc PC** : reliez une entrée HTTP pour l'utiliser comme proxy système Windows pendant qu'elle tourne ; les paramètres précédents sont sauvegardés et remis à l'arrêt ou à la fermeture (boutons Restaurer l'état précédent / Désactivé)
- **Chaînage** : la sortie 'via' d'un bloc de sortie fait passer sa propre connexion au serveur par une autre sortie (ex. Shadowsocks à travers le proxy HTTP de l'entreprise, proxy HTTPS amont dans un tunnel Hysteria2), sur autant d'étapes que nécessaire
- **Routeur** : un bloc routeur entre une entrée et ses sorties donne à chaque règle (domaines, plages d'IP, ports,
  programmes comme chrome.exe, pays GeoIP, catégories Geosite) sa propre sortie ; le reste prend la sortie par défaut.
  Les règles peuvent mener à n'importe quelle sortie, à la sortie **Blocage** (refus) ou à un autre routeur
- **Interface réseau** : toute sortie peut passer par une carte réseau choisie - Tailscale, WireGuard, une application VPN, le Wi-Fi - sans modifier les routes du PC ; seul le trafic relié à elle l'utilise (moteur sing-box)
- **WireGuard** : un bloc tunnel WireGuard exécuté dans ProxyDock - sans application VPN, plusieurs pays peuvent être actifs en même temps pour des applications différentes ; import du `.conf` d'un fournisseur ; un bloc WireGuard « par » un autre donne un double VPN (moteur sing-box)
- **TUN** : une carte réseau virtuelle qui capture toutes les applications de ce PC, même sans réglage de proxy - reliée à un routeur, les règles de programme (game.exe, chrome.exe) s'appliquent à toutes. Demande les droits administrateur (UAC) au démarrage, sauf si ProxyDock tourne déjà en administrateur (moteur sing-box)
- **VLESS / Trojan / TUIC** : VLESS avec TLS ou REALITY (flow Vision), Trojan, TUIC v5, sur TCP, WebSocket ou gRPC. Copiez le lien `vless://`, `trojan://`, `tuic://`, `ss://` ou `hysteria2://` d'un fournisseur puis **Ajouter une sortie → Depuis un lien de partage** (ou **Coller un lien de partage** dans l'éditeur). Cloudflare WARP : importez un profil WireGuard WARP (fait par ex. avec wgcf) dans un bloc WireGuard (moteur sing-box)
- **Moteur** : sing-box (par défaut) exécute chaque proxy dans son propre processus - multicœur, UDP à travers les chaînes, Hysteria2 via d'autres sorties. Le moteur Python intégré sert de secours (Paramètres → Moteur de proxy)
- **Shadowsocks** : AEAD 2018 (aes-128/256-gcm, chacha20-ietf-poly1305) et 2022 (SIP022, blake3), TCP+UDP, copie des liens `ss://`
- **Hysteria2** : interopérable dans les deux sens avec l'implémentation officielle (TCP et UDP), épinglage du certificat, copie des liens `hysteria2://`
- **Paramètres** : valeurs globales par défaut + paramètres locaux par proxy (le local prime) - domaines autorisés, blocage du réseau privé, délai d'inactivité, journal
- **Vérifier la connexion** dans l'éditeur : teste la sortie (amont/tunnel, mots de passe) et le port d'entrée avant d'enregistrer
- **Test de connexion** : se connecte en HTTPS à 21 sites de nombreux pays via un proxy en cours d'exécution et mesure les temps de réponse
- **Journal / statistiques** par proxy, graphique de débit en direct, fichier journal facultatif
- **Sécurité** : mots de passe stockés avec Windows DPAPI (fichiers 0600 sur les autres systèmes), limite de connexions par IP,
  blocage temporaire après des échecs d'authentification répétés, détection des boucles dans les chaînes de proxys, bouton de blocage du pare-feu Windows
- **Langues** : English (par défaut), 한국어, 日本語, 简体中文, 繁體中文, Deutsch, Français, Español (Paramètres → Affichage → Langue)
- Zone de notification, démarrage à l'ouverture de session, thème clair/sombre, polices intégrées (Pretendard / Pretendard JP / Noto Sans SC·TC / JetBrains Mono)

## Lancement

Utilisez une version des [Releases](https://github.com/NenkaLab/proxy-dock/releases) s'il y en a une, ou lancez depuis les sources :

```bash
python -m venv .venv
# Windows : .venv\Scripts\activate   /   macOS, Linux : source .venv/bin/activate
pip install -r requirements.txt
python tools/fetch_singbox.py   # moteur sing-box (sans lui, les proxys utilisent le moteur Python)
python proxydock.py            # --hidden : démarrer dans la zone de notification sans fenêtre
```

Développé et testé avec Python 3.14.

Au premier lancement, il n'y a aucun proxy. Choisissez un modèle dans **Nouveau proxy** (SOCKS5 local, HTTP local,
serveur Shadowsocks, Local → tunnel Shadowsocks, ...) et démarrez-le avec l'interrupteur de sa carte.
La page **Mode d'emploi** de l'application donne les détails. L'interface est en anglais par défaut ; changez la langue
dans Paramètres → Affichage → Langue (après redémarrage).

Emplacement des paramètres

| Système | Chemin |
|---|---|
| Windows | `%APPDATA%\ProxyDock` |
| macOS | `~/Library/Application Support/ProxyDock` |
| Linux | `$XDG_CONFIG_HOME/ProxyDock` (par défaut `~/.config/ProxyDock`) |

## Compilation

Tout ce qui sert à la compilation se trouve dans [`tools/`](../tools/). PyInstaller produit un exécutable unique (une `.app` sur macOS).
Les scripts créent un environnement virtuel par système, n'installent que les paquets qui diffèrent de `requirements.txt`,
puis compilent avec `tools/ProxyDock.spec`. Ils peuvent être lancés depuis n'importe quel dossier.

| Commande | Résultat |
|---|---|
| `tools\build_win.bat` | `dist/win/ProxyDock.exe` (sous Windows) |
| `tools\build_linux.bat` / `tools/build_linux.sh` | `dist/linux/ProxyDock` (en natif sous Linux, via Docker sous Windows/macOS) |
| `tools/build_mac.sh` | `dist/mac/ProxyDock.app` (macOS uniquement) |
| `tools\build.bat` / `tools/build.sh` | tout ce que cet ordinateur peut compiler, avec un récapitulatif |

La compilation macOS n'a pas encore été essayée sur un vrai Mac.

## Versions et CI

- Chaque push et pull request lance les tests sous Windows, Linux et macOS ([CI](../.github/workflows/ci.yml),
  en local `python tests/run_tests.py` - sans internet).
- `tools\release.bat 2.1.0` (ou `python tools/release.py 2.1.0`) vérifie l'arbre de travail, lance les tests, augmente
  `__version__`, commite, crée le tag `v2.1.0` et pousse. Le tag démarre le [workflow de publication](../.github/workflows/release.yml),
  qui compile Windows / Linux / macOS et publie une GitHub Release avec les archives et `SHA256SUMS.txt`.
  Ajoutez `--dry-run` pour une répétition.
- `python tools/build.py package win|linux|mac` produit les mêmes archives en local dans `dist/release/`.

## Limites connues

- Le TUN n'a pas encore été essayé en conditions réelles. Une seule carte TUN à la fois ; le DNS des applications capturées
  est résolu par ProxyDock sur ce PC ; la route stricte peut casser l'IPv6 sous Windows (désactivée par défaut). Sous macOS/Linux, ProxyDock doit tourner en root.
- Les blocs Routeur et Blocage nécessitent le moteur sing-box. Les règles IP et GeoIP résolvent d'abord le domaine sur ce PC :
  un tunnel derrière elles se connecte à l'IP au lieu de résoudre le domaine à distance. Les règles de programme ne voient que les connexions de ce PC.
- Moteur sing-box : un serveur Hysteria2 ouvert aux autres PC tourne sur le cœur Hysteria2 officiel, qui interroge ProxyDock à chaque
  connexion (sing-box ne signale pas les mauvais mots de passe, le bannissement d'IP ne marcherait pas) ; le trafic passe ensuite par
  sing-box. L'épinglage de certificat reste sur le moteur Python (sing-box 1.15 requis). La limite par IP ferme les connexions en trop
  après coup ; le délai d'inactivité ne vaut que pour l'UDP ; « Détails de fermeture » n'existe pas.
- Moteur Python : Hysteria2 ne peut pas passer *par* d'autres sorties, l'UDP n'est pas relayé à travers les sorties chaînées, et
  Hysteria2 (aioquic) atteint environ 100 Mbit/s. Aucun moteur ne gère les réglages de bande passante Hysteria2 (Brutal), Salamander ni le saut de ports.
- Le serveur Hysteria2 officiel refuse les connexions dont le SNI ne correspond pas à son certificat - saisissez le domaine du
  certificat du serveur dans le champ SNI de la sortie.
- L'entrée HTTP ne traite qu'une requête ordinaire (non CONNECT) par connexion.
- SOCKS4 n'a pas d'authentification par mot de passe : ne l'utilisez que sur ce PC.

## Organisation

```
proxydock.py            point d'entrée
proxydock/
  common.py             constantes communes, journal, gestion des adresses, blocages
  config.py             format des paramètres (v2), validation, fusion global/local, stockage des mots de passe, migration
  graph.py              graphe de blocs (entrée / sortie / PC) -> proxys d'exécution
  runtime.py            ProxyInstance / ProxyManager (plusieurs proxys sur un seul thread asyncio)
  relay.py              cœur du relais TCP/UDP
  inbound.py            protocoles d'entrée
  outbound.py           protocoles de sortie
  ss.py                 chiffrement Shadowsocks 2018/2022
  hysteria2.py          serveur et client Hysteria2 (aioquic)
  singbox/              moteur sing-box : conversion de configuration, lancement du processus, bannissement / limite par IP
  tls.py                certificats auto-signés, contextes TLS
  probe.py              test de connexion / vérification de connexion
  platform_utils.py     démarrage à l'ouverture de session, pare-feu, vérification de port
  i18n.py, locales/     traductions (textes source anglais -> <langue>.json)
  ui/                   interface PySide6 + Fluent Widgets
    canvas.py           page canevas (NodeGraphQt)
tools/                  scripts de compilation, spec PyInstaller, Dockerfile de compilation Linux
licenses/               licences des composants tiers inclus
docs/                   README traduits, captures d'écran
```

### Traductions

Les textes de l'interface sont écrits en anglais dans le code (`_t("...")`) et traduits via `proxydock/locales/<langue>.json`.
Une entrée manquante s'affiche en anglais.

## Licence

[GPL-3.0](../LICENSE). La bibliothèque d'interface [PySide6-Fluent-Widgets](https://github.com/zhiyiYo/PyQt-Fluent-Widgets) est
sous GPL v3 ; ce programme est donc également distribué sous GPL v3, et la distribution d'exécutables compilés doit respecter les
conditions de la GPL v3 (fourniture du code source, etc.).

Les licences des polices et bibliothèques incluses se trouvent dans le dossier [`licenses/`](../licenses/).

| Composant | Licence |
|---|---|
| Pretendard, Pretendard JP, Noto Sans SC/TC, JetBrains Mono (polices intégrées) | SIL OFL 1.1 |
| PySide6-Fluent-Widgets | GPL v3 |
| sing-box | GPL v3 |
| Hysteria2 | MIT |
| Qt 6 / PySide6 / Shiboken6, PySideSix-Frameless-Window | LGPL v3 |
| aioquic, pylsqpack, darkdetect | BSD 3-Clause |
| cryptography | Apache 2.0 / BSD 3-Clause |
| pyOpenSSL | Apache 2.0 |
| service-identity, attrs, NodeGraphQt, Qt.py | MIT |
| certifi | MPL 2.0 |
| Environnement d'exécution Python | PSF License |
| Chargeur d'amorçage PyInstaller | GPL v2+ (exception du chargeur d'amorçage) |

Copyright (C) 2026 NenkaLab
