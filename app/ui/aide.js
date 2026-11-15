/* V.34 Tinyverse — AIDE : la notice d'utilisation du TV-5600.
 * Bouton « ? AIDE » de la barre du haut, F1, ou « ? » hors d'un champ de
 * saisie. Le contenu est une simple liste de rubriques ci-dessous : à tenir
 * à jour quand on ajoute une touche, une commande AT ou une page au site.
 *
 * Format d'une entrée : [touches, texte]
 *  - touches : tableau de « touches » (rendues en capuchons de clavier) ;
 *    « … », « / » et « + » restent du texte entre deux touches ; null = note.
 *  - texte : `code` → capuchon, **gras** → gras.
 * Fichier autonome (IIFE) : aucun nom ne fuit dans app.js. */
(() => {
  "use strict";

  const AIDE = [
    {
      id: "demarrer", titre: "Démarrer", court: "DÉMARRER",
      items: [
        [["⚠ BOUCLE LOCALE"], "Badge rayé orange, en haut : tout est simulé sur ce PC. Un second modem répond à n'importe quel numéro et te renvoie tes messages en écho (au 08 36 67 22 53, il joue l'oracle)."],
        [["☎ RÉSEAU · N° …"], "Badge vert : ton modem est branché au central, avec ton propre numéro. Les autres abonnés peuvent t'appeler."],
        [["SERVEUR", "ABONNÉ", "MOT DE PASSE"], "Clique le badge pour ouvrir **CONNEXION AU CENTRAL** : l'adresse du central, par défaut `dialup.tinyverse.fr:3434`, ton nom d'abonné (attention aux majuscules) et ton mot de passe."],
        [["SE CONNECTER"], "Branche le modem au central ; la fenêtre se referme dès que ton numéro s'affiche."],
        [["mémoriser"], "Garde le mot de passe sur ce PC : au lancement suivant, l'appli se connecte toute seule. Sinon, la fenêtre de connexion s'ouvre au démarrage."],
        [["BOUCLE LOCALE"], "Le bouton de cette fenêtre : quitte le central et revient à la simulation (l'appel en cours est raccroché)."],
        [["HP ✓", "/", "HP ✗"], "Sortie audio trouvée ou non (survole le badge pour savoir laquelle). Sans carte son, tout marche… en silence."],
        [null, "Pas encore de numéro ? Inscris-toi sur **dialup.tinyverse.fr** (voir « Le site » plus bas)."],
      ],
    },
    {
      id: "appeler", titre: "Appeler et raccrocher", court: "APPELER",
      items: [
        [["APPELER", "/", "Entrée"], "Compose le numéro du champ du terminal (ATDT…) : tonalité, numérotation, sonnerie, puis la négociation qui chante."],
        [["RACCROCHER"], "Raccroche (ATH), proprement si la liaison est établie (cleardown V.34)."],
        [["TINYVERSE", "☎ APPELER"], "En tête de l'ANNUAIRE, toujours là (même en boucle locale) : compose le **08 36 15 42 86**, le fournisseur d'accès décroche et la ligne transporte Internet."],
        [["08 00 00 00 00"], "Ligne de test du central : elle te renvoie tout ce que tu lui envoies."],
        [["08 36 67 22 53"], "**3615 ORACLE** : une intelligence artificielle persuadée qu'on est en 1998. On lui parle dans la MESSAGERIE (rubrique ORACLE)."],
        [["☎ APPELER"], "Dans l'ANNUAIRE, à côté d'un abonné en ligne ou d'un numéro spécial : le compose pour toi (les contacts de la MESSAGERIE aussi)."],
        [["RING"], "Appel entrant (mode réseau) : avec la réponse automatique (DIP 2 sur ON), ça sonne deux fois puis le modem décroche tout seul (LED AA) ; sinon, ATA ou DÉCROCHER."],
        [["RETRAIN"], "En ligne : réentraînement complet, le modem refait le sondage de la ligne et l'apprentissage de l'égaliseur."],
        [["RENÉGO"], "En ligne : renégocie seulement le débit, plus vite. Utile après avoir changé la ligne."],
        [["CONNECT 33600/V34"], "Liaison établie (débit reçu, puis émis) : place au chat, aux fichiers, au visiophone ou à Internet. Entre abonnés, la LED du bouton MESSAGERIE clignote : clique dessus pour discuter."],
        [["BUSY"], "Occupé : tu entends d'abord la tonalité d'occupation (440 Hz, une demi-seconde sur deux). Ton correspondant est déjà en ligne, ou, pour le FAI et l'oracle, ton accès Internet n'est pas encore activé."],
        [["NO ANSWER"], "Ce numéro n'est attribué à personne : les trois notes montantes (950, 1400, 1800 Hz) des numéros non attribués précèdent le message. Aussi quand personne ne décroche."],
        [["NO CARRIER"], "Après une tonalité d'occupation rapide : réseau encombré (tous les modems du FAI sont pris). Sinon : la liaison s'est coupée."],
        [["NO DIALTONE"], "Mode réseau, mais pas (encore) connecté au central."],
        [null, "En boucle locale, tous les numéros répondent, le FAI compris : il est alors simulé sur ce PC, et Internet passe quand même par le modem."],
      ],
    },
    {
      id: "at", titre: "Commandes AT (terminal)", court: "AT",
      intro: "À taper dans le terminal vert, en majuscules ou en minuscules. Le modem répond OK, ERROR ou un code résultat.",
      items: [
        [["ATDT0142424242"], "Numérote en fréquences vocales, puis négocie. `ATD` et `ATDP` marchent aussi."],
        [[",", "."], "Dans le numéro : `,` fait une pause de 2 s, `.` est composé comme `*` (ex. `ATD192.168.1.20`)."],
        [["ATA"], "Décroche quand un abonné t'appelle et que la réponse automatique (DIP 2) est coupée. En boucle locale : le modem simulé t'appelle, RING, et le tien décroche."],
        [["ATH"], "Raccroche."],
        [["AT*B0836202600-0836202699", "AT*B08362026XX"], "Balayage, façon « wardialer » des années 90 : compose chaque numéro de la plage (100 au plus) et note ceux qui décrochent (PORTEUSE !). Sur le central seulement. ATH l'arrête. Chaque porteuse est facturée comme un appel abouti…"],
        [["ATM0", "ATM1", "ATM2"], "Haut-parleur coupé / jusqu'à la connexion / toujours (comme OFF, AUTO, ON)."],
        [["ATL0", "…", "ATL3"], "Niveau du haut-parleur, du murmure au réveil des voisins (en plus du curseur VOL)."],
        [["ATZ"], "Raccroche et remet le haut-parleur par défaut (ATM1, ATL2)."],
        [["AT&F"], "Réglages d'usine du haut-parleur (ATM1, ATL2), sans raccrocher."],
        [["ATI0", "ATI1", "ATI3"], "Nom du modem, somme de contrôle (255, pour la forme) et version."],
        [["ATI4"], "Capacités : débits V.34 et état du 56K, d'après le panneau CONFIG."],
        [["ATO"], "Retour en ligne : CONNECT si la porteuse est là, sinon NO CARRIER."],
        [["ATS0=1", "ATS7?"], "Registres S : acceptés… et poliment ignorés."],
        [["ATE", "ATV", "ATQ", "ATX", "ATB", "ATN", "ATW"], "Acceptés (OK), sans effet : souvenirs d'un autre siècle."],
        [["+++"], "Répond OK. Inutile ici : le terminal reste en mode commande pendant l'appel, ATH raccroche directement."],
        [["ATM2L3DT0142424242"], "Plusieurs commandes sur une ligne ; la numérotation (D) en dernier."],
        [["↑", "↓"], "Rappellent les 50 dernières commandes."],
      ],
    },
    {
      id: "appel", titre: "Pendant l'appel : chat, fichiers, annuaire", court: "CHAT",
      intro: "L'annuaire est sous le modem ; le chat, les fichiers et le visiophone vivent dans la fenêtre du navigateur, mode MESSAGERIE.",
      items: [
        [["MESSAGERIE"], "Le bouton-poussoir du bas, entre le terminal et l'annuaire (et le mode du même nom dans la barre du navigateur) : ouvre la messagerie. Elle ne s'ouvre pas toute seule : quand un appel entre abonnés aboutit, sa LED verte clignote jusqu'à ce que tu l'ouvres, puis reste allumée tant que dure l'appel ; rouge qui clignote : un message t'attend, comme sur un répondeur."],
        [["Entrée"], "Dans la conversation ambre : envoie ton message, une fois CONNECT affiché. En boucle locale, l'écho te le renvoie. Les messages arrivés avant l'ouverture de la fenêtre y sont aussi."],
        [["Envoyer un fichier…"], "Envoie un fichier au correspondant (8 Mo max : c'est un modem, pas la fibre). Jauge, débit en Ko/s, acquittement à la fin."],
        [null, "Fichiers reçus : vérifiés (CRC) puis rangés dans **Téléchargements/V34 reçus**."],
        [null, "Les fichiers vont d'abonné à abonné : pas pendant une session avec le FAI."],
        [["ANNUAIRE"], "Le panneau sous le modem : les abonnés du central, en ligne (vert), occupé (orange) ou absent ; il se met à jour à chaque clic sur l'onglet."],
        [["SUCCÈS"], "L'onglet voisin : ta facture du mois et tes succès (★ obtenus, ☆ à gagner, ??? cachés), tenus par le central. Il clignote quand tu en gagnes un ; le commentateur l'annonce aussi. Le détail de la facture est dans Mon compte, sur le site."],
        [["CHAT ↗"], "L'autre onglet du panneau : ouvre la MESSAGERIE. Il clignote quand un message t'y attend."],
        [["Contacts"], "Dans la MESSAGERIE, le même annuaire, pour appeler sans changer de fenêtre."],
        [["FAX"], "Le troisième bouton-poussoir, sous NAVIGATEUR et MESSAGERIE : ouvre la fenêtre du navigateur sur la TÉLÉCOPIE. LED verte quand la ligne est libre (un fax ne part que hors appel), verte qui clignote pendant un fax, rouge qui clignote quand un fax est arrivé."],
        [["TÉLÉCOPIE"], "Le bouton de la barre du navigateur, à côté de MESSAGERIE (ou FAX sur la façade) : un vrai télécopieur du groupe 3. Écris, dessine ou importe une image, tape un numéro, Envoyer : le modem siffle son CNG, négocie en T.30 (V.21), descend de 9600 à 7200 ou 4800 bit/s si la ligne est mauvaise, et envoie la page en V.29. Le correspondant doit décrocher tout seul (DIP RÉP. AUTO) ; son modem reconnaît le fax et imprime la page sur papier thermique, ligne par ligne, puis l'enregistre en PNG dans V34 reçus. La LED FAX et le voyant du bouton TÉLÉCOPIE clignotent en rouge."],
        [null, "En bas de l'annuaire : les numéros spéciaux (FAI, ligne de test, 3615 ORACLE). En boucle locale, pas de central, donc pas d'annuaire : seuls TINYVERSE et l'oracle, simulés."],
        [null, "Il paraît que certains numéros ne figurent dans aucun annuaire. L'oracle en connaîtrait au moins un… Chaque numéro trouvé rapporte un succès (et parfois une belle facture)."],
        [["Appel :", "Raccrocher"], "En mode MESSAGERIE, la barre d'adresse du navigateur laisse place à l'état de l'appel, avec de quoi raccrocher."],
      ],
    },
    {
      id: "visio", titre: "Le visiophone V.Hallucination", court: "VISIO",
      intro: "Pendant un appel entre abonnés, chacun voit l'autre… à sa façon. En haut de la MESSAGERIE.",
      items: [
        [["OFF", "CAMÉRA", "MIRE"], "La source de ton image. OFF d'usine, à chaque lancement : rien n'est filmé tant que tu n'as pas choisi CAMÉRA."],
        [["MIRE"], "Monsieur Mire, présentateur de synthèse, pour essayer sans caméra. En boucle locale, l'écho te renvoie ses images : tu te vois en grand."],
        [null, "Chaque image est redessinée en une quarantaine de triangles et d'ellipses semi-transparents, comme les images « V.Hallucination » du web 1998 : **environ 250 octets** au lieu de 9 Ko, jusqu'à 3 images par seconde. Ressemblant comme un rêve ressemble à la veille. Sans intelligence artificielle."],
        [["MOI"], "Le médaillon : ton image telle que ton correspondant la reçoit (en miroir pour la caméra, comme dans une glace)."],
        [["↑", "↓"], "Images par seconde, octets par image et débit, dans chaque sens. ● REC : tes images partent."],
        [null, "Le chat et les fichiers passent avant : les images attendent que la ligne soit presque libre, prennent au plus la moitié du débit, et se mettent en pause pendant l'envoi d'un fichier."],
        [null, "Sur Mac, le système demande l'accès à la caméra la première fois. Refusé par erreur ? **Réglages Système › Confidentialité et sécurité › Caméra**, puis relance l'appli."],
        [null, "Ton correspondant doit avoir une appli assez récente ; sinon le visiophone reste poliment muet (« pas de visiophone en face ») et rien ne lui est envoyé."],
      ],
    },
    {
      id: "internet", titre: "Internet par le FAI", court: "INTERNET",
      items: [
        [["TINYVERSE"], "Dans l'ANNUAIRE, ☎ APPELER : le FAI. Après CONNECT, « Tunnel Internet établi » : la ligne transporte maintenant tes connexions."],
        [["NAVIGATEUR"], "Le bouton-poussoir du haut, entre le terminal et l'annuaire : ouvre Tinyverse Navigator. Sa LED verte clignote quand Internet est prêt, jusqu'à ce que tu l'ouvres (il ne s'ouvre jamais tout seul) ; tout son trafic passe par le modem."],
        [["WEB 1998", "MODERNE", "MESSAGERIE"], "Les trois modes du navigateur : les deux façons de voir le web, et la messagerie (chat, fichiers, visio), sans web."],
        [["Tinyblog"], "Le bouton de la barre du navigateur, ou l'adresse tinyblog : les blogs des abonnés, lisibles seulement par le modem. Écris des articles (une image, du texte mis en forme : [b]gras[/b], [couleur=rose]…[/couleur], [clignote]…[/clignote]), choisis ton thème, et lâche tes com's chez les autres. Une photo met une bonne minute à partir à 33 600 bit/s : c'était ça, 1998."],
        [["Salon du FAI"], "Pendant la session, la conversation de la MESSAGERIE parle à tous les abonnés connectés au FAI. `/qui` : qui est là ; `/aide` : l'aide du salon."],
        [["WEB 1998"], "Le FAI va chercher la page et te la sert allégée : HTML simple, images GIF en 256 couleurs. Ça tient dans 33,6k."],
        [["Images : V.Hallucination"], "Lien en haut des pages web 1998 : chaque image arrive en une cinquantaine de formes (≈ 2,5 Ko au lieu de 15), comme un souvenir flou. Garanti sans intelligence artificielle. « normales » pour revenir aux GIF."],
        [["MODERNE"], "Le vrai web, tel quel… à 56 kbit/s au mieux (40 en ligne urbaine). Prévois un café. Le mode choisi est mémorisé."],
        [["Adresse :", "Aller"], "Tape `fr.wikipedia.org/wiki/Modem` ou une adresse complète, puis Entrée."],
        [["◀ Précédente", "▶ Suivante", "⟳ Actualiser", "■ Arrêter"], "Comme en 1998, en plus lent."],
        [["⌂ Accueil"], "Web 1998 : le portail du FAI (Aller à…, recherche FrogFind, sites conseillés). Moderne : DuckDuckGo Lite."],
        [null, "**Chargement** : des météores filent sur le logo T, une jauge avance dans l'adresse et dans la barre d'état (avec le débit) ; cadenas doré si la page est en HTTPS."],
        [["Appeler le FAI"], "Le bouton du navigateur quand tu es hors ligne : compose le FAI sans repasser par le modem."],
        [null, "Ligne coupée ? Le navigateur affiche une page « hors ligne » et recharge la tienne quand le tunnel revient."],
        [["127.0.0.1:1080"], "Pendant la session, un proxy local SOCKS5 et HTTP : d'autres logiciels peuvent, eux aussi, passer par le modem (l'adresse exacte s'affiche dans le terminal)."],
        [null, "Sur Mac, le navigateur intégré demande **macOS 14 (Sonoma)** ou plus récent pour le web. La MESSAGERIE (chat, fichiers, visio, salon) marche partout."],
      ],
    },
    {
      id: "oracle", titre: "3615 ORACLE", court: "ORACLE",
      intro: "Une intelligence artificielle qu'on ne joint qu'au modem, et qui est persuadée qu'on est en 1998.",
      items: [
        [["3615 ORACLE", "☎ APPELER"], "En bas de l'ANNUAIRE, dans les numéros spéciaux (et toujours là en boucle locale) : compose le **08 36 67 22 53**, « ORACLE » sur le clavier du téléphone. Le modem du central décroche, en V.34 ou en 56K."],
        [["MESSAGERIE"], "À CONNECT, sa LED clignote : ouvre-la, l'écran d'accueil du service t'y attend : la bannière, le tarif (2,23 F la minute, jamais facturés) et les questions qu'il te reste aujourd'hui."],
        [["Entrée"], "Tape ta question dans la conversation ambre, puis Entrée (la touche ENVOI du Minitel). L'oracle consulte les astres (les points de suspension), puis répond en quelques phrases, à la vitesse d'un Minitel en 1200 bit/s. Le compteur en francs tourne après chaque réponse."],
        [null, "Il se souvient de vos derniers échanges jusqu'à ce que tu raccroches, et ne connaît que 1998 : le Minitel, le franc, Windows 98, la Coupe du monde. Au-delà, il ne voit que des brumes."],
        [["/aide", "/sommaire", "/fin"], "Le GUIDE, l'écran d'accueil, et raccrocher (RACCROCHER ou ATH marchent aussi). Sans question pendant cinq minutes, l'oracle raccroche de lui-même."],
        [null, "Réservé aux abonnés qui ont l'accès Internet, une trentaine de questions par jour. Si l'IA est injoignable, et toujours en **boucle locale**, l'oracle tire les cartes à la place, et le dit."],
        [null, "Tes questions partent chez OpenRouter et le fournisseur du modèle pour fabriquer la réponse ; le central ne les garde pas après l'appel. N'y mets rien de personnel."],
      ],
    },
    {
      id: "ligne", titre: "La ligne téléphonique simulée", court: "LIGNE",
      items: [
        [["PARFAITE", "URBAINE", "CAMPAGNE", "LONGUE DISTANCE", "ORAGE"], "Préréglages, du laboratoire au poteau de campagne un soir d'orage."],
        [["RAPPORT SIGNAL/BRUIT", "ATTÉNUATION"], "Bruit de fond et perte de niveau."],
        [["BANDE · BAS", "BANDE · HAUT", "PENTE DES AIGUS"], "Bande passante et affaiblissement des aigus."],
        [["ÉCHO LOCAL", "DÉLAI DE L'ÉCHO", "DÉLAI DE PROPAGATION"], "Écho de ta propre émission, et temps de trajet."],
        [["PARASITES (CLICS)", "COUPURES", "DÉCALAGE DE FRÉQUENCE"], "Les misères des vieilles lignes."],
        [null, "Tout se règle aussi pendant l'appel : regarde le modem encaisser, renégocier… ou raccrocher."],
        [["RÉPONSE EN FRÉQUENCE"], "Courbe verte : la ligne réglée. Points orange : ce que le modem a mesuré pendant le sondage (phase 2)."],
        [null, "En mode réseau, la ligne simulée s'applique au son reçu du central."],
        [["×0,1", "…", "×5"], "TEMPS (barre du haut) : ralentit ou accélère toute la simulation, le son suit comme un disque. ×¼ pour entendre chaque phase. Boucle locale seulement."],
      ],
    },
    {
      id: "modem", titre: "Le modem : voyants et instruments", court: "MODEM",
      items: [
        [["HS", "AA", "CD", "OH", "RD", "SD", "TR", "MR"], "Les LEDs de façade. Survole-les : chacune explique son rôle et donne son état en direct."],
        [["V.8", "PH2", "PH3", "PH4", "DATA"], "La frise suit la négociation ; survole une étape pour savoir ce qui s'y chante."],
        [["REPOS", "NUMÉROTATION", "EN LIGNE"], "La pastille d'état de l'appel (aussi SONNERIE, NÉGOCIATION, RETRAIN, ÉCHEC)."],
        [["DÉBIT", "SYMBOLES", "PORTEUSE", "SNR"], "L'afficheur cyan : paramètres négociés, signaux TX/RX en cours, durée. Survole une valeur pour le détail émission/réception."],
        [["TAXES"], "Le compteur de Tinyverse Télécom, en francs de 1998 (jamais facturés) : local 0,74 F les 3 premières minutes puis 0,28 F la minute (0,14 F en heures creuses, 22 h – 8 h et le week-end), 3615 ORACLE 2,23 F la minute, numéro vert gratuit. Seul l'appelant paie."],
        [["SPECTRE", "OSCILLO"], "Le son de la ligne en fréquences (crêtes en orange) ou en forme d'onde ; jauges de niveau TX/RX dessous."],
        [["CONSTELLATION"], "Les points reçus après l'égaliseur : plus ils sont nets, meilleure est la ligne."],
        [["NIVEAUX 56K"], "En 56K, pas de constellation : le central envoie directement des niveaux de la loi A, 8 000 par seconde. L'écran devient une échelle à six lignes, une par position de la trame (chacune a son jeu de niveaux) : les traits sont les niveaux possibles, les points ce qui arrive. Grappes fines = ligne propre ; si elles débordent jusqu'au trait voisin, les erreurs commencent."],
        [["OFF", "AUTO", "ON", "VOL"], "Haut-parleur : coupé, jusqu'à la connexion, toujours ; et son volume. La grille vibre quand il joue."],
        [["MONITEUR DE LIAISON"], "RÉSEAU (ping, gigue, tampon, pertes), MODEM (débits, SNR, recalages), LAPM (trames, retransmissions, rejets), TUNNEL (connexions du navigateur), et le débit utile sur 60 s."],
      ],
    },
    {
      id: "config", titre: "CONFIG : la face arrière", court: "CONFIG",
      items: [
        [["CONFIG"], "Retourne le modem : interrupteurs DIP et molette de débit. Réglages mémorisés sur ce PC et lus au décroché : ils valent pour l'appel suivant."],
        [["DIP 1 · 56K V.90"], "ON : le modem propose le 56K (V.90) au FAI ; OFF : V.34 seul. En 56K, la descente passe en PCM (jusqu'à 56 000 bit/s sur une ligne parfaite, ~40 000 sur une ligne urbaine) et la montée reste en V.34. Poignée de main plus longue, avec le fameux DIL. Les appels entre abonnés restent en V.34 : il faut un bout numérique, le FAI."],
        [null, "Le 56K tombe en V.34 tout seul si la ligne est trop bruitée ou si elle décale les fréquences (« Longue distance ») : il exige une liaison numérique jusqu'à ton central, comme en 1998."],
        [["DIP 2 · RÉP. AUTO"], "ON : le modem décroche tout seul après deux sonneries. OFF : ça sonne (RING) jusqu'à ce que tu décroches avec ATA ou le bouton DÉCROCHER, qui remplace APPELER."],
        [["DIP 3 · 56K EXPR."], "OFF (d'usine) : toutes les lenteurs de 1999 — long DIL écouté trois fois, temps de réflexion, SCR, puis la fenêtre « Connexion à… » de Windows (« Vérification du nom d'utilisateur et du mot de passe… ») : 20 à 25 s de la numérotation à Internet. ON : une dizaine de secondes."],
        [["DIP 4", "…", "8"], "Réservés usine, sous le ruban « NE PAS MODIFIER ». On a dit : ne pas modifier."],
        [["DÉBIT MAX"], "Plafond du débit, de 2 400 à 56 000 bit/s, pour imiter un modem plus ancien. Le V.34 s'arrête à 33 600 ; sous 28 000, le 56K se coupe."],
        [["▲", "▼", "molette"], "Tournent la molette ; au clavier : flèches (un cran), `Pg↑` `Pg↓` (quatre), `Début` `Fin` (les extrêmes). Clique un cran voisin pour y aller."],
        [null, "L'afficheur résume ce que fera le modem au prochain appel ; `ATI4` le redit dans le terminal."],
      ],
    },
    {
      id: "site", titre: "Le site dialup.tinyverse.fr", court: "SITE",
      intro: "Le fournisseur d'accès, version 1998 : https://dialup.tinyverse.fr",
      items: [
        [["Inscription"], "Crée ton abonnement : nom d'abonné, numéro à 10 chiffres que tu choisis, adresse électronique."],
        [["Activation"], "Un courrier t'apporte le lien d'activation, valable 48 heures. Un clic, et ton numéro est à toi."],
        [["Accès Internet"], "Accordé par l'opérateur, un courrier te prévient. En attendant, le 08 36 15 42 86 sonne occupé ; les abonnés et la ligne de test marchent déjà."],
        [["Mon compte"], "Ton numéro et l'état de ton accès Internet ; changer de mot de passe ou supprimer le compte."],
        [["Mot de passe oublié"], "Un lien par courrier pour en choisir un nouveau, valable une heure. Change-le aussi dans l'appli si tu l'avais mémorisé."],
        [["État du réseau"], "Le central en direct : abonnés connectés, appels en cours, modems et salon du FAI."],
        [["Logiciel"], "Télécharge l'appli (Mac, Windows, Linux), avec l'astuce pour passer les avertissements de sécurité."],
        [["Livre d'or"], "Laisse un mot ; publié après relecture par le webmestre."],
        [["Aide"], "Dépannage : le central utilise le port UDP 3434, que certains réseaux bloquent."],
      ],
    },
    {
      id: "clavier", titre: "Raccourcis clavier", court: "CLAVIER",
      items: [
        [["F1"], "Ouvre ou ferme cette aide. `?` aussi, hors d'un champ de saisie."],
        [["Échap"], "Ferme la fenêtre ouverte (aide, central, CONFIG)."],
        [["Entrée"], "Valide : SE CONNECTER dans la fenêtre du central, OK dans CONFIG ; compose depuis le champ numéro ; envoie la commande ou le message."],
        [["↑", "↓"], "Historique des commandes, dans le terminal."],
        [["Tab", "Espace"], "Dans CONFIG : passe d'un réglage à l'autre, bascule l'interrupteur."],
        [null, "Un clic à côté d'une fenêtre la ferme aussi."],
      ],
    },
  ];

  /* ---------------------------------------------------------------- rendu */
  const SEP = new Set(["…", "/", "+"]);
  const el = (tag, cls, text) => {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  };
  /** `code` → capuchon, **gras** → gras ; le reste en texte brut. */
  function rich(parent, text) {
    for (const part of text.split(/(`[^`]+`|\*\*[^*]+\*\*)/)) {
      if (!part) continue;
      if (part.startsWith("`")) parent.appendChild(el("kbd", null, part.slice(1, -1)));
      else if (part.startsWith("**")) parent.appendChild(el("b", null, part.slice(2, -2)));
      else parent.appendChild(document.createTextNode(part));
    }
  }
  /** Minuscules sans accents, pour la recherche. */
  const plain = (s) => s.normalize("NFD").replace(/\p{Mn}/gu, "").toLowerCase();

  const modal = el("div", "modal aide-modal");
  modal.id = "aide-modal";
  modal.hidden = true;
  const box = el("div", "modal-box panel aide-box");
  box.id = "aide-box";
  box.tabIndex = -1;
  box.setAttribute("role", "dialog");
  box.setAttribute("aria-modal", "true");
  box.setAttribute("aria-labelledby", "aide-title");
  for (const c of ["tl", "tr", "bl", "br"]) box.appendChild(el("div", "screw " + c));
  const title = el("h2", "panel-title", "AIDE · NOTICE D'UTILISATION");
  title.id = "aide-title";
  box.appendChild(title);

  const head = el("div", "aide-head");
  const help = el("p", "modal-help aide-intro");
  rich(help, "Tout ce que sait faire ton modem TV-5600. Les LEDs, les étapes et la face arrière ont aussi leur infobulle : survole-les.");
  const find = el("input", "lcd-input aide-find");
  find.type = "text";
  find.spellcheck = false;
  find.placeholder = "chercher… (ATM, fichier, 56K)";
  find.setAttribute("aria-label", "Chercher dans l'aide");
  const index = el("nav", "seg small aide-index");
  index.setAttribute("aria-label", "Rubriques");
  head.append(help, find, index);

  const body = el("div", "aide-body");
  const none = el("p", "aide-none");
  none.hidden = true;

  const secs = []; // { sec, chip, text, rows: [{ row, text }] }
  for (const s of AIDE) {
    const sec = el("section", "aide-sec");
    sec.id = "aide-" + s.id;
    sec.appendChild(el("h3", null, s.titre));
    if (s.intro) sec.appendChild(el("p", "aide-sub", s.intro));
    const list = el("div", "aide-list");
    const entry = { sec, chip: null, text: plain(s.titre + " " + s.court), rows: [] };
    for (const [keys, text] of s.items) {
      const row = el("div", "aide-row" + (keys ? "" : " note"));
      if (keys) {
        const k = el("div", "aide-k");
        for (const key of keys) k.appendChild(SEP.has(key) ? el("span", "aide-sep", key) : el("kbd", null, key));
        row.appendChild(k);
      }
      const d = el("div", "aide-d");
      rich(d, text);
      row.appendChild(d);
      list.appendChild(row);
      entry.rows.push({ row, text: plain((keys || []).join(" ") + " " + text) });
    }
    sec.appendChild(list);
    body.appendChild(sec);
    const chip = el("button", null, s.court);
    chip.type = "button";
    chip.title = s.titre;
    chip.addEventListener("click", () => body.scrollTo({ top: sec.offsetTop - 6, behavior: "smooth" }));
    index.appendChild(chip);
    entry.chip = chip;
    secs.push(entry);
  }
  body.appendChild(none);

  const actions = el("div", "modal-actions aide-actions");
  const hint = el("span", "aide-hint");
  rich(hint, "`F1` ouvre · `Échap` ferme");
  const ok = el("button", "key primary", "OK");
  ok.type = "button";
  actions.append(hint, ok);
  box.append(head, body, actions);
  modal.appendChild(box);
  document.body.appendChild(modal);

  /* ---------------------------------------------------------------- recherche */
  function filter() {
    const q = plain(find.value.trim());
    let any = false;
    for (const s of secs) {
      // Le titre de la rubrique correspond : on la montre en entier.
      const whole = !q || s.text.includes(q);
      let n = 0;
      for (const r of s.rows) {
        const show = whole || r.text.includes(q);
        if (r.row.hidden === show) r.row.hidden = !show;
        if (show) n++;
      }
      s.sec.hidden = n === 0;
      s.chip.hidden = n === 0;
      if (n) any = true;
    }
    none.hidden = any;
    if (!any) none.textContent = `Rien sur « ${find.value.trim()} ». Essaie ATDT, fichier, navigateur, 56K…`;
    body.scrollTop = 0;
    spy();
  }
  find.addEventListener("input", filter);

  /** Rubrique en cours de lecture : allumée dans l'index. */
  function spy() {
    const shown = secs.filter((s) => !s.sec.hidden);
    let cur = shown[0];
    for (const s of shown) if (s.sec.offsetTop - 12 <= body.scrollTop) cur = s;
    // Tout en bas, la dernière rubrique (même courte) s'allume.
    const scrolls = body.scrollHeight > body.clientHeight + 4;
    if (scrolls && body.scrollTop + body.clientHeight >= body.scrollHeight - 4) cur = shown[shown.length - 1];
    for (const s of secs) if (s.chip.classList.contains("on") !== (s === cur)) s.chip.classList.toggle("on", s === cur);
  }
  body.addEventListener("scroll", spy, { passive: true });

  /* ---------------------------------------------------------------- ouverture / fermeture */
  let before = null; // élément qui avait le focus avant l'ouverture
  function open() {
    if (!modal.hidden) return;
    before = document.activeElement;
    if (typeof window.tipHide === "function") window.tipHide();
    modal.hidden = false;
    spy();
    find.focus();
    find.select();
  }
  function close() {
    if (modal.hidden) return;
    modal.hidden = true;
    const back = before && before.isConnected && before !== document.body ? before : document.getElementById("crt-input");
    if (back) back.focus();
  }
  const btn = document.getElementById("btn-aide");
  if (btn) btn.addEventListener("click", () => (modal.hidden ? open() : close()));
  ok.addEventListener("click", close);
  modal.addEventListener("click", (e) => { if (e.target === modal) close(); });
  // Comme les autres fenêtres : Échap ferme ; Entrée aussi, sauf dans la recherche
  // et sur les rubriques (où elle sert à y aller).
  modal.addEventListener("keydown", (e) => {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); }
    else if (e.key === "Enter") {
      if (e.target === find) { e.preventDefault(); find.blur(); box.focus(); }
      else if (!index.contains(e.target)) { e.preventDefault(); close(); }
    }
  });
  // F1 partout ; « ? » quand on n'écrit pas dans un champ (terminal, chat…).
  document.addEventListener("keydown", (e) => {
    if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === "F1") { e.preventDefault(); modal.hidden ? open() : close(); return; }
    if (e.key === "?" && modal.hidden) {
      const t = e.target;
      const typing = t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName));
      if (!typing) { e.preventDefault(); open(); }
    }
  });

  // Pour les tests (V34_UIEVAL) et les autres scripts.
  window.aide = { open, close };
})();
