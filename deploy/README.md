# Déployer le central V.34 (`v34-server`) sur un VPS

Le VPS héberge le **central téléphonique** : les PC (appli ou `v34-cli`) s'y
connectent avec un nom d'abonné et un mot de passe, reçoivent leur numéro, et
s'appellent ; le central relaie l'audio (G.711 sur UDP). Il héberge aussi des
**numéros spéciaux** desservis par des modems logiciels : le **FAI**
(`0836656565`), qui donne accès à Internet par modem, et une **ligne de test**
en écho (`0800000000`).

```text
 PC A (v34-cli / appli)                VPS dialup.tinyverse.fr                PC B
 modem V.34 ── audio G.711 ──UDP 3434──► central ──── relais audio ──────────► modem V.34
                                          │
                                          └─ 0836656565 : modem du FAI ── TCP ──► Internet
                                             (le PC A obtient un proxy SOCKS5 sur 127.0.0.1:1080 ;
                                              l'hôte virtuel http://web1998/ est servi par le VPS
                                              lui-même : le web allégé « comme en 1998 »)
```

Un seul port à ouvrir : **UDP 3434** (signalisation et audio). Rien à voir avec
Caddy (TCP 80/443, UDP 443) : les deux cohabitent sans configuration.

Machine visée : Debian 13 « trixie » x86_64 (marche aussi sur Debian 12 /
Ubuntu 22.04+), systemd.

---

## 1. Nom de domaine

Créer chez le registraire un enregistrement DNS **A** :

```text
dialup.tinyverse.fr.   3600   IN   A   <adresse IPv4 du VPS>
```

(Un AAAA est possible ; mettre alors `ecoute = "[::]:3434"` dans la config.)
Vérifier : `dig +short dialup.tinyverse.fr` doit renvoyer l'IP du VPS.
`dialup.tinyverse.fr:3434` est le serveur par défaut de `v34-cli`.

## 2. Compiler — sans sudo, sur le VPS

Rust s'installe dans le dossier personnel, sans droits administrateur :

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
. "$HOME/.cargo/env"
git clone <dépôt> ~/v34 && cd ~/v34        # ou rsync/scp depuis le Mac
cargo build --release -p v34-server
./target/release/v34-server --version
```

- Seul `v34-server` et ses dépendances sont compilés (pas l'appli Tauri ni
  ses bibliothèques graphiques). Compter quelques minutes sur 4 cœurs.
- Rust a besoin d'un éditeur de liens : si la compilation échoue avec
  « linker `cc` not found », c'est que `gcc` manque ; une seule commande sudo
  avant de recommencer : `sudo apt install -y gcc libc6-dev`. Le même `gcc`
  compile la bibliothèque cryptographique `ring` (TLS du navigateur web
  1998, en rustls : pas d'OpenSSL ni d'autre bibliothèque système).

**Alternative : compilation croisée depuis le Mac** (rien à installer sur le VPS) :

```sh
brew install zig && cargo install cargo-zigbuild
rustup target add x86_64-unknown-linux-gnu
cargo zigbuild --release -p v34-server --target x86_64-unknown-linux-gnu
scp target/x86_64-unknown-linux-gnu/release/v34-server vps:~/v34/target/release/
```

## 3. Installer — les seules étapes sudo

Tout ce qui demande sudo tient dans ce bloc (à copier-coller depuis `~/v34`) :

```sh
cd ~/v34
sudo useradd --system --user-group --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin v34
sudo install -m 755 target/release/v34-server /usr/local/bin/v34-server
sudo install -d -m 750 -o root -g v34 /etc/v34-server
sudo install -m 640 -o root -g v34 deploy/config.example.toml /etc/v34-server/config.toml   # PAS si elle existe déjà !
sudo install -m 644 deploy/v34-server.service /etc/systemd/system/v34-server.service
sudo systemctl daemon-reload
sudo systemctl enable --now v34-server
command -v ufw >/dev/null && sudo ufw allow 3434/udp comment v34-server
sudo nft list ruleset        # sans ufw : vérifier qu'aucune chaîne input « policy drop » ne bloque UDP 3434
sudo v34-server ajouter-abonne gaston 0142424242     # mot de passe demandé au clavier
```

Ou, de façon équivalente et relançable sans risque (ne remplace jamais une
config existante) :

```sh
sudo ./deploy/install.sh
sudo v34-server ajouter-abonne gaston 0142424242
```

Si `nft list ruleset` montre une chaîne `hook input` avec `policy drop`,
ajouter `udp dport 3434 accept` dans `/etc/nftables.conf` puis
`sudo systemctl reload nftables`. Penser aussi au pare-feu éventuel de
l'hébergeur (console web) : UDP 3434 entrant.

Conseillé : mettre l'adresse publique du VPS dans `fai.reseaux_interdits`
(`/etc/v34-server/config.toml`) pour que le tunnel FAI ne puisse pas viser
la machine elle-même, puis `sudo systemctl restart v34-server`.

## 4. Gérer les abonnés

```sh
sudo v34-server ajouter-abonne alice 0612345678   # ajoute, ou change numéro / mot de passe
sudo v34-server retirer-abonne alice
sudo v34-server liste-abonnes
sudo v34-server verifier                          # valide toute la configuration
```

- Le mot de passe est demandé au clavier sans écho (ou lu sur l'entrée
  standard dans un script : `echo "$MDP" | sudo v34-server ajouter-abonne …`).
- Le fichier ne contient que des **empreintes** (PBKDF2-HMAC-SHA256, 100 000
  itérations, sel par abonné, clé stockée façon SCRAM). Même volé, il ne
  permet pas de se connecter.
- Les commentaires du fichier sont conservés ; le résultat est validé avant
  d'être écrit, et le fichier garde ses droits (`root:v34`, 640).
- **Pas besoin de redémarrer** : le central relit l'annuaire dans les 2 s.
  Un abonné retiré ou renuméroté est déconnecté (il reçoit son nouveau numéro
  en se reconnectant). Les autres sections de la config demandent un
  `sudo systemctl restart v34-server`.

### Site d'inscription (facultatif, Docker)

Un site façon FAI de 1998 (`v34-portail`) permet aux visiteurs de créer
leur compte :
- ils choisissent leur numéro, puis valident leur adresse e-mail ;
- le compte est créé **sans accès Internet** ;
- l'administrateur reçoit un e-mail avec deux liens, [Accorder] et
  [Refuser], et dispose aussi d'une page `/admin` (connexion avec son mot de
  passe d'abonné) ;
- le site propose aussi le téléchargement de l'appli, d'après la dernière
  version publiée sur GitHub.

| Page | Contenu |
|---|---|
| `/` | accueil, rubrique « Quoi de neuf ? » (3 dernières versions publiées sur GitHub + ouverture du site) |
| `/inscription`, `/valider` | inscription, validation de l'adresse e-mail |
| `/compte` | **espace abonné** : connexion (nom d'abonné + mot de passe du logiciel, 5 essais par IP et par quart d'heure), numéro, état de l'accès Internet, adresse du central ; pour les comptes du site : changer de mot de passe, résilier (le numéro est libéré). Les abonnés de `config.toml` y sont en lecture seule (« gérés par l'opérateur ») |
| `/oubli`, `/reinitialiser` | **mot de passe oublié** : lien par e-mail valable 1 h, à usage unique, annulé par tout changement de mot de passe ; réponse identique que l'adresse soit abonnée ou non ; 3 demandes par IP et par heure |
| `/etat` | **état du réseau** d'après le fichier que le central réécrit chaque seconde (`etat.json` à côté des comptes, voir `[serveur] etat`) : en service depuis…, abonnés connectés, appels, modems du FAI, présents au salon ; « hors service » si le fichier a plus de 30 s |
| `/livre` | **livre d'or** : messages publiés après relecture dans `/admin` (Publier / Supprimer) ; 3 messages par IP et par heure, 500 messages au plus (`/var/lib/v34-server/livre-dor.toml`) |
| `/mentions`, `/conditions`, `/confidentialite` | pages légales (LCEN, CGU, RGPD), liées en bas de toutes les pages |
| `/telechargement`, `/aide`, `/webring` | logiciel, aide, membres du « Webring du Web rétro » (`/webring/hasard` : un membre au hasard) |
| `/admin` | administration : accès Internet, comptes, modération du livre d'or |

Le site tourne dans un conteneur, derrière Caddy :
- il écrit les comptes dans `/var/lib/v34-server/comptes.toml` (et le livre
  d'or à côté, en 0640) ;
- le central lit ce fichier (clé `comptes` de la section `[serveur]`) et le
  relit dès qu'il change ; il publie son état dans
  `/var/lib/v34-server/etat.json`, que le site lit ;
- les abonnés de `config.toml` gardent la priorité, et le site ne modifie
  jamais ce fichier.

```sh
# dépôt cloné dans /srv/dialup, central déjà installé (étapes 2 et 3)
cd /srv/dialup
sudo ./deploy/portail/preparer.sh dialup.example.org \
     --admin alice --admin-courriel moi@example.org \
     --smtp-depuis /chemin/vers/un/.env/qui/a/deja/les/SMTP_*
```

Le script (on peut le relancer sans risque) :
1. crée `/var/lib/v34-server` ;
2. ajoute la clé `comptes` à la configuration et redémarre le central ;
3. crée `.env` depuis [`.env.example`](../.env.example) ;
4. ajoute `dialup.example.org { reverse_proxy 127.0.0.1:8034 }` à Caddy
   (après `caddy validate`) ;
5. lance `docker compose up -d --build`.

Réglages du `.env` propres aux pages légales :

| Variable | Défaut | Rôle |
|---|---|---|
| `PORTAIL_EDITEUR` | « Un particulier — identité communiquée à l'hébergeur (LCEN, art. 6-III-2) » | éditeur du site (mentions légales, responsable du traitement) |
| `PORTAIL_CONTACT` | `PORTAIL_ADMIN_COURRIEL` | adresse de contact des pages légales |
| `PORTAIL_HEBERGEUR` | (vide : avertissement au démarrage) | nom, adresse et téléphone de l'hébergeur ; `\n` pour aller à la ligne. **Obligatoire** si l'éditeur reste anonyme |
| `V34_ETAT` | `/var/lib/v34-server/etat.json` | état publié par le central |
| `V34_LIVRE` | `/var/lib/v34-server/livre-dor.toml` | livre d'or |

Pour l'envoi des e-mails, il faut un serveur SMTP authentifié pour le
domaine de l'expéditeur (sinon : dossier spam). Avec `SMTP_HOST=journal`,
les e-mails sont écrits dans `docker compose logs portail` au lieu d'être
envoyés : pratique pour un essai. Tous les e-mails du site (validation,
mot de passe, notifications) partagent un plafond de 60 par heure.

Mise à jour du site : `git pull && docker compose up -d --build`. Mettez à
jour le central en même temps (section 5) : un central plus ancien ne
connaît ni le fichier d'état ni la section `[[reinitialisation]]` que le
site ajoute au fichier des comptes pendant une demande de mot de passe
oublié (il garderait alors son annuaire précédent jusqu'à la fin de la
demande).

## 5. Mettre à jour

```sh
cd ~/v34 && git pull && cargo build --release -p v34-server     # sans sudo
sudo install -m 755 target/release/v34-server /usr/local/bin/v34-server
sudo systemctl restart v34-server
```

(ou `sudo ./deploy/install.sh`, qui fait la même chose et garde la config).
Les clients se réenregistrent tout seuls après le redémarrage (keepalive).

## 6. Journaux

```sh
journalctl -u v34-server -f              # en direct
journalctl -u v34-server -p warning      # alertes : échecs d'authentification, IP bloquées…
journalctl -u v34-server --since today
```

Niveau réglable par `serveur.journal` (`error`, `warn`, `info`, `debug`) ;
en `debug`, les destinations ouvertes par le tunnel FAI sont journalisées.

## 7. Côté client

```sh
v34-cli register --user alice                      # attend les appels (mot de passe demandé)
v34-cli dial --user gaston 0612345678             # appelle alice
v34-cli dial --user gaston 0800000000             # ligne de test : écho des octets
v34-cli dial --user gaston 0836656565             # FAI : affiche « proxy SOCKS5 sur 127.0.0.1:1080 »
curl --socks5-hostname 127.0.0.1:1080 https://example.com/
```

Options : `--server HOTE:PORT` (défaut `dialup.tinyverse.fr:3434`),
`--password` (ou variable `V34_PASSWORD`, sinon saisie au clavier),
`--socks ADRESSE`, `--codec alaw|ulaw|l16`, `--preset NOM` (simulateur de
ligne et d'écho côté client). Le proxy accepte SOCKS5 (navigateurs : « SOCKS
v5 » + « DNS via le proxy ») et HTTP `CONNECT` (proxy HTTPS).

## 8. Navigateur web 1998

À 2-4 Ko/s, le web moderne est inutilisable (une page d'accueil de journal
pèse 1 à 3 Mo, scripts et images compris). En « mode 1998 », le navigateur
de l'appli passe par le proxy du tunnel et va sur l'**hôte virtuel
`http://web1998/`** : le VPS ne l'ouvre pas sur Internet, il le sert lui-même
(petit serveur HTTP/1.0 interne) et récupère pour l'abonné des versions
allégées des sites.

| Adresse | Contenu |
|---|---|
| `http://web1998/` | accueil « Tinyverse 1998 » : GIF animé « en construction », compteur de visites, FrogFind, franceinfo, CNN Lite, Wikipédia, Hacker News, champ « Aller à » |
| `http://web1998/p?u=URL` | la page `URL` simplifiée en HTML 3.2, avec un bandeau « page allégée de X Ko à Y Ko — [voir l'original] » |
| `http://web1998/i?u=URL` | l'image `URL` en GIF entrelacé (≤ 320 px, ≤ 256 couleurs, ~15 Ko visés) |
| `http://web1998/f?u=ACTION&…` | envoi d'un formulaire GET (moteurs de recherche) d'une page simplifiée |

Essai depuis un PC connecté au FAI :

```sh
curl --socks5-hostname 127.0.0.1:1080 http://web1998/
curl --socks5-hostname 127.0.0.1:1080 'http://web1998/p?u=https://fr.wikipedia.org/wiki/Modem'
```

- **Simplification** : titres, paragraphes, listes, tableaux, gras/italique,
  liens et images gardés ; scripts, styles, `iframe`, vidéos, `svg`,
  éléments cachés, publicités et bandeaux évidents (cookies, partage,
  newsletter…) supprimés, ainsi que tous les attributs inutiles. Quand la
  page a un `<main>`, seul le contenu principal est rendu. Texte converti en
  UTF-8 (pages ISO-8859-1/Windows-1252 comprises). Liens réécrits vers
  `/p?u=…`, images vers `/i?u=…`. Le champ « Aller à » accepte une adresse,
  un nom de domaine ou des mots-clés (recherche FrogFind).
- **Images** : réduites à 320 px de large, 256 couleurs puis moins, puis
  plus petites jusqu'à tenir dans ~15 Ko ; GIF **entrelacé** : l'image se
  dessine d'abord grossièrement puis s'affine, comme en 1998.
- **Ordres de grandeur** : Wikipédia « Modem » 174 Ko → 34 Ko, page
  d'accueil du Monde 905 Ko → 81 Ko (sans compter les dizaines de Mo de
  scripts et d'images évités), Hacker News 33 Ko → 21 Ko, photo → 12 Ko.
- **Erreurs** en pages rétro : « Adresse interdite », « Délai dépassé »
  (15 s), « Serveur introuvable », « Fichier introuvable » (404), document
  trop lourd ou de type non géré, certificat https invalide (avec un lien
  pour réessayer en `http://`).
- **Cache** mémoire LRU (32 Mo, 10 min) partagé par tous les abonnés ; le
  navigateur garde aussi les images (en-têtes `Cache-Control`).
- **Réglages** : section `[web1998]` de la configuration (actif, tailles
  maximales, largeur des images, poids visé, délai, cache).
- **Pour l'appli** (FAI simulé en local) : rien à faire, l'hôte virtuel est
  compris dans `v34_server::tunnel::run` avec `TunnelPolicy::default()` (ou
  `from_config`/`from_configs`) ; API documentée dans `v34_server::web1998`.

## 9. Sécurité

- **Pas de relais ouvert** : seul l'audio d'un abonné authentifié, venant de
  l'adresse de ses derniers messages authentifiés, est relayé, et seulement
  vers l'autre partie de SON appel. Le reste est ignoré sans réponse.
- **Mots de passe** : jamais transmis (défi-réponse façon SCRAM, nonce lié à
  l'adresse du client et valable 30 s) ; refus identique pour un nom inconnu
  et un mauvais mot de passe ; au-delà de 5 échecs par minute depuis une IP,
  « trop de tentatives » (`central.echecs_max`, `echecs_fenetre_s`). Choisir
  des mots de passe longs.
- **Pas d'amplification** : les seules réponses à un inconnu (CHALLENGE,
  AUTH_FAIL) sont plus petites que sa requête.
- **Tunnel FAI** : ports 80/443 seulement par défaut ; adresses privées,
  locales et réservées refusées APRÈS résolution DNS (127/8, 10/8,
  172.16/12, 192.168/16, 169.254/16 dont les métadonnées cloud, 100.64/10,
  ::1, fc00::/7, fe80::/10, IPv4 mappées…) ; `fai.reseaux_interdits` en plus.
- **Navigateur web 1998** : l'hôte virtuel est interne (paire de sockets sur
  la boucle locale, aucun port en écoute) ; ses récupérations sont filtrées
  comme le tunnel (http/https, ports 80/443, adresse vérifiée après
  résolution DNS et connexion à cette adresse, chaque redirection
  revérifiée, ≤ 5 redirections, 15 s, 3 Mo par page, 5 Mo par image), et
  `fai.autoriser_prive` ne s'y applique PAS : le réseau local du VPS reste
  fermé même en test. Les adresses visitées ne sont journalisées qu'en
  `debug`.
- **Ce qui n'est PAS chiffré** : l'audio (comme une vraie ligne
  téléphonique). Le trafic du tunnel FAI l'est quand l'application l'est
  (HTTPS via CONNECT/SOCKS : TLS de bout en bout jusqu'au site).
- **Service** : utilisateur `v34` sans privilèges ni shell, `NoNewPrivileges`,
  `ProtectSystem=strict`, `ProtectHome`, filtre d'appels système, aucune
  capacité ; config lisible seulement par root et le groupe `v34`.

## 10. Protocoles (résumé)

- **Audio** : paquets `V3` existants de `v34-net` (en-tête 16 o, 20 ms de
  G.711 A/µ ou L16) ; le champ `call_id` est l'identifiant d'appel attribué
  par le central. Le central relaie sans décoder (sauf simulateur de ligne).
- **Signalisation** (`v34_net::signal`, même port) : en-tête `VC`, version,
  type, session, n° de message ; REGISTER(nom) → CHALLENGE(nonce, sel,
  itérations) → AUTH(preuve) → WELCOME(numéro attribué, numéros spéciaux) ou
  AUTH_FAIL ; puis messages étiquetés HMAC-SHA256 (clé de session) :
  KEEPALIVE, DIAL, RINGING, RING, ANSWER, HANGUP, BUSY, UNKNOWN_NUMBER,
  UNREGISTER, ACK. Retransmission jusqu'à acquittement (0,3 s → 2 s, 8 essais).
- **Tunnel FAI** (`v34_net::tunnel`, dans les octets du modem) : bannière
  texte, marqueur `TUNNEL`, puis trames `A5 | type | seq | flux | long. |
  charge | CRC-16` : HELLO, OPEN(hôte, port), OPEN_OK, OPEN_FAIL, DATA, CLOSE.
  Pas de « login: » : l'abonné est déjà authentifié par le central, et son
  mot de passe ne doit pas passer en clair dans le signal du modem.

## 11. Limites connues

- Pas de chiffrement de l'audio ; un attaquant sur le chemin réseau peut
  écouter le modem (et injecter de l'audio en usurpant l'adresse d'un abonné).
- Tunnel : TCP uniquement (pas d'UDP, donc pas de DNS direct : utiliser
  `socks5h`/« DNS via le proxy »), pas de demi-fermeture TCP ; une erreur de
  transmission détectée coupe toutes les connexions en cours.
- Le simulateur de ligne du central (`central.simulateur_ligne`) n'applique
  pas l'écho local : il reste côté client (`--preset`).
- Arrêt du service : les appels en cours sont coupés sans HANGUP propre ; les
  clients s'en aperçoivent au keepalive suivant et se réenregistrent.
- Navigateur web 1998 : pas de JavaScript (sites qui en dépendent vides),
  pas de formulaires POST (connexion, commentaires), pas de cookies (pas de
  sessions), pas de SVG ; les pages déjà légères (CNN Lite, FrogFind)
  peuvent grossir un peu (préfixe `/p?u=` sur chaque lien) ; pas de
  découpage des très longues pages ; GIF animés gardés seulement s'ils sont
  déjà petits ; compteur de visites et cache remis à zéro au redémarrage.
