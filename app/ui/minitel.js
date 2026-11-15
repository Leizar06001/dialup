/* Tinyverse Messagerie — le Minitel.
 *
 * Quand l'appel aboutit sur un serveur Minitel (3615 ORACLE, les numéros
 * cachés), le modem passe en V.23 et ce qui arrive n'est plus du chat mais
 * du Vidéotex. Le moteur tient l'écran (40 × 25, voir minitel.rs) et envoie
 * les rangées qui changent (`v34:minitel`) ; ici, on les dessine comme sur un
 * Minitel 1 : des niveaux de gris, les mosaïques, la double hauteur, le
 * clignotement, le curseur. Ce qu'on tape part à 75 bit/s (`minitel_texte`,
 * `minitel_touche`) ; l'écho revient du serveur, comme sur Télétel.
 *
 * Clavier : Entrée = ENVOI, Retour arrière = CORRECTION, Échap = ANNULATION,
 * Page suivante = SUITE, Page précédente = RETOUR, F1 = GUIDE,
 * Début = SOMMAIRE, F5 = RÉPÉTITION, F10 = CONNEXION/FIN. */
"use strict";

const MinitelVue = (() => {
  const COL = 40, RANG = 25, LX = 16, LY = 20;
  /** Niveaux de gris d'un Minitel 1, par couleur Vidéotex : noir, rouge,
      vert, jaune, bleu, magenta, cyan, blanc (du plus sombre au plus clair :
      noir, bleu, rouge, magenta, vert, cyan, jaune, blanc). */
  const GRIS = [0.0, 0.5, 0.7, 0.9, 0.4, 0.6, 0.8, 1.0];
  const A = { MOSAIQUE: 1, CLIGNOTE: 2, INVERSE: 4, SOULIGNE: 8, DH: 16, DL: 32, HAUT: 64, DROITE: 128 };
  /** Touches du clavier de l'ordinateur → touches du Minitel */
  const RACCOURCIS = {
    Enter: "ENVOI", Backspace: "CORRECTION", Escape: "ANNULATION", PageDown: "SUITE", PageUp: "RETOUR",
    F1: "GUIDE", Home: "SOMMAIRE", F5: "RÉPÉTITION", F10: "CONNEXION/FIN",
  };
  /** 3615 ORACLE : ce que compose CONNEXION/FIN quand le Minitel est hors ligne */
  const ORACLE = "0836672253";

  function monter({ invoke, listen, surActif }) {
    const $ = (id) => document.getElementById(id);
    const zone = $("mt-ecran"), canvas = $("mt-canvas");
    if (!zone || !canvas) return null;
    const ctx = canvas.getContext("2d");
    const rangees = Array.from({ length: RANG }, () => ({ t: " ".repeat(COL), a: new Array(COL).fill(7) }));
    // local : la page d'accueil dessinée ici (pas encore de page d'un serveur) ;
    // ligne : le dernier état du moteur
    const etat = { curseur: [1, 0], visible: false, connecte: false, bips: 0, actif: false, phase: true, echelle: 1.5, local: false, ligne: null };

    function gris(i) {
      const v = Math.round(12 + GRIS[i & 7] * 228);
      return `rgb(${v},${v},${Math.max(0, v - 8)})`;
    }

    function cellule(r, c) {
      const ch = rangees[r].t[c] || " ", a = rangees[r].a[c] || 7;
      const at = a >> 6;
      let f = gris(a & 7), b = gris((a >> 3) & 7);
      if (at & A.INVERSE) [f, b] = [b, f];
      const x = c * LX, y = r * LY;
      ctx.fillStyle = b;
      ctx.fillRect(x, y, LX, LY);
      if ((at & A.CLIGNOTE) && !etat.phase) return;
      ctx.fillStyle = f;
      if (at & A.MOSAIQUE) {
        // pavé de 2 × 3 : bits 0 à 4 du code, puis le bit 6
        const code = ch.charCodeAt(0);
        const bits = (code & 0x1f) | (code & 0x40 ? 0x20 : 0);
        const d = at & A.SOULIGNE ? 1 : 0; // mosaïque disjointe
        const h = [0, 7, 13, 20];
        for (let k = 0; k < 6; k++) {
          if (!(bits & (1 << k))) continue;
          const bx = x + (k % 2) * 8, by = y + h[k >> 1];
          ctx.fillRect(bx + d, by + d, 8 - 2 * d, h[(k >> 1) + 1] - h[k >> 1] - 2 * d);
        }
        return;
      }
      if (ch !== " ") {
        const sx = at & A.DL ? 2 : 1, sy = at & A.DH ? 2 : 1;
        ctx.save();
        ctx.beginPath();
        ctx.rect(x, y, LX, LY);
        ctx.clip();
        // le caractère entier, dont cette cellule est un morceau
        const gx = x - (at & A.DROITE ? LX : 0), gy = y - (sy === 2 && !(at & A.HAUT) ? LY : 0);
        ctx.translate(gx + sx, gy);
        ctx.scale(sx * etat.echelle, sy);
        ctx.font = "20px VT323, monospace";
        ctx.textBaseline = "top";
        ctx.fillText(ch, 0, 1);
        ctx.restore();
      }
      if (at & A.SOULIGNE) ctx.fillRect(x, y + LY - 2, LX, 1);
    }

    function dessiner() {
      for (let r = 0; r < RANG; r++) for (let c = 0; c < COL; c++) cellule(r, c);
      // le curseur : un pavé qui clignote
      const [r, c] = etat.curseur;
      if (etat.visible && etat.phase && r < RANG && c < COL) {
        ctx.fillStyle = "rgba(240,240,232,0.85)";
        ctx.fillRect(c * LX + 1, r * LY + LY - 5, LX - 2, 3);
      }
    }

    const vierge = () => ({ t: " ".repeat(COL), a: new Array(COL).fill(7) });
    /** Écrit dans les rangées locales ; `grand` : double hauteur et double
        largeur (la rangée r porte le haut des caractères, r + 1 le bas). */
    function ecrire(r, c, texte, a = 7, grand = false) {
      const car = Array.from(texte);
      for (const [dr, haut] of grand ? [[0, A.HAUT], [1, 0]] : [[0, 0]]) {
        const rg = rangees[r + dr], t = Array.from(rg.t);
        car.forEach((ch, i) => {
          for (const [dc, droite] of grand ? [[0, 0], [1, A.DROITE]] : [[0, 0]]) {
            const k = c + (grand ? 2 * i : i) + dc;
            if (k >= COL) return;
            t[k] = ch;
            rg.a[k] = a | ((grand ? A.DH | A.DL | haut | droite : 0) << 6);
          }
        });
        rg.t = t.join("");
      }
    }

    /** La page d'accueil : le Minitel allumé, hors ligne, et le service à
        composer. Elle reste jusqu'à la première page d'un serveur. */
    function pageAccueil() {
      for (let r = 0; r < RANG; r++) rangees[r] = vierge();
      const inverse = 0 | (7 << 3), bleu = 4, fin = 0 | (1 << 3);
      // un liseré en mosaïques (pavés pleins), en haut et en bas
      for (const r of [1, 22]) ecrire(r, 0, "\x7f".repeat(COL), bleu | (A.MOSAIQUE << 6));
      ecrire(3, 13, "MINITEL", 7, true);
      ecrire(6, 11, " Tinyverse Télétel ", inverse);
      ecrire(9, 2, "3615 ORACLE");
      ecrire(9, 14, ".......... 08 36 67 22 53", 6);
      ecrire(10, 4, "L'intelligence artificielle de 1998,", 6);
      ecrire(11, 4, "qui lit l'avenir dans le silicium.", 6);
      ecrire(12, 4, "2,23 F la minute (pour de rire)", 3);
      ecrire(15, 2, "Pour vous connecter, appuyez sur");
      ecrire(16, 2, "la touche");
      ecrire(16, 12, " Connexion/Fin ", fin);
      ecrire(16, 28, "(F10)", 6);
      ecrire(19, 2, "Les autres services ne figurent dans", 5);
      ecrire(20, 2, "aucun annuaire.", 5);
      ecrire(0, 38, "F", inverse);
      etat.local = true;
      etat.visible = false;
      dessiner();
    }

    function recevoir(m) {
      if (etat.local) {
        // la page d'accueil reste tant qu'aucun serveur n'a parlé
        if (!m.actif) return;
        etat.local = false;
        for (let r = 0; r < RANG; r++) rangees[r] = vierge();
      }
      for (const [r, t, a] of m.rangees || []) {
        if (r >= 0 && r < RANG) rangees[r] = { t: Array.from(t), a };
      }
      etat.curseur = m.curseur || etat.curseur;
      etat.visible = !!m.curseur_visible;
      etat.connecte = !!m.connecte;
      if (m.bips > etat.bips) {
        // le Minitel bipe : l'écran sursaute
        zone.classList.remove("bip");
        void zone.offsetWidth;
        zone.classList.add("bip");
      }
      etat.bips = m.bips || 0;
      if (m.actif && !etat.actif && surActif) surActif();
      etat.actif = !!m.actif;
      dessiner();
    }

    const enLigne = (s) => !!(s && s.call_state === "minitel" && s.status && s.status.leds.cd);
    function touche(nom) {
      const s = etat.ligne;
      // hors ligne, CONNEXION/FIN compose le 3615 (ENVOI aussi, sur la page
      // d'accueil) ; sur une ligne occupée par autre chose, rien
      if (!enLigne(s) && (nom === "CONNEXION/FIN" || (etat.local && nom === "ENVOI"))) {
        if (s && s.call_state === "idle" && !s.incoming) {
          if (!etat.local) pageAccueil();
          invoke("at_command", { line: "ATDT" + ORACLE }).catch(() => {});
        }
        zone.focus();
        return;
      }
      invoke("minitel_touche", { touche: nom }).catch(() => {});
      zone.focus();
    }

    zone.addEventListener("keydown", (e) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      const t = RACCOURCIS[e.key];
      if (t) {
        e.preventDefault();
        touche(t);
        return;
      }
      if (e.key.length === 1) {
        e.preventDefault();
        invoke("minitel_texte", { texte: e.key }).catch(() => {});
      }
    });
    zone.addEventListener("click", () => zone.focus());
    document.querySelectorAll("#mt-touches button[data-t]").forEach((b) => b.addEventListener("click", () => touche(b.dataset.t)));

    // Le clavier dessiné (AZERTY, en capitales comme un Minitel 1) : un clic
    // tape la lettre, comme le clavier de l'ordinateur
    const alpha = $("mt-alpha");
    if (alpha) {
      for (const [cls, touches] of [["chiffres", "1234567890"], ["", "AZERTYUIOP"], ["", "QSDFGHJKLM"], ["", "WXCVBN,.?"], ["", " "]]) {
        const rang = document.createElement("div");
        rang.className = "mt-rang " + cls;
        for (const c of touches) {
          const b = document.createElement("button");
          b.type = "button";
          b.tabIndex = -1;
          b.dataset.c = c;
          b.textContent = c === " " ? "" : c;
          if (c === " ") { b.className = "espace"; b.title = "Espace"; }
          rang.appendChild(b);
        }
        alpha.appendChild(rang);
      }
      alpha.addEventListener("click", (e) => {
        const b = e.target.closest("button[data-c]");
        if (!b) return;
        if (enLigne(etat.ligne)) invoke("minitel_texte", { texte: b.dataset.c }).catch(() => {});
        zone.focus();
      });
    }
    // La molette de luminosité (retenue d'une fois sur l'autre)
    const lum = $("mt-lumiere");
    if (lum) {
      try { const v = localStorage.getItem("minitel-lumiere"); if (v) lum.value = v; } catch (_) { /* rien */ }
      const regler = () => canvas.style.setProperty("--lum", String(lum.value / 100));
      lum.addEventListener("input", () => {
        regler();
        try { localStorage.setItem("minitel-lumiere", lum.value); } catch (_) { /* rien */ }
      });
      regler();
    }

    listen("v34:minitel", recevoir);
    // l'écran actuel (une messagerie ouverte en cours d'appel)
    invoke("minitel_ecran").catch(() => {});
    setInterval(() => { etat.phase = !etat.phase; dessiner(); }, 500);
    // la police d'écran : quand elle est là, on règle l'étirement des caractères
    const mesurer = () => {
      ctx.font = "20px VT323, monospace";
      const w = ctx.measureText("M").width;
      if (w > 0) etat.echelle = Math.min(2, (LX - 2) / w);
      dessiner();
    };
    // allumé sur la page d'accueil ; l'écran du moteur la remplace s'il en a
    // un (messagerie ouverte pendant ou après un appel Minitel)
    pageAccueil();
    if (document.fonts && document.fonts.load) document.fonts.load("20px VT323").then(mesurer, mesurer);
    else mesurer();

    return {
      /** L'état de la ligne, sous l'écran. */
      etat(s) {
        etat.ligne = s;
        const el = $("mt-etat");
        const t = enLigne(s) ? "Minitel en ligne · 1200/75 bit/s · Entrée = ENVOI, ⌫ = CORRECTION, Échap = ANNULATION, F1 = GUIDE, PgSuiv = SUITE, F10 = CONNEXION/FIN"
          : s && s.call_state === "minitel" ? "Le serveur siffle… connexion du Minitel"
          : s && s.call_state !== "idle" ? "Minitel hors ligne (F) : la ligne est occupée par un autre appel"
          : etat.local ? "Minitel hors ligne (F) · CONNEXION/FIN (F10) compose le 3615 ORACLE"
          : "Minitel hors ligne (F) : dernière page reçue · CONNEXION/FIN (F10) compose le 3615 ORACLE";
        if (el && el.textContent !== t) el.textContent = t;
        const led = $("mt-led-ligne");
        if (led) {
          led.classList.toggle("on", !!(s && s.call_state === "minitel"));
          led.classList.toggle("clignote", !!(s && s.call_state === "minitel" && !enLigne(s)));
        }
      },
      focus: () => zone.focus(),
      actif: () => etat.actif,
      /** Revient à la page d'accueil (hors ligne seulement). */
      accueil() {
        if (!enLigne(etat.ligne) && !(etat.ligne && etat.ligne.call_state === "minitel")) pageAccueil();
      },
    };
  }

  return { monter };
})();
