# Publier une version

## En bref

```sh
git tag v0.2.0
git push origin v0.2.0
```

Le workflow [`.github/workflows/release.yml`](../.github/workflows/release.yml)
(onglet **Actions > Publication**) :

1. vérifie l'étiquette : `vX.Y.Z`, chiffres seulement (l'installateur MSI
   de Windows refuse `v1.0.0-beta`) ;
2. compile l'appli sur quatre machines GitHub, en parallèle :

   | Machine | Cible | Paquets |
   |---|---|---|
   | `macos-latest` (ARM) | `aarch64-apple-darwin` | `.app`, `.dmg` |
   | `macos-latest` (ARM, compilation croisée) | `x86_64-apple-darwin` | `.app`, `.dmg` |
   | `windows-latest` | `x86_64-pc-windows-msvc` | NSIS `-setup.exe`, `.msi` |
   | `ubuntu-22.04` | `x86_64-unknown-linux-gnu` | `.AppImage`, `.deb` |

3. si **les quatre** ont réussi, publie la version GitHub `vX.Y.Z`, qui
   devient la « latest ».

La **version vient de l'étiquette** : elle est injectée au moment de la
compilation (`tauri build --config {"version": "X.Y.Z"}`), dans les noms de
fichiers comme dans l'appli. Mettre aussi à jour `version` dans
`app/src-tauri/tauri.conf.json` et `Cargo.toml` reste conseillé, pour que
les compilations locales affichent la même version, mais n'est pas
obligatoire.

Compter 10 à 20 minutes. Les dépendances du `Cargo.lock` sont imposées
(`--locked`) : penser à le committer avec tout changement de dépendance.

## Fichiers publiés

Noms stables (sans espace), avec `X.Y.Z` la version :

| Suffixe | Système |
|---|---|
| `_macos_aarch64.dmg` | macOS Apple Silicon |
| `_macos_x64.dmg` | macOS Intel |
| `_windows_x64-setup.exe` | Windows, installateur NSIS (conseillé) |
| `_windows_x64.msi` | Windows, installateur MSI |
| `_linux_amd64.AppImage` | Linux, toutes distributions |
| `_linux_amd64.deb` | Debian, Ubuntu |
| `SHA256SUMS.txt` | sommes SHA-256 des fichiers ci-dessus |

Exemple : `V34-Modem_0.2.0_macos_aarch64.dmg`.

### Pour le site web

La dernière version publiée (ni brouillon, ni pré-version) :

```
GET https://api.github.com/repos/Leizar06001/dialup/releases/latest
```

Dans la réponse, `tag_name` donne la version (`v0.2.0`) et chaque élément
de `assets[]` a un `name` et un `browser_download_url`. Il suffit de choisir
l'asset dont le `name` se termine par le suffixe voulu :

```sh
curl -s https://api.github.com/repos/Leizar06001/dialup/releases/latest \
  | jq -r '.assets[] | select(.name | endswith("_macos_aarch64.dmg")) | .browser_download_url'
```

L'API sans jeton est limitée à 60 requêtes par heure et par adresse IP :
appelée depuis le navigateur du visiteur, ça passe ; appelée depuis un
serveur, mettre la réponse en cache quelques minutes.

## Lancement manuel

**Actions > Publication > Run workflow** :

- sur une **branche** (ex. `main`) : compile seulement, sans rien publier.
  Les installateurs se téléchargent en bas de la page de l'exécution
  (« Artifacts », gardés 14 jours). Pratique pour tester avant d'étiqueter.
- sur une **étiquette** `vX.Y.Z` : recompile et republie cette version ; si
  elle existe déjà, ses fichiers sont remplacés.

## Si ça échoue

- **Une plateforme a échoué** : rien n'est publié. Une fois la cause
  corrigée sur `main`, le plus simple est de publier `vX.Y.(Z+1)`. Pour une
  panne passagère de GitHub, **Re-run failed jobs** suffit : la publication
  repart ensuite.
- **« the lock file needs to be updated »** : `Cargo.lock` n'a pas été
  committé après un changement de dépendance.
- **Étiquette refusée** : elle doit ressembler à `v0.2.0`.

## Réglages GitHub

- Le dépôt doit être **public** : sinon l'API `releases/latest` et les
  téléchargements demandent une authentification, et le site ne peut pas
  les lire. (Les minutes d'Actions sont aussi gratuites pour un dépôt public ;
  sur un dépôt privé, une minute macOS compte pour dix.)
- **Settings > Actions > General** :
  - *Actions permissions* : « Allow all actions », ou au minimum les actions
    de GitHub plus `Swatinem/rust-cache` (utilisée par la CI) ;
  - *Workflow permissions* : rien à changer. Le workflow demande lui-même
    `contents: write` pour le seul job de publication ; les jobs de
    compilation n'ont qu'un accès en lecture.
- Facultatif : une règle (Settings > Rules > Rulesets, cible « tags »,
  motif `v*`) pour que seul le propriétaire puisse créer les étiquettes de
  version.
- Si l'option **Release immutability** est activée, une version publiée ne
  peut plus être modifiée : republier sur une étiquette existante échouera,
  il faudra une nouvelle étiquette.

## Pas de signature de code

Il n'y a pas de certificat : sous macOS l'appli est seulement signée
« ad hoc » (`bundle.macOS.signingIdentity = "-"` dans `tauri.conf.json`),
ce qui ne demande aucun certificat et évite le message « appli endommagée »
sur les Mac Apple Silicon, mais Gatekeeper avertit quand même au premier
lancement. Sous Windows, SmartScreen avertit aussi. Les contournements pour
les utilisateurs sont dans le [README](../README.md#télécharger-lappli).

Pour signer plus tard : certificat Apple Developer ID + notarisation
(variables `APPLE_CERTIFICATE`, `APPLE_ID`… lues par `tauri build`), et
certificat Authenticode pour Windows.
