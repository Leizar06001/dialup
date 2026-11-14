# V34 Modem — l'appli rétro

Interface graphique du modem logiciel V.8 / V.34 / V.90 (56K), en **Tauri v2** : backend
Rust (moteur temps réel, audio), interface en HTML/CSS/JS statique, sans
npm ni bundler. Elle fonctionne sous **Windows, Linux et macOS**.

```
app/
├── src-tauri/          backend Rust (package `v34-app`)
│   ├── src/main.rs     commandes Tauri
│   ├── src/engine.rs   thread temps réel 8 kHz, console AT, HP, état → IHM
│   ├── src/link.rs     trait `Link` : `LocalLoop` (2 modems + 2 lignes simulées), `NetLink` (central)
│   ├── src/bbs.rs      protocole du chat, des fichiers et du visiophone dans le flux du modem
│   ├── src/browser.rs  fenêtre du navigateur (web 1998 / moderne / messagerie)
│   ├── src/visio.rs    visiophone V.Hallucination (webcam → ~40 formes, ~250 octets l'image)
│   ├── src/audio.rs    haut-parleur (cpal), rééchantillonnage 8 kHz → carte son
│   ├── src/tones.rs    tonalité, DTMF, retour d'appel, occupation, numéro non attribué
│   ├── src/fft.rs      FFT pour l'analyseur de spectre
│   ├── capabilities/   permissions Tauri v2
│   └── icons/          générées par tools/gen_icons.py
├── ui/                 interface (index.html, style.css, app.js, aide.js, commentateur.js,
│                       navigateur.html, messagerie.html…, police VT323)
└── tools/gen_icons.py  régénère les icônes (Python, sans dépendance)
```

## Lancer

```sh
cargo run -p v34-app            # depuis la racine du dépôt
cargo build -p v34-app --release
```

L'interface est embarquée à la compilation : après une modification dans
`app/ui/`, relance simplement `cargo run -p v34-app`.

### Dépendances système

| Système | À installer |
|---|---|
| **macOS** | Xcode Command Line Tools (`xcode-select --install`) |
| **Windows 10/11** | Visual Studio Build Tools (C++), runtime **WebView2** (déjà présent sur Windows 11 et les Windows 10 à jour) |
| **Debian / Ubuntu** | `sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev libasound2-dev` |
| **Fedora** | `sudo dnf install webkit2gtk4.1-devel openssl-devel libxdo-devel libappindicator-gtk3-devel librsvg2-devel alsa-lib-devel` |
| **Arch** | `sudo pacman -S webkit2gtk-4.1 base-devel openssl xdotool libappindicator-gtk3 librsvg alsa-lib` |

Sans carte son, l'appli fonctionne quand même : le badge « HP » passe à ✗.

## Ce qu'on voit

- **MODEM** : LEDs façon modem externe (HS AA CD OH RD SD TR MR), frise des
  phases V.8 ▸ PH2 ▸ PH3 ▸ PH4 ▸ DATA, afficheur des paramètres négociés,
  signaux TX/RX courants, spectre ou oscilloscope du son de la ligne,
  constellation, haut-parleur (OFF / AUTO / ON + volume). Le bouton
  **CONFIG** montre la face arrière : interrupteurs DIP 1 « 56K V.90 » (ON
  d'usine : 56K avec le FAI, jusqu'à 56 000 bit/s en réception, ~40 000 sur
  la ligne Urbaine ; l'envoi reste en V.34), 2 « RÉP. AUTO » (décroche après
  deux sonneries) et 3 « 56K EXPRESS » (OFF d'usine : la poignée de main
  56K et la fenêtre « Connexion à… » de Windows 98 gardent leurs lenteurs
  d'époque, une vingtaine de secondes), et molette DÉBIT MAX (2 400 à
  56 000 bit/s), mémorisés et pris en compte à l'appel suivant (`ATI4` les
  affiche). Le bouton **COMMENTAIRES** fait commenter la négociation par le
  modem, avec une piste karaoké de ce qui passe sur la ligne.
- **LIGNE TÉLÉPHONIQUE** : préréglages (Parfaite, Urbaine, Campagne, Longue
  distance, Orage) et curseurs pour chaque défaut de ligne, réglables pendant
  la connexion, courbe de réponse en fréquence (en orange, la réponse mesurée
  par le modem pendant le sondage L1/L2).
- **TERMINAL** : console AT phosphore vert, champ numéro, APPELER,
  RACCROCHER, RETRAIN, RENÉGO. En mode réseau : tonalités d'occupation et de
  numéro non attribué, comme sur le vrai réseau.
- **NAVIGATEUR** et **MESSAGERIE** : deux boutons-poussoirs de façade, entre
  le terminal et l'annuaire, qui ouvrent la fenêtre du navigateur. Modes
  WEB 1998 et MODERNE (tout passe par le modem, barre de chargement), et
  MESSAGERIE : chat (ou salon du FAI), envoi de fichiers (8 Mo au plus) et
  visiophone V.Hallucination entre abonnés. En boucle locale, le modem
  distant renvoie ce qu'il reçoit (écho).
- **ANNUAIRE** : les abonnés du central (en ligne, occupé, absent), avec
  TINYVERSE, le FAI (`0836154286`), épinglé en tête ; onglet CHAT ↗ vers la
  messagerie.
- **MONITEUR DE LIAISON** : réseau (ping, gigue, pertes), modem, LAPM,
  tunnel, débit utile.
- **AIDE** (en haut à droite, ou F1) : la notice complète, avec recherche.

## Commandes AT

| Commande | Effet |
|---|---|
| `ATDT0142424242`, `ATD192.168.1.20` | numérote (tonalité + DTMF ; un `.` est composé comme `*`, `,` = pause 2 s) puis lance la négociation |
| `ATA` | décroche un appel entrant (mode réseau, réponse auto coupée) ; en boucle locale, simule un appel entrant et décroche |
| `ATH` | raccroche (raccrochage V.34 propre si on est en données) |
| `ATM0` / `ATM1` / `ATM2` | HP coupé / jusqu'à la porteuse / toujours |
| `ATL0` … `ATL3` | volume du HP |
| `ATZ`, `AT&F` | réinitialise |
| `ATI0`, `ATI1`, `ATI3`, `ATI4` | informations (`ATI4` : débits V.34 et état du 56K) |
| `ATO` | retour en ligne |

Plusieurs commandes peuvent s'enchaîner : `ATM2L3DT0123`.

## Aperçu sans compiler

Ouvre `app/ui/index.html` dans un navigateur : la page passe en **mode
démo**, avec un faux backend qui simule une négociation. Pour un aperçu
directement « en ligne », utilise `index.html#t=15`.

## Architecture du temps réel

Le thread `v34-engine` avance par blocs de 20 ms (160 échantillons à
8 kHz), découpés en sous-blocs de 5 ms. Le délai ajouté par la boucle reste
donc de 5 ms par sens. À chaque sous-bloc, `Link::process` fait tourner les
modems :

```
modem local ──tx──► LineSim (local→distant) ──► modem distant
     ▲  └─ EchoPath (écho local)                     │
     └──────── LineSim (distant→local) ◄──tx─────────┘
```

Le haut-parleur joue ce que le modem local entend (tx + rx), plus les sons
de numérotation. L'état part vers l'interface à ~25 Hz (événement
`v34:state`), avec en plus `v34:event` (événements des modems),
`v34:console` (réponses AT) et `v34:chat`.

Le mode réseau est une autre implémentation de `Link`, `NetLink` : un seul
modem local, le son part en UDP vers le central via `v34-net` (chaque
paquet répète les deux trames précédentes, contre les pertes). Le reste de
l'appli ne change pas.
