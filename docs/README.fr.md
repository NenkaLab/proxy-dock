# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · [日本語](README.ja.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [Deutsch](README.de.md) · **Français** · [Español](README.es.md)

Un « dock » de proxys pour le bureau : créez plusieurs proxys et activez ou désactivez chacun d'eux. Conçu pour un usage personnel.

Chaque proxy associe une **entrée** (comment il accepte les connexions sur un port de ce PC) et une **sortie**
(comment il atteint la destination). Par exemple, « réception sur un SOCKS5 local → tunnel chiffré vers un serveur
Shadowsocks distant » tient sur une seule carte.

<p align="center">
  <img src="images/proxies.png" width="49%" alt="Liste des proxys">
  <img src="images/editor.png" width="49%" alt="Éditeur de proxy">
</p>

## Fonctionnalités

| | Pris en charge |
|---|---|
| Entrée | SOCKS5 · SOCKS4/4a · HTTP · HTTPS (TLS) · serveur Shadowsocks · serveur Hysteria2 |
| Sortie | Direct · proxy amont HTTP / HTTPS / SOCKS4 / SOCKS5 · tunnel Shadowsocks · tunnel Hysteria2 (QUIC) |
| UDP | entrée SOCKS5 · Shadowsocks · Hysteria2, sortie Direct · Shadowsocks · Hysteria2 |

- **Shadowsocks** : AEAD 2018 (aes-128/256-gcm, chacha20-ietf-poly1305) et 2022 (SIP022, blake3), TCP+UDP, copie des liens `ss://`
- **Hysteria2** : interopérable dans les deux sens avec l'implémentation officielle (TCP et UDP), épinglage du certificat, copie des liens `hysteria2://`
- **Paramètres** : valeurs globales par défaut + paramètres locaux par proxy (le local prime) - domaines autorisés, blocage du réseau privé, délai d'inactivité, journal
- **Vérifier la connexion** dans l'éditeur : teste la sortie (amont/tunnel, mots de passe) et le port d'entrée avant d'enregistrer
- **Test de connexion** : se connecte en HTTPS à 21 sites de nombreux pays via un proxy en cours d'exécution et mesure les temps de réponse
- **Journal / statistiques** par proxy, graphique de débit en direct, fichier journal facultatif
- **Sécurité** : mots de passe stockés avec Windows DPAPI (fichiers 0600 sur les autres systèmes), limite de connexions par IP,
  blocage temporaire après des échecs d'authentification répétés, détection des boucles dans les chaînes de proxys, bouton de blocage du pare-feu Windows
- **Langues** : English (par défaut), 한국어, 日本語, 简体中文, 繁體中文, Deutsch, Français, Español (Paramètres → Affichage → Langue)
- Zone de notification, démarrage à l'ouverture de session, thème clair/sombre, polices intégrées (Pretendard / JetBrains Mono)

## Lancement

Utilisez une version des [Releases](https://github.com/NenkaLab/proxy-dock/releases) s'il y en a une, ou lancez depuis les sources :

```bash
python -m venv .venv
# Windows : .venv\Scripts\activate   /   macOS, Linux : source .venv/bin/activate
pip install -r requirements.txt
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

## Limites connues

- Hysteria2 tourne sur une implémentation QUIC en Python (aioquic) : comptez environ 100 Mbit/s. Les réglages de bande passante (Brutal),
  l'obfuscation Salamander et le saut de ports ne sont pas pris en charge.
- Le serveur Hysteria2 officiel refuse les connexions dont le SNI ne correspond pas à son certificat - saisissez le domaine du
  certificat du serveur dans le champ SNI de la sortie.
- L'entrée HTTP ne traite qu'une requête ordinaire (non CONNECT) par connexion.
- SOCKS4 n'a pas d'authentification par mot de passe : ne l'utilisez que sur ce PC.
- Le japonais et le chinois utilisent la police d'interface du système (le Pretendard intégré couvre le coréen et l'alphabet latin).

## Organisation

```
proxydock.py            point d'entrée
proxydock/
  common.py             constantes communes, journal, gestion des adresses, blocages
  config.py             format des paramètres (v2), validation, fusion global/local, stockage des mots de passe, migration
  runtime.py            ProxyInstance / ProxyManager (plusieurs proxys sur un seul thread asyncio)
  relay.py              cœur du relais TCP/UDP
  inbound.py            protocoles d'entrée
  outbound.py           protocoles de sortie
  ss.py                 chiffrement Shadowsocks 2018/2022
  hysteria2.py          serveur et client Hysteria2 (aioquic)
  tls.py                certificats auto-signés, contextes TLS
  probe.py              test de connexion / vérification de connexion
  platform_utils.py     démarrage à l'ouverture de session, pare-feu, vérification de port
  i18n.py, locales/     traductions (textes source anglais -> <langue>.json)
  ui/                   interface PySide6 + Fluent Widgets
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
| Pretendard, JetBrains Mono (polices intégrées) | SIL OFL 1.1 |
| PySide6-Fluent-Widgets | GPL v3 |
| Qt 6 / PySide6 / Shiboken6, PySideSix-Frameless-Window | LGPL v3 |
| aioquic, pylsqpack, darkdetect | BSD 3-Clause |
| cryptography | Apache 2.0 / BSD 3-Clause |
| pyOpenSSL | Apache 2.0 |
| service-identity, attrs | MIT |
| certifi | MPL 2.0 |
| Environnement d'exécution Python | PSF License |
| Chargeur d'amorçage PyInstaller | GPL v2+ (exception du chargeur d'amorçage) |

Copyright (C) 2026 NenkaLab
