//! Pages légales : mentions légales (LCEN), conditions générales
//! d'utilisation et politique de confidentialité (RGPD).
//!
//! L'éditeur, l'adresse de contact et l'hébergeur viennent des variables
//! `PORTAIL_EDITEUR`, `PORTAIL_CONTACT` et `PORTAIL_HEBERGEUR` ; les valeurs
//! par défaut conviennent à un particulier qui héberge son propre central.

use std::fmt::Write;

use super::pages::esc;

/// Date de la version en vigueur des conditions et de la politique de
/// confidentialité.
pub const VERSION: &str = "15 novembre 2026";

/// Ce que les pages légales affichent sur l'éditeur du site.
pub struct Editeur<'a> {
    /// Nom du fournisseur d'accès (PORTAIL_NOM)
    pub nom: &'a str,
    pub editeur: &'a str,
    /// Adresse de contact (peut être vide)
    pub contact: &'a str,
    /// Hébergeur, une ligne par élément (peut être vide)
    pub hebergeur: &'a str,
    /// Dépôt GitHub du code source (peut être vide)
    pub depot: &'a str,
}

impl Editeur<'_> {
    fn contact(&self) -> String {
        if self.contact.is_empty() {
            "<i>adresse non renseignée par l'opérateur</i>".into()
        } else {
            format!(r#"<a href="mailto:{c}">{c}</a>"#, c = esc(self.contact))
        }
    }
}

const VOIR_AUSSI: &str = r#"<p class="aide">Voir aussi : <a href="/mentions">mentions légales</a> · <a href="/conditions">conditions générales</a> · <a href="/confidentialite">politique de confidentialité</a></p>"#;

pub fn mentions(e: &Editeur) -> String {
    let hebergeur = if e.hebergeur.trim().is_empty() {
        "<i>Hébergeur non renseigné : demandez-le à l'adresse de contact ci-dessus.</i>".to_string()
    } else {
        esc(e.hebergeur.trim()).replace('\n', "<br>")
    };
    let source = if e.depot.is_empty() { String::new() } else { format!(r#" Code source : <a href="https://github.com/{d}">github.com/{d}</a>."#, d = esc(e.depot)) };
    let mut h = String::new();
    let _ = write!(
        h,
        r#"<h2>Mentions légales</h2>
<p class="aide">Informations prévues par la loi n° 2004-575 du 21 juin 2004 pour la confiance dans l'économie numérique (LCEN).</p>
<h3>Éditeur du site</h3>
<p>{editeur}<br>Contact : {contact}<br>Directeur de la publication : l'éditeur.</p>
<h3>Hébergeur</h3>
<p>{hebergeur}</p>
<h3>Le service</h3>
<p>{nom} est un fournisseur d'accès <i>pour de rire</i> : un central téléphonique simulé sur Internet, où un modem V.34 logiciel chante comme en 1998. Il est gratuit, sans publicité, et ne permet pas d'appeler de vrais numéros de téléphone (ni les numéros d'urgence). Le logiciel, le central et ce site sont des logiciels libres, publiés sous licence MIT.{source}</p>
<h3>Contenus publiés par les visiteurs</h3>
<p>Les messages du livre d'or sont relus avant publication. Les articles et commentaires de Tinyblog, lisibles seulement par les abonnés connectés au fournisseur d'accès, ne le sont pas : leurs auteurs en sont responsables. Pour signaler un contenu illicite, écrivez à {contact} en indiquant la page, le message concerné et la raison : il sera retiré rapidement s'il y a lieu.</p>
<h3>Marques et liens</h3>
<p>Les badges du bas de page sont des clins d'œil aux années 1990, dessinés en CSS : Netscape, le Bloc-notes et les autres marques citées appartiennent à leurs propriétaires, qui n'ont rien demandé. Les sites du webring sont indépendants de {nom}, qui ne contrôle pas leur contenu.</p>
{VOIR_AUSSI}"#,
        editeur = esc(e.editeur),
        contact = e.contact(),
        nom = esc(e.nom),
    );
    h
}

pub fn conditions(e: &Editeur) -> String {
    format!(
        r#"<h2>Conditions générales d'utilisation</h2>
<p class="aide">Version du {VERSION}. Elles s'appliquent dès l'inscription. Les lire prend moins de temps que de télécharger une image de 50 Ko à 28 800 bit/s.</p>
<h3>1. Le service</h3>
<p>{nom} vous attribue un numéro de téléphone sur un central téléphonique simulé, et fournit un modem V.34 logiciel pour appeler les autres abonnés, un salon de discussion et, sur décision de l'opérateur, un accès à Internet à travers le modem. C'est un projet de passionné, pas un opérateur de télécommunications : on ne peut pas appeler de vrais numéros, ni les services d'urgence.</p>
<h3>2. Gratuit, et sans garantie</h3>
<p>Le service est <b>gratuit</b> et fourni <b>« en l'état »</b>, sans garantie de disponibilité, de débit, de conservation des données ni de bon fonctionnement. Il peut être interrompu, modifié ou arrêté à tout moment, sans préavis ni indemnité. La ligne grésille parfois : c'est d'époque. Dans les limites permises par la loi, l'opérateur n'est pas responsable des dommages liés à son utilisation.</p>
<h3>3. Inscription et compte</h3>
<p>Une inscription par personne, avec une adresse électronique valable que vous consultez. Vous êtes responsable de votre mot de passe et de ce qui est fait avec votre compte. Votre nom d'abonné et votre numéro sont visibles des autres abonnés, dans l'annuaire du logiciel.</p>
<h3>4. Usage légal uniquement</h3>
<p>Vous vous engagez à utiliser le service dans le respect de la loi et des droits d'autrui. Sont notamment interdits : toute activité illégale, l'envoi de messages non sollicités (spam), les attaques et balayages de réseaux ou de serveurs, la diffusion de contenus illicites, haineux ou portant atteinte à la vie privée, le contournement des limites techniques du service et le harcèlement des autres abonnés, au salon comme au téléphone.</p>
<h3>5. Accès Internet</h3>
<p>L'accès Internet est accordé au cas par cas par l'opérateur. Votre trafic sort sur Internet avec <b>l'adresse IP du serveur de l'opérateur</b> : vu de l'extérieur, c'est lui qui navigue, et c'est lui qu'on viendra voir en cas d'abus. Il peut donc limiter les destinations (ports web 80 et 443 seulement, réseaux privés interdits) et <b>retirer l'accès à tout moment</b>, sans préavis ni justification.</p>
<h3>6. Ne monopolisez pas la ligne</h3>
<p>Les modems du FAI sont en petit nombre et partagés entre tous les abonnés : raccrochez quand vous avez fini, ne laissez pas un appel ouvert pour rien et n'automatisez pas les appels. Comme en 1998, quand tout le monde surfe en même temps, ça sonne occupé. L'opérateur peut couper un appel qui s'éternise.</p>
<h3>7. Suspension et résiliation</h3>
<p>En cas d'abus, de manquement à ces conditions ou de danger pour le service, l'opérateur peut suspendre ou supprimer un compte et retirer l'accès Internet, sans préavis. Vous pouvez résilier à tout moment depuis <a href="/compte">votre espace abonné</a> : le compte est supprimé et le numéro libéré. Frais de résiliation : un pincement au cœur.</p>
<h3>8. Livre d'or</h3>
<p>Les messages sont relus avant publication, et peuvent être refusés ou retirés à tout moment. Vous restez responsable de ce que vous écrivez ; en l'envoyant, vous acceptez qu'il soit affiché sur le site sous votre pseudo.</p>
<h3>8 bis. Tinyblog et messages laissés sur le réseau</h3>
<p>Vous restez responsable de ce que vous publiez sur Tinyblog (articles, images, commentaires) et de ce que vous laissez sur les services du central (messages, scores) : rien d'illicite, de haineux, ni de données personnelles d'autrui. Ces contenus s'affichent sous votre nom d'abonné. L'opérateur peut les retirer à tout moment, et suspendre un compte en cas d'abus.</p>
<h3>9. Données personnelles</h3>
<p>Voir la <a href="/confidentialite">politique de confidentialité</a>.</p>
<h3>10. Modifications, droit applicable</h3>
<p>Ces conditions peuvent évoluer ; la version en vigueur est celle de cette page. Elles sont soumises au droit français. Une question ? {contact}.</p>
{VOIR_AUSSI}"#,
        nom = esc(e.nom),
        contact = e.contact(),
    )
}

pub fn confidentialite(e: &Editeur) -> String {
    format!(
        r#"<h2>Politique de confidentialité</h2>
<p class="aide">Version du {VERSION}. En bref : on garde le strict nécessaire pour faire marcher le téléphone, on ne vend rien, on ne piste personne.</p>
<h3>Responsable du traitement</h3>
<p>{editeur} — contact : {contact}.</p>
<h3>Les données traitées</h3>
<table class="grille">
<tr><th>Données</th><th>Pour quoi faire</th><th>Combien de temps</th></tr>
<tr><td>Nom d'abonné, adresse électronique, numéro, <b>empreinte</b> du mot de passe (PBKDF2-SHA-256 salé : jamais le mot de passe lui-même), date d'inscription, décision d'accès Internet, suspension éventuelle par l'opérateur</td><td>Créer et faire fonctionner votre abonnement (connexion au central, annuaire, accès Internet) ; vous écrire au sujet de votre compte</td><td>Inscription non confirmée : <b>48 heures</b>.<br>Compte : <b>jusqu'à sa suppression</b>, par vous ou par l'opérateur</td></tr>
<tr><td>Empreinte des liens envoyés par e-mail (activation, mot de passe oublié)</td><td>Vérifier qu'un lien est bien le vôtre</td><td>48 heures (activation) ou 1 heure (mot de passe), effacée dès l'usage</td></tr>
<tr><td>Adresse IP</td><td>Sécurité et lutte contre les abus : journaux du serveur, limites du nombre d'essais (inscriptions, connexions, messages)</td><td>Journaux : selon la durée de rétention du système du serveur.<br>Limites anti-abus : en mémoire, 24 heures au plus, oubliées au redémarrage</td></tr>
<tr><td>Pseudo, message et date (livre d'or)</td><td>Publier votre message après relecture</td><td>Jusqu'à sa suppression : sur demande, ou quand les plus récents le poussent hors du livre</td></tr>
<tr><td><b>Facture et succès</b> : tickets de vos appels aboutis (numéro composé, correspondant, début, durée, prix fictif en francs), succès obtenus, jours de connexion au central</td><td>Afficher votre facture (jamais réclamée) et vos succès, et le classement des plus grosses factures entre abonnés</td><td>Tickets : un fichier par mois sur le serveur ; succès : sur le serveur. Supprimés sur demande</td></tr>
<tr><td><b>Tinyblog</b> : vos articles, images, commentaires et « j'kiffe », sous votre nom d'abonné</td><td>Les publier pour les abonnés connectés au fournisseur d'accès</td><td>Jusqu'à ce que vous les supprimiez (ou l'opérateur)</td></tr>
<tr><td><b>Services du central</b> : messages que vous y laissez et meilleurs scores des jeux, sous votre nom d'abonné</td><td>Les faire lire aux autres abonnés</td><td>Les derniers messages seulement (les plus anciens s'effacent) ; scores : jusqu'à suppression sur demande. L'opérateur peut aussi retirer un message ou un score</td></tr>
<tr><td><b>Central téléphonique</b> : présence (abonné connecté, adresse IP et port du logiciel), appels (qui appelle quel numéro, début et fin), entrées et sorties du salon</td><td>Acheminer les appels, diagnostiquer les pannes, lutter contre les abus ; montrer à l'opérateur, sur sa page d'administration, qui est en ligne et qui appelle qui</td><td>Présence et appels en cours : en mémoire, et dans le petit fichier d'état que le central réécrit chaque seconde (noms et numéros, sans adresse IP), le temps de la connexion ou de l'appel.<br>Journaux : selon la durée de rétention du système</td></tr>
<tr><td><b>Accès Internet</b> : destinations ouvertes par le tunnel (nom du site et port)</td><td>Diagnostic</td><td>Journalisées <b>seulement en mode débogage</b> ; les tentatives refusées ou en échec sont notées dans le journal ordinaire</td></tr>
<tr><td><b>3615 ORACLE</b> : vos questions et les réponses de la consultation en cours ; nombre de questions posées dans la journée</td><td>Fabriquer la réponse : le texte des questions (et des échanges précédents du même appel) est <b>transmis à OpenRouter et au fournisseur du modèle d'IA</b> (voir ci-dessous) ; compter les questions pour appliquer les quotas</td><td>Questions et réponses : en mémoire, <b>le temps de l'appel</b>, jamais journalisées ni enregistrées par le central ; une réponse à une première question identique peut resservir, sans nom d'abonné, 6 heures au plus.<br>Nombre de questions : en mémoire, jusqu'au lendemain</td></tr>
</table>
<p>Le contenu de vos communications (données échangées par le modem, fichiers, images du visiophone, contenu des pages, messages du salon, questions posées à 3615 ORACLE) n'est jamais enregistré par le central.</p>
<p><b>Bases légales</b> : l'exécution du service que vous demandez en vous inscrivant (article 6.1.b du RGPD) ; pour la sécurité et les journaux, l'intérêt légitime de l'opérateur à protéger son service et son adresse IP (article 6.1.f). Aucune décision automatisée, aucun profilage, aucune publicité, aucune revente.</p>
<h3>Destinataires et sous-traitants</h3>
<p>Vos données ne sont lues que par l'opérateur du central. Votre nom d'abonné et votre numéro sont visibles des autres abonnés (annuaire du logiciel). Interviennent aussi :</p>
<ul>
<li>l'<b>hébergeur</b> du serveur (voir les <a href="/mentions">mentions légales</a>) ;</li>
<li>le <b>serveur d'envoi d'e-mails</b> (SMTP) de l'opérateur, qui achemine les courriers d'activation, de mot de passe et d'accès Internet : il voit votre adresse et le contenu de ces messages ;</li>
<li><b>GitHub</b> (Microsoft, États-Unis), qui héberge les fichiers du logiciel : le site interroge GitHub sans rien lui transmettre sur vous, mais quand vous cliquez sur un téléchargement, votre navigateur se connecte directement à GitHub, qui reçoit votre adresse IP selon sa propre politique de confidentialité ;</li>
<li><b>OpenRouter</b> (États-Unis) et le <b>fournisseur du modèle d'IA</b> (DeepSeek, ou l'hébergeur de ce modèle choisi par OpenRouter), pour 3615 ORACLE seulement : ils reçoivent le texte de vos questions et des échanges précédents de la consultation, pour produire la réponse, et les traitent selon leurs propres politiques de confidentialité et de conservation. Ils ne reçoivent ni votre nom d'abonné, ni votre numéro, ni votre adresse IP (c'est le central qui les interroge) : n'écrivez rien de personnel dans vos questions. Sans clé d'accès configurée par l'opérateur, ou quand l'oracle « tire les cartes », rien ne leur est envoyé.</li>
</ul>
<p>Les sites du webring sont des sites tiers : les visiter leur donne votre adresse IP, comme n'importe quel lien.</p>
<h3>Cookies</h3>
<p>Ce site n'utilise que des cookies <b>strictement nécessaires</b> : <code>v34abo</code> (votre session dans l'espace abonné) et <code>v34adm</code> (session d'administration), déposés seulement quand vous vous connectez, et valables 12 heures au plus. Pas de mesure d'audience, pas de pisteur, pas de publicité : donc pas de bandeau de consentement à cliquer. Le compteur de visites n'est qu'un nombre en mémoire.</p>
<h3>Vos droits</h3>
<p>Vous disposez d'un droit d'accès, de rectification, d'effacement, de limitation, de portabilité et d'opposition. <a href="/compte">Votre espace abonné</a> montre ce que le site garde sur vous et permet de supprimer votre compte vous-même ; pour tout le reste, écrivez à {contact}. Vous pouvez aussi adresser une réclamation à la <a href="https://www.cnil.fr/fr/plaintes">CNIL</a> (3 place de Fontenoy, TSA 80715, 75334 Paris Cedex 07).</p>
<h3>Sécurité</h3>
<p>Site en HTTPS ; mots de passe conservés sous forme d'empreinte seulement, et jamais transmis au central (le logiciel prouve qu'il le connaît, façon SCRAM) ; liens d'e-mail à usage unique ; fichiers des comptes lisibles par le seul serveur.</p>
{VOIR_AUSSI}"#,
        editeur = esc(e.editeur),
        contact = e.contact(),
    )
}
