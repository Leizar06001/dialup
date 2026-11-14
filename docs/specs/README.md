# Recommandations UIT-T de référence

Le modem suit trois recommandations de l'UIT-T (Union internationale des
télécommunications, secteur de la normalisation). Leurs PDF ne sont **pas**
dans le dépôt : leur redistribution n'est pas clairement permise. Elles se
téléchargent gratuitement sur le site officiel de l'UIT :

| Fichier attendu ici | Recommandation | Page officielle |
|---|---|---|
| `ITU-T_V.34_1998-02.pdf` | **V.34** (02/98) — modem jusqu'à 33 600 bit/s sur le réseau téléphonique commuté | <https://www.itu.int/rec/T-REC-V.34-199802-I/en> |
| `ITU-T_V.8_2000-11.pdf` | **V.8** (11/2000) — procédures de début de session (négociation V.8) | <https://www.itu.int/rec/T-REC-V.8-200011-I/en> |
| `ITU-T_V.90_1998-09.pdf` | **V.90** (09/98) — paire modem numérique / modem analogique, jusqu'à 56 000 bit/s vers l'abonné | <https://www.itu.int/rec/T-REC-V.90-199809-I/en> |

Sur chaque page, télécharger le PDF de la recommandation et l'enregistrer
dans ce dossier sous le nom indiqué : les commentaires du code y renvoient
(par exemple `crates/v34-core/src/v8.rs` : « docs/specs/ITU-T_V.8_2000-11.pdf,
clauses 5 à 8 »). Toutes les éditions sont listées sur
<https://www.itu.int/rec/T-REC-V.34> et <https://www.itu.int/rec/T-REC-V.8> ;
le code suit les éditions ci-dessus.

## Version texte (facultatif)

Pour chercher dans les recommandations avec `grep`, on peut en extraire le
texte à côté des PDF (`ITU-T_V.34_1998-02.txt`, `ITU-T_V.8_2000-11.txt`), par
exemple avec `pdftotext` (paquet poppler) :

```sh
pdftotext -layout docs/specs/ITU-T_V.34_1998-02.pdf docs/specs/ITU-T_V.34_1998-02.txt
pdftotext -layout docs/specs/ITU-T_V.8_2000-11.pdf  docs/specs/ITU-T_V.8_2000-11.txt
pdftotext -layout docs/specs/ITU-T_V.90_1998-09.pdf docs/specs/ITU-T_V.90_1998-09.txt
```

Les `.pdf` et `.txt` de ce dossier sont ignorés par git (voir `.gitignore`).
