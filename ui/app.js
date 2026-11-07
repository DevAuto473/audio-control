const { invoke } = window.__TAURI__.core;
const $ = (id) => document.getElementById(id);
const vol = $("vol"), pct = $("pct"), hint = $("hint"), status = $("status");
const store = {
  get(k, d) { try { const v = localStorage.getItem(k); return v === null ? d : JSON.parse(v); } catch { return d; } },
  set(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch {} },
};
let gain = store.get("gain", 100);
let muted = false;
let output = store.get("output", null);
let running = false;

function paint() {
  vol.value = gain;
  pct.textContent = Math.round(gain);
  vol.style.setProperty("--fill", (gain / 300) * 100 + "%");
  const col = gain <= 100 ? "--accent" : gain <= 200 ? "--warn" : "--hot";
  document.documentElement.style.setProperty("--accent-now", `var(${col})`);
  hint.textContent = gain === 0 ? "Silent" : gain < 100 ? "Quieter than normal" : gain === 100 ? "Normal volume" : gain <= 200 ? "Boosted" : "Maximum boost";
  document.querySelectorAll("#presets button").forEach((b) => b.classList.toggle("on", +b.dataset.v === Math.round(gain)));
}
function setGain(v) {
  gain = Math.max(0, Math.min(300, v));
  store.set("gain", gain);
  paint();
  if (running) invoke("set_gain", { gain: gain / 100 });
}
function say(msg, err) { status.textContent = msg; status.classList.toggle("err", !!err); }

async function start() {
  try {
    const s = await invoke("start", { output, gain: gain / 100 });
    running = s.running;
    if (muted) invoke("set_muted", { muted });
    say(s.platform === "windows" ? "Running. Windows playback device must be CABLE Input." : "Running. All system audio goes through Audio Boost.");
  } catch (e) {
    running = false;
    $("power").checked = false;
    say(String(e), true);
  }
}
async function stop() { await invoke("stop"); running = false; say("Off. Audio goes straight to your speakers."); }

async function loadOutputs() {
  const list = await invoke("outputs");
  const sel = $("output");
  sel.innerHTML = "";
  for (const d of list) sel.add(new Option(d.name, d.id));
  if (output && list.some((d) => d.id === output)) sel.value = output; else output = sel.value || null;
}

vol.addEventListener("input", () => setGain(+vol.value));
$("up").onclick = () => setGain(gain + 5);
$("down").onclick = () => setGain(gain - 5);
document.querySelectorAll("#presets button").forEach((b) => (b.onclick = () => setGain(+b.dataset.v)));
$("mute").onclick = () => {
  muted = !muted;
  $("mute").classList.toggle("on", muted);
  $("mute").textContent = muted ? "Unmute" : "Mute";
  if (running) invoke("set_muted", { muted });
};
$("power").onchange = (e) => (e.target.checked ? start() : stop());
$("output").onchange = (e) => { output = e.target.value; store.set("output", output); if ($("power").checked) start(); };
window.addEventListener("keydown", (e) => {
  if (e.key === "ArrowUp" || e.key === "ArrowRight") { setGain(gain + 5); e.preventDefault(); }
  if (e.key === "ArrowDown" || e.key === "ArrowLeft") { setGain(gain - 5); e.preventDefault(); }
});

setInterval(async () => {
  if (document.hidden) return; // window hidden in tray: no UI work
  if (!running) { $("level").style.width = "0"; $("limiter").textContent = "Limiter off"; return; }
  const m = await invoke("meter");
  if (!m) return;
  $("level").style.width = Math.min(100, m.peak * 100) + "%";
  $("level").style.background = m.peak > 0.95 ? "var(--warn)" : "var(--accent)";
  const lim = $("limiter");
  const on = m.reduction_db < -0.5;
  lim.textContent = on ? `Limiter ${m.reduction_db.toFixed(1)} dB` : "Limiter idle";
  lim.classList.toggle("active", on);
}, 100);

paint();
loadOutputs().then(start);
