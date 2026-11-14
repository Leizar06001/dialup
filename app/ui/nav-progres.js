// Suivi du chargement de la page du navigateur, partagé par la barre
// d'outils (animation, jauge dans l'adresse) et la barre d'état (texte).
//
// Sources : les événements de la vue web (`nav:load`, début / fin) et le
// compteur du proxy du tunnel (`v34:state` → `nav`) : octets reçus et, pour
// web 1998, taille du document et nombre d'images annoncés par le FAI.
//
// `suivreChargement(rappel)` appelle `rappel(etat)` à chaque changement, avec
// etat = { texte, part (0..1 ou null), vague (taille inconnue), occupe,
//          https, debit (o/s), enLigne }.
(function () {
  const T = window.__TAURI__;
  const ko = (n) => n < 1024 ? `${n} o` : `${(n / 1024).toLocaleString("fr-FR", { maximumFractionDigits: n < 10240 ? 1 : 0 })} Ko`;
  const interne = (u) => u.includes("hors-ligne.html") || u.startsWith("tauri:") || u.startsWith("http://tauri.localhost");

  window.navKo = ko;
  window.suivreChargement = function (rappel) {
    let load = null;   // { host, v1998, https, seq, rx0, t0, finished, t1 }
    let last = null;   // dernier compteur du proxy
    let online = false;
    const vitesse = { t: 0, rx: 0, v: 0 };
    let avant = "";

    T.event.listen("nav:load", (e) => {
      const { url, done } = e.payload;
      if (interne(url)) { load = null; return calcul(); }
      let u; try { u = new URL(url); } catch (_) { return; }
      if (!done) {
        load = { host: u.hostname, v1998: u.hostname === "web1998", https: u.protocol === "https:",
          seq: last && last.page ? last.page.seq : 0, rx0: last ? last.rx_bytes : 0,
          t0: performance.now(), finished: false, t1: 0 };
      } else if (load) {
        load.finished = true;
        load.t1 = performance.now();
      }
      calcul();
    });

    T.event.listen("v34:state", (e) => {
      const s = e.payload;
      online = !!s.proxy;
      last = s.nav || null;
      const now = performance.now();
      if (now - vitesse.t > 900) {
        const rx = last ? last.rx_bytes : 0;
        const inst = vitesse.t && rx >= vitesse.rx ? (rx - vitesse.rx) / ((now - vitesse.t) / 1000) : 0;
        vitesse.v = vitesse.v * 0.4 + inst * 0.6;
        vitesse.t = now; vitesse.rx = rx;
      }
      calcul();
    });

    function emettre(texte, part, vague, occupe) {
      const etat = { texte, part: part === null ? null : Math.max(0, Math.min(1, part || 0)), vague: !!vague,
        occupe: !!occupe, https: !!(load && load.https), debit: online ? vitesse.v : 0, enLigne: online };
      const cle = JSON.stringify([etat.texte, Math.round((etat.part || 0) * 200), etat.vague, etat.occupe, etat.https, Math.round(etat.debit / 50)]);
      if (cle === avant) return;
      avant = cle;
      rappel(etat);
    }

    function calcul() {
      if (!online && !load) return emettre("Hors ligne", null);
      if (!load) return emettre("Prêt", null);
      const got = last ? Math.max(0, last.rx_bytes - load.rx0) : 0;
      const p = last && last.page && last.page.seq > load.seq ? last.page : null;
      const imagesEnCours = p && load.v1998 && p.active > 0;
      if (load.finished && !imagesEnCours) {
        const secs = (load.t1 - load.t0) / 1000;
        const detail = got > 0 ? ` — ${ko(got)} en ${secs.toLocaleString("fr-FR", { maximumFractionDigits: secs < 10 ? 1 : 0 })} s` : "";
        return emettre(`Document : terminé${detail}`, null);
      }
      if (!online) return emettre("Connexion perdue : rappelez le FAI", null);
      if (load.v1998) {
        if (!p) return got > 0 ? emettre("Hôte contacté. En attente de réponse…", 0.02, false, true)
                               : emettre("Connexion à l'hôte web1998…", 0, false, true);
        const part = p.fraction ?? (p.doc_len ? p.doc_rx / p.doc_len : null);
        if (!p.doc_done) {
          const txt = p.doc_len ? `Document : ${Math.round(100 * p.doc_rx / p.doc_len)} % de ${ko(p.doc_len)}` : `Document : ${ko(p.doc_rx)} reçus`;
          return emettre(txt, part, part === null, true);
        }
        const n = p.elements;
        if (n === null || n === undefined) return emettre(`Images : ${p.done} reçues · ${ko(p.elements_rx)}`, null, true, true);
        if (n === 0) return emettre("Mise en page…", 1, false, true);
        return emettre(`Images : ${Math.min(p.done + 1, n)} sur ${n} · ${ko(p.elements_rx)}`, part, false, true);
      }
      // HTTPS : chiffré, on ne connaît ni la taille ni le nombre d'éléments
      if (got === 0) return emettre(`Connexion à l'hôte ${load.host}…`, null, true, true);
      const c = last ? last.streams : 0;
      emettre(`Transfert des données depuis ${load.host} : ${ko(got)} reçus · ${c} connexion${c > 1 ? "s" : ""}`, null, true, true);
    }
    calcul();
  };
})();
