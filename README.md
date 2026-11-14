# V.34 — le modem logiciel qu'on entend

Un modem **V.34 à 33 600 bit/s entièrement logiciel**, écrit en Rust, qui
négocie exactement comme un modem de 1998 : deux modems ne se parlent **que
par le son** (échantillons à 8 kHz), et on les entend. Tonalités V.8,
sondage de la ligne, apprentissage, puis le chuintement des données. Il
sait aussi faire le **56K (V.90)** face au modem numérique du FAI : jusqu'à
56 000 bit/s en réception, avec le bourdonnement du DIL.

Autour du modem, tout un petit monde « années 90 » :

- **Une appli rétro** pour Windows, macOS et Linux : LEDs de modem externe,
  console de commandes AT, haut-parleur, spectre et constellation en direct,
  un modem qui commente sa propre négociation (avec sa piste karaoké), une
  face arrière à interrupteurs DIP (56K, réponse automatique…), une notice
  intégrée (bouton AIDE, F1), et un simulateur de ligne téléphonique (bruit,
  écho, ligne de campagne, orage…) réglable pendant la connexion.
- **Un central téléphonique sur IP** : chaque abonné reçoit un numéro, les
  appels sont relayés en audio G.711 sur UDP, et les modems négocient à
  travers le réseau comme sur une vraie ligne. Le mot de passe ne circule
  jamais (défi-réponse).
- **Un fournisseur d'accès à Internet** : composer le `0836154286` décroche
  un modem côté serveur, qui ouvre un tunnel vers Internet (proxy SOCKS5
  local) à la vitesse d'un vrai modem. Raccordé en numérique au central, il
  propose le 56K (V.90) : ~40 000 bit/s en réception sur une ligne
  « Urbaine », jusqu'à 56 000 sur une ligne parfaite (l'envoi reste en V.34).
- **Le navigateur « web 1998 »** : à 3 Ko/s le web moderne est inutilisable,
  alors l'hôte virtuel `http://web1998/` sert des versions allégées des
  sites, en HTML 3.2 avec des images GIF de 256 couleurs qui se dessinent
  d'abord grossièrement, puis s'affinent, ou « V.Hallucination » :
  redessinées en une cinquantaine de formes (SVG de 2,5 Ko).
- **Une messagerie entre abonnés** : chat, transfert de fichiers et
  **visiophone V.Hallucination** (la webcam redessinée en une quarantaine de
  formes, ~250 octets l'image, 2 à 3 images par seconde).
- **Un salon de discussion** où se retrouvent tous les abonnés connectés au
  FAI, façon IRC ou 3615.

## Télécharger l'appli

Les installateurs de la dernière version sont sur la page
[Releases](https://github.com/Leizar06001/dialup/releases/latest) :

| Système | Fichier |
|---|---|
| macOS Apple Silicon (M1 et suivants) | `V34-Modem_X.Y.Z_macos_aarch64.dmg` |
| macOS Intel | `V34-Modem_X.Y.Z_macos_x64.dmg` |
| Windows 10/11 (64 bits) | `V34-Modem_X.Y.Z_windows_x64-setup.exe` (ou `.msi`) |
| Linux, toutes distributions | `V34-Modem_X.Y.Z_linux_amd64.AppImage` |
| Debian / Ubuntu | `V34-Modem_X.Y.Z_linux_amd64.deb` |

> **L'appli n'est pas signée** (pas de certificat de développeur Apple ou
> Microsoft). Le système va donc se méfier au premier lancement :
>
> - **macOS** : clic droit sur l'appli > **Ouvrir**, puis **Ouvrir** dans la
>   fenêtre d'avertissement. Depuis macOS 15, il faut passer par Réglages
>   système > Confidentialité et sécurité > **Ouvrir quand même**. Si macOS
>   affirme que l'appli est « endommagée », retirer l'attribut de
>   quarantaine dans le Terminal :
>   ```sh
>   xattr -dr com.apple.quarantine "/Applications/V34 Modem.app"
>   ```
> - **Windows** : SmartScreen > **Informations complémentaires** >
>   **Exécuter quand même**.
> - **Linux** : `chmod +x V34-Modem_*.AppImage` avant de lancer l'AppImage.

L'appli démarre en **boucle locale** (deux modems dans la même machine) :
ni compte ni réseau nécessaires. En mode réseau, elle propose par défaut le
central du projet, `dialup.tinyverse.fr:3434`, où il faut un compte
d'abonné.

## Organisation du dépôt

```
crates/v34-core     moteur V.8 / V.34 / V.90 : DSP, machine d'états, LAPM, simulateur de ligne
crates/v34-net      audio sur IP (G.711/UDP, gigue), signalisation du central, tunnel
crates/v34-server   central téléphonique, modem du FAI, web 1998, salon, site d'inscription (pour un VPS)
crates/v34-cli      ligne de commande : deux modems en local, WAV, appels via le central
app/                l'appli Tauri v2 (backend Rust dans src-tauri, interface statique dans ui)
deploy/             installation du central sur un VPS (systemd) et du site d'inscription (Docker)
docs/specs/         où mettre les recommandations UIT-T V.34, V.8 et V.90
```

## Compiler

Il faut [Rust](https://rustup.rs) stable et, pour l'appli, les bibliothèques
système de Tauri (WebView, audio) : la liste par système est dans
[`app/README.md`](app/README.md#dépendances-système). L'interface est en
HTML/CSS/JS statique : ni npm ni bundler.

```sh
cargo run -p v34-app                       # lancer l'appli
cargo test --release -p v34-core -p v34-net -p v34-server   # tests
cargo run --release -p v34-cli -- local --wav negociation.wav   # deux modems, sans interface
cargo build --release -p v34-server        # le central (voir deploy/)
```

`V34_MUTE=1` lance l'appli haut-parleur coupé.

### Fabriquer les installateurs

```sh
cargo install tauri-cli --version "^2" --locked
cd app/src-tauri
cargo tauri build          # → target/release/bundle/ (.app et .dmg sur macOS, etc.)
```

## Publier une version

Pousser une étiquette `vX.Y.Z` suffit :

```sh
git tag v0.2.0
git push origin v0.2.0
```

Le workflow [`release.yml`](.github/workflows/release.yml) compile alors
l'appli sur des machines macOS (Apple Silicon et Intel), Windows et Linux, et
publie une version GitHub avec les installateurs ci-dessus et leurs sommes
de contrôle. La version de l'appli est **celle de l'étiquette** : pas besoin
de modifier `tauri.conf.json` avant. Détails, noms de fichiers et
dépannage : [`docs/RELEASE.md`](docs/RELEASE.md).

À chaque push, [`ci.yml`](.github/workflows/ci.yml) lance les tests sous
Linux.

## Héberger son propre central

Le central (`v34-server`) tourne sur n'importe quel petit VPS Linux, avec
un seul port à ouvrir (UDP 3434). Installation, gestion des abonnés, FAI et
web 1998, site d'inscription (conteneur Docker) : voir
[`deploy/README.md`](deploy/README.md). Dans l'appli, il
suffit ensuite de remplacer l'adresse du serveur dans la fenêtre de
connexion.

## Spécifications

Le modem suit les recommandations UIT-T **V.34** (02/98), **V.8**
(11/2000) et **V.90** (09/98). Elles ne sont pas dans le dépôt : liens de
téléchargement dans [`docs/specs/README.md`](docs/specs/README.md).

## Licence

[MIT](LICENSE) © 2026 Leizar06001. La police VT323 est sous licence SIL
Open Font License ([`app/ui/fonts/OFL.txt`](app/ui/fonts/OFL.txt)).
