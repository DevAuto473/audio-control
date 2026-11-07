const { invoke } = window.__TAURI__.core;
const $ = (id) => document.getElementById(id);
const vol = $("vol"), pct = $("pct"), status = $("status");

const T = {
  en: {
    power: "Turn on / off", mute: "Mute", unmute: "Unmute", autostart: "Start with system",
    on_linux: "Running. All system audio goes through Audio Boost.",
    on_windows: "Running. Windows playback device must be CABLE Input.",
    off: "Off. Audio goes straight to your speakers.",
    died: "The audio engine stopped. Normal audio was restored.",
  },
  ar: {
    power: "تشغيل / إيقاف", mute: "كتم", unmute: "إلغاء الكتم", autostart: "التشغيل مع النظام",
    on_linux: "يعمل. كل صوت النظام يمر عبر Audio Boost.",
    on_windows: "يعمل. يجب أن يكون جهاز التشغيل في ويندوز هو CABLE Input.",
    off: "متوقف. الصوت يذهب مباشرة إلى السماعات.",
    died: "توقف محرك الصوت. تمت استعادة الصوت العادي.",
  },
};
const store = {
  get(k, d) { try { const v = localStorage.getItem(k); return v === null ? d : JSON.parse(v); } catch { return d; } },
  set(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch {} },
};
let lang = store.get("lang", (navigator.language || "en").startsWith("ar") ? "ar" : "en");
let gain = store.get("gain", 100);
let output = store.get("output", null);
let muted = false, running = false, lastMsg = null;
const t = (k) => T[lang][k];

function applyLang() {
  document.documentElement.lang = lang;
  document.documentElement.dir = lang === "ar" ? "rtl" : "ltr";
  document.querySelectorAll("[data-i18n]").forEach((e) => (e.textContent = t(e.dataset.i18n)));
  document.querySelectorAll("[data-i18n-title]").forEach((e) => (e.title = t(e.dataset.i18nTitle)));
  $("mute").textContent = muted ? t("unmute") : t("mute");
  $("lang").value = lang;
  if (lastMsg) say(lastMsg);
}
function say(key, err) { lastMsg = key; status.textContent = T[lang][key] || key; status.classList.toggle("err", !!err); }

function paint() {
  vol.value = gain;
  pct.textContent = Math.round(gain);
  vol.style.setProperty("--fill", (gain / 300) * 100 + "%");
  document.querySelectorAll("#presets button").forEach((b) => b.classList.toggle("on", +b.dataset.v === Math.round(gain)));
}
function setGain(v) {
  gain = Math.max(0, Math.min(300, Math.round(v)));
  store.set("gain", gain);
  paint();
  if (running) invoke("set_gain", { gain: gain / 100 });
}

async function start() {
  try {
    const s = await invoke("start", { output, gain: gain / 100 });
    running = s.running;
    if (s.output) $("output").value = s.output;
    if (muted) invoke("set_muted", { muted });
    say(s.platform === "windows" ? "on_windows" : "on_linux");
  } catch (e) {
    running = false;
    $("power").checked = false;
    say(String(e), true);
  }
}
async function stop() { await invoke("stop"); running = false; say("off"); }

async function loadOutputs() {
  const list = await invoke("outputs");
  const sel = $("output");
  sel.innerHTML = "";
  for (const d of list) sel.add(new Option(d.name, d.id));
  // only force a device when the user picked one; otherwise follow the system default
  if (output && list.some((d) => d.id === output)) sel.value = output; else output = null;
}

vol.addEventListener("input", () => setGain(+vol.value));
vol.addEventListener("wheel", (e) => { e.preventDefault(); setGain(gain + (e.deltaY < 0 ? 5 : -5)); }, { passive: false });
vol.addEventListener("dblclick", () => setGain(100));
$("up").onclick = () => setGain(gain + 5);
$("down").onclick = () => setGain(gain - 5);
document.querySelectorAll("#presets button").forEach((b) => (b.onclick = () => setGain(+b.dataset.v)));
$("mute").onclick = () => {
  muted = !muted;
  $("mute").classList.toggle("on", muted);
  $("mute").textContent = muted ? t("unmute") : t("mute");
  if (running) invoke("set_muted", { muted });
};
$("power").onchange = (e) => (e.target.checked ? start() : stop());
$("output").onchange = (e) => { output = e.target.value; store.set("output", output); if ($("power").checked) start(); };
$("lang").onchange = (e) => { lang = e.target.value; store.set("lang", lang); applyLang(); };
$("autostart").onchange = async (e) => {
  try { await invoke(e.target.checked ? "plugin:autostart|enable" : "plugin:autostart|disable"); }
  catch (err) { e.target.checked = !e.target.checked; say(String(err), true); }
};
window.addEventListener("keydown", (e) => {
  if (e.target.tagName === "SELECT") return;
  if (e.key === "ArrowUp" || e.key === "ArrowRight") { setGain(gain + 5); e.preventDefault(); }
  if (e.key === "ArrowDown" || e.key === "ArrowLeft") { setGain(gain - 5); e.preventDefault(); }
  if (e.key.toLowerCase() === "m") $("mute").click();
});

// if the engine dies, say so instead of leaving a switch that lies
setInterval(async () => {
  if (document.hidden || !$("power").checked) return;
  const s = await invoke("state");
  if (running && !s.running) { running = false; $("power").checked = false; say("died", true); }
}, 3000);

paint();
applyLang();
invoke("plugin:autostart|is_enabled").then((v) => ($("autostart").checked = !!v)).catch(() => {});
loadOutputs().then(start);
