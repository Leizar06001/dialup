/* Tinyverse Messagerie — mode MESSAGERIE du navigateur : chat (ou salon du
 * FAI), fichiers, visiophone V.Hallucination et contacts du central (le
 * même annuaire que dans la fenêtre du modem, pour appeler d'ici).
 * Les événements du moteur (`v34:state`, `v34:file`, `v34:visio`) arrivent
 * ici comme dans la fenêtre du modem ; le chat passe par le journal de
 * l'appli (`messagerie:chat`, numéroté), qui garde aussi ce qui s'est dit
 * avant l'ouverture de la fenêtre. */
"use strict";

const T = window.__TAURI__;
const $ = (id) => document.getElementById(id);
const invoke = (cmd, args) => (T ? T.core.invoke(cmd, args) : Promise.reject("hors de l'appli"));
const listen = (ev, cb) => (T ? T.event.listen(ev, (e) => cb(e.payload)) : Promise.resolve(() => {}));

const fr1 = (x) => x.toFixed(1).replace(".", ",");
const fr2 = (x) => x.toFixed(2).replace(".", ",");
const frInt = (x) => Math.round(x).toLocaleString("fr-FR");
/** « 0142424242 » → « 01 42 42 42 42 » */
const fmtNum = (n) => (n ? String(n).replace(/(\d{2})(?=\d)/g, "$1 ") : "");
/** Ne réécrit que si le texte change (sinon WebKit perd les clics en cours). */
function setText(id, txt) {
  const el = $(id);
  if (el.textContent !== txt) el.textContent = txt;
}

let state = null; // dernier état du moteur

/* ---------------------------------------------------------------- chat */
const bbsOut = $("bbs-out");
let remoteLine = null;
function bbsLine(cls, who, text) {
  const div = document.createElement("div");
  div.className = cls;
  if (who) {
    const b = document.createElement("b");
    b.textContent = who + " ";
    div.appendChild(b);
  }
  div.appendChild(document.createTextNode(text));
  bbsOut.appendChild(div);
  while (bbsOut.childElementCount > 400) bbsOut.firstChild.remove();
  bbsOut.scrollTop = bbsOut.scrollHeight;
  return div;
}
bbsLine("sys", "", "*** Bienvenue ! Appelle un abonné (contacts ci-contre, annuaire ou terminal du modem), puis discute ici une fois connecté.");
bbsLine("sys", "", "*** En boucle locale, le modem distant te renvoie tes messages (écho)… et tes images. Au 08 36 67 22 53, c'est 3615 ORACLE qui répond.");

function onChat({ from, text }) {
  // Une ligne à nous (ou du système) clôt la ligne distante en cours : la
  // suite du distant repart en dessous, sur une nouvelle ligne.
  if (from === "local") { remoteLine = null; bbsLine("me", "<moi>", text); return; }
  if (from === "system") { remoteLine = null; bbsLine("sys", "", "*** " + text); return; }
  if (from === "salon") {
    // « <nom> texte » : message d'un autre abonné du salon du FAI
    const m = /^<([^>]{1,40})> ?(.*)$/.exec(text);
    if (m) bbsLine("them", `<${m[1]}>`, m[2]); else bbsLine("them", "", text);
    return;
  }
  // Flux distant : on assemble jusqu'au retour à la ligne (3615 ORACLE
  // écrit ainsi ses réponses au fil de l'eau, chaque ligne précédée d'un
  // retour à la ligne). La ligne n'est créée qu'au premier caractère.
  const parts = text.split("\n");
  parts.forEach((part, i) => {
    if (part) {
      if (!remoteLine) remoteLine = bbsLine("them", "<distant>", "");
      remoteLine.lastChild.textContent += part;
    }
    if (i < parts.length - 1) remoteLine = null;
  });
  bbsOut.scrollTop = bbsOut.scrollHeight;
}

/** Messages lus : la fenêtre du modem éteint son voyant « nouveau message ». */
function seen() {
  if (T && document.hasFocus()) T.event.emit("messagerie:vu").catch(() => {});
}
window.addEventListener("focus", seen);

/* Historique d'abord (ce qui s'est dit avant l'ouverture), puis le direct ;
   les numéros évitent les doublons entre les deux. */
(async () => {
  let lastSeq = 0, pending = [], ready = false;
  await listen("messagerie:chat", (m) => {
    if (!ready) { pending.push(m); return; }
    if (m.seq > lastSeq) { lastSeq = m.seq; onChat(m); seen(); }
  });
  try {
    const h = await invoke("messagerie_historique");
    for (const m of h.chat || []) { onChat(m); lastSeq = Math.max(lastSeq, m.seq || 0); }
    if (h.file) onFile(h.file);
  } catch (_) { /* hors de l'appli : pas d'historique */ }
  for (const m of pending) if (m.seq > lastSeq) { lastSeq = m.seq; onChat(m); }
  pending = null;
  ready = true;
})();

$("bbs-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const inp = $("bbs-input");
  if (!inp.value) return;
  invoke("send_chat", { text: inp.value }).catch((err) => bbsLine("sys", "", "*** " + err));
  inp.value = "";
});

/* ---------------------------------------------------------------- fichiers */
$("file-input").addEventListener("change", async (e) => {
  const f = e.target.files[0];
  e.target.value = "";
  if (!f) return;
  if (f.size > 8 * 1024 * 1024) {
    bbsLine("sys", "", `*** « ${f.name} » est trop gros (max 8 Mo) — c'est un modem 33,6k !`);
    return;
  }
  $("file-label").textContent = `↑ ${f.name} · 0 %`;
  $("file-progress").style.width = "0%";
  try {
    const buf = new Uint8Array(await f.arrayBuffer());
    await invoke("send_file", { name: f.name, data: Array.from(buf) });
  } catch (err) {
    bbsLine("sys", "", "*** " + err);
    $("file-label").textContent = "aucun transfert";
  }
});

// Progression des transferts (↑ envoi, ↓ réception)
function onFile({ dir, name, done, total, rate, state: st }) {
  const pct = total ? Math.min(100, (100 * done) / total) : 0;
  const arrow = dir === "tx" ? "↑" : "↓";
  $("file-progress").style.width = pct.toFixed(1) + "%";
  $("file-progress").classList.toggle("err", st === "error");
  const what = st === "done" ? "terminé" : st === "error" ? "ERREUR" : `${pct.toFixed(0)} %`;
  $("file-label").textContent = `${arrow} ${name} · ${what}`;
  $("file-rate").textContent = `${fr2(rate)} Ko/s`;
}
listen("v34:file", onFile);

/* ---------------------------------------------------------------- contacts (l'annuaire du central) */
$("dir-list").addEventListener("click", (e) => {
  const b = e.target.closest("button[data-num]");
  if (!b) return;
  // le numéro s'affiche aussi dans le terminal du modem
  if (T) T.event.emit("messagerie:compose", b.dataset.num).catch(() => {});
  invoke("at_command", { line: "ATDT" + b.dataset.num }).catch((err) => bbsLine("sys", "", "*** " + err));
});
let dirKey = null, dirAsked = false;
/** Noms des numéros spéciaux annoncés par le central (service → nom) */
const DIR_NAMES = { fai: "Fournisseur d'accès (Internet)", echo: "Ligne de test (écho)", oracle: "3615 ORACLE · voyance 1998" };
const ORACLE_NUMBER = "0836672253";
function renderDir(net) {
  // Ne reconstruire la liste que si elle change (sinon les clics se perdent).
  const key = JSON.stringify(net ? [net.registered, net.number, net.directory, net.services, net.server, net.isp_number] : null);
  if (key === dirKey) return;
  dirKey = key;
  if (net && net.registered && !dirAsked) { dirAsked = true; invoke("net_directory").catch(() => {}); }
  const list = $("dir-list");
  list.textContent = "";
  const esc = (t) => String(t).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  // Le FAI en tête, comme dans l'annuaire du modem
  const svcAll = (net && net.services) || [];
  const isp = (svcAll.find(([, kind]) => kind === "fai") || [(net && net.isp_number) || "0836154286"])[0];
  const fai = `<div class="dir-row online"><i class="dot"></i><span class="who">TINYVERSE <small>· accès Internet</small></span><span class="num">${fmtNum(isp)}</span><button data-num="${esc(isp)}">☎ Appeler</button></div>`;
  if (!net || !net.registered) {
    setText("dir-title", "Contacts");
    setText("dir-count", "");
    // En boucle locale, le modem simulé joue aussi l'oracle (hors ligne : les cartes)
    const oracle = net ? "" : `<div class="dir-row online"><i class="dot"></i><span class="who">${DIR_NAMES.oracle} <small>· simulé</small></span><span class="num">${fmtNum(ORACLE_NUMBER)}</span><button data-num="${ORACLE_NUMBER}">☎ Appeler</button></div>`;
    list.innerHTML = fai + oracle + `<div class="dir-note">${net ? "Connexion au central en cours…" :
      "Tu es en <b>boucle locale</b> : pas de central, donc pas d'autres abonnés. Tous les numéros répondent ici (modem simulé) : compose-les dans le terminal du modem.<br><br>Le badge <b>BOUCLE LOCALE</b> de la fenêtre du modem te connecte à ton central."}</div>`;
    return;
  }
  setText("dir-title", `Contacts · ${net.server.replace(/:\d+$/, "")}`);
  const dir = net.directory || [];
  const on = dir.filter((d) => d.status !== "absent").length;
  setText("dir-count", dir.length ? `· ${on}/${dir.length} en ligne` : "");
  const label = { online: "en ligne", busy: "occupé", absent: "absent" };
  const order = { online: 0, busy: 1, absent: 2 };
  const rows = [...dir].sort((a, b) => (a.number === net.number ? -1 : b.number === net.number ? 1 : order[a.status] - order[b.status] || a.name.localeCompare(b.name)));
  let html = fai;
  for (const d of rows) {
    const me = d.number === net.number;
    const action = me ? `<span class="state">c'est toi</span>`
      : d.status === "online" ? `<button data-num="${esc(d.number)}">☎ Appeler</button>`
      : `<span class="state">${label[d.status]}</span>`;
    html += `<div class="dir-row ${d.status}"><i class="dot"></i><span class="who">${esc(d.name)}${me ? " <small>(toi)</small>" : ""}</span><span class="num">${fmtNum(d.number)}</span>${action}</div>`;
  }
  if (!dir.length) html += `<div class="dir-note">Le central n'a pas encore envoyé l'annuaire (ou il est trop ancien : mets-le à jour).</div>`;
  const svc = svcAll.filter(([, kind]) => kind !== "fai");
  if (svc.length) {
    html += `<div class="dir-sep">Numéros spéciaux</div>`;
    for (const [num, kind] of svc) {
      html += `<div class="dir-row online"><i class="dot"></i><span class="who">${esc(DIR_NAMES[kind] || kind)}</span><span class="num">${fmtNum(num)}</span><button data-num="${esc(num)}">☎ Appeler</button></div>`;
    }
  }
  list.innerHTML = html;
}
// Les contacts se rafraîchissent à chaque retour sur la messagerie.
window.addEventListener("focus", () => { if (state && state.net && state.net.registered) invoke("net_directory").catch(() => {}); });

/* ---------------------------------------------------------------- visiophone V.Hallucination
   La caméra (ou la mire) est lue dans un canevas de 64 × 48 ; le moteur Rust
   redessine chaque image en une quarantaine de formes (le codec des images
   du web 1998) et l'envoie quand la ligne le permet. Rien n'est filmé tant
   que la source est sur OFF, réglage d'usine (jamais mémorisé). */
const VISIO_W = 64, VISIO_H = 48;
const visio = {
  src: "off",       // "off" | "cam" | "mire"
  want: 0,          // jeton : la dernière demande de source gagne
  stream: null, video: null, ctx: null,
  busy: false,      // une image en cours de calcul
  last: 0,          // dernière image prise (ms)
  lastSent: 0,      // dernière image confiée au moteur (ms)
  enc: null,        // dernier aperçu : octets, formes, temps de calcul
  rxAt: -1e9,       // dernière image reçue (ms)
  remote: false,    // une image du correspondant est à l'écran
  err: "",          // erreur de caméra (prioritaire sur les autres messages)
  t0: performance.now(),
  osd: null,
};

/** Affiche un SVG dans un écran, en fondu enchaîné avec le précédent. */
function visioShow(id, svg) {
  const box = $(id), imgs = box.querySelectorAll("img");
  const front = box.dataset.front === "1" ? 1 : 0, back = imgs[1 - front];
  back.onload = () => {
    back.classList.add("on");
    imgs[front].classList.remove("on");
    box.dataset.front = String(1 - front);
  };
  back.src = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svg);
}
function visioBlank(id) {
  $(id).querySelectorAll("img").forEach((i) => { i.onload = null; i.classList.remove("on"); i.removeAttribute("src"); });
}

function visioStop() {
  if (visio.stream) visio.stream.getTracks().forEach((t) => t.stop());
  visio.stream = null;
  if (visio.video) visio.video.srcObject = null;
}

/** Message clair pour une caméra refusée, absente ou occupée. */
function visioCamError(e) {
  const n = (e && e.name) || "";
  if (n === "NotAllowedError" || n === "SecurityError" || n === "PermissionDeniedError")
    return /Mac OS X|Macintosh/.test(navigator.userAgent)
      ? "Caméra refusée. Sur Mac : Réglages Système › Confidentialité et sécurité › Caméra, coche V34 Modem, puis relance l'appli. En attendant : la MIRE."
      : "Caméra refusée par le système : autorise la caméra dans les réglages de confidentialité du bureau (KDE, GNOME…), puis réessaie. En attendant : la MIRE.";
  if (n === "NotFoundError" || n === "DevicesNotFoundError" || n === "OverconstrainedError")
    return "Pas de caméra sur ce PC. Monsieur Mire peut la remplacer : MIRE.";
  if (n === "NotReadableError" || n === "TrackStartError" || n === "AbortError")
    return "Caméra occupée par une autre appli (une visio de bureau ?). Ferme-la, ou essaie la MIRE.";
  return `Caméra indisponible (${n || e}) : essaie la MIRE.`;
}

function visioButtons(src) {
  document.querySelectorAll("#visio-src button").forEach((b) => b.classList.toggle("on", b.dataset.src === src));
}

/** Change de source : "off", "cam" (webcam) ou "mire". */
async function visioSource(src) {
  const token = ++visio.want;
  visioStop();
  visio.err = "";
  visio.src = "off"; // plus rien ne part de l'ancienne source pendant l'ouverture
  visioButtons(src);
  if (src === "cam") {
    const md = navigator.mediaDevices;
    if (!T) {
      visio.err = "Visiophone : seulement dans l'appli (le codec tourne côté Rust).";
      src = "off";
    } else if (!md || !md.getUserMedia) {
      // vue web sans accès aux caméras (Linux sans WebRTC, contexte non sécurisé…)
      visio.err = "Cette vue web n'a pas accès aux caméras" + (window.isSecureContext === false ? " (contexte non sécurisé)" : "") + " : essaie la MIRE.";
      src = "off";
    } else {
      setText("visio-msg", "Ouverture de la caméra…");
      try {
        // Souhaits seulement (une contrainte stricte écarte les caméras qui ne
        // savent faire que 30 images/s) ; en dernier recours, n'importe laquelle.
        const stream = await md.getUserMedia({ audio: false, video: { width: { ideal: 160 }, height: { ideal: 120 }, frameRate: { ideal: 10 } } })
          .catch((e) => (e && e.name === "OverconstrainedError" ? md.getUserMedia({ audio: false, video: true }) : Promise.reject(e)));
        if (token !== visio.want) { stream.getTracks().forEach((t) => t.stop()); return; }
        if (!visio.video) {
          visio.video = document.createElement("video");
          visio.video.muted = true;
          visio.video.playsInline = true;
        }
        visio.stream = stream;
        visio.video.srcObject = stream;
        await visio.video.play().catch(() => {});
        // caméra débranchée, ou reprise par le système
        stream.getVideoTracks().forEach((t) => t.addEventListener("ended", () => {
          if (visio.stream !== stream) return;
          visioSource("off").then(() => { visio.err = "La caméra s'est arrêtée (débranchée ?)."; });
        }));
      } catch (e) {
        if (token !== visio.want) return;
        visio.err = visioCamError(e);
        src = "off";
      }
    }
  }
  if (token !== visio.want) return;
  visio.src = src;
  visio.enc = null;
  visioButtons(src);
  const pip = $("visio-pip");
  pip.hidden = src === "off";
  pip.classList.toggle("mirror", src === "cam"); // on se voit comme dans un miroir
  if (src === "off") visioBlank("visio-self");
  invoke("visio_camera", { on: src !== "off" }).catch(() => {});
  if (state) visioState(state);
}
document.querySelectorAll("#visio-src button").forEach((b) => b.addEventListener("click", () => visioSource(b.dataset.src)));
// La fenêtre se ferme : la caméra s'éteint (le moteur le fait aussi de son côté).
window.addEventListener("pagehide", () => { visioStop(); });

/** Monsieur Mire, présentateur de synthèse : il se balance, cligne des yeux
 *  et parle (dessiné en 160 × 120, réduit comme une webcam). */
function visioMire(ctx, t) {
  const oval = (x, y, rx, ry, c) => { ctx.fillStyle = c; ctx.beginPath(); ctx.ellipse(x, y, rx, ry, 0, 0, 2 * Math.PI); ctx.fill(); };
  ctx.save();
  ctx.setTransform(VISIO_W / 160, 0, 0, VISIO_H / 120, 0, 0);
  const wall = ctx.createLinearGradient(0, 0, 0, 120);
  wall.addColorStop(0, "#dcc69c");
  wall.addColorStop(1, "#9c7c52");
  ctx.fillStyle = wall;
  ctx.fillRect(0, 0, 160, 120);
  // la fenêtre, et le soleil qui la traverse en vingt secondes
  ctx.fillStyle = "#5aa6ea";
  ctx.fillRect(10, 12, 44, 40);
  oval(16 + 32 * ((t / 20) % 1), 26, 7, 7, "#ffd23f");
  ctx.strokeStyle = "#f6f1e7";
  ctx.lineWidth = 3;
  ctx.strokeRect(10, 12, 44, 40);
  ctx.beginPath(); ctx.moveTo(32, 12); ctx.lineTo(32, 52); ctx.stroke();
  // la plante verte
  oval(141, 66, 12, 19, "#2e7d32");
  ctx.fillStyle = "#8d4b2a";
  ctx.fillRect(133, 82, 16, 14);
  // Monsieur Mire
  const x = 84 + 16 * Math.sin(t * 0.8), y = 60 + 2 * Math.sin(t * 1.7);
  oval(x, 124, 46, 28, "#c62828");               // pull rouge
  ctx.translate(x, y);
  ctx.rotate(0.09 * Math.sin(t * 0.8 + 0.7));
  oval(0, 2, 26, 33, "#f0bf89");                 // tête
  oval(0, -24, 27, 13, "#3d2414");               // cheveux
  oval(-26, 4, 4, 8, "#e3a970"); oval(26, 4, 4, 8, "#e3a970"); // oreilles
  if (t % 3.3 < 0.18) {                          // clignement
    ctx.fillStyle = "#3d2414";
    ctx.fillRect(-16, -4, 12, 2); ctx.fillRect(4, -4, 12, 2);
  } else {
    const look = 2 * Math.sin(t * 0.5);
    oval(-10, -3, 6, 5, "#ffffff"); oval(10, -3, 6, 5, "#ffffff");
    oval(-10 + look, -3, 3, 3, "#1d3c6e"); oval(10 + look, -3, 3, 3, "#1d3c6e");
  }
  oval(0, 11, 12, 3.5, "#3d2414");               // moustache
  const talk = Math.max(0, Math.sin(t * 9) * Math.sin(t * 1.3 + 1));
  oval(0, 19, 8, 1.5 + 6 * talk, "#7a1f1f");     // bouche
  ctx.restore();
}

/** Lit une image de la source (pixels RGBA 64 × 48), ou `null`. */
function visioGrab(now) {
  if (!visio.ctx) {
    const c = document.createElement("canvas");
    c.width = VISIO_W;
    c.height = VISIO_H;
    visio.ctx = c.getContext("2d", { willReadFrequently: true });
  }
  const ctx = visio.ctx;
  if (visio.src === "cam") {
    const v = visio.video;
    if (!v || v.readyState < 2 || !v.videoWidth) return null;
    // recadrage 4:3 au centre, réduction lissée
    const vw = v.videoWidth, vh = v.videoHeight, k = Math.min(vw / 4, vh / 3);
    ctx.imageSmoothingEnabled = true;
    ctx.imageSmoothingQuality = "high";
    ctx.drawImage(v, (vw - 4 * k) / 2, (vh - 3 * k) / 2, 4 * k, 3 * k, 0, 0, VISIO_W, VISIO_H);
  } else {
    visioMire(ctx, (now - visio.t0) / 1000);
  }
  return ctx.getImageData(0, 0, VISIO_W, VISIO_H).data;
}

/** Minuterie qui tourne aussi fenêtre cachée : WebKit cale alors celles de
 *  la page sur la seconde, pas celles d'un Worker. Tes images continuent de
 *  partir pendant que tu surfes ou que la fenêtre est sous une autre. */
function everyMs(ms, fn) {
  try {
    const src = `setInterval(() => postMessage(0), ${ms});`;
    const w = new Worker(URL.createObjectURL(new Blob([src], { type: "text/javascript" })));
    w.onmessage = fn;
    return;
  } catch (_) { /* pas de Worker : minuterie ordinaire */ }
  setInterval(fn, ms);
}

/* Prise de vue : une image dès que le moteur en veut une (il règle la
   cadence sur la ligne), sinon l'aperçu seul, deux fois par seconde. */
everyMs(100, () => {
  if (visio.src === "off" || visio.busy) return;
  const now = performance.now(), v = state && state.visio;
  const send = !!(v && v.ready) && now - visio.lastSent >= 300;
  if (!send && now - visio.last < 500) return;
  const px = visioGrab(now);
  if (!px) return;
  visio.busy = true;
  visio.last = now;
  if (send) visio.lastSent = now;
  invoke("visio_frame", { rgba: Array.from(px), w: VISIO_W, h: VISIO_H, send })
    .then((p) => {
      if (!p || visio.src === "off") return;
      visio.enc = p;
      visioShow("visio-self", p.svg);
    })
    .catch((e) => { visio.err = "Visiophone : " + e; })
    .finally(() => { visio.busy = false; });
});

listen("v34:visio", (f) => {
  visio.rxAt = performance.now();
  visio.remote = true;
  visioShow("visio-remote", f.svg);
});

/** Écran, débits et messages du visiophone, à chaque état du moteur. */
function visioState(s) {
  const v = s.visio || {}, now = performance.now();
  const live = now - visio.rxAt < 3000;
  const inet = s.session === "internet" && s.call_state !== "idle";
  if (!v.online && visio.remote) { visio.remote = false; visioBlank("visio-remote"); }
  // Incrustation sur l'écran du correspondant
  let osd = "", sub = "";
  if (!v.online) {
    osd = inet ? "LIGNE PRISE PAR INTERNET" : "AUCUN SIGNAL";
    sub = inet ? "le visiophone va d'abonné à abonné" : s.call_state === "idle" ? "appelle un abonné" : "connexion en cours…";
  } else if (v.peer === "muet") {
    [osd, sub] = ["PAS DE VISIOPHONE EN FACE", "appli trop ancienne"];
  } else if (!live) {
    if (v.peer === "ok" && !v.peer_camera) [osd, sub] = ["CAMÉRA COUPÉE", "chez ton correspondant"];
    else if (v.peer === "ok") osd = "ATTENTE D'IMAGE…";
    else [osd, sub] = ["EN LIGNE", v.camera ? "appel du visiophone d'en face…" : "allume ta caméra (ou la MIRE)"];
  }
  const html = osd && osd + (sub ? `<small>${sub}</small>` : "");
  if (visio.osd !== html) {
    visio.osd = html;
    $("visio-osd").innerHTML = html;
    $("visio-osd").hidden = !html;
  }
  // Débits dans chaque sens
  const flow = (f, a) => (f && f.fps > 0 ? `${a} ${fr1(f.fps)} img/s · ${frInt(f.bytes)} o · ${fr2(f.rate / 1024)} Ko/s` : `${a} —`);
  setText("visio-up", flow(v.tx, "↑"));
  setText("visio-down", flow(v.rx, "↓"));
  // Ligne d'état
  let msg = visio.err, warn = !!msg;
  if (!msg) {
    if (v.camera && v.pause === "fichier") msg = "Images en pause : un fichier passe d'abord.";
    else if (v.camera && v.pause === "file") msg = "Images en attente : la ligne est chargée.";
    else if (v.online && v.peer === "muet") [msg, warn] = ["Pas de réponse : l'appli d'en face ne connaît pas le visiophone (rien ne lui est envoyé).", true];
    else if (v.online && v.peer === "attente") msg = "Annonce envoyée ; en attente du visiophone d'en face…";
    else if (visio.src === "cam" && visio.enc && visio.enc.shapes === 0)
      msg = /Mac OS X|Macintosh/.test(navigator.userAgent)
        ? "Image toute unie : caméra masquée, capot du Mac fermé, ou caméra encore endormie ?"
        : "Image toute unie : volet de la webcam fermé, caméra coupée par une touche du clavier, ou encore endormie ?";
    else if (visio.src !== "off" && visio.enc)
      msg = `${visio.src === "cam" ? "Caméra" : "Mire"} : ${visio.enc.shapes} formes, ${visio.enc.bytes} octets l'image (${fr1(visio.enc.ms)} ms de rêve)` + (v.online ? "" : " · aperçu, hors appel");
    else if (visio.src === "off") msg = "Caméra éteinte : rien n'est filmé. CAMÉRA pour te montrer, MIRE pour essayer sans.";
  }
  setText("visio-msg", msg || "");
  const m = $("visio-msg");
  if (m.classList.contains("warn") !== warn) m.classList.toggle("warn", warn);
  const rec = !(v.tx && v.tx.fps > 0);
  if ($("visio-rec").hidden !== rec) $("visio-rec").hidden = rec;
}

/* ---------------------------------------------------------------- état du moteur */
listen("v34:state", (s) => {
  state = s;
  // Session Internet : le chat parle au salon du FAI.
  const inet = s.session === "internet" && s.call_state !== "idle";
  const ph = inet ? "parle au salon du FAI… (/qui : présents)" : "tape ton message…";
  if ($("bbs-input").placeholder !== ph) $("bbs-input").placeholder = ph;
  setText("chat-title", inet ? "Salon du FAI" : "Conversation · fichiers");
  renderDir(s.net);
  visioState(s);
});
if (!T) renderDir(null);
// Démo / tests : V34_VISIO=mire allume la mire à l'ouverture
invoke("app_info").then((i) => { if (i && (i.visio === "mire" || i.visio === "cam")) visioSource(i.visio); }).catch(() => {});
$("bbs-input").focus();
