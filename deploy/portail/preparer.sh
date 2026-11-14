#!/usr/bin/env bash
# Prépare le VPS pour le site d'inscription (conteneur v34-portail).
#
# À lancer PAR UN HUMAIN, avec sudo, depuis la racine du dépôt cloné (par
# exemple /srv/dialup), APRÈS deploy/install.sh (le central doit tourner) :
#
#   sudo ./deploy/portail/preparer.sh dialup.example.org \
#        --admin gaston --admin-courriel moi@example.org \
#        [--smtp-depuis /chemin/vers/un/autre/.env] [--port 8034]
#
# Ce que fait le script (on peut le relancer sans risque) :
#   1. crée /var/lib/v34-server (v34:v34, 750) : le fichier des comptes,
#      écrit par le site et lu par le central ;
#   2. ajoute `comptes = ".../comptes.toml"` à la section [serveur] de
#      /etc/v34-server/config.toml s'il manque, vérifie, redémarre le central ;
#   3. crée .env depuis .env.example s'il n'existe pas (adresse du site,
#      administrateur, utilisateur v34) ; --smtp-depuis recopie les lignes
#      SMTP_HOST/PORT/USER/PASSWORD/SSL d'un autre .env SANS les afficher ;
#   4. ajoute le site à Caddy (/etc/caddy/sites-enabled/dialup.caddy) s'il
#      n'y est pas, valide la configuration, recharge Caddy ;
#   5. construit et démarre le conteneur (docker compose up -d --build), au
#      nom de l'utilisateur qui a lancé sudo.
set -euo pipefail

say() { printf '\033[1m▶ %s\033[0m\n' "$*"; }
die() { printf '✗ %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" = 0 ] || die "à lancer avec sudo"
DOMAINE="${1:-}"
[ -n "$DOMAINE" ] && [ "${DOMAINE#-}" = "$DOMAINE" ] || die "usage : sudo $0 DOMAINE [--admin NOM] [--admin-courriel ADRESSE] [--smtp-depuis FICHIER] [--port N]"
shift
ADMIN="" ; ADMIN_MAIL="" ; SMTP_SRC="" ; PORT=8034
while [ $# -gt 0 ]; do
    case "$1" in
        --admin) ADMIN="${2:?}"; shift 2 ;;
        --admin-courriel) ADMIN_MAIL="${2:?}"; shift 2 ;;
        --smtp-depuis) SMTP_SRC="${2:?}"; shift 2 ;;
        --port) PORT="${2:?}"; shift 2 ;;
        *) die "option inconnue : $1" ;;
    esac
done

here="$(cd "$(dirname "$0")" && pwd)"
racine="$(cd "$here/../.." && pwd)"
CONF=/etc/v34-server/config.toml
ETAT=/var/lib/v34-server
COMPTES="$ETAT/comptes.toml"
QUI="${SUDO_USER:-root}"

id v34 >/dev/null 2>&1 || die "utilisateur v34 absent : lancez d'abord deploy/install.sh"
[ -f "$CONF" ] || die "$CONF absent : lancez d'abord deploy/install.sh"
command -v docker >/dev/null || die "docker absent"

say "1. Répertoire des comptes → $ETAT"
install -d -o v34 -g v34 -m 0750 "$ETAT"

say "2. Le central lit les comptes du site"
if grep -qE '^[[:space:]]*comptes[[:space:]]*=' "$CONF"; then
    echo "   déjà configuré : $(grep -E '^[[:space:]]*comptes[[:space:]]*=' "$CONF")"
else
    cp -p "$CONF" "$CONF.avant-portail"
    if grep -qE '^\[serveur\]' "$CONF"; then
        sed -i "/^\[serveur\]/a comptes = \"$COMPTES\"" "$CONF"
    else
        { printf '[serveur]\ncomptes = "%s"\n\n' "$COMPTES"; cat "$CONF.avant-portail"; } > "$CONF"
    fi
    if v34-server verifier --config "$CONF"; then
        systemctl restart v34-server
        echo "   ajouté (copie de l'ancienne config : $CONF.avant-portail), central redémarré"
    else
        cp -p "$CONF.avant-portail" "$CONF"
        die "configuration refusée par v34-server verifier : ancienne version remise"
    fi
fi

say "3. Réglages du site → $racine/.env"
ENV="$racine/.env"
if [ -f "$ENV" ]; then
    echo "   $ENV existe déjà : inchangé"
else
    install -m 600 -o "$QUI" "$racine/.env.example" "$ENV"
    regle() { # clé valeur : remplace la ligne `clé=` (commentée ou non)
        local k="$1" v="$2"
        if grep -qE "^#?$k=" "$ENV"; then
            # valeur passée par l'environnement d'awk : aucun échappement à craindre
            V="$v" awk -v k="$k" 'BEGIN{FS=OFS="="} $0 ~ "^#?"k"=" {print k"="ENVIRON["V"]; next} {print}' "$ENV" > "$ENV.tmp"
            cat "$ENV.tmp" > "$ENV" && rm -f "$ENV.tmp"
        else
            printf '%s=%s\n' "$k" "$v" >> "$ENV"
        fi
    }
    regle PORTAIL_URL "https://$DOMAINE"
    regle V34_UID "$(id -u v34)"
    regle V34_GID "$(id -g v34)"
    regle PORTAIL_PORT "$PORT"
    [ -n "$ADMIN" ] && regle PORTAIL_ADMIN "$ADMIN"
    [ -n "$ADMIN_MAIL" ] && regle PORTAIL_ADMIN_COURRIEL "$ADMIN_MAIL"
    if [ -n "$SMTP_SRC" ]; then
        [ -r "$SMTP_SRC" ] || die "$SMTP_SRC illisible"
        for k in SMTP_HOST SMTP_PORT SMTP_USER SMTP_PASSWORD SMTP_SSL; do
            v="$(grep -E "^$k=" "$SMTP_SRC" | tail -n1 | cut -d= -f2- || true)"
            regle "$k" "$v"
        done
        u="$(grep -E '^SMTP_USER=' "$ENV" | cut -d= -f2- | tr -d '"')"
        [ -n "$u" ] && regle SMTP_FROM "\"Tinyverse Dialup <$u>\""
        echo "   réglages SMTP recopiés depuis $SMTP_SRC (non affichés)"
    fi
    chown "$QUI" "$ENV"
    chmod 600 "$ENV"
    echo "   créé ; à relire : PORTAIL_ADMIN, PORTAIL_ADMIN_COURRIEL, PORTAIL_HEBERGEUR, SMTP_*"
fi

say "4. Caddy : https://$DOMAINE → 127.0.0.1:$PORT"
if [ -d /etc/caddy/sites-enabled ] && command -v caddy >/dev/null; then
    SITE=/etc/caddy/sites-enabled/dialup.caddy
    if [ -f "$SITE" ]; then
        echo "   $SITE existe déjà : inchangé"
    else
        printf '%s {\n\treverse_proxy 127.0.0.1:%s\n}\n' "$DOMAINE" "$PORT" > "$SITE"
        if caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null 2>&1; then
            systemctl reload caddy
            echo "   site ajouté, Caddy rechargé (certificat HTTPS automatique)"
        else
            rm -f "$SITE"
            die "Caddy refuse la configuration : fichier retiré, rien n'a changé"
        fi
    fi
else
    echo "   Caddy absent : mettez vous-même un proxy HTTPS devant 127.0.0.1:$PORT"
fi

say "5. Conteneur v34-portail"
cd "$racine"
sudo -u "$QUI" docker compose up -d --build
sleep 2
sudo -u "$QUI" docker compose ps
if curl -fsS -o /dev/null "http://127.0.0.1:$PORT/"; then
    echo "✓ site en ligne : https://$DOMAINE"
else
    echo "! le site ne répond pas encore : docker compose logs portail"
fi
