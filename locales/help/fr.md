## Bases

- Les blocs d'**entrée** acceptent les connexions sur ce PC, par exemple un port SOCKS5.
- Les blocs de **sortie** sont la voie de sortie du trafic : Direct, un autre proxy ou un tunnel.
- Sur le **Canevas**, faites glisser la sortie d'un bloc vers l'entrée d'un autre pour les relier. Double-cliquez sur un bloc pour le modifier.
- Démarrez une entrée avec **Lancer** sur son bloc ou depuis le menu de la zone de notification.

## L'utiliser dans une appli

- Clic droit sur une entrée → **Copier l'adresse/le lien pour les clients**, puis saisissez-la comme proxy de l'appli.
- Tout le PC : clic droit sur une entrée HTTP → **Utiliser comme proxy du PC** (proxy système Windows tant qu'elle tourne).
- Téléphones et autres PC : copiez le lien d'une entrée serveur Shadowsocks, Hysteria2 ou WireGuard.

## Blocs intermédiaires

- **Router** : règles par domaine, IP, programme ou pays, chacune avec sa propre sortie.
- **IF** : une condition (alors / sinon), ou un interrupteur manuel comme un kill switch.
- **Groupe** : plusieurs sorties comme une seule : la plus rapide, la première qui marche ou celle choisie.
- **via** : la connexion d'une sortie passe par une autre sortie (chaîne).
- **Passe-plat** et **paire sans fil** : servent seulement à ranger les liens.

## Touches du canevas

| Touche | Action |
|---|---|
| Ctrl+Z / Ctrl+Y | Annuler / rétablir |
| F2 · Ctrl+D · Suppr | Renommer · dupliquer · supprimer |
| ← → | Bloc précédent / suivant le long des liens |
| ↑ ↓ | Bloc au-dessus / en dessous (après ← →, l'autre choix s'il y en a plusieurs) |

## Contournement du VPN (Windows)

Même avec un VPN actif sur tout le PC, un bloc peut sortir en dehors : réglez son **Interface réseau** sur **Hors du VPN**. Une petite machine virtuelle Hyper-V reliée directement à une carte réseau transporte ce trafic (**Réglages > Contournement du VPN** : carte réseau, mémoire, adresse ; **Créer** demande d'abord votre accord). Nécessite Windows Pro, Entreprise ou Éducation avec Hyper-V activé.

Ce trafic montre votre vraie adresse IP, et le kill switch et la protection DNS du VPN ne s'y appliquent pas ; avec un VPN d'entreprise ou d'école, cela peut enfreindre leurs règles. Vous êtes responsable des conséquences de son activation.

## Contournement du VPN (Windows)

Même avec un VPN actif sur tout le PC, un bloc peut sortir en dehors : réglez son **Interface réseau** sur **Hors du VPN**. Une petite machine virtuelle Hyper-V reliée directement à une carte réseau transporte ce trafic (**Réglages > Contournement du VPN** : carte réseau, mémoire, adresse ; **Créer** demande d'abord votre accord). Nécessite Windows Pro, Entreprise ou Éducation avec Hyper-V activé.

Ce trafic montre votre vraie adresse IP, et le kill switch et la protection DNS du VPN ne s'y appliquent pas ; avec un VPN d'entreprise ou d'école, cela peut enfreindre leurs règles. Vous êtes responsable des conséquences de son activation.

## Autres PC

Mettez l'adresse de l'entrée sur `0.0.0.0` et activez un mot de passe. Sous Windows, la règle du pare-feu est dans les **Paramètres**.

## Fermer

X masque seulement la fenêtre ; les proxys continuent dans la zone de notification. Quittez depuis son menu.
