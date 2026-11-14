/* V.34 Tinyverse — logique de l'interface.
 * Parle au backend Rust via window.__TAURI__ (withGlobalTauri).
 * Ouvert dans un simple navigateur, la page passe en mode démo. */
"use strict";

const TAURI = window.__TAURI__;
const DEMO = !(TAURI && TAURI.core && TAURI.event);

const $ = (id) => document.getElementById(id);
const fmt1 = (v) => (Math.round(v * 10) / 10).toLocaleString("fr-FR");

/* ---------------------------------------------------------------- pont */
const bridge = DEMO ? makeDemoBridge() : {
  invoke: (cmd, args) => TAURI.core.invoke(cmd, args),
  listen: (ev, cb) => TAURI.event.listen(ev, (e) => cb(e.payload)),
};
const invoke = (cmd, args) => bridge.invoke(cmd, args);

/* ---------------------------------------------------------------- état */
let state = null;          // dernier UiState reçu
let stateFresh = false;    // nouveau paquet pour la constellation
let lineParams = null;     // paramètres de ligne côté IHM
let presets = [];
let scopeMode = "spectrum";
let draggingSlider = false;
let lineSendTimer = null;
let peak = new Array(128).fill(-120);
let constScale = 1.5;

/* ---------------------------------------------------------------- horloge */
setInterval(() => {
  const d = new Date();
  $("clock").textContent = d.toLocaleTimeString("fr-FR", { hour12: false });
}, 250);

/* ---------------------------------------------------------------- console CRT */
const crtOut = $("crt-out");
function crt(text, cls = "resp") {
  const atBottom = crtOut.scrollHeight - crtOut.scrollTop - crtOut.clientHeight < 30;
  const div = document.createElement("div");
  div.className = cls;
  div.textContent = text;
  crtOut.appendChild(div);
  while (crtOut.childElementCount > 600) crtOut.firstChild.remove();
  if (atBottom) crtOut.scrollTop = crtOut.scrollHeight;
}

const cmdHistory = [];  // (« history » est réservé par WebKit)
let histPos = -1;
function sendAt(line) {
  if (!line.trim()) return;
  crt("> " + line, "cmd");
  cmdHistory.unshift(line);
  cmdHistory.length = Math.min(cmdHistory.length, 50);
  histPos = -1;
  invoke("at_command", { line }).catch((e) => crt(String(e), "ev warn"));
}

$("crt-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const inp = $("crt-input");
  sendAt(inp.value);
  inp.value = "";
});
$("crt-input").addEventListener("keydown", (e) => {
  const inp = e.target;
  if (e.key === "ArrowUp" && cmdHistory.length) {
    histPos = Math.min(histPos + 1, cmdHistory.length - 1);
    inp.value = cmdHistory[histPos];
    e.preventDefault();
  } else if (e.key === "ArrowDown") {
    histPos = Math.max(histPos - 1, -1);
    inp.value = histPos >= 0 ? cmdHistory[histPos] : "";
    e.preventDefault();
  }
});
$("crt").addEventListener("click", () => {
  if (!window.getSelection().toString()) $("crt-input").focus();
});

$("btn-dial").addEventListener("click", () => sendAt(state && state.incoming ? "ATA" : "ATDT" + $("dial-number").value.trim()));
$("dial-number").addEventListener("keydown", (e) => {
  if (e.key === "Enter") sendAt("ATDT" + e.target.value.trim());
});
$("btn-hangup").addEventListener("click", () => sendAt("ATH"));
$("btn-isp").addEventListener("click", () => {
  $("dial-number").value = "0836154286";
  invoke("at_command", { line: "ATDT0836154286" });
});
$("btn-browser").addEventListener("click", () => invoke("open_browser").catch((e) => crt("Navigateur : " + e, "info")));
$("btn-retrain").addEventListener("click", () => {
  crt("> [RETRAIN]", "cmd");
  invoke("modem_command", { cmd: { type: "Retrain" } });
});
$("btn-reneg").addEventListener("click", () => {
  crt("> [RENÉGOCIATION]", "cmd");
  invoke("modem_command", { cmd: { type: "Renegotiate" } });
});

const PHASE_NAMES = {
  Idle: "REPOS", Phase1: "PHASE 1 · V.8", Phase2: "PHASE 2 · SONDAGE", Phase3: "PHASE 3 · ÉGALISEUR",
  Phase4: "PHASE 4 · ENTRAÎNEMENT FINAL", Data: "MODE DONNÉES", Retrain: "RÉENTRAÎNEMENT",
  Cleardown: "RACCROCHAGE", Failed: "ÉCHEC",
};

bridge.listen("v34:console", (c) => crt(c.text, c.kind));
bridge.listen("v34:event", ({ side, event }) => {
  const who = side === "remote" ? "    [distant] " : "";
  const cls = "ev" + (side === "remote" ? " remote" : "");
  const d = event.data;
  switch (event.type) {
    case "PhaseChanged": crt(`${who}── ${PHASE_NAMES[d] || d} ──`, cls); break;
    case "TxSignal": if (d) crt(`${who}TX ▸ ${d}`, cls); break;
    case "RxSignal": if (d) crt(`${who}RX ◂ ${d}`, cls); break;
    case "Log": crt(`${who}${d.msg}`, cls + (d.level === "Warn" ? " warn" : "")); break;
    case "Disconnected": crt(`${who}déconnexion : ${d.reason}`, cls + " warn"); break;
    default: break; // ParamsUpdated, Data, Connected : affichés ailleurs
  }
});

/* ---------------------------------------------------------------- BBS */
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
bbsLine("sys", "", "*** Bienvenue ! Compose un numéro (ATDT…) puis discute ici une fois connecté.");
bbsLine("sys", "", "*** En boucle locale, le modem distant te renvoie tes messages (écho).");

bridge.listen("v34:chat", ({ from, text }) => {
  if (from === "local") { bbsLine("me", "<moi>", text); return; }
  if (from === "system") { bbsLine("sys", "", "*** " + text); return; }
  if (from === "salon") {
    // « <nom> texte » : message d'un autre abonné du salon du FAI
    const m = /^<([^>]{1,40})> ?(.*)$/.exec(text);
    if (m) bbsLine("them", `<${m[1]}>`, m[2]); else bbsLine("them", "", text);
    return;
  }
  // Flux distant : on assemble jusqu'au retour à la ligne
  const parts = text.split("\n");
  parts.forEach((part, i) => {
    if (!remoteLine) remoteLine = bbsLine("them", "<distant>", "");
    remoteLine.lastChild.textContent += part;
    if (i < parts.length - 1) remoteLine = null;
  });
  bbsOut.scrollTop = bbsOut.scrollHeight;
});

$("bbs-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const inp = $("bbs-input");
  if (!inp.value) return;
  invoke("send_chat", { text: inp.value }).catch((err) => bbsLine("sys", "", "*** " + err));
  inp.value = "";
});

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
bridge.listen("v34:file", ({ dir, name, done, total, rate, state }) => {
  const pct = total ? Math.min(100, (100 * done) / total) : 0;
  const arrow = dir === "tx" ? "↑" : "↓";
  $("file-progress").style.width = pct.toFixed(1) + "%";
  $("file-progress").classList.toggle("err", state === "error");
  const what = state === "done" ? "terminé" : state === "error" ? "ERREUR" : `${pct.toFixed(0)} %`;
  $("file-label").textContent = `${arrow} ${name} · ${what}`;
  $("file-rate").textContent = `${rate.toFixed(2).replace(".", ",")} Ko/s`;
});

/* ---------------------------------------------------------------- central (mode réseau) */
const store = {
  get: (k) => { try { return localStorage.getItem(k); } catch (_) { return null; } },
  set: (k, v) => { try { v == null ? localStorage.removeItem(k) : localStorage.setItem(k, v); } catch (_) {} },
};
$("net-server").value = store.get("net-server") || "dialup.tinyverse.fr:3434";
$("net-user").value = store.get("net-user") || "";
$("net-pass").value = store.get("net-pass") || "";
$("net-remember").checked = !!store.get("net-pass");
const netModal = $("net-modal");
$("link-badge").addEventListener("click", () => { netModal.hidden = false; ($("net-user").value ? $("net-pass") : $("net-user")).focus(); });
$("net-close").addEventListener("click", () => { netModal.hidden = true; });
netModal.addEventListener("click", (e) => { if (e.target === netModal) netModal.hidden = true; });
function netConnect() {
  const server = $("net-server").value.trim(), user = $("net-user").value.trim(), password = $("net-pass").value;
  store.set("net-server", server);
  store.set("net-user", user);
  store.set("net-pass", $("net-remember").checked ? password : null);
  $("net-status").textContent = "Connexion…";
  invoke("net_connect", { server, user, password }).catch((e) => { $("net-status").textContent = "✗ " + e; netModal.hidden = false; });
}
$("net-go").addEventListener("click", netConnect);
/* Au lancement : connexion automatique si les identifiants sont mémorisés,
   sinon on demande (central ou boucle locale). */
let autoNet = false;
function netStartup(info) {
  if (DEMO || !info.autonet) return;
  if ($("net-user").value && $("net-pass").value) {
    autoNet = true;
    crt(`Connexion automatique au central ${$("net-server").value}…`, "info");
    netConnect();
  } else {
    netModal.hidden = false;
    ($("net-user").value ? $("net-pass") : $("net-user")).focus();
  }
}
// Entrée valide (SE CONNECTER), Échap ferme.
netModal.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && e.target.tagName === "INPUT" && e.target.type !== "checkbox") { e.preventDefault(); $("net-go").click(); }
  else if (e.key === "Escape") { e.preventDefault(); netModal.hidden = true; }
});
$("net-local").addEventListener("click", () => { invoke("net_local"); $("net-status").textContent = ""; netModal.hidden = true; });
function updateNet(net) {
  const badge = $("link-badge");
  // Pas de connexion au central : on ne touche pas au message d'état (il peut
  // afficher une erreur de connexion renvoyée par net_connect).
  document.body.classList.toggle("mode-local", !net);
  badge.classList.toggle("local", !net);
  if (!net) { setText("link-badge", "⚠ BOUCLE LOCALE · SIMULATION"); badge.classList.remove("net-on", "net-wait"); updateNet.closed = false; return; }
  badge.classList.toggle("net-on", net.registered);
  badge.classList.toggle("net-wait", !net.registered);
  // setText ne réécrit que si le texte change : remplacer le texte d'un bouton
  // à chaque état annule les clics en cours (WebKit perd le nœud pressé).
  setText("link-badge", net.registered ? `☎ RÉSEAU · N° ${fmtNum(net.number)}` : "RÉSEAU · connexion…");
  setText("net-status", net.registered
    ? `✓ Connecté à ${net.server} — ton numéro : ${net.number}`
    : (net.error ? `✗ Refusé : ${net.error}` : `Connexion à ${net.server}…`));
  if (!net.registered) updateNet.closed = false;
  // Échec de la connexion automatique : on montre la fenêtre et l'erreur.
  if (autoNet && net.error && !net.registered) { autoNet = false; netModal.hidden = false; }
  if (net.registered) autoNet = false;
  if (net.registered && !updateNet.closed) { updateNet.closed = true; setTimeout(() => { netModal.hidden = true; }, 1200); }
}

/* ---------------------------------------------------------------- configuration (face arrière)
   Interrupteur DIP 1 (56K V.90) et molette DÉBIT MAX, mémorisés sur ce PC
   et envoyés au moteur au lancement puis à chaque changement. */
/** Crans de la molette (bit/s) : paliers du V.90, puis ceux du V.34. */
const RATES = [56000, 53333, 50667, 48000, 45333, 42667, 40000, 37333, 33600, 31200, 28800, 26400,
  24000, 21600, 19200, 16800, 14400, 12000, 9600, 7200, 4800, 2400];
const cfg = {
  v90: store.get("cfg-v90") !== "0",
  auto: store.get("cfg-auto") !== "0",
  rate: RATES.includes(+store.get("cfg-debit")) ? +store.get("cfg-debit") : RATES[0],
  eff: null, // configuration obtenue, renvoyée par le moteur
  seq: 0,
};
const cfgModal = $("cfg-modal");
function cfgSend() {
  store.set("cfg-v90", cfg.v90 ? "1" : "0");
  store.set("cfg-auto", cfg.auto ? "1" : "0");
  store.set("cfg-debit", String(cfg.rate));
  const seq = ++cfg.seq;
  invoke("set_modem_config", { v90: cfg.v90, maxRate: cfg.rate, autoAnswer: cfg.auto })
    .then((c) => { if (seq === cfg.seq && c) { cfg.eff = c; cfgRender(); } })
    .catch((e) => crt("Configuration : " + e, "ev warn"));
}
/** Ce que le modem fera au prochain appel (d'après la réponse du moteur). */
function cfgSummary() {
  const c = cfg.eff;
  if (!c) return "";
  // Afficheur en VT323 : pas d'espace insécable étroite dans cette police
  const k = (v) => `${frInt(v).replace(/\s/g, " ")} bit/s`;
  if (c.v90) return `▸ 56K V.90 : jusqu'à ${k(c.max_rate_pcm)} en réception · V.34 : ${k(c.max_rate)}`;
  return `▸ V.34 seul : jusqu'à ${k(c.max_rate)}` + (cfg.v90 ? ` · 56K impossible sous ${frInt(28000).replace(/\s/g, " ")}` : "");
}
function cfgRender() {
  const sw = $("dip1");
  sw.classList.toggle("on", cfg.v90);
  sw.setAttribute("aria-checked", String(cfg.v90));
  const sw2 = $("dip2");
  sw2.classList.toggle("on", cfg.auto);
  sw2.setAttribute("aria-checked", String(cfg.auto));
  const i = RATES.indexOf(cfg.rate);
  // Sur le tambour, une vraie espace fine : l'espace insécable étroite y est invisible.
  const fin = (v) => frInt(v).replace(/\s/g, "\u2009");
  setText("rate-prev", i > 0 ? fin(RATES[i - 1]) : "");
  setText("rate-cur", fin(cfg.rate));
  setText("rate-next", i < RATES.length - 1 ? fin(RATES[i + 1]) : "");
  const wheel = $("rate-wheel");
  wheel.setAttribute("aria-valuenow", String(cfg.rate));
  wheel.setAttribute("aria-valuetext", kbit(cfg.rate));
  $("rate-up").disabled = i === 0;
  $("rate-down").disabled = i === RATES.length - 1;
  setText("cfg-eff", cfgSummary());
  tipRefresh();
}
/** Tourne la molette de `d` crans (positif : débit supérieur). */
function cfgStep(d) {
  const i = RATES.indexOf(cfg.rate), j = Math.max(0, Math.min(RATES.length - 1, i - d));
  if (j === i) return;
  cfg.rate = RATES[j];
  // Le tambour roule : la nouvelle valeur arrive du dessus (débit supérieur) ou du dessous.
  const drum = $("rate-drum");
  drum.classList.remove("roll-dn", "roll-up");
  void drum.offsetWidth;
  drum.classList.add(d > 0 ? "roll-dn" : "roll-up");
  cfgRender();
  cfgSend();
}
$("dip1").addEventListener("click", () => { cfg.v90 = !cfg.v90; cfgRender(); cfgSend(); });
$("dip2").addEventListener("click", () => { cfg.auto = !cfg.auto; cfgRender(); cfgSend(); });
$("rate-up").addEventListener("click", () => cfgStep(1));
$("rate-down").addEventListener("click", () => cfgStep(-1));
$("rate-prev").addEventListener("click", () => cfgStep(1));
$("rate-next").addEventListener("click", () => cfgStep(-1));
let wheelAcc = 0;
$("rate-wheel").addEventListener("wheel", (e) => {
  // Un cran tous les 40 px : un geste du pavé tactile ne fait pas défiler toute la molette.
  e.preventDefault();
  wheelAcc += e.deltaMode ? e.deltaY * 40 : e.deltaY;
  const n = Math.trunc(wheelAcc / 40);
  if (n) { wheelAcc -= n * 40; cfgStep(-n); }
}, { passive: false });
$("rate-wheel").addEventListener("keydown", (e) => {
  const steps = { ArrowUp: 1, ArrowRight: 1, ArrowDown: -1, ArrowLeft: -1, PageUp: 4, PageDown: -4, Home: RATES.length, End: -RATES.length };
  if (!(e.key in steps)) return;
  e.preventDefault();
  cfgStep(steps[e.key]);
});
function cfgClose() {
  cfgModal.hidden = true;
  tipHide();
  $("crt-input").focus();
}
// Le focus va à la fenêtre : Entrée et Échap marchent tout de suite, Tab mène aux réglages.
$("btn-config").addEventListener("click", () => { cfgModal.hidden = false; $("cfg-box").focus(); });
$("cfg-ok").addEventListener("click", cfgClose);
cfgModal.addEventListener("click", (e) => { if (e.target === cfgModal) cfgClose(); });
// Comme la fenêtre du central : Entrée valide, Échap ferme (les réglages sont déjà appliqués).
cfgModal.addEventListener("keydown", (e) => {
  if (e.key === "Enter" || e.key === "Escape") { e.preventDefault(); cfgClose(); }
});

/* ---------------------------------------------------------------- échelle de temps */
document.querySelectorAll("#speed-seg button").forEach((b) =>
  b.addEventListener("click", () => invoke("set_speed", { speed: +b.dataset.speed })));

/* ---------------------------------------------------------------- HP */
document.querySelectorAll("#spk-mode button").forEach((b) =>
  b.addEventListener("click", () => invoke("set_speaker", { mode: +b.dataset.mode, level: null, volume: null })));
let volTimer = null;
$("spk-vol").addEventListener("input", (e) => {
  clearTimeout(volTimer);
  const v = e.target.value / 100;
  volTimer = setTimeout(() => invoke("set_speaker", { mode: null, level: null, volume: v }), 30);
});

/* ---------------------------------------------------------------- scope : onglets */
document.querySelectorAll("#scope-mode button").forEach((b) =>
  b.addEventListener("click", () => {
    scopeMode = b.dataset.mode;
    document.querySelectorAll("#scope-mode button").forEach((x) => x.classList.toggle("on", x === b));
  }));

/* ---------------------------------------------------------------- ligne */
const SLIDERS = [
  { key: "snr_db", label: "RAPPORT SIGNAL/BRUIT", min: 10, max: 80, step: 0.5, unit: "dB", off: (v) => v >= 80, offText: "∞" },
  { key: "attenuation_db", label: "ATTÉNUATION", min: 0, max: 40, step: 0.5, unit: "dB" },
  { key: "band_low_hz", label: "BANDE · BAS", min: 0, max: 600, step: 10, unit: "Hz" },
  { key: "band_high_hz", label: "BANDE · HAUT", min: 2600, max: 4000, step: 50, unit: "Hz" },
  { key: "hf_tilt_db", label: "PENTE DES AIGUS (3 kHz)", min: 0, max: 20, step: 0.5, unit: "dB" },
  { key: "echo_db", label: "ÉCHO LOCAL", min: -80, max: -6, step: 1, unit: "dB", off: (v) => v <= -80, offText: "aucun" },
  { key: "echo_delay_ms", label: "DÉLAI DE L'ÉCHO", min: 0, max: 40, step: 0.5, unit: "ms" },
  { key: "delay_ms", label: "DÉLAI DE PROPAGATION", min: 0, max: 300, step: 1, unit: "ms" },
  { key: "impulses_per_s", label: "PARASITES (CLICS)", min: 0, max: 10, step: 0.1, unit: "/s", off: (v) => v <= 0, offText: "aucun" },
  { key: "dropouts_per_s", label: "COUPURES", min: 0, max: 1, step: 0.01, unit: "/s", off: (v) => v <= 0, offText: "aucune" },
  { key: "freq_offset_hz", label: "DÉCALAGE DE FRÉQUENCE", min: -10, max: 10, step: 0.1, unit: "Hz" },
];

function sliderText(def, v) {
  if (def.off && def.off(v)) return def.offText;
  return `${fmt1(v)} ${def.unit}`;
}

function buildSliders() {
  const box = $("sliders");
  box.innerHTML = "";
  for (const def of SLIDERS) {
    const row = document.createElement("div");
    row.className = "sl";
    row.innerHTML = `<label for="sl-${def.key}">${def.label}</label><output id="out-${def.key}"></output>
      <input type="range" id="sl-${def.key}" min="${def.min}" max="${def.max}" step="${def.step}">`;
    box.appendChild(row);
    const inp = row.querySelector("input");
    inp.addEventListener("pointerdown", () => (draggingSlider = true));
    inp.addEventListener("input", () => {
      if (!lineParams) return;
      let v = parseFloat(inp.value);
      if (def.key === "snr_db" && v >= 80) v = 100;
      if (def.key === "echo_db" && v <= -80) v = -100;
      lineParams[def.key] = v;
      $("out-" + def.key).textContent = sliderText(def, v);
      markPreset();
      clearTimeout(lineSendTimer);
      lineSendTimer = setTimeout(() => invoke("set_line", { params: lineParams }), 40);
    });
  }
}

window.addEventListener("pointerup", () => (draggingSlider = false));

function syncSliders(p) {
  for (const def of SLIDERS) {
    if (!(def.key in p)) continue;
    const inp = $("sl-" + def.key);
    if (document.activeElement !== inp || !draggingSlider) inp.value = Math.min(def.max, Math.max(def.min, p[def.key]));
    $("out-" + def.key).textContent = sliderText(def, p[def.key]);
  }
}

function sameParams(a, b) {
  return a && b && Object.keys(b).every((k) => Math.abs((a[k] ?? 0) - b[k]) < 1e-6);
}
function markPreset() {
  document.querySelectorAll("#presets .key").forEach((btn, i) =>
    btn.classList.toggle("on", sameParams(lineParams, presets[i]?.params)));
}

async function loadPresets() {
  presets = await invoke("line_presets");
  const box = $("presets");
  box.innerHTML = "";
  presets.forEach((p) => {
    const b = document.createElement("button");
    b.className = "key";
    b.textContent = p.name.toUpperCase();
    b.addEventListener("click", () => {
      lineParams = { ...p.params };
      syncSliders(lineParams);
      markPreset();
      invoke("set_line", { params: lineParams });
    });
    box.appendChild(b);
  });
  if (!lineParams) lineParams = { ...presets[0].params };
  syncSliders(lineParams);
  markPreset();
}

/* ---------------------------------------------------------------- état → IHM */
const PHASE_ORDER = ["Phase1", "Phase2", "Phase3", "Phase4", "Data"];
const CALL_PILL = {
  idle: ["REPOS", ""], dialing: ["NUMÉROTATION", "warn"], ringing: ["SONNERIE", "warn"],
  connecting: ["NÉGOCIATION", "warn"], online: ["EN LIGNE", "good"],
};
const ledEls = {};
document.querySelectorAll(".led-cell").forEach((c) => (ledEls[c.dataset.led] = c.querySelector(".led")));

function setText(id, txt) {
  const el = $(id);
  if (el.textContent !== txt) el.textContent = txt;
}

/** Met à jour une valeur du panneau VFD et son infobulle (valeur complète). */
function setVfd(id, text, title) {
  setText(id, text);
  const el = $(id);
  if (el && el.title !== title) el.title = title || "";
}

function onState(s) {
  state = s;
  stateFresh = true;
  const st = s.status;

  for (const [k, el] of Object.entries(ledEls)) el.classList.toggle("on", !!st.leds[k]);

  let idx = PHASE_ORDER.indexOf(st.phase);
  if (st.phase === "Retrain") idx = 1;
  document.querySelectorAll(".ph").forEach((el, i) => {
    el.classList.toggle("active", i === idx);
    el.classList.toggle("done", idx >= 0 && i < idx);
  });

  let [pill, cls] = CALL_PILL[s.call_state] || ["?", ""];
  if (st.phase === "Failed") [pill, cls] = ["ÉCHEC", "bad"];
  if (st.phase === "Retrain") [pill, cls] = ["RETRAIN", "warn"];
  if (st.phase === "Cleardown") [pill, cls] = ["RACCROCHE", "warn"];
  const pillEl = $("call-pill");
  setText("call-pill", pill);
  pillEl.className = "call-pill " + cls;

  // Touche NAVIGATEUR : s'allume quand le tunnel Internet est prêt.
  // Appel entrant qui sonne (réponse manuelle) : APPELER devient DÉCROCHER
  const dial = $("btn-dial");
  setText("btn-dial", s.incoming ? "DÉCROCHER" : "APPELER");
  dial.classList.toggle("ringing", !!s.incoming);
  const web = $("btn-browser"), dialingIsp = s.session === "internet" || (s.call_state !== "idle" && $("dial-number").value.trim() === "0836154286");
  web.classList.toggle("ready", !!s.proxy);
  // Session Internet : le chat n'a pas de destinataire (la ligne porte le tunnel).
  const inet = s.session === "internet" && s.call_state !== "idle";
  const ph = inet ? "parle au salon du FAI… (/qui : présents)" : "tape ton message…";
  if ($("bbs-input").placeholder !== ph) $("bbs-input").placeholder = ph;
  if (bbsView === "chat") setText("bbs-title", inet ? "SALON DU FAI" : "CHAT · FICHIERS");
  setText("web-sub", s.proxy ? "Internet prêt · clique !" : dialingIsp && s.call_state !== "idle" ? "connexion au FAI…" : s.call_state === "online" ? "en ligne (chat) · appelle le FAI" : "appelle d'abord le FAI");

  updateNet(s.net);
  updateMonitor(s);
  tipRefresh();
  // Panneau CONFIG : pendant un appel, les réglages attendent le suivant.
  const cfgNote = $("cfg-note"), quiet = s.call_state === "idle";
  if (cfgNote.hidden !== quiet) cfgNote.hidden = quiet;
  renderDir(s.net);
  if (s.speed != null) {
    document.querySelectorAll("#speed-seg button").forEach((b) =>
      b.classList.toggle("on", Math.abs(+b.dataset.speed - s.speed) < 1e-6));
  }
  const p = st.params;
  // Valeurs compactes (la valeur complète est en infobulle)
  const pair = (a, b, unit, f = (v) => String(v)) => {
    if (a == null && b == null) return null;
    if (a == null || b == null || a === b) return `${f(a ?? b)}${unit}`;
    return `${f(a)}/${f(b)}${unit}`;
  };
  const kilo = (v) => (v >= 1000 ? `${(v / 1000).toFixed(1)}k` : String(v));
  const spaced = (v) => String(v).replace(/\B(?=(\d{3})+(?!\d))/g, " ");
  const rateSym = p.data_rate_tx && p.data_rate_rx && p.data_rate_tx === p.data_rate_rx;
  const rate = p.data_rate_tx || p.data_rate_rx
    ? (rateSym ? spaced(p.data_rate_tx) : `${kilo(p.data_rate_tx ?? 0)}↑${kilo(p.data_rate_rx ?? 0)}↓`)
    : "-----";
  setVfd("p-rate", rate, p.data_rate_tx ? `émission ${p.data_rate_tx} bit/s · réception ${p.data_rate_rx} bit/s` : "");
  setVfd("p-baud", pair(p.symbol_rate_tx, p.symbol_rate_rx, " Bd") ?? "----",
    p.symbol_rate_tx ? `émission ${p.symbol_rate_tx} bauds · réception ${p.symbol_rate_rx} bauds` : "");
  const hz = (v) => Math.round(v);
  setVfd("p-carrier", pair(p.carrier_tx_hz, p.carrier_rx_hz, " Hz", hz) ?? "----",
    p.carrier_tx_hz ? `émission ${hz(p.carrier_tx_hz)} Hz · réception ${hz(p.carrier_rx_hz ?? 0)} Hz` : "");
  setVfd("p-rtd", p.rtd_ms != null ? `${fmt1(p.rtd_ms)} ms` : "---", "délai aller-retour mesuré en phase 2");
  setVfd("p-snr", p.snr_db != null ? `${fmt1(p.snr_db)} dB` : "--", "rapport signal/bruit mesuré (sondage puis égaliseur)");
  setVfd("p-trellis", p.trellis_states ? `${p.trellis_states} ét.` : "--", p.trellis_states ? `codeur en treillis ${p.trellis_states} états` : "");
  const modes = p.v8_modes || [];
  setVfd("p-modes", modes.length ? modes[0] + (modes.length > 1 ? "+" : "") : "--", modes.length ? `modes communs (JM) : ${modes.join(", ")}` : "");
  setText("p-tx", st.tx_signal || "—");
  setText("p-rx", st.rx_signal || "—");
  const t = Math.floor(s.elapsed_s);
  setText("p-time", `${String(Math.floor(t / 60)).padStart(2, "0")}:${String(t % 60).padStart(2, "0")}`);

  const meter = (db) => `inset(0 ${100 - Math.max(0, Math.min(100, (db + 60) / 60 * 100))}% 0 0)`;
  $("m-tx").style.clipPath = meter(s.tx_level_db);
  $("m-rx").style.clipPath = meter(s.rx_level_db);

  $("grill").classList.toggle("active", s.speaker.active);
  document.querySelectorAll("#spk-mode button").forEach((b) => b.classList.toggle("on", +b.dataset.mode === s.speaker.mode));
  if (document.activeElement !== $("spk-vol")) $("spk-vol").value = Math.round(s.speaker.volume * 100);

  const ab = $("audio-badge");
  setText("audio-badge", s.audio_device ? "HP ✓" : "HP ✗");
  ab.title = s.audio_device || "Aucune sortie audio";

  if (!draggingSlider && s.line && !sameParams(lineParams, s.line)) {
    lineParams = { ...s.line };
    syncSliders(lineParams);
    markPreset();
  }
  drawResponse();
}
bridge.listen("v34:state", onState);

/* ---------------------------------------------------------------- canvases */
const canvases = {};
function setupCanvas(id) {
  const c = $(id);
  const ctx = c.getContext("2d");
  const info = { c, ctx, w: 0, h: 0 };
  const fit = () => {
    const r = c.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    info.w = Math.max(1, r.width);
    info.h = Math.max(1, r.height);
    c.width = Math.round(info.w * dpr);
    c.height = Math.round(info.h * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    if (id === "response") drawResponse();
  };
  new ResizeObserver(fit).observe(c);
  fit();
  canvases[id] = info;
  return info;
}
["scope", "constellation", "response", "mon-spark"].forEach(setupCanvas);

const PH = "125,255,154";
function fade(info, alpha) {
  const { ctx, w, h } = info;
  ctx.fillStyle = `rgba(4,18,10,${alpha})`;
  ctx.fillRect(0, 0, w, h);
}

function drawScope() {
  const info = canvases.scope;
  const { ctx, w, h } = info;
  fade(info, 0.38);
  const top = 26, bottom = h - 22, H = bottom - top;
  ctx.lineWidth = 1;
  ctx.strokeStyle = `rgba(${PH},0.10)`;
  ctx.fillStyle = `rgba(${PH},0.35)`;
  ctx.font = "13px VT323, monospace";
  ctx.beginPath();
  if (scopeMode === "spectrum") {
    for (let f = 500; f < 4000; f += 500) {
      const x = (f / 4000) * w;
      ctx.moveTo(x, top); ctx.lineTo(x, bottom);
      if (f % 1000 === 0) ctx.fillText(`${f / 1000}k`, x + 3, bottom - 4);
    }
    for (let db = -80; db <= 0; db += 20) {
      const y = top + (-db / 90) * H;
      ctx.moveTo(0, y); ctx.lineTo(w, y);
    }
  } else {
    for (let i = 1; i < 8; i++) { const x = (i / 8) * w; ctx.moveTo(x, top); ctx.lineTo(x, bottom); }
    ctx.moveTo(0, top + H / 2); ctx.lineTo(w, top + H / 2);
  }
  ctx.stroke();
  if (!state) return;

  ctx.shadowColor = `rgba(${PH},0.9)`;
  ctx.shadowBlur = 6;
  ctx.strokeStyle = `rgba(${PH},0.95)`;
  ctx.lineWidth = 1.6;
  ctx.beginPath();
  if (scopeMode === "spectrum") {
    const sp = state.spectrum;
    const n = sp.length;
    const grad = ctx.createLinearGradient(0, top, 0, bottom);
    grad.addColorStop(0, `rgba(${PH},0.45)`);
    grad.addColorStop(1, `rgba(${PH},0.02)`);
    for (let i = 0; i < n; i++) {
      const x = (i / n) * w;
      const y = top + Math.min(1, Math.max(0, -sp[i] / 90)) * H;
      i ? ctx.lineTo(x, y) : ctx.moveTo(x, y);
      peak[i] = Math.max(sp[i], peak[i] - 0.6);
    }
    ctx.stroke();
    ctx.shadowBlur = 0;
    ctx.lineTo(w, bottom); ctx.lineTo(0, bottom); ctx.closePath();
    ctx.fillStyle = grad;
    ctx.fill();
    ctx.fillStyle = "rgba(255,179,71,0.8)";
    for (let i = 0; i < n; i++) {
      const y = top + Math.min(1, Math.max(0, -peak[i] / 90)) * H;
      ctx.fillRect((i / n) * w, y - 1, Math.max(1, w / n - 1), 1.5);
    }
  } else {
    const sc = state.scope;
    let max = 0.05;
    for (const v of sc) max = Math.max(max, Math.abs(v));
    const g = (H / 2) * 0.9 / max;
    for (let i = 0; i < sc.length; i++) {
      const x = (i / (sc.length - 1)) * w;
      const y = top + H / 2 - sc[i] * g;
      i ? ctx.lineTo(x, y) : ctx.moveTo(x, y);
    }
    ctx.stroke();
    ctx.shadowBlur = 0;
    ctx.fillStyle = `rgba(${PH},0.35)`;
    ctx.fillText(`×${fmt1(1 / max)}`, 8, top - 8);
  }
  ctx.shadowBlur = 0;
}

function drawConstellation() {
  const info = canvases.constellation;
  const { ctx, w, h } = info;
  fade(info, 0.16);
  const cx = w / 2, cy = h / 2, r = Math.min(w, h) / 2 - 8;
  ctx.strokeStyle = `rgba(${PH},0.12)`;
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(cx - r, cy); ctx.lineTo(cx + r, cy);
  ctx.moveTo(cx, cy - r); ctx.lineTo(cx, cy + r);
  ctx.stroke();
  if (!state || !stateFresh) return;
  stateFresh = false;
  const pts = state.status.constellation || [];
  if (!pts.length) return;
  let m = 0;
  for (const [x, y] of pts) m = Math.max(m, Math.abs(x), Math.abs(y));
  constScale += (Math.max(0.2, m * 1.1) - constScale) * 0.1;
  const k = r / constScale;
  ctx.fillStyle = `rgba(${PH},0.95)`;
  ctx.shadowColor = `rgba(${PH},1)`;
  ctx.shadowBlur = 5;
  for (const [x, y] of pts) ctx.fillRect(cx + x * k - 1, cy - y * k - 1, 2.2, 2.2);
  ctx.shadowBlur = 0;
}

function drawResponse() {
  const info = canvases.response;
  if (!info || !state) return;
  const { ctx, w, h } = info;
  ctx.clearRect(0, 0, w, h);
  const top = 22, bottom = h - 16, H = bottom - top;
  const yOf = (db) => top + Math.min(1, Math.max(0, (10 - db) / 60)) * H; // +10 … -50 dB
  ctx.font = "13px VT323, monospace";
  ctx.strokeStyle = `rgba(${PH},0.10)`;
  ctx.fillStyle = `rgba(${PH},0.35)`;
  ctx.beginPath();
  for (let f = 1000; f < 4000; f += 1000) {
    const x = (f / 4000) * w;
    ctx.moveTo(x, top); ctx.lineTo(x, bottom);
    ctx.fillText(`${f / 1000}k`, x + 2, bottom + 12);
  }
  for (let db = 0; db >= -40; db -= 20) {
    const y = yOf(db);
    ctx.moveTo(0, y); ctx.lineTo(w, y);
    ctx.fillText(`${db}`, w - 22, y - 2);
  }
  ctx.stroke();
  // bande passante
  if (state.line) {
    ctx.fillStyle = "rgba(103,247,228,0.05)";
    const x0 = (state.line.band_low_hz / 4000) * w, x1 = (state.line.band_high_hz / 4000) * w;
    ctx.fillRect(x0, top, x1 - x0, H);
  }
  const resp = state.line_response || [];
  if (resp.length) {
    ctx.strokeStyle = `rgba(${PH},0.95)`;
    ctx.shadowColor = `rgba(${PH},0.8)`;
    ctx.shadowBlur = 5;
    ctx.lineWidth = 1.6;
    ctx.beginPath();
    resp.forEach(([f, db], i) => {
      const x = (f / 4000) * w, y = yOf(db);
      i ? ctx.lineTo(x, y) : ctx.moveTo(x, y);
    });
    ctx.stroke();
    ctx.shadowBlur = 0;
  }
  // réponse mesurée par le modem (sondage L1/L2)
  const meas = state.status.params.line_response || [];
  ctx.fillStyle = "rgba(255,179,71,0.9)";
  for (const [f, db] of meas) ctx.fillRect((f / 4000) * w - 2, yOf(db) - 2, 4, 4);
}

function frame() {
  drawScope();
  drawConstellation();
  requestAnimationFrame(frame);
}

/* ---------------------------------------------------------------- infobulles LEDs et phases */
const TIPS = {
  hs: ["HS · High Speed", "Allumée quand la liaison dépasse 14 400 bit/s (V.32bis, V.34). Sur un vrai modem, c'était la LED des frimeurs."],
  aa: ["AA · Auto Answer", "Le modem décrochera tout seul si on l'appelle (registre S0). Les appels du central décrochent automatiquement."],
  cd: ["CD · Carrier Detect", "La porteuse du modem distant est détectée : la liaison de données est établie. Quand elle s'éteint : NO CARRIER."],
  oh: ["OH · Off Hook", "Le modem a décroché la ligne, comme quand on soulève le combiné. Elle reste allumée de la numérotation au raccrochage."],
  rd: ["RD · Receive Data", "Clignote quand des octets arrivent du modem distant (après correction d'erreurs LAPM)."],
  sd: ["SD · Send Data", "Clignote quand des octets partent vers le modem distant."],
  tr: ["TR · Terminal Ready", "Le signal DTR : l'ordinateur dit au modem qu'il est prêt. S'il tombe, le modem raccroche."],
  mr: ["MR · Modem Ready", "Le modem est sous tension et prêt. Toujours allumée… sauf panne de courant."],
  Phase1: ["Phase 1 · V.8", "L'appelé émet ANSam : 2 100 Hz modulé à 15 Hz, avec une inversion de phase toutes les 450 ms qui coupe les annuleurs d'écho du réseau (le « piiiii »). L'appelant annonce ses capacités avec CM en V.21 à 300 bit/s, l'appelé choisit avec JM, l'appelant confirme par CJ (le « brrrr »)."],
  Phase2: ["Phase 2 · Sondage de la ligne", "Échange des capacités (INFO0), tons A et B avec inversions de phase pour mesurer le délai aller-retour, puis sonde L1/L2 : 21 fréquences espacées de 150 Hz pour mesurer la réponse et le bruit de la ligne. INFO1 fixe le débit symbole et la porteuse (le « dong-dong »)."],
  Phase3: ["Phase 3 · Égaliseur", "S et S̄ servent de repère, PP cale finement le rythme et la phase, puis TRN, un signal pseudo-aléatoire connu des deux côtés : l'égaliseur apprend à compenser les déformations de la ligne, l'annuleur d'écho aussi. J annonce la constellation (le « shhhh »)."],
  Phase4: ["Phase 4 · Débit final", "TRN sur la vraie constellation, puis MP (débit de données, précodage, mise en forme), E (accord) et B1 : la première trame de données brouillée."],
  Data: ["Données", "Les octets sont codés en treillis à 16 états, répartis sur une constellation jusqu'à 960 points (shell mapping) et protégés par LAPM : trames avec contrôle d'erreur et retransmission. Le modem surveille la qualité et renégocie si la ligne se dégrade (le « kshhhh »)."],
  cfg: ["CONFIG · Face arrière", "Les interrupteurs DIP du dos du modem : 56K V.90 et débit maximum. Ils sont lus au décroché : un changement vaut pour l'appel suivant."],
  dip1: ["DIP 1 · 56K V.90", "ON : le modem propose le V.90 pendant la négociation V.8 et peut recevoir jusqu'à 56 000 bit/s d'un FAI raccordé en numérique (l'émission reste en V.34). OFF : V.34 seul, 33 600 bit/s au mieux."],
  dip2: ["DIP 2 · Réponse automatique", "ON : quand un abonné t'appelle, le modem décroche tout seul après deux sonneries (comme ATS0=2). OFF : ça sonne jusqu'à ce que tu décroches, avec ATA ou le bouton DÉCROCHER."],
  dipres: ["DIP 3 à 8 · Réservé", "Réservé usine — ne pas modifier. Réglés en usine et sans effet sur ce modèle ; le technicien qui savait à quoi ils servaient est parti avec le mode d'emploi."],
  debit: ["DÉBIT MAX", "Plafond du débit négocié, pour imiter un modem plus ancien ou brider la ligne. Le V.34 s'arrête à 33 600 bit/s ; au-dessus, seul le 56K (V.90) monte, par paliers de 2 667 bit/s, et il ne descend pas sous 28 000. Molette de la souris, ▲ ▼ ou flèches du clavier."],
  cast: ["COMMENTAIRES", "Le modem commente sa propre négociation comme un match à la télé : ANSam, sondage, débit symbole, CONNECT… Les phrases sont tirées au hasard dans des banques écrites à la main, avec les vraies valeurs mesurées. Aucune intelligence artificielle n'a été dérangée."],
  karaoke: ["♪ KARAOKÉ", "Les paroles de la chanson du modem, en direct : à gauche ce que notre modem émet, à droite ce qu'on entend de l'autre bout. La boule rebondit sur la syllabe en cours. Au ralenti (TEMPS ×0,1), on arrive presque à chanter juste."],
};
const tip = document.createElement("div");
tip.id = "tip";
tip.hidden = true;
document.body.appendChild(tip);
let tipKey = null, tipEl = null;
const kbit = (v) => (v ? `${frInt(v)} bit/s` : "—");
/** Ligne « en direct » de l'infobulle, d'après le dernier état. */
function tipLive(key) {
  // Panneau CONFIG : la ligne en direct suit les réglages, pas l'état du modem.
  if (key === "cfg" || key === "debit") return cfgSummary();
  if (key === "dipres") return "○ OFF, scellé";
  if (key === "cast") return cast.actif() ? "● micro ouvert" : "○ micro coupé : la régie a débranché le commentateur";
  if (key === "karaoke") {
    const s = state && state.status;
    return s ? `TX ▸ ${s.tx_signal || "—"} · RX ◂ ${s.rx_signal || "—"}` : "";
  }
  if (key === "dip2") return cfg.auto ? "● ON — décroche après 2 sonneries" : "○ OFF — décroche avec ATA";
  if (key === "dip1") {
    if (!cfg.v90) return "○ OFF — V.34 seul";
    return cfg.eff && !cfg.eff.v90 ? `● ON, mais débit max sous ${kbit(28000)} : 56K coupé` : "● ON — 56K proposé au prochain appel";
  }
  const st = state && state.status;
  if (!st) return "";
  const p = st.params || {};
  if (key in (st.leds || {})) {
    const on = !!st.leds[key];
    let extra = "";
    if (key === "hs" && on) extra = ` — ${kbit(p.data_rate_rx)}`;
    if (key === "cd" && on) extra = ` — ${kbit(p.data_rate_rx)} reçus, ${kbit(p.data_rate_tx)} émis`;
    if ((key === "rd" || key === "sd") && st.link) extra = ` — ${fr1((key === "rd" ? st.link.bytes_delivered : st.link.bytes_acked) / 1024)} Ko depuis le début`;
    return (on ? "● allumée" : "○ éteinte") + extra;
  }
  const order = ["Phase1", "Phase2", "Phase3", "Phase4", "Data"];
  let idx = order.indexOf(st.phase);
  if (st.phase === "Retrain") idx = 1;
  const me = order.indexOf(key);
  const etat = idx < 0 ? "en attente d'appel" : me === idx ? "▶ en cours" : me < idx ? "✓ terminée" : "à venir";
  const d = [];
  if (key === "Phase1" && p.v8_modes && p.v8_modes.length) d.push(`modes communs : ${p.v8_modes.join(", ")}`);
  if (key === "Phase2") {
    if (p.rtd_ms != null) d.push(`délai aller-retour : ${frInt(p.rtd_ms)} ms`);
    if (p.symbol_rate_rx) d.push(`débit symbole : ${frInt(p.symbol_rate_rx)} bauds, porteuse ${frInt(p.carrier_rx_hz || 0)} Hz`);
    if (p.snr_db != null) d.push(`SNR sondé : ${fr1(p.snr_db)} dB`);
  }
  if ((key === "Phase4" || key === "Data") && p.data_rate_rx) d.push(`débit : ${kbit(p.data_rate_rx)} reçus, ${kbit(p.data_rate_tx)} émis`);
  if (key === "Data" && p.trellis_states) d.push(`treillis : ${p.trellis_states} états`);
  return [etat, ...d].join("\n");
}
function tipRender() {
  if (!tipKey) return;
  const [titre, texte] = TIPS[tipKey];
  tip.innerHTML = "";
  const h = document.createElement("b");
  h.textContent = titre;
  const t = document.createElement("p");
  t.textContent = texte;
  const l = document.createElement("i");
  l.textContent = tipLive(tipKey);
  tip.append(h, t, l);
}
/** Infobulle ouverte : seule la ligne « en direct » change. */
function tipRefresh() {
  if (!tipKey) return;
  const live = tip.querySelector("i"), v = tipLive(tipKey);
  if (live && live.textContent !== v) live.textContent = v;
}
function tipHide() { tipKey = null; tipEl = null; tip.hidden = true; }
function tipPlace(e) {
  const r = tip.getBoundingClientRect();
  let x = e.clientX + 14, y = e.clientY + 18;
  if (x + r.width > window.innerWidth - 8) x = Math.max(8, e.clientX - r.width - 14);
  if (y + r.height > window.innerHeight - 8) y = Math.max(8, e.clientY - r.height - 12);
  tip.style.left = x + "px";
  tip.style.top = y + "px";
}
document.querySelectorAll(".led-cell, .ph, [data-tip]").forEach((el) => {
  const key = el.dataset.led || el.dataset.phase || el.dataset.tip;
  if (!TIPS[key]) return;
  el.addEventListener("mouseenter", (e) => { tipKey = key; tipEl = el; tipRender(); tip.hidden = false; tipPlace(e); });
  el.addEventListener("mousemove", tipPlace);
  el.addEventListener("mouseleave", tipHide);
});

/* ---------------------------------------------------------------- numéros */
/** « 0142424242 » → « 01 42 42 42 42 » */
function fmtNum(n) {
  return n ? String(n).replace(/(\d{2})(?=\d)/g, "$1 ") : "";
}
const fr1 = (x) => x.toFixed(1).replace(".", ",");
const frInt = (x) => Math.round(x).toLocaleString("fr-FR");

/* ---------------------------------------------------------------- moniteur de liaison */
const mon = { last: null, hist: [], rate: { rx: 0, tx: 0 } };
function monSet(id, txt, cls) {
  setText(id, txt);
  const el = $(id);
  const want = cls || "";
  if (el.className !== want) el.className = want;
}
function updateMonitor(s) {
  const st = s.status || {}, link = st.link, net = s.net, ns = net && net.stats, jb = ns && ns.jitter, c = s.counters || {};
  // Réseau
  $("mon-net").classList.toggle("off", !net);
  setText("mn-net-tag", net ? (ns ? "EN APPEL" : net.registered ? "EN LIGNE" : "…") : "BOUCLE LOCALE");
  if (!net) {
    monSet("mn-ping", "local"); monSet("mn-jit", "—"); monSet("mn-bufv", "—"); monSet("mn-loss", "—");
    $("mn-buf").style.width = "0%";
  } else {
    const ping = net.rtt_ms;
    monSet("mn-ping", ping == null ? "…" : `${frInt(ping)} ms`, ping > 250 ? "bad" : ping > 120 ? "warn" : "");
    if (jb) {
      monSet("mn-jit", `${fr1(jb.jitter_ms)} ms`, jb.jitter_ms > 40 ? "bad" : jb.jitter_ms > 15 ? "warn" : "");
      const pct = Math.min(100, (jb.depth_ms / Math.max(1, jb.target_ms * 2)) * 100);
      const bar = $("mn-buf");
      bar.style.width = pct.toFixed(0) + "%";
      bar.classList.toggle("warn", jb.depth_ms < jb.target_ms * 0.4);
      monSet("mn-bufv", `${frInt(jb.depth_ms)} ms`);
      const holes = jb.underruns + jb.late;
      monSet("mn-loss", `${jb.lost} · ${holes}${jb.resyncs ? " · ⚠" + jb.resyncs : ""}`, jb.resyncs ? "bad" : jb.lost + holes > 0 ? "warn" : "");
    } else {
      monSet("mn-jit", "—"); monSet("mn-bufv", "—"); monSet("mn-loss", "—");
      $("mn-buf").style.width = "0%";
    }
  }
  // Modem
  const p = st.params || {};
  const online = st.phase === "Data" || st.phase === "Retrain";
  monSet("mn-rx", online && p.data_rate_rx ? `${frInt(p.data_rate_rx)} bit/s` : "—");
  monSet("mn-tx", online && p.data_rate_tx ? `${frInt(p.data_rate_tx)} bit/s` : "—");
  const snr = p.snr_db;
  monSet("mn-snr", online && snr ? `${fr1(snr)} dB` : "—", snr && snr < 20 ? "warn" : "");
  monSet("mn-rec", String(c.recoveries || 0), c.recoveries > 3 ? "bad" : c.recoveries ? "warn" : "");
  // LAPM
  if (link) {
    monSet("mn-frames", `${frInt(link.frames_tx)} / ${frInt(link.frames_rx)}`);
    monSet("mn-retx", frInt(link.retransmissions), link.retransmissions ? "warn" : "");
    monSet("mn-fcs", frInt(link.fcs_errors), link.fcs_errors ? "warn" : "");
    monSet("mn-queue", link.tx_queue ? `${fr1(link.tx_queue / 1024)} Ko` : "0");
  } else {
    for (const id of ["mn-frames", "mn-retx", "mn-fcs", "mn-queue"]) monSet(id, "—");
  }
  // Tunnel
  const tun = s.session === "internet";
  monSet("mn-conn", tun ? String(c.tunnel_open || 0) : "—");
  monSet("mn-opened", tun ? frInt(c.tunnel_opened || 0) : "—");
  monSet("mn-fail", tun ? frInt(c.tunnel_failed || 0) : "—", c.tunnel_failed ? "warn" : "");
  monSet("mn-reset", tun ? frInt(c.tunnel_resets || 0) : "—", c.tunnel_resets ? "bad" : "");
  // Débit utile (octets livrés / acquittés par LAPM), échantillonné chaque seconde
  const now = performance.now();
  const rx = link ? link.bytes_delivered : st.bytes_rx || 0, tx = link ? link.bytes_acked : st.bytes_tx || 0;
  if (!mon.last || rx < mon.last.rx || tx < mon.last.tx) mon.last = { t: now, rx, tx };
  const dt = (now - mon.last.t) / 1000;
  if (dt >= 1) {
    mon.rate = { rx: (rx - mon.last.rx) / dt / 1024, tx: (tx - mon.last.tx) / dt / 1024 };
    mon.last = { t: now, rx, tx };
    mon.hist.push(mon.rate);
    if (mon.hist.length > 60) mon.hist.shift();
    setText("mn-rate", `↓ ${fr1(mon.rate.rx)} · ↑ ${fr1(mon.rate.tx)} Ko/s`);
    drawSpark();
  }
}
function drawSpark() {
  const info = canvases["mon-spark"];
  if (!info) return;
  const { ctx, w, h } = info;
  ctx.clearRect(0, 0, w, h);
  const top = 16, H = h - top - 2;
  const max = Math.max(1, ...mon.hist.map((r) => Math.max(r.rx, r.tx)));
  const bw = w / 60;
  mon.hist.forEach((r, i) => {
    const x = w - (mon.hist.length - i) * bw;
    const hr = (r.rx / max) * H, ht = (r.tx / max) * H;
    ctx.fillStyle = "rgba(125,255,154,0.75)";
    ctx.fillRect(x + 1, top + H - hr, Math.max(1, bw - 2), hr);
    ctx.fillStyle = "rgba(255,179,71,0.85)";
    ctx.fillRect(x + 1, top + H - ht, Math.max(1, bw - 2), Math.min(ht, 2) || 0);
    if (ht > 2) ctx.fillRect(x + bw / 2 - 1, top + H - ht, 2, ht);
  });
  ctx.fillStyle = "rgba(125,255,154,0.45)";
  ctx.font = "12px " + getComputedStyle(document.body).getPropertyValue("--screen-font");
  ctx.fillText(`${fr1(max)} Ko/s`, 2, top + 10);
}

/* ---------------------------------------------------------------- chat / annuaire */
let bbsView = "chat";
function setBbsView(v) {
  bbsView = v;
  document.querySelectorAll("#bbs-tabs button").forEach((b) => b.classList.toggle("on", b.dataset.view === v));
  document.querySelectorAll(".bbs .chat-view").forEach((el) => (el.hidden = v !== "chat"));
  $("dir").hidden = v !== "dir";
  setText("bbs-title", v === "dir" ? "ANNUAIRE" : "CHAT · FICHIERS"); // corrigé au prochain état si salon
  if (v === "dir") invoke("net_directory").catch(() => {});
}
document.querySelectorAll("#bbs-tabs button").forEach((b) => b.addEventListener("click", () => setBbsView(b.dataset.view)));
function dialFromDir(num) {
  $("dial-number").value = num;
  invoke("at_command", { line: "ATDT" + num });
}
$("dir-list").addEventListener("click", (e) => {
  const b = e.target.closest("button[data-num]");
  if (b) dialFromDir(b.dataset.num);
});
let dirKey = null;
function renderDir(net) {
  // Ne reconstruire la liste que si elle change (sinon les clics se perdent).
  const key = JSON.stringify(net ? [net.registered, net.number, net.directory, net.services, net.server] : null);
  if (key === dirKey) return;
  dirKey = key;
  const list = $("dir-list");
  list.textContent = "";
  const esc = (t) => String(t).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  if (!net || !net.registered) {
    setText("dir-title", "ANNUAIRE");
    setText("dir-count", "");
    list.innerHTML = `<div class="dir-note">${net ? "Connexion au central en cours…" :
      "Tu es en <b>boucle locale</b> : pas de central, donc pas d'annuaire. Tous les numéros répondent ici (modem simulé), le 08 36 15 42 86 compris.<br><br>Clique sur le badge en haut à droite pour te connecter à ton central."}</div>`;
    return;
  }
  setText("dir-title", `ANNUAIRE · ${net.server.replace(/:\d+$/, "")}`);
  const dir = net.directory || [];
  const on = dir.filter((d) => d.status !== "absent").length;
  setText("dir-count", dir.length ? `${on}/${dir.length} en ligne` : "");
  const label = { online: "en ligne", busy: "en ligne · occupé", absent: "absent" };
  const order = { online: 0, busy: 1, absent: 2 };
  const rows = [...dir].sort((a, b) => (a.number === net.number ? -1 : b.number === net.number ? 1 : order[a.status] - order[b.status] || a.name.localeCompare(b.name)));
  let html = "";
  for (const d of rows) {
    const me = d.number === net.number;
    const action = me ? `<span class="state">c'est toi</span>`
      : d.status === "online" ? `<button class="key primary" data-num="${esc(d.number)}">☎ APPELER</button>`
      : `<span class="state">${label[d.status]}</span>`;
    html += `<div class="dir-row ${d.status}"><i class="dot"></i><span class="who">${esc(d.name)}${me ? " <small>(toi)</small>" : ""}</span><span class="num">${fmtNum(d.number)}</span>${action}</div>`;
  }
  if (!dir.length) html += `<div class="dir-note">Le central n'a pas encore envoyé l'annuaire (ou il est trop ancien : mets-le à jour).</div>`;
  const svc = net.services || [];
  if (svc.length) {
    html += `<div class="dir-sep">── NUMÉROS SPÉCIAUX ──</div>`;
    const names = { fai: "Fournisseur d'accès (Internet)", echo: "Ligne de test (écho)" };
    for (const [num, kind] of svc) {
      html += `<div class="dir-row online"><i class="dot"></i><span class="who">${esc(names[kind] || kind)}</span><span class="num">${fmtNum(num)}</span><button class="key" data-num="${esc(num)}">☎ APPELER</button></div>`;
    }
  }
  list.innerHTML = html;
}

/* ---------------------------------------------------------------- démarrage */
buildSliders();
// Réglages du panneau CONFIG d'abord : avant toute connexion ou numérotation.
cfgRender();
cfgSend();
// Le modem commentateur (commentateur.js) : bandeau EN DIRECT + karaoké
const cast = typeof Commentateur !== "undefined" ? Commentateur.monter({ bridge, store }) : { actif: () => false };
loadPresets().catch((e) => crt("Préréglages indisponibles : " + e, "ev warn"));
invoke("app_info").then((info) => {
  crt(`V34 TINYVERSE v${info.version} — ${DEMO ? "MODE DÉMO (hors Tauri)" : "prêt"}`, "info");
  crt("Tape ATDT<numéro> ou clique APPELER. ATM2 = HP toujours, ATL0-3 = volume, ATH = raccrocher.", "info");
  crt("OK", "result");
  netStartup(info);
});
requestAnimationFrame(frame);
$("crt-input").focus();

/* ================================================================ mode démo
 * Simule un backend pour prévisualiser l'IHM dans un navigateur. */
function makeDemoBridge() {
  const listeners = {};
  const emit = (ev, p) => (listeners[ev] || []).forEach((cb) => cb(p));
  let t0 = null, line = null, spk = { mode: 1, level: 2, volume: 0.8 };
  const P = (o) => Object.assign({
    band_low_hz: 0, band_high_hz: 4000, attenuation_db: 0, hf_tilt_db: 0, snr_db: 100, impulses_per_s: 0,
    freq_offset_hz: 0, delay_ms: 0, echo_db: -100, echo_delay_ms: 0, dropouts_per_s: 0 }, o);
  const presetsDemo = [
    { name: "Parfaite", params: P({}) },
    { name: "Urbaine", params: P({ band_low_hz: 200, band_high_hz: 3700, attenuation_db: 6, hf_tilt_db: 2, snr_db: 42 }) },
    { name: "Campagne", params: P({ band_low_hz: 300, band_high_hz: 3400, attenuation_db: 12, hf_tilt_db: 6, snr_db: 33 }) },
    { name: "Longue distance", params: P({ band_low_hz: 300, band_high_hz: 3300, attenuation_db: 18, hf_tilt_db: 9, snr_db: 27 }) },
    { name: "Orage", params: P({ band_low_hz: 300, band_high_hz: 3200, attenuation_db: 15, hf_tilt_db: 8, snr_db: 20, impulses_per_s: 3 }) },
  ];
  line = { ...presetsDemo[0].params };
  // index.html#t=15 : démarre la démo directement à 15 s (aperçu en ligne)
  const m = /#t=([\d.]+)/.exec(location.hash);
  if (m) t0 = performance.now() - parseFloat(m[1]) * 1000;
  const timeline = [
    [0, "dialing", "Idle", "Tonalité · DTMF"], [3.2, "connecting", "Phase1", "ANSam"], [5.5, "connecting", "Phase1", "CM"],
    [7, "connecting", "Phase2", "INFO0c"], [8, "connecting", "Phase2", "L1/L2"], [10, "connecting", "Phase3", "PP · TRN"],
    [12, "connecting", "Phase4", "MP"], [13.5, "online", "Data", "B1 / données"],
  ];
  let prev = null; // étape précédente : événements pour le commentateur
  setInterval(() => {
    const el = t0 ? (performance.now() - t0) / 1000 : 0;
    let cur = [0, "idle", "Idle", ""];
    if (t0) for (const step of timeline) if (el >= step[0]) cur = step;
    const [, call, phase, sig] = cur;
    if (cur !== prev) {
      const ev = (type, data) => emit("v34:event", { side: "local", event: { type, data } });
      if (!prev || prev[2] !== phase) ev("PhaseChanged", phase);
      if (sig === "ANSam") ev("Log", { level: "Info", msg: "ANSam détecté (démo)" });
      else if (sig) ev("TxSignal", sig);
      if (phase === "Data") ev("Connected", { tx_rate: 33600, rx_rate: 33600 });
      prev = cur;
    }
    const online = phase === "Data";
    const spectrum = Array.from({ length: 128 }, (_, i) => {
      const f = i * 31.25;
      let v = -85 + Math.random() * 6;
      if (call === "dialing") v = Math.max(v, -18 - Math.abs(f - 440) / 3);
      if (phase === "Phase1") v = Math.max(v, -10 - Math.abs(f - 2100) / 4, -30 - Math.abs(f - 1080) / 6);
      if (phase === "Phase2") v = Math.max(v, f > 150 && f < 3750 ? -30 + Math.random() * 6 : v);
      if (phase === "Phase3" || phase === "Phase4" || online) v = Math.max(v, Math.abs(f - 1959) < 1700 ? -24 + Math.random() * 4 : v);
      return v;
    });
    const scope = Array.from({ length: 256 }, (_, i) => (t0 ? 0.3 : 0.01) * Math.sin(i / (online ? 2.1 : 3.8)) * (online ? Math.random() + 0.3 : 1));
    const constellation = (phase === "Phase3" || phase === "Phase4" || online)
      ? Array.from({ length: 64 }, () => {
          const n = online ? 4 : 2, g = () => (Math.floor(Math.random() * n) * 2 - (n - 1)) / (n - 1);
          return [g() + (Math.random() - 0.5) * 0.12, g() + (Math.random() - 0.5) * 0.12];
        }) : [];
    emit("v34:state", {
      status: {
        role: "Call", phase, tx_signal: sig, rx_signal: sig, constellation, bytes_tx: 0, bytes_rx: 0,
        leds: { hs: online, aa: false, cd: online, oh: !!t0, rd: online && Math.random() > 0.6, sd: online && Math.random() > 0.7, tr: true, mr: true },
        params: {
          v8_modes: el > 6 ? ["V.34", "V.32bis"] : [], symbol_rate_tx: el > 10 ? 3429 : null, symbol_rate_rx: el > 10 ? 3429 : null,
          carrier_tx_hz: el > 10 ? 1959 : null, carrier_rx_hz: null, preemphasis_tx: null,
          data_rate_tx: online ? 33600 : null, data_rate_rx: online ? 33600 : null, rtd_ms: el > 8 ? 12.5 : null,
          snr_db: el > 9 ? 41.2 : null, projected_rates: [], trellis_states: online ? 16 : null, line_response: [],
        },
      },
      remote_phase: phase, call_state: call, link: "démo", speaker: { ...spk, active: !!t0 && !online },
      spectrum, scope, line, rx_level_db: t0 ? -14 : -100, tx_level_db: t0 ? -12 : -100, elapsed_s: el,
      audio_device: null,
      line_response: Array.from({ length: 96 }, (_, i) => {
        const f = (i / 95) * 4000;
        const inBand = f >= line.band_low_hz && f <= line.band_high_hz;
        return [f, -line.attenuation_db - line.hf_tilt_db * (f / 3000) - (inBand ? 0 : 30)];
      }),
    });
  }, 40);
  return {
    listen: (ev, cb) => { (listeners[ev] = listeners[ev] || []).push(cb); return Promise.resolve(() => {}); },
    invoke: (cmd, args) => {
      if (cmd === "at_command") {
        const l = args.line.toUpperCase();
        if (l.startsWith("ATD")) { t0 = performance.now(); emit("v34:console", { text: `Numérotation ${l.slice(4)}…`, kind: "info" }); setTimeout(() => emit("v34:console", { text: "CONNECT 33600/V34", kind: "result" }), 13500); }
        else if (l.startsWith("ATH")) { t0 = null; emit("v34:console", { text: "OK", kind: "result" }); }
        else emit("v34:console", { text: "OK", kind: "result" });
      }
      if (cmd === "line_presets") return Promise.resolve(presetsDemo);
      if (cmd === "set_line") line = { ...args.params };
      if (cmd === "set_speaker") for (const k of ["mode", "level", "volume"]) if (args[k] != null) spk[k] = args[k];
      if (cmd === "send_chat") { emit("v34:chat", { from: "local", text: args.text }); setTimeout(() => emit("v34:chat", { from: "remote", text: args.text + "\n" }), 300); }
      if (cmd === "send_file") return Promise.reject("Transfert de fichiers indisponible en mode démo (navigateur).");
      if (cmd === "app_info") return Promise.resolve({ version: "0.1.0", audio_device: null });
      if (cmd === "set_modem_config") {
        const r = args.maxRate;
        return Promise.resolve({ v90: args.v90 && r >= 28000, max_rate: Math.min(r, 33600), max_rate_pcm: Math.min(56000, Math.max(28000, r)) });
      }
      return Promise.resolve(null);
    },
  };
}
