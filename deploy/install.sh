#!/usr/bin/env bash
# Installation / mise à jour de v34-server sur un VPS Debian / Ubuntu (systemd).
#
# À lancer PAR UN HUMAIN, avec sudo, depuis la racine du dépôt, APRÈS avoir
# compilé sans sudo (cargo build --release -p v34-server) :
#
#   sudo ./deploy/install.sh                      # binaire target/release/v34-server
#   sudo ./deploy/install.sh /chemin/v34-server   # autre binaire (compilation croisée)
#
# Ce que fait le script (on peut le relancer sans risque) :
#   1. crée l'utilisateur système « v34 » (sans connexion, sans maison) ;
#   2. copie le binaire dans /usr/local/bin ;
#   3. installe /etc/v34-server/config.toml depuis l'exemple S'IL N'EXISTE PAS
#      (root:v34, 640) — une configuration existante n'est jamais touchée ;
#   4. installe le service systemd, l'active et le (re)démarre ;
#   5. ouvre UDP 3434 dans ufw s'il est actif, sinon affiche l'état de nftables.
set -euo pipefail

PORT=3434
USER_NAME=v34
CONF_DIR=/etc/v34-server
CONF=$CONF_DIR/config.toml
BIN_DEST=/usr/local/bin/v34-server
UNIT=/etc/systemd/system/v34-server.service

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
bin="${1:-$repo/target/release/v34-server}"

say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

if [ "$(id -u)" -ne 0 ]; then
    echo "Ce script doit être lancé avec sudo : sudo $0 $*" >&2
    exit 1
fi
if [ ! -x "$bin" ]; then
    echo "Binaire introuvable : $bin" >&2
    echo "Compilez d'abord (sans sudo) : cargo build --release -p v34-server" >&2
    exit 1
fi

say "Utilisateur système $USER_NAME"
if id "$USER_NAME" >/dev/null 2>&1; then
    echo "existe déjà"
else
    useradd --system --user-group --no-create-home --home-dir /nonexistent \
            --shell /usr/sbin/nologin "$USER_NAME"
    echo "créé"
fi

say "Binaire → $BIN_DEST"
install -m 755 -o root -g root "$bin" "$BIN_DEST"
"$BIN_DEST" --version

say "Configuration $CONF"
install -d -m 750 -o root -g "$USER_NAME" "$CONF_DIR"
if [ -e "$CONF" ]; then
    echo "existe déjà : conservée telle quelle"
else
    install -m 640 -o root -g "$USER_NAME" "$here/config.example.toml" "$CONF"
    echo "installée depuis l'exemple (annuaire vide pour l'instant)"
fi
"$BIN_DEST" verifier --config "$CONF"

say "Service systemd"
install -m 644 -o root -g root "$here/v34-server.service" "$UNIT"
systemctl daemon-reload
systemctl enable v34-server >/dev/null
systemctl restart v34-server
sleep 1
systemctl --no-pager --lines=5 status v34-server || true

say "Pare-feu : UDP $PORT"
if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q "Status: active"; then
    ufw allow "$PORT/udp" comment "v34-server"
elif command -v nft >/dev/null 2>&1; then
    echo "ufw absent ou inactif. Règles nftables actuelles (chaînes d'entrée) :"
    nft list ruleset 2>/dev/null | grep -E "hook input|policy|dport" || echo "  (aucune règle : rien ne bloque UDP $PORT)"
    echo "Si une chaîne « hook input » a « policy drop », ajoutez par exemple :"
    echo "  nft add rule inet filter input udp dport $PORT accept   (et rendez-le permanent dans /etc/nftables.conf)"
else
    echo "Aucun pare-feu local détecté : rien à faire."
fi
echo "Pensez aussi au pare-feu de l'hébergeur (console web du VPS) : UDP $PORT entrant."

say "Terminé"
cat <<EOF
Prochaines étapes :
  sudo v34-server ajouter-abonne NOM NUMERO --config $CONF   # mot de passe demandé
  sudo v34-server liste-abonnes --config $CONF
  journalctl -u v34-server -f
Adresses de cette machine (à mettre éventuellement dans fai.reseaux_interdits) :
  $(hostname -I 2>/dev/null || echo "?")
EOF
