/* V.34 Tinyverse — le modem commentateur.
 *
 * Pendant un appel, le modem commente sa propre négociation comme un match
 * retransmis à la télé, et une piste karaoké affiche les « paroles » de ce
 * qui passe sur la ligne (Biiiii, di-di-di, Krrrkrrr, Pschhhh…) pour que
 * l'on puisse chanter avec lui.
 *
 * Tout est local : des banques de phrases par situation, tirées au hasard
 * (jamais deux fois la même d'affilée), dans lesquelles on glisse les valeurs
 * mesurées. Pas d'IA, pas de réseau, pas de facture.
 *
 * Commentateur.creer() et Commentateur.paroles() ne touchent pas au DOM
 * (testables sous Node) ; Commentateur.monter() branche l'affichage. */
"use strict";

const Commentateur = (() => {
  /* ------------------------------------------------------------ réglages */
  const ECART_MS = 1200;            // au plus une ligne toutes les 1,2 s…
  const FILE_MAX = 3;               // … avec une petite file d'attente
  const VIE_MS = [2500, 5000, 15000]; // une ligne périmée n'est plus dite (par priorité)
  const MEUBLE_MS = 6500;           // silence d'antenne en négociation : on meuble
  const AMBIANCE_MS = 40000;        // en données : un mot de temps en temps
  const RETX_MS = 15000;            // retransmissions LAPM : pas plus d'un mot par 15 s

  /* ------------------------------------------------------------ phrases
   * {Autre}/{autre} : l'autre modem (le FAI, un abonné…), {Notre}/{notre} :
   * le nôtre ; {emet}/{recoit} : qui émet / reçoit le signal commenté.
   * Les majuscules initiales donnent la forme de début de phrase. */
  const P = {
    accueil: [
      "Bonsoir à tous ! Le commentateur est en cabine. Coup d'envoi au premier ATDT.",
      "Bienvenue au stade de la ligne téléphonique ! Les modems s'échauffent.",
      "Ici le commentateur. Rien à commenter pour l'instant : le modem attend son heure.",
      "Le micro est ouvert ! Il ne manque plus qu'un appel… et un peu de suspense.",
    ],
    retour_antenne: [
      "Rebonsoir ! Le commentateur reprend l'antenne.",
      "Et nous revoilà ! Pardon pour la coupure, la régie avait débranché le micro.",
      "Retour en cabine. Qu'est-ce qu'on a raté ?",
      "Micro rallumé ! On reprend le fil… téléphonique.",
    ],

    // -------------------------------------------------- avant la négociation
    numerotation: [
      "Et c'est parti ! {Notre} compose le {num}… les doigts ne tremblent pas.",
      "Coup d'envoi ! Numérotation du {num}, en fréquences vocales s'il vous plaît.",
      "Tonalité, puis le {num} au clavier : du DTMF de toute beauté.",
      "Bonsoir à tous et bienvenue ! {Notre} décroche et compose le {num}.",
      "On décroche, on compose le {num}… Le public retient son souffle.",
      "Le {num} part au clavier. Bip, bop, bip : du travail d'orfèvre.",
    ],
    numerotation_fai: [
      "Et c'est parti ! {Notre} appelle le FAI au {num}… Internet, nous voilà !",
      "Numérotation du {num} : direction le fournisseur d'accès. La facture tremble déjà.",
      "Le {num} ! Le numéro qui a fait trembler tant de factures dans les années 90.",
      "Bonsoir ! Ce soir, {notre} affronte le FAI. Numérotation du {num}…",
      "On compose le {num} : ce soir, c'est Internet ou rien !",
    ],
    sonne: [
      "Ça sonne chez {autre}… décrochera, décrochera pas ?",
      "Tuuut… Tuuut… {Autre} se fait désirer.",
      "Ça sonne ! Le public est debout, les modems sont prêts.",
      "Ça sonne dans le vide pour l'instant… allez, décroche !",
      "Sonnerie chez {autre}. Le suspense est total.",
    ],
    ring: [
      "Ça sonne ! On nous appelle… {notre} se prépare à décrocher.",
      "RING ! Un appel entrant, et {notre} n'a même pas eu le temps de s'échauffer.",
      "Driiing ! Qui peut bien appeler un modem à cette heure-ci ?",
      "Appel entrant ! {Notre} va décrocher, il adore ça.",
      "On sonne à la porte ! Quelqu'un veut nous parler… en V.34, j'espère.",
    ],

    // -------------------------------------------------- V.8
    ansam_rx: [
      "{Autre} décroche… ANSam à 2100 hertz, le public retient son souffle !",
      "Le voilà ! ANSam, 2100 hertz modulé à 15 hertz : c'est bien un modem en face !",
      "Biiiii ! La tonalité de réponse ! {Autre} est dans la partie !",
      "ANSam détecté ! Les annuleurs d'écho du réseau sont priés de quitter le terrain.",
      "Et c'est l'ANSam ! Inversion de phase toutes les 450 millisecondes, du grand classique.",
      "{Autre} répond par un ANSam impeccable. On va avoir du spectacle !",
    ],
    ansam_tx: [
      "{Notre} décroche et lance son ANSam, 2100 hertz pleine puissance !",
      "Biiiii ! C'est {notre} qui chante l'ANSam. Annuleurs d'écho, dégagez !",
      "ANSam en émission : {notre} annonce qu'il est un modem, un vrai.",
      "L'ANSam part sur la ligne… {autre} va-t-il oser répondre ?",
      "On décroche, 2100 hertz, inversions de phase : la grande classe.",
    ],
    cm_tx: [
      "{Notre} répond du tac au tac : menu CM en V.21, 300 bit/s, à l'ancienne !",
      "CM ! Il annonce ses capacités. Il ne manque pas d'aplomb, le petit.",
      "Message CM : {notre} déballe ses modulations comme un CV.",
      "Le CM part en V.21. À 300 bit/s, on prend le temps de bien se présenter.",
      "Di-di-di-di : {notre} récite son menu V.8. Il le connaît par cœur.",
    ],
    cm_rx: [
      "CM reçu : {autre} propose {modes}. Voyons ce que {notre} en pense…",
      "{Autre} sort son CV en V.21 : {modes}. Pas mal.",
      "Menu CM reçu : {modes}. On va bien trouver un terrain d'entente.",
      "{Autre} annonce la couleur : {modes}.",
    ],
    jm_tx: [
      "JM : {notre} tranche, ce sera {modes} !",
      "Réponse JM : {modes}. Le choix est fait, pas de discussion.",
      "{Notre} répond par JM : {modes}. Décision de capitaine.",
      "JM en V.21 : on jouera {modes}.",
    ],
    jm_rx: [
      "JM reçu ! On parlera {modes}. Accord trouvé en un temps record.",
      "{Autre} répond par JM : ce sera {modes}. Les présentations sont faites.",
      "JM ! Les deux modems tombent d'accord sur {modes}. Quelle entente !",
      "Réponse JM : {modes}. Pas de dispute sur le protocole, c'est rare.",
      "JM ! {modes} pour tout le monde, et dans la bonne humeur.",
    ],
    cj: [
      "CJ ! Fin des présentations, place aux choses sérieuses.",
      "CJ, 75 millisecondes de silence… le calme avant la tempête.",
      "CJ envoyé : le V.8 est dans la boîte !",
      "CJ ! Poignée de main V.8 terminée. On enlève les survêtements.",
    ],
    v90: [
      "Le FAI est raccordé en numérique ! Le 56k est possible… le stade est debout !",
      "Du numérique en face ! On va tenter le V.90, le 56k, la légende !",
      "Mesdames et messieurs, ce soir on joue en V.90 ! 56 000 bit/s en ligne de mire.",
      "V.90 en vue ! {Notre} sera le modem analogique, et il en est fier.",
      "56k ! Le mot magique est prononcé. Les vieux de la vieille ont la larme à l'œil.",
    ],
    v90_num: [
      "L'abonné réclame du 56k : {notre} jouera le modem numérique. Responsabilité énorme !",
      "V.90 ! {Notre} sera le modem numérique, celui qui a le central dans la manche.",
      "Du 56k demandé ! {Notre} enfile le maillot du numérique.",
      "Le V.90 est sur la table, et {notre} tient le rôle du numérique. Pression maximale.",
    ],
    v90_rate: [
      "Pas de 56k ce soir… on se rabat sur le bon vieux V.34.",
      "Le 56k passe son tour. Le V.34 fera très bien l'affaire.",
      "Raté pour le V.90 ! Le plan B s'appelle V.34, et il est solide.",
      "Pas de V.90 : le 56k reste au vestiaire.",
    ],

    // -------------------------------------------------- phase 2
    info0: [
      "Phase 2 ! INFO0 en DPSK à 600 bit/s : on échange les cartes de visite.",
      "On passe au sondage de la ligne. INFO0 : chacun dit ce qu'il sait faire.",
      "INFO0 ! Brrr-brrt : les capacités V.34 s'échangent à 600 bit/s.",
      "Phase 2, l'heure des mesures. Un INFO0 pour commencer, en toute courtoisie.",
      "Brrrt ! INFO0 : débits symbole, options, tout est déclaré. Rien à cacher.",
    ],
    info0_bis: [
      "Il répète son INFO0… {autre} n'a pas bien entendu ?",
      "INFO0, le retour. On reprend depuis le début, calmement.",
      "Répétition des INFO0 ! Un peu de friture sur la ligne, sans doute.",
      "Deuxième INFO0 : quand on n'est pas compris, on répète. Plus fort.",
    ],
    info0d: [
      "INFO0d : {emet} se déclare numérique. La classe internationale !",
      "Brrrt ! INFO0d : le numérique entre sur le terrain.",
      "INFO0d ! Le modem numérique présente ses lettres de noblesse.",
      "INFO0d : pas de doute, en face c'est du numérique, branché en direct sur le central.",
    ],
    tons: [
      "Tons A et B, inversions de phase : on mesure la distance au millimètre.",
      "Tuuuu… Tons A et B ! Le chronomètre est lancé.",
      "Ping-pong de tons purs : on mesure le délai aller-retour.",
      "Les tons A et B se répondent. C'est de la géométrie, c'est de la physique !",
      "Inversion de phase, top chrono ! Combien de temps pour l'aller-retour ?",
    ],
    // Le sondage L1/L2 suit la mesure du délai de près : une seule ligne pour les deux.
    rtd_court: [
      "Il sonde la ligne… délai aller-retour {rtd} ms, à bout portant !",
      "{rtd} ms aller-retour : {autre} habite sur le palier ? Place au sondage !",
      "Il sonde la ligne… écho à {rtd} millisecondes, ça ne lui fait pas peur !",
      "Délai de {rtd} ms : la ligne est courte. Et maintenant, Krrr, le sondage !",
    ],
    rtd_moyen: [
      "Délai aller-retour : {rtd} ms. Rien d'insurmontable. Place au sondage !",
      "Il sonde la ligne… écho à {rtd} millisecondes, ça ne lui fait pas peur !",
      "Le chrono affiche {rtd} ms aller-retour. Krrr ! On ausculte la ligne.",
      "Délai mesuré : {rtd} ms. L'annuleur d'écho a pris note, la sonde est lancée.",
    ],
    rtd_long: [
      "{rtd} millisecondes aller-retour ! On appelle l'autre bout du monde ?",
      "Délai de {rtd} ms… la ligne fait le tour de la Terre, ou presque !",
      "Aïe, {rtd} ms aller-retour. L'annuleur d'écho va devoir s'employer.",
      "Il sonde la ligne… {rtd} ms d'aller-retour ! Par satellite, peut-être ?",
    ],
    sondage: [
      "Il sonde la ligne… Krrr ! 21 fréquences d'un coup !",
      "Sonde L1/L2 : on ausculte la ligne, fréquence par fréquence.",
      "Krrrkrrr ! Le sondage ! La ligne passe sa visite médicale.",
      "L1, L2 : vingt et une raies espacées de 150 hertz. Rien ne lui échappera !",
      "Il sonde la ligne de 150 à 3750 hertz. Tout y passe, même les aigus.",
    ],
    snr_top: [
      "Verdict du sondage : {snr} dB de rapport signal/bruit. Une ligne de rêve !",
      "{snr} dB ! Ligne propre comme un sou neuf.",
      "SNR {snr} dB : le bruit est resté au vestiaire.",
      "Sondage terminé : {snr} dB. Conditions idéales, pelouse parfaite !",
    ],
    snr_moyen: [
      "Le sondage annonce {snr} dB. Correct, sans plus.",
      "SNR {snr} dB : la ligne est honnête. On peut jouer.",
      "{snr} dB de rapport signal/bruit : une ligne de campagne, mais solide.",
      "Sondage : {snr} dB. Ni une autoroute, ni un chemin de terre.",
    ],
    snr_bas: [
      "Seulement {snr} dB… ça crachouille, il va falloir de la technique !",
      "Sondage inquiétant : {snr} dB. La ligne grésille comme un vieux 45 tours.",
      "{snr} dB. On a connu des lignes plus propres. Le terrain est lourd.",
      "SNR {snr} dB : la pelouse est grasse, les débits vont souffrir.",
    ],
    // SNR et débit symbole tombent presque en même temps : « {snr} dB… il tente le 3429 ! »
    pre_snr_top: [
      "{snr} dB au sondage, une ligne de rêve !",
      "Ligne impeccable, {snr} dB !",
      "{snr} dB : un billard !",
      "Sondage parfait, {snr} dB.",
    ],
    pre_snr_moyen: [
      "{snr} dB au sondage, correct.",
      "Ligne honnête, {snr} dB.",
      "{snr} dB : ça ira.",
      "Sondage : {snr} dB, sans plus.",
    ],
    pre_snr_bas: [
      "Seulement {snr} dB au sondage…",
      "{snr} dB, la ligne grésille…",
      "Sondage difficile, {snr} dB.",
      "{snr} dB, terrain lourd…",
    ],
    baud_max: [
      "Il tente le 3429 bauds ! Oh, l'audace !",
      "3429 bauds ! Le débit symbole maximum. Il n'a peur de rien !",
      "Vitesse de symbole : 3429 bauds. C'est le grand braquet !",
      "3429 bauds, porteuse {porteuse} hertz : il joue l'attaque à outrance !",
    ],
    baud_moyen: [
      "{baud} bauds, porteuse {porteuse} hertz. Le choix de la raison.",
      "Il opte pour {baud} bauds. Pas de folie, mais du solide.",
      "{baud} bauds ! Il garde un peu de marge, c'est malin.",
      "Débit symbole : {baud} bauds. Tactique prudente… mais payante ?",
    ],
    baud_min: [
      "2400 bauds seulement… prudence, prudence.",
      "Il se replie sur 2400 bauds. La ligne ne lui laisse pas le choix.",
      "2400 bauds : on joue la défense. Le catenaccio du modem.",
      "Repli tactique sur 2400 bauds. L'important, c'est d'arriver.",
    ],
    decalage: [
      "Décalage de fréquence de {fo} Hz : il corrige sans broncher.",
      "{fo} hertz de décalage, rattrapés au vol. Quel sang-froid !",
      "La ligne décale de {fo} Hz… il recale tout, l'air de rien.",
      "Décalage de {fo} hertz mesuré. Rien ne lui échappe.",
    ],
    inversion_ratee: [
      "Il a raté l'inversion du ton A… et continue quand même. Culotté !",
      "Inversion de A manquée ! Il fait comme si de rien n'était.",
      "Pas vu l'inversion de phase ? Tant pis, on avance !",
      "L'inversion du ton A lui a échappé. On ne va pas en faire un drame.",
    ],
    crc: [
      "Un {quoi} avec un CRC faux ! Carton jaune pour la ligne.",
      "CRC faux sur l'{quoi}… la friture a frappé.",
      "{quoi} illisible ! Un parasite est passé par là.",
      "{quoi} reçu… mais abîmé. L'arbitre fait rejouer.",
    ],

    // -------------------------------------------------- phase 3
    phase3: [
      "Phase 3 ! S, S barre, PP : il cale le rythme…",
      "L'égaliseur entre en piste ! S, S barre, puis PP pour la synchro.",
      "Pschhh ! L'entraînement de l'égaliseur commence.",
      "Phase 3 : place à l'égaliseur. Ça va souffler fort.",
      "Phase 3 ! On passe aux choses sérieuses : l'égaliseur s'échauffe.",
    ],
    trn: [
      "TRN ! Du bruit pseudo-aléatoire… mais connu des deux côtés. Malin !",
      "Le TRN déroule : l'égaliseur apprend les défauts de la ligne.",
      "Pschhhhh… TRN ! Le bruit blanc le plus organisé du monde.",
      "TRN en cours : l'égaliseur ajuste ses coefficients, un par un.",
      "Séquence TRN : entraînement intensif, à huis clos.",
    ],
    echo: [
      "L'annuleur d'écho encaisse {erle} dB. Mur défensif !",
      "Annuleur d'écho entraîné : {erle} dB d'atténuation. L'écho n'a qu'à bien se tenir.",
      "{erle} dB d'écho effacés. Propre, net, sans bavure.",
      "L'écho est neutralisé : {erle} dB. Une défense de fer !",
    ],
    egaliseur: [
      "Égaliseur entraîné : {snr} dB de SNR. Les réglages sont faits !",
      "L'égaliseur a fini ses gammes : {snr} dB.",
      "{snr} dB après égalisation. Il a bien bossé, l'égaliseur.",
      "Égaliseur au point : {snr} dB. Le plus dur est fait… ou presque.",
    ],
    j16: [
      "J : il réclame 16 points en phase 4. Il a faim !",
      "J, 16 points ! Il veut la grande constellation.",
      "Message J : 16 points. Pas de demi-mesure !",
      "16 points demandés ! Il vise haut, très haut.",
    ],
    j4: [
      "J : 4 points seulement. On reste humble.",
      "J, 4 points : il joue petits bras… mais c'est plus sûr.",
      "4 points en phase 4. La prudence est mère de la connexion.",
      "Message J, 4 points : on ne prend pas de risques.",
    ],

    // -------------------------------------------------- phase 4
    phase4: [
      "Phase 4 ! La dernière ligne droite…",
      "On entre dans la phase finale : TRN sur la vraie constellation !",
      "Phase 4, le sprint final ! Ne lâche rien !",
      "Dernier virage : phase 4 ! Le public est debout !",
      "Phase 4 ! Plus que quelques symboles avant la gloire.",
    ],
    mp: [
      "MP : il annonce son débit. Le moment de vérité approche…",
      "Le paramètre MP part sur la ligne : débit, précodage, mise en forme. Tout est dit.",
      "MP ! Les débits sont sur la table.",
      "MP envoyé. Il ne reste plus qu'à se serrer la main.",
    ],
    e: [
      "E ! Poignée de main. Accord conclu.",
      "Signal E : c'est signé, c'est validé !",
      "E, comme « En avant » !",
      "Le E est parti… on y est presque !",
    ],

    // -------------------------------------------------- V.90 (56k)
    ja: [
      "Ja : {emet} commande sa séquence d'apprentissage sur mesure !",
      "Ja ! Commande passée pour la DIL. Service à la carte.",
      "Ja : le modem analogique dicte ses conditions. Audacieux !",
      "Signal Ja : on prépare le grand numéro du 56k.",
    ],
    sd: [
      "Sd, S barre d : {emet} cale son horloge sur celle du central.",
      "TRN1d ! {Emet} envoie ses premiers niveaux PCM…",
      "Pschhh ! TRN1d : l'égaliseur se met en jambes, version numérique.",
      "Sd, TRN1d : le numérique entre en scène.",
    ],
    jd: [
      "Jd : {emet} déballe ses capacités 56k.",
      "Jd ! {Emet} dit ce qu'il sait faire. Et il sait faire beaucoup.",
      "J prime d : confirmation. On y va !",
      "Jd : les cartes du 56k sont sur la table.",
    ],
    dil: [
      "DIL ! {Recoit} apprend les 128 niveaux du central… le chant mythique du 56k !",
      "Ouiii-ouuu ! La séquence DIL ! On cartographie chaque niveau du codec !",
      "La DIL : le moment où les modems 56k chantaient faux… et c'était beau.",
      "DIL ! Les vieux de la vieille ont la larme à l'œil.",
      "DIL : {recoit} écoute les niveaux PCM un par un. Quelle oreille !",
      "Voici la DIL ! Les niveaux de la loi A, un par un : un vrai cours de téléphonie.",
    ],
    ri: [
      "Ri : {emet} signale qu'il est prêt pour la suite.",
      "Ri, R barre i : le modem analogique lève la main.",
      "Ri ! On passe à l'entraînement final.",
      "Ri : c'est bon pour lui, on enchaîne.",
    ],
    trn2d: [
      "TRN2d : réglages fins de l'égaliseur numérique.",
      "TRN2d ! On peaufine, on peaufine.",
      "Le TRN2d déroule : derniers ajustements avant le 56k.",
      "TRN2d : l'égaliseur révise ses gammes une dernière fois.",
    ],
    cp: [
      "CP : {emet} compose ses constellations à la carte !",
      "Paramètres CP : on choisit ses niveaux, un par un. Du sur-mesure !",
      "CP ! Les constellations PCM sont choisies. Ça sent le 56k !",
      "CP : on fixe les règles du jeu pour la voie descendante.",
    ],
    ed: [
      "Ed ! Fin de l'entraînement numérique.",
      "Ed : on y est presque, le 56k est à portée de main !",
      "Ed : poignée de main version 56k.",
      "Signal Ed : la messe est dite.",
    ],

    // -------------------------------------------------- connexion
    connect_56k: [
      "CONNECT {rx} ! Du 56k ! C'est historique !",
      "CONNECT {rx} en réception ! Le V.90 est passé ! Le stade exulte !",
      "{rx} bit/s ! Le 56k, mesdames et messieurs, le vrai !",
      "CONNECT {rx} ! Les modems 33,6k peuvent aller se rhabiller !",
      "CONNECT {rx} ! Et {tx} en émission. Le V.90 dans toute sa splendeur !",
    ],
    connect_top: [
      "CONNECT {rx} ! C'est un festival !",
      "CONNECT {rx} ! Quelle connexion ! Mais quelle connexion !",
      "{rx} bit/s ! Le maximum du V.34 ! Du jamais vu depuis 1996 !",
      "CONNECT {rx} ! Il l'a fait ! Il l'a fait !",
      "CONNECT {rx} ! Plein débit, pleine lucarne !",
    ],
    connect_bien: [
      "CONNECT {rx}. Pas le record, mais une belle connexion.",
      "CONNECT {rx} ! Solide, efficace, sans fioritures.",
      "{rx} bit/s : le contrat est rempli.",
      "CONNECT {rx}. Une victoire aux points.",
    ],
    connect_bof: [
      "CONNECT {rx}… Bon. Ça roule. Au pas, mais ça roule.",
      "CONNECT {rx}. L'essentiel, c'est de participer.",
      "{rx} bit/s… On a gagné, mais sans la manière.",
      "CONNECT {rx}. Le Minitel a eu peur… un peu.",
    ],
    reconnexion: [
      "Ça repart à {rx} bit/s ! Il est toujours là !",
      "Retour en jeu à {rx} bit/s. Quel mental !",
      "{rx} bit/s ! Il se relève et repart de plus belle.",
      "Et ça reconnecte à {rx} ! Les modems de cette époque étaient des guerriers.",
    ],
    ambiance: [
      "Les octets défilent… {ko} Ko échangés, il ne lâche rien.",
      "Toujours {rx} bit/s. Régularité de métronome.",
      "Le treillis tourne à plein régime. Rien à signaler, et c'est tant mieux.",
      "{trames} trames LAPM au compteur, et pas une de travers. Quel sérieux.",
      "La ligne tient. C'est presque ennuyeux… et c'est exactement ce qu'on veut.",
      "{ko} Ko au compteur. À ce rythme, la photo JPEG sera là avant minuit.",
    ],
    retransmission: [
      "Une trame abîmée ! LAPM la renvoie : rien vu, rien perdu.",
      "Petite erreur de transmission, retransmise aussitôt. L'arbitre n'a rien sifflé.",
      "Retransmission LAPM ! Un parasite a voulu jouer les trouble-fête.",
      "Trame rejetée, trame renvoyée. LAPM, c'est l'arbitrage vidéo du modem.",
    ],
    tunnel: [
      "Le tunnel Internet est ouvert : à vous le Web, version années 90 !",
      "Internet est en ligne ! Le navigateur n'attend plus que vous.",
      "Tunnel ouvert : les pages web vont arriver… à leur rythme.",
      "Proxy prêt ! Le Web à {rx} bit/s : préparez le café.",
    ],

    // -------------------------------------------------- incidents
    retrain: [
      "Réentraînement… il a glissé sur une coupure, mais il se relève !",
      "Oh ! Réentraînement complet ! On reprend depuis la mesure du délai.",
      "Il faut tout recommencer ! Réentraînement. Le public s'inquiète.",
      "Réentraînement ! Ce n'est qu'un contretemps, il en a vu d'autres.",
      "Retour au vestiaire pour un réentraînement express.",
    ],
    retrain_nous: [
      "Réentraînement demandé depuis le banc de touche ! Changement tactique.",
      "On a appuyé sur RETRAIN. Le coach veut voir autre chose.",
      "RETRAIN à la demande : tout le monde reprend ses positions.",
      "Le sélectionneur demande un réentraînement. Il doit avoir ses raisons.",
    ],
    retrain_eux: [
      "{Autre} demande un réentraînement. On s'exécute, beau joueur.",
      "Réentraînement réclamé par {autre}. Il n'était pas à l'aise ?",
      "{Autre} siffle un réentraînement. On reprend les mesures.",
      "C'est {autre} qui demande à tout recommencer. Soit !",
    ],
    retrain_err: [
      "Trop d'erreurs, même après les renégociations : réentraînement complet !",
      "Ça ne passe plus… réentraînement ! On repart de zéro, ou presque.",
      "Les erreurs s'accumulent : retour à l'entraînement. Dur, dur.",
      "Réentraînement forcé ! La ligne a eu raison de sa patience.",
    ],
    reprise: [
      "Reprise {n} sur 3 : {why}. Il s'accroche !",
      "Ouh là ! {Why}. Reprise numéro {n}, il ne lâche rien.",
      "Accroc : {why}. Il retente sa chance, {n} sur 3.",
      "Incident de jeu : {why}. On rejoue, tentative {n} sur 3.",
    ],
    reneg_nous: [
      "Renégociation demandée ! On rediscute les tarifs en plein match.",
      "RENÉGO ! Les débits retournent sur la table.",
      "Renégociation de débit : tout le monde reste en ligne, on rediscute.",
      "Le coach veut renégocier. Les deux modems s'exécutent.",
    ],
    reneg_eux: [
      "{Autre} renégocie ! Il trouvait le rythme un peu élevé ?",
      "Renégociation lancée par {autre}. On discute, entre gentlemen.",
      "{Autre} veut renégocier le débit. Pourquoi pas.",
      "Tiens, {autre} rouvre les négociations.",
    ],
    reneg_bas: [
      "Trop d'erreurs ({err} %) : il lève le pied. La sagesse !",
      "Ça accroche ! Renégociation vers le bas, il baisse d'un braquet.",
      "{err} % d'erreurs : on ralentit avant la sortie de route.",
      "La ligne se dégrade… il rétrograde. Prudent, le garçon.",
    ],
    reneg_haut: [
      "La ligne s'améliore ({snr} dB) : il remet les gaz !",
      "{snr} dB ! Ça va mieux, on accélère !",
      "Embellie sur la ligne : renégociation vers le haut. Allez !",
      "Il sent que la ligne tient : on monte d'un cran !",
    ],

    // -------------------------------------------------- fin
    no_carrier: [
      "NO CARRIER. C'est fini, les amis.",
      "NO CARRIER ! La porteuse s'est éteinte. Rideau.",
      "Et c'est la fin : NO CARRIER. Merci de nous avoir suivis.",
      "NO CARRIER. Il ne reste que le silence… et la facture.",
      "Coup de sifflet final : NO CARRIER.",
    ],
    raccroche_nous: [
      "{Notre} raccroche. Fin de la retransmission.",
      "On raccroche ! Le match est terminé, à bientôt pour de nouvelles aventures.",
      "Raccrochage propre : le geste est élégant, la porteuse s'éteint.",
      "Fin de communication. {Notre} rentre aux vestiaires, la tête haute.",
    ],
    abandon: [
      "On raccroche avant la fin ! Match interrompu.",
      "Il raccroche en pleine négociation ! Le public siffle.",
      "Abandon ! {Notre} quitte le terrain avant le coup de sifflet.",
      "Fin prématurée. On ne saura jamais à quel débit il aurait connecté.",
    ],
    raccroche_eux: [
      "{Autre} raccroche ! Sans même dire au revoir.",
      "MP à débit nul : {autre} quitte le terrain. Proprement.",
      "{Autre} tire sa révérence. Raccrochage propre.",
      "C'est {autre} qui met fin à la partie. Beau match quand même.",
    ],
    echec: [
      "Échec : {why}. Le modem rentre aux vestiaires.",
      "Et c'est l'échec ! {Why}. Quelle déception…",
      "Raté : {why}. Ce n'était pas son soir.",
      "{Why}… C'est fini. On n'oubliera pas ce match. Malheureusement.",
    ],
    occupe: [
      "Occupé ! {Autre} est déjà au téléphone. Encore un ado sur le chat…",
      "BUSY ! La ligne est prise. On réessaiera à la mi-temps.",
      "Tuu-tuu-tuu… occupé ! Match reporté.",
      "Occupé. Quelqu'un est sur Internet chez {autre}, c'est sûr.",
    ],
    pas_de_reponse: [
      "Numéro non attribué… on a appelé dans le vide.",
      "NO ANSWER : personne au bout du fil. Forfait !",
      "Personne ne répond… Match annulé faute d'adversaire.",
      "Pas de réponse. Le stade est vide.",
    ],
    pas_de_tonalite: [
      "Pas de tonalité ! Le central fait la sieste ?",
      "NO DIALTONE : sans central, pas de match. Connectez-vous d'abord !",
      "Pas de tonalité… le câble est-il bien branché ?",
      "NO DIALTONE. Le terrain est fermé.",
    ],

    // -------------------------------------------------- on meuble
    meuble_sonne: [
      "Ça sonne toujours… {autre} est sous la douche ?",
      "Toujours personne. On meuble, on meuble.",
      "La sonnerie s'éternise. Le suspense devient insoutenable.",
      "Allô ? Allô ? Toujours rien.",
    ],
    meuble_v8: [
      "Toujours en V.8… on attend que quelqu'un se décide.",
      "Ça discute encore en V.21. À 300 bit/s, forcément, ça prend du temps.",
      "Le suspense est insoutenable.",
      "On patiente. {Autre} prend son temps.",
    ],
    meuble_p2: [
      "Le sondage continue… on mesure, on remesure.",
      "Phase 2 toujours. Il ne veut rien laisser au hasard.",
      "Les mesures s'éternisent. Un perfectionniste, ce modem.",
      "On sonde encore. La ligne doit avoir des secrets.",
    ],
    meuble_p3: [
      "L'égaliseur peaufine… c'est long, mais c'est beau.",
      "Toujours l'entraînement. Il travaille ses gammes.",
      "Phase 3 interminable. L'égaliseur a du pain sur la planche !",
      "Le TRN s'étire. Patience, patience.",
    ],
    meuble_p4: [
      "Dernière ligne droite… qui n'en finit pas !",
      "Phase 4 encore. Le suspense est total.",
      "On y est presque. Presque. Presque…",
      "La phase finale joue les prolongations.",
    ],
    meuble_retrain: [
      "Réentraînement en cours… on serre les dents.",
      "Il se remet en jambes. Patience.",
      "Les mesures reprennent. Le public retient son souffle.",
      "Ça reprend tout depuis le début, ou presque. Courage !",
    ],
    ralenti: [
      "On revoit l'action au ralenti, ×{v}… Regardez-moi cette inversion de phase !",
      "Ralenti ×{v} ! Chaque symbole prend son temps.",
      "Au ralenti, on voit tout. La régie se fait plaisir.",
      "Passage au ralenti ×{v} : savourez chaque bit.",
    ],
    accelere: [
      "Accéléré ×{v} ! On zappe les temps morts.",
      "Avance rapide ×{v} ! Les modems ne se rendent compte de rien.",
      "×{v} ! On n'a pas toute la nuit.",
      "Accéléré ×{v} : le résumé du match, en direct.",
    ],
    direct: [
      "Retour au temps réel.",
      "Et on revient en direct, à vitesse normale.",
      "Fin du ralenti, retour au direct.",
      "Vitesse normale. Le direct reprend ses droits.",
    ],
  };
  /** Situations qui terminent l'appel (affichées en rouge). */
  const FINS = new Set(["no_carrier", "abandon", "raccroche_eux", "raccroche_nous", "echec", "occupe", "pas_de_reponse", "pas_de_tonalite"]);
  /** Signaux du 56k (V.90), commentés dans les deux sens. */
  const V90 = {
    INFO0d: "info0d", Ja: "ja", Sd: "sd", "S̄d": "sd", TRN1d: "sd", Jd: "jd", "J′d": "jd",
    DIL: "dil", Ri: "ri", "R̄i": "ri", TRN2d: "trn2d", CPt: "cp", CP: "cp", "CP′": "cp", Ed: "ed",
  };
  const MEUBLES = { Phase1: "meuble_v8", Phase2: "meuble_p2", Phase3: "meuble_p3", Phase4: "meuble_p4", Retrain: "meuble_retrain" };

  /* ------------------------------------------------------------ karaoké */
  /** Paroles par signal (expression sur le nom, variantes). */
  const PAROLES = [
    [/^ANSam/, ["Biiiiiiiiiiiiiiiiiiiii", "Piiiiiiiiii-iiiiiiiiii", "Biiiiiii-iiiiii-iiiiiii"]],
    [/^(CM|JM|CJ)$/, ["di-di-di-di-di-di-di", "dou-di-dou-di-di-dou", "didididi-dididi-di"]],
    [/^INFO/, ["brrr-brrt brrr-brrt", "brrrt-brrrt-brrrt", "brr-brr-brrrrt"]],
    [/^DIL$/, ["Ouiii-ouuu-ouiii-ouuu-ouiii", "Wiiou-wiiou-wiiou-wiiou", "Ouin-ouin-ouiiin-ouin-ouiiin"]],
    [/^(A|Ā)$/, ["Tuuuuuuuuuuu…", "Tiiiiiiiiiii…"]],
    [/^(B|B̄)$/, ["Toooooooooooo…", "Touuuuuuuuuu…"]],
    [/^L[12]/, ["Krrrkrrrkrrrkrrr", "Krrr-krrr-krrr-krrr", "Grrrkrrrgrrrkrrr"]],
    [/^(S|PP|TRN)/, ["Pschhhhhhhhhhhhh", "Fschhhhhhhhhhhhh", "Pshhhhhhh-hhhhhh"]],
    [/^(J|MP|E|Ri|R̄i|CP)/, ["Pschhh-tt pschhh-tt", "Tchhh-tt-tchhh-tt", "Pschhht-pschhht"]],
    [/^B1/, ["Chhhhhhhhhhhhhhhhh", "Kchhhhhhhhhhhhhhh"]],
  ];
  /** Syllabes DTMF par rangée du clavier. */
  const DTMF = { 1: "bip", 2: "bip", 3: "bip", 4: "bup", 5: "bup", 6: "bup", 7: "bep", 8: "bep", 9: "bep", "*": "bop", 0: "bop", "#": "bop" };
  /** Chronologie de la numérotation (s) : comme tones.rs. */
  const NUM = { tonalite: 0.9, pause: 0.15, chiffre: 0.16, attente: 0.6, sonnerie: 1.5, cycle: 5 };

  /** Paroles d'un signal : { txt, mode: "boucle" | "repos", cle }. Le choix
   *  de la variante est stable pour un signal donné (`graine`). */
  function paroles(nom, graine = 0) {
    const n = String(nom || "").replace(/\s*×$/, "").trim();
    if (!n || n === "silence" || n === "—") return { txt: "", mode: "repos", cle: "" };
    const base = n.replace(/\s*\(.*\)$/, "");
    for (const [re, v] of PAROLES) if (re.test(base)) return { txt: v[graine % v.length], mode: "boucle", cle: base };
    return { txt: "Bzzzzzzzzzz", mode: "boucle", cle: base };
  }

  /* ------------------------------------------------------------ outils */
  const milliers = (v) => String(Math.round(v)).replace(/\B(?=(\d{3})+(?!\d))/g, " ");
  const virgule = (v, d = 1) => Number(v).toFixed(d).replace(".", ",");
  const majuscule = (s) => (s ? s.charAt(0).toUpperCase() + s.slice(1) : s);
  const remplir = (s, v) => s.replace(/\{(\w+)\}/g, (m, k) => (v[k] != null ? String(v[k]) : m));
  const chiffres = (s) => String(s || "").replace(/[^\d*#]/g, "");
  const enAppel = (p) => p && p !== "Idle" && p !== "Failed";
  /** « V.34, V.32bis, V.21 » → « V.34 (et 2 roues de secours) » */
  function modes(liste) {
    const m = String(liste || "").split(",").map((x) => x.trim()).filter(Boolean);
    if (!m.length) return "rien du tout";
    if (m.length === 1) return m[0];
    return `${m[0]} (et ${m.length - 1} roue${m.length > 2 ? "s" : ""} de secours)`;
  }
  const NOTRE = [
    { le: "notre modem", du: "de notre modem" },
    { le: "notre modem", du: "de notre modem" },
    { le: "notre TV-3400", du: "de notre TV-3400" },
    { le: "notre champion", du: "de notre champion" },
    { le: "le petit Tinyverse", du: "du petit Tinyverse" },
  ];
  const FAI = [
    { le: "le FAI", du: "du FAI", au: "au FAI" },
    { le: "le FAI", du: "du FAI", au: "au FAI" },
    { le: "le fournisseur d'accès", du: "du fournisseur d'accès", au: "au fournisseur d'accès" },
    { le: "le modem du FAI", du: "du modem du FAI", au: "au modem du FAI" },
  ];
  const AUTRES = [
    { le: "l'autre modem", du: "de l'autre modem", au: "à l'autre modem" },
    { le: "le modem d'en face", du: "du modem d'en face", au: "au modem d'en face" },
    { le: "l'adversaire", du: "de l'adversaire", au: "à l'adversaire" },
    { le: "le correspondant", du: "du correspondant", au: "au correspondant" },
  ];
  const ISP_NUMBER = "0836656565";

  /* ------------------------------------------------------------ moteur */
  /**
   * @param sortie  appelée avec { texte, prio, cle, fin, chrono } à chaque ligne dite
   * @param hasard  générateur dans [0, 1) (Math.random par défaut)
   * @param maintenant horloge en ms
   */
  function creer({ sortie = () => {}, hasard = Math.random, maintenant = () => Date.now() } = {}) {
    const derniere = {};   // dernière variante tirée, par situation
    let file = [];
    let derniereLigne = -1e9, texteAvant = "";
    let etat = null;       // dernier UiState
    let appel = null;      // l'appel en cours (null : pas encore d'appel)

    const choix = (liste) => liste[Math.floor(hasard() * liste.length) % liste.length];

    function nouvelAppel() {
      appel = {
        dit: new Set(),        // déjà commenté pendant l'appel…
        manche: new Set(),     // … ou depuis le dernier (ré)entraînement
        fini: false, clos: false, inactifDepuis: 0,
        connecte: false, connecteA: 0, rx: 0,
        numero: "", fai: false, nom: null, sortant: null,
        distantTx: null,       // signal émis par l'autre (boucle locale)
        repriseA: -1e9, meubleA: 0,
        attenteSnr: 0, params: null, retx: null, retxA: -1e9, tunnel: false,
        graines: {},           // variante des paroles, par signal
      };
    }

    function premiere(jeton, portee = "appel") {
      if (!appel) return true;
      const set = portee === "manche" ? appel.manche : appel.dit;
      if (set.has(jeton)) return false;
      set.add(jeton);
      return true;
    }

    /** Variante au hasard, jamais la même que la dernière fois. */
    function tirer(cle) {
      const b = P[cle];
      let i = Math.floor(hasard() * b.length) % b.length;
      if (b.length > 1 && i === derniere[cle]) i = (i + 1 + Math.floor(hasard() * (b.length - 1))) % b.length;
      derniere[cle] = i;
      return b[i];
    }

    function fai() {
      return !!(appel && appel.fai) || (etat && etat.session === "internet");
    }
    function vars(extra) {
      const a = appel && appel.nom && !fai()
        ? { le: appel.nom, du: (/^[aeiouyhéèêàâîôû]/i.test(appel.nom) ? "d'" : "de ") + appel.nom, au: "à " + appel.nom }
        : choix(fai() ? FAI : AUTRES);
      const n = choix(NOTRE);
      const v = { autre: a.le, Autre: majuscule(a.le), du_autre: a.du, au_autre: a.au, notre: n.le, Notre: majuscule(n.le), du_notre: n.du, ...extra };
      if (extra.why) v.Why = majuscule(extra.why);
      // Qui émet / qui reçoit le signal commenté
      const nous = n.le, eux = a.le;
      v.emet = extra.sens === "rx" ? eux : nous;
      v.recoit = extra.sens === "rx" ? nous : eux;
      v.Emet = majuscule(v.emet);
      v.Recoit = majuscule(v.recoit);
      return v;
    }

    function chrono() {
      if (!appel || appel.clos || !etat || !etat.elapsed_s) return null;
      const t = Math.floor(etat.elapsed_s);
      return `${String(Math.floor(t / 60)).padStart(2, "0")}:${String(t % 60).padStart(2, "0")}`;
    }

    function emettre(it) {
      derniereLigne = maintenant();
      texteAvant = it.texte;
      sortie({ texte: it.texte, prio: it.prio, cle: it.cle, fin: FINS.has(it.cle), chrono: chrono() });
    }

    /**
     * Met une ligne dans la file. prio 0 : anecdote, 1 : temps fort,
     * 2 : grande nouvelle (CONNECT, NO CARRIER…) qui passe devant tout.
     * groupe : une ligne en attente du même groupe est remplacée (on ne
     * commente pas avec trois secondes de retard ce qui est déjà dépassé).
     */
    function dire(cle, extra = {}, { prio = 0, groupe = null, prefixe = "", suffixe = "", libre = false } = {}) {
      if (!P[cle]) return false;
      // libre : simple anecdote, dite seulement si l'antenne est libre.
      if (libre && (file.length || maintenant() - derniereLigne < ECART_MS)) return false;
      let texte = prefixe + remplir(tirer(cle), vars(extra)) + suffixe;
      if (texte === texteAvant) texte = prefixe + remplir(tirer(cle), vars(extra)) + suffixe;
      const it = { texte, prio, groupe, cle, extra, ne: maintenant() };
      if (prio >= 2) {
        file = file.filter((x) => x.prio >= 2);
        if (maintenant() - derniereLigne >= 400 && !file.length) emettre(it);
        else file.push(it);
        return true;
      }
      if (groupe) file = file.filter((x) => x.groupe !== groupe || x.prio > prio);
      file.push(it);
      while (file.length > FILE_MAX) {
        let j = 0;
        for (let k = 1; k < file.length; k++) if (file[k].prio < file[j].prio) j = k;
        file.splice(j, 1);
      }
      tic();
      return true;
    }

    /** Fin d'appel : une seule ligne, la première arrivée. */
    function fin(cle, extra = {}) {
      if (appel && !appel.clos) {
        if (appel.fini) return;
        appel.fini = true;
      }
      dire(cle, extra, { prio: 2 });
    }

    /* -------------------------------------------- événements du modem */
    function surEvenement(side, ev) {
      if (!ev) return;
      const d = ev.data;
      if (side === "remote") {
        // Boucle locale : ce que l'autre émet, c'est ce que nous entendons.
        if (ev.type === "TxSignal" && appel) appel.distantTx = d;
        return;
      }
      switch (ev.type) {
        case "PhaseChanged": return surPhase(d);
        case "TxSignal": return surSignal(d, "tx");
        case "RxSignal": return surSignal(d, "rx");
        case "Log": return surLog(String((d && d.msg) || ""));
        case "Connected": return surConnexion(d.tx_rate, d.rx_rate);
        case "ParamsUpdated": if (appel) appel.params = d; return;
        case "Disconnected":
          if (appel && !appel.clos) fin(appel.connecte ? "raccroche_nous" : "abandon");
          return;
        default:
      }
    }

    function role() {
      return etat && etat.status ? etat.status.role : "Call";
    }

    function surPhase(p) {
      if (p === "Phase1" && (!appel || appel.clos)) nouvelAppel();
      if (!appel) return;
      switch (p) {
        case "Phase1":
          if ((appel.sortant != null ? appel.sortant : role() === "Call") && premiere("sonne")) dire("sonne");
          break;
        case "Phase3":
          if (premiere("phase3", "manche")) dire("phase3", {}, { groupe: "p34" });
          break;
        case "Phase4":
          if (premiere("phase4", "manche")) dire("phase4", {}, { groupe: "p34" });
          break;
        case "Cleardown":
          fin("raccroche_nous");
          break;
        default:
      }
    }

    function surSignal(nom, sens) {
      if (!appel) return;
      const n = String(nom || "");
      const base = n.replace(/\s*×$/, "").replace(/\s*\(.*\)$/, "").trim();
      if (sens === "tx") {
        switch (base) {
          case "ANSam": if (premiere("ansam")) dire("ansam_tx", {}, { prio: 1 }); break;
          case "CM": if (premiere("cm")) dire("cm_tx"); break;
          case "CJ": if (premiere("cj")) dire("cj", {}, { libre: true }); break;
          case "INFO0a": case "INFO0c":
            if (/×$/.test(n)) { if (premiere("info0bis", "manche")) dire("info0_bis", {}, { prio: 1 }); }
            else if (premiere("info0")) dire("info0", {}, { groupe: "p2" });
            break;
          case "A": case "B": if (premiere("tons", "manche")) dire("tons", {}, { groupe: "p2" }); break;
          case "L1": case "L2": if (premiere("sondage", "manche")) dire("sondage", {}, { groupe: "p2" }); break;
          case "TRN": if (premiere("trn", "manche")) dire("trn", {}, { groupe: "p34" }); break;
          case "J": if (premiere("j", "manche")) dire(/16/.test(n) ? "j16" : "j4", {}, { groupe: "p34" }); break;
          case "MP": case "MP′": if (premiere("mp", "manche")) dire("mp", {}, { groupe: "p34" }); break;
          case "E": if (premiere("e", "manche")) dire("e", {}, { groupe: "p34" }); break;
          default:
        }
      }
      // 56k : les signaux V.90 sont commentés dans les deux sens.
      const v = V90[base];
      if (v && premiere(v, "manche")) dire(v, { sens }, { prio: v === "dil" ? 1 : 0, groupe: v === "dil" ? null : "v90" });
    }

    function surLog(msg) {
      if (!appel) return;
      let m;
      const t = maintenant();
      if (/^ANSam détecté/.test(msg)) {
        if (premiere("ansam")) dire("ansam_rx", {}, { prio: 1 });
      } else if ((m = /^JM reçu ×2 : modes communs (.*)$/.exec(msg))) {
        if (premiere("jm")) dire("jm_rx", { modes: modes(m[1]) }, { prio: 1, groupe: "v8" });
      } else if ((m = /^CM reçu ×2 : l'appelant propose (.*)$/.exec(msg))) {
        if (premiere("cmrx")) dire("cm_rx", { modes: modes(m[1]) }, { groupe: "v8" });
      } else if ((m = /^Envoi JM \(V\.21 haut\) : (.*)$/.exec(msg))) {
        if (premiere("jmtx")) dire("jm_tx", { modes: modes(m[1]) }, { groupe: "v8" });
      } else if ((m = /^CONNECT 56k.*descendant ([\d\s]+) bit\/s, montant ([\d\s]+) bit\/s/.exec(msg))) {
        // Le 56k annonce sa connexion dans le journal ; l'événement Connected
        // qui suit (ou précède) ne doit pas compter comme une reconnexion.
        if (!appel.connecte) surConnexion(+m[2].replace(/\s/g, ""), +m[1].replace(/\s/g, ""));
      } else if (/56k|V\.90/.test(msg) && !/^(Envoi |CONNECT)/.test(msg)) {
        if (!premiere("v90")) return;
        if (/impossible|refus|pas de|échec|abandon|sans /i.test(msg)) dire("v90_rate", {}, { prio: 1, groupe: "v8" });
        else dire(/modem numérique/.test(msg) ? "v90_num" : "v90", {}, { prio: 1, groupe: "v8" });
      } else if ((m = /^Délai aller-retour mesuré : ([\d.]+) ms/.exec(msg))) {
        const rtd = parseFloat(m[1]);
        if (!premiere("rtd", "manche")) return;
        premiere("sondage", "manche"); // la ligne parle aussi du sondage
        dire(rtd < 5 ? "rtd_court" : rtd < 60 ? "rtd_moyen" : "rtd_long", { rtd: virgule(rtd) }, { prio: 1, groupe: "p2" });
      } else if ((m = /^Décalage de fréquence mesuré : ([+-]?[\d.]+) Hz/.exec(msg))) {
        const fo = Math.abs(parseFloat(m[1]));
        if (fo >= 1 && premiere("fo", "manche")) dire("decalage", { fo: virgule(fo) }, { libre: true });
      } else if (/^Sondage L1\/L2 analysé/.test(msg)) {
        appel.attenteSnr = t; // le SNR arrive juste après, avec les paramètres
      } else if ((m = /^Phase 3 — émission (\d+) bauds \/ porteuse (\d+) Hz, réception (\d+) bauds/.exec(msg))) {
        if (!premiere("baud", "manche")) return;
        const tx = +m[1], rx = +m[3];
        const cle = tx >= 3429 ? "baud_max" : tx <= 2400 ? "baud_min" : "baud_moyen";
        // SNR du sondage pas encore annoncé : une seule ligne pour les deux.
        let prefixe = "";
        const i = file.findIndex((x) => /^snr_/.test(x.cle));
        if (i >= 0) {
          const it = file.splice(i, 1)[0];
          prefixe = remplir(tirer("pre_" + it.cle), it.extra) + " ";
        } else if (appel.attenteSnr && snrSondage() != null && premiere("snr", "manche")) {
          const snr = snrSondage();
          appel.attenteSnr = 0;
          prefixe = remplir(tirer("pre_" + cleSnr(snr)), { snr: virgule(snr) }) + " ";
        }
        dire(cle, { baud: tx, porteuse: m[2] }, { prio: 1, groupe: "baud", prefixe, suffixe: rx !== tx ? ` Et ${rx} en réception : chacun son rythme.` : "" });
      } else if ((m = /^Annuleur d'écho entraîné.*atténuation ([\d.]+) dB/.exec(msg))) {
        if (premiere("echo", "manche")) dire("echo", { erle: virgule(parseFloat(m[1])) }, { groupe: "p34" });
      } else if ((m = /^Égaliseur (?:ré)?entraîné.*SNR ([\d.]+) dB/.exec(msg))) {
        if (premiere("eq", "manche")) dire("egaliseur", { snr: virgule(parseFloat(m[1])) }, { prio: 1, groupe: "p34" });
      } else if ((m = /demande (\d+) points/.exec(msg))) {
        if (premiere("j", "manche")) dire(m[1] === "16" ? "j16" : "j4", {}, { groupe: "p34" });
      } else if (/^Renégociation de débit demandée/.test(msg)) {
        dire("reneg_nous", {}, { prio: 1, groupe: "reneg" });
      } else if (/^S reçu en mode données/.test(msg)) {
        dire("reneg_eux", {}, { prio: 1, groupe: "reneg" });
      } else if ((m = /^Trop d'erreurs \(([\d.]+) %/.exec(msg))) {
        dire("reneg_bas", { err: virgule(parseFloat(m[1])) }, { prio: 1, groupe: "reneg" });
      } else if ((m = /^La ligne s'est améliorée \(SNR ([\d.]+) dB\)/.exec(msg))) {
        dire("reneg_haut", { snr: virgule(parseFloat(m[1])) }, { prio: 1, groupe: "reneg" });
      } else if (/^Toujours trop d'erreurs/.test(msg)) {
        appel.repriseA = t;
        appel.manche.clear();
        dire("retrain_err", {}, { prio: 1, groupe: "retrain" });
      } else if ((m = /^Reprise (\d+)\/3 : (.*)$/.exec(msg))) {
        appel.repriseA = t;
        appel.manche.clear();
        dire("reprise", { n: m[1], why: m[2] }, { prio: 1, groupe: "retrain" });
      } else if ((m = /^Réentraînement complet \((.*)\)$/.exec(msg))) {
        appel.manche.clear();
        if (t - appel.repriseA < 2500) return; // déjà annoncé (reprise, erreurs)
        const why = m[1];
        dire(/utilisateur/.test(why) ? "retrain_nous" : /autre modem/.test(why) ? "retrain_eux" : /erreur/.test(why) ? "retrain_err" : "retrain",
          { why }, { prio: 1, groupe: "retrain" });
      } else if (/répétition des INFO0/.test(msg)) {
        if (premiere("info0bis", "manche")) dire("info0_bis", {}, { prio: 1 });
      } else if ((m = /^(INFO\w+) reçu avec CRC faux/.exec(msg))) {
        if (premiere("crc" + m[1], "manche")) dire("crc", { quoi: m[1] });
      } else if (/^Inversion de A non détectée/.test(msg)) {
        if (premiere("inversion", "manche")) dire("inversion_ratee", {}, { libre: true });
      } else if ((m = /^Échec : (.*)$/.exec(msg))) {
        fin("echec", { why: m[1] });
      } else if (/^L'autre modem raccroche/.test(msg)) {
        fin("raccroche_eux");
      }
    }

    function surConnexion(tx, rx) {
      if (!appel) nouvelAppel();
      const v = { rx: milliers(rx), tx: milliers(tx) };
      if (appel.connecte) {
        // Même connexion annoncée deux fois (journal 56k + événement) : on l'a déjà dite.
        if (maintenant() - appel.connecteA < 2000 && rx === appel.rx) return;
        appel.rx = rx;
        dire("reconnexion", v, { prio: 1, groupe: "reneg" });
        return;
      }
      appel.rx = rx;
      appel.connecte = true;
      appel.connecteA = maintenant();
      const cle = rx > 33600 ? "connect_56k" : rx >= 31200 ? "connect_top" : rx >= 19200 ? "connect_bien" : "connect_bof";
      const asym = rx <= 33600 && tx !== rx ? ` Et ${milliers(tx)} en émission.` : "";
      dire(cle, v, { prio: 2, suffixe: asym });
    }

    /* -------------------------------------------- console (codes Hayes) */
    function surConsole(texte) {
      let m;
      if ((m = /^Numérotation (.+?)…?$/.exec(texte))) {
        nouvelAppel();
        appel.numero = chiffres(m[1]);
        appel.sortant = true;
        const net = etat && etat.net;
        appel.fai = appel.numero === ISP_NUMBER || !!(net && net.isp_number && net.isp_number === appel.numero);
        const ent = net && (net.directory || []).find((x) => x.number === appel.numero);
        if (ent && ent.name) appel.nom = ent.name;
        const num = /^\d{10}$/.test(appel.numero) ? appel.numero.replace(/(\d{2})(?=\d)/g, "$1 ") : m[1];
        dire(appel.fai ? "numerotation_fai" : "numerotation", { num }, { prio: 1 });
      } else if (/^Appel entrant simulé/.test(texte) || /^RING\b/.test(texte)) {
        if (!appel || appel.fini || appel.clos) nouvelAppel();
        appel.sortant = false;
        if ((m = /appel de (.+)\)$/.exec(texte))) appel.nom = m[1];
        if (premiere("ring")) dire("ring", {}, { prio: 1 });
      } else if (/^Ça sonne chez/.test(texte)) {
        if (premiere("sonne")) dire("sonne");
      } else if (/^NO CARRIER/.test(texte)) {
        fin("no_carrier");
      } else if (/^BUSY/.test(texte)) {
        fin("occupe");
      } else if (/^NO ANSWER/.test(texte)) {
        fin("pas_de_reponse");
      } else if (/^NO DIALTONE/.test(texte)) {
        fin("pas_de_tonalite");
      }
    }

    /* -------------------------------------------- état (≈ 25 Hz) */
    function surEtat(s) {
      const avant = etat;
      etat = s;
      const t = maintenant(), st = s.status || {};
      if (s.call_state && s.call_state !== "idle" && (!appel || appel.clos)) nouvelAppel();
      if (!appel || appel.clos) return;
      // Fin d'appel : l'état reste au repos un moment (on laisse passer un
      // bref « repos » entre la numérotation et l'appel réseau).
      if (s.call_state === "idle" && !enAppel(st.phase)) {
        if (!appel.inactifDepuis) appel.inactifDepuis = t;
        if (t - appel.inactifDepuis > 1500) {
          if (!appel.fini) fin(appel.connecte ? "raccroche_nous" : "abandon");
          appel.clos = true;
        }
        return;
      }
      appel.inactifDepuis = 0;
      // Échelle de temps : la régie passe au ralenti.
      if (avant && avant.speed != null && s.speed != null && Math.abs(s.speed - avant.speed) > 1e-6) {
        const v = virgule(s.speed, s.speed < 1 && s.speed !== 0.5 ? 2 : 1).replace(/,0+$/, "").replace(/(,\d)0$/, "$1");
        dire(s.speed < 1 ? "ralenti" : s.speed > 1 ? "accelere" : "direct", { v }, { prio: 1, groupe: "vitesse" });
      }
      const link = st.link;
      if (appel.connecte && link) {
        const r = (link.retransmissions || 0) + (link.fcs_errors || 0);
        if (appel.retx == null) appel.retx = r;
        if (r > appel.retx && t - appel.retxA > RETX_MS && st.phase === "Data") {
          appel.retxA = t;
          dire("retransmission");
        }
        appel.retx = r;
      }
      if (s.proxy && !appel.tunnel) {
        appel.tunnel = true;
        dire("tunnel", { rx: milliers(appel.rx || (st.params && st.params.data_rate_rx) || 0) });
      }
    }

    /** SNR mesuré par le sondage (null : pas encore connu). */
    function snrSondage() {
      if (appel && appel.params && appel.params.snr_db != null) return appel.params.snr_db;
      return etat && etat.status && etat.status.params ? etat.status.params.snr_db : null;
    }
    const cleSnr = (snr) => (snr >= 40 ? "snr_top" : snr >= 28 ? "snr_moyen" : "snr_bas");

    /* -------------------------------------------- horloge (5 Hz) */
    function tic() {
      const t = maintenant();
      if (appel && appel.attenteSnr && t - appel.attenteSnr > 150) {
        const snr = snrSondage();
        if (snr != null) {
          appel.attenteSnr = 0;
          if (premiere("snr", "manche")) dire(cleSnr(snr), { snr: virgule(snr) }, { prio: 1, groupe: "snr" });
          return;
        }
        if (t - appel.attenteSnr > 2000) appel.attenteSnr = 0;
      }
      file = file.filter((x) => t - x.ne <= VIE_MS[x.prio]);
      if (file.length) {
        if (t - derniereLigne >= ECART_MS || (file[0].prio >= 2 && t - derniereLigne >= 400)) emettre(file.shift());
        return;
      }
      meubler(t);
    }

    /** Silence d'antenne trop long : on meuble (négociation) ou on donne des
     *  nouvelles (données). */
    function meubler(t) {
      if (!appel || appel.fini || appel.clos || !etat || !etat.status) return;
      const ph = etat.status.phase, cs = etat.call_state;
      if (t - derniereLigne < MEUBLE_MS || t - appel.meubleA < MEUBLE_MS) return;
      let cle = null, v = {};
      if (cs === "dialing" || cs === "ringing") cle = "meuble_sonne";
      else if (MEUBLES[ph]) cle = MEUBLES[ph];
      else if (ph === "Data" && appel.connecte && t - derniereLigne > AMBIANCE_MS) {
        const st = etat.status, l = st.link;
        cle = "ambiance";
        const octets = l ? (l.bytes_delivered || 0) + (l.bytes_acked || 0) : (st.bytes_rx || 0) + (st.bytes_tx || 0);
        v = { rx: milliers(appel.rx), ko: milliers(octets / 1024), trames: milliers(l ? (l.frames_tx || 0) + (l.frames_rx || 0) : 0) };
      }
      if (!cle) return;
      appel.meubleA = t;
      dire(cle, v);
    }

    /* -------------------------------------------- karaoké */
    function voix(nom, sens) {
      const p = paroles(nom, 0);
      if (p.mode === "repos") return p;
      const g = appel.graines;
      if (!g[sens] || g[sens].cle !== p.cle) g[sens] = { cle: p.cle, n: Math.floor(hasard() * 6) };
      return paroles(nom, g[sens].n);
    }

    /**
     * Ce qui passe sur la ligne : { nous, eux, nomEux, message }. Chaque voix :
     * { txt, mode: "boucle" (balayage en boucle) | "temps" (balayage réglé
     * sur l'horloge, k ∈ [0, 1]) | "repos" }. `message` remplace les deux voix.
     */
    function karaoke() {
      const repos = { txt: "", mode: "repos", cle: "" };
      const r = { nous: repos, eux: repos, nomEux: nomCourt(), message: null };
      const s = etat;
      if (!s || !s.status || !appel || appel.clos) { r.message = "attente"; return r; }
      const st = s.status, cs = s.call_state;
      if (cs === "dialing") {
        // Numérotation : tonalité, DTMF, retour d'appel (chronologie de tones.rs)
        const t = (s.elapsed_s || 0) * (s.speed || 1);
        const n = Math.max(1, appel.numero.length), d0 = NUM.tonalite + NUM.pause, d1 = d0 + n * NUM.chiffre;
        const r0 = d1 + NUM.attente;
        r.nomEux = "CENTRAL";
        if (t < NUM.tonalite) r.eux = { txt: "Tuuuuuuuuuuuuuuuuut", mode: "temps", k: t / NUM.tonalite, cle: "tonalite" };
        else if (t >= d0 && t < d1) {
          const syl = appel.numero.split("").map((c) => DTMF[c]).filter(Boolean).join("-") || "bip-bop";
          r.nous = { txt: syl, mode: "temps", k: (t - d0) / (d1 - d0), cle: "dtmf" };
        } else if (t >= r0) {
          const ph = (t - r0) % NUM.cycle;
          if (ph < NUM.sonnerie) r.eux = { txt: "Tuuuuuuuuuut…", mode: "temps", k: ph / NUM.sonnerie, cle: "sonnerie" + Math.floor((t - r0) / NUM.cycle) };
        }
        return r;
      }
      if (cs === "ringing") {
        r.eux = { txt: "Driiiiiing ! Driiiiiing !", mode: "boucle", cle: "ring" };
        return r;
      }
      if (!enAppel(st.phase) && st.phase !== "Failed") {
        if (appel.fini) r.message = "fin";
        else if (cs === "idle") r.message = "attente";
        return r;
      }
      if (st.phase === "Failed") { r.message = "fin"; return r; }
      if (st.phase === "Data" && appel.connecte && maintenant() - appel.connecteA > 3500 && !(s.speaker && s.speaker.active)) {
        r.message = "donnees";
        return r;
      }
      r.nous = voix(st.tx_signal, "tx");
      r.eux = voix(appel.distantTx != null && !s.net ? appel.distantTx : st.rx_signal, "rx");
      return r;
    }

    function nomCourt() {
      if (fai()) return "FAI";
      if (appel && appel.nom) return appel.nom.toUpperCase().slice(0, 12);
      return "EUX";
    }

    return {
      evenement: surEvenement,
      etat: surEtat,
      console: surConsole,
      tic,
      karaoke,
      /** Un appel est en cours (le voyant EN DIRECT clignote). */
      enDirect: () => !!(appel && !appel.clos && !appel.fini),
      accueil: () => dire("accueil", {}, { prio: 2 }),
      retour: () => dire("retour_antenne", {}, { prio: 2 }),
    };
  }

  /* ------------------------------------------------------------ affichage */
  const MESSAGES = {
    attente: "♪ En attente du coup d'envoi… ♪",
    donnees: "♫ Fin du morceau : place aux octets, qui chantent en silence ♫",
    fin: "♫ Merci, vous avez été formidables ! ♫",
  };

  /** Branche le commentateur sur la façade (bandeau #cast, touche #btn-cast). */
  function monter({ bridge, store }) {
    const $ = (id) => document.getElementById(id);
    const boite = $("cast"), pile = $("cast-pile"), btn = $("btn-cast");
    if (!boite || !pile || !btn) return { actif: () => false };
    let actif = store.get("cast") !== "0";

    const moteur = creer({ sortie: afficher, maintenant: () => performance.now() });
    // Le moteur suit l'appel même micro coupé : au retour, il sait où on en est.
    function afficher(l) {
      if (!actif) return;
      const div = document.createElement("div");
      div.className = "cl neuf" + (l.prio >= 2 ? " fort" : "") + (l.fin ? " fin" : "");
      const t = document.createElement("time");
      t.textContent = l.chrono || "··:··";
      const p = document.createElement("span");
      p.textContent = l.texte;
      div.append(t, p);
      pile.appendChild(div);
      while (pile.childElementCount > 5) pile.firstChild.remove();
      // La pile monte d'un cran (animation relancée)
      pile.classList.remove("monte");
      void pile.offsetWidth;
      pile.classList.add("monte");
    }

    // Karaoké : on ne touche au DOM que si les paroles changent.
    const vx = {
      nous: { el: $("kara-nous"), cle: null },
      eux: { el: $("kara-eux"), cle: null },
    };
    const nomEux = $("kara-eux-nom"), msg = $("kara-msg"), kara = $("kara");
    function rendreVoix(v, p) {
      const cle = p.mode + "|" + (p.cle || "") + "|" + p.txt;
      if (v.cle !== cle) {
        v.cle = cle;
        v.el.classList.toggle("repos", p.mode === "repos");
        v.el.classList.toggle("temps", p.mode === "temps");
        // Nouveau nœud : les animations repartent du début de la ligne.
        const old = v.el.querySelector("b");
        const b = document.createElement("b");
        b.innerHTML = '<i class="balle"><i></i></i>';
        const s = document.createElement("span");
        s.className = "kara-txt";
        s.textContent = p.txt || "…";
        b.appendChild(s);
        old.replaceWith(b);
      }
      if (p.mode === "temps") v.el.style.setProperty("--k", Math.max(0, Math.min(1, p.k)).toFixed(3));
    }
    function rendreKaraoke() {
      const k = moteur.karaoke();
      const m = k.message ? MESSAGES[k.message] : "";
      if (msg.textContent !== m) msg.textContent = m;
      if (msg.hidden !== !m) msg.hidden = !m;
      if (kara.classList.contains("msg") !== !!m) kara.classList.toggle("msg", !!m);
      if (!m) {
        rendreVoix(vx.nous, k.nous);
        rendreVoix(vx.eux, k.eux);
      }
      const nom = "◂ " + k.nomEux;
      if (nomEux.textContent !== nom) nomEux.textContent = nom;
      const direct = moteur.enDirect();
      if (boite.classList.contains("direct") !== direct) boite.classList.toggle("direct", direct);
    }

    bridge.listen("v34:event", ({ side, event }) => moteur.evenement(side, event));
    bridge.listen("v34:console", (c) => moteur.console(c.text || ""));
    bridge.listen("v34:state", (s) => {
      moteur.etat(s);
      if (actif) rendreKaraoke();
    });
    setInterval(() => moteur.tic(), 200);

    function appliquer() {
      boite.hidden = !actif;
      const modem = boite.closest(".modem");
      if (modem) modem.classList.toggle("cast-off", !actif);
      btn.classList.toggle("on", actif);
      btn.setAttribute("aria-pressed", String(actif));
    }
    btn.addEventListener("click", () => {
      actif = !actif;
      store.set("cast", actif ? "1" : "0");
      appliquer();
      if (actif) {
        rendreKaraoke();
        moteur.retour();
      }
    });
    appliquer();
    rendreKaraoke();
    moteur.accueil();
    return { actif: () => actif };
  }

  return { creer, paroles, monter, PHRASES: P, V90 };
})();

if (typeof module !== "undefined" && module.exports) module.exports = Commentateur;
