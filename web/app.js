"use strict";
// Storm picker front end. No dependencies.

const $ = (id) => document.getElementById(id);
const dark = () => matchMedia("(prefers-color-scheme: dark)").matches;
const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
const SERIES = {
  light: ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6", "#e34948",
          "#0e9aa7", "#8b5a2b", "#7aa81e", "#b5359b", "#27408b", "#8e2c3e", "#c49a5a"],
  dark: ["#3987e5", "#d95926", "#199e70", "#c98500", "#d55181", "#0ca30c", "#9085e9", "#e66767",
         "#23b5c2", "#b07a45", "#93c43a", "#d158b8", "#6f86d6", "#c2566a", "#d6b072"],
};
const palette = () => SERIES[dark() ? "dark" : "light"];

// ---------------------------------------------------------------- worker RPC
// All analysis runs in a Web Worker (the Rust engine as WebAssembly); nothing is sent to a
// server. It answers one request at a time and cannot be interrupted, so slow work that is
// no longer wanted (similar regions, after a new threshold or Stop) is abandoned by ending
// the worker and starting another on the same strikes with the saved graph (see
// restartEngine): never two graphs in memory.
let nextId = 1;
function startWorker(onProgress) {
  const pending = new Map();
  const w = new Worker(`worker.js?v=${Date.now()}`, { type: "module" }); // never a stale cached copy
  w.onmessage = (ev) => {
    const m = ev.data;
    if (m.type === "simProgress") return onProgress(m.pass, m.fraction);
    if (m.type === "buildProgress") {
      if (busyText) showBusy(`${busyText} · Building graph`, m.fraction);
      return;
    }
    if (m.type === "status") { $("eng-status").textContent = m.text; $("eng-status").hidden = !m.text; return; }
    if (m.type === "graphSaved") { savedGraphs.add(m.key); return; }
    const p = pending.get(m.id);
    if (!p) return;
    pending.delete(m.id);
    if (m.error) p.reject(new Error(m.error)); else p.resolve(m.result);
  };
  w.onerror = (e) => { $("eng-status").textContent = `Engine failed: ${e.message}`; $("eng-status").hidden = false; };
  return {
    call(cmd, ...args) {
      const id = nextId++;
      return new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject });
        w.postMessage({ id, cmd, args });
      });
    },
    // Stop it; what it was doing fails with `why`.
    end(why) {
      w.terminate();
      for (const p of pending.values()) p.reject(new Error(why));
      pending.clear();
    },
  };
}
let engine = startWorker((pass, fraction) => showSimProgress(pass, fraction));
const call = (cmd, ...args) => engine.call(cmd, ...args);

// Abandon the similar-region work under way (if any): a new worker on the loaded file. Only
// once its graph is saved (else the new worker would build it again: waiting is quicker).
let restoreInfo = null; // from loadSource: what restore needs
const savedGraphs = new Set(); // keys of graphs known to be in the browser's storage
async function restartEngine(why) {
  if (!simRunning.length || !restoreInfo || !savedGraphs.has(restoreInfo.key)) return;
  engine.end(why);
  simRunning.length = 0; simProg.clear(); renderProgress();
  engine = startWorker((pass, fraction) => showSimProgress(pass, fraction));
  busy("Restarting the engine", null);
  try { await call("restore", sourceFile, restoreInfo, A.t0, A.t); }
  finally { busy(null); }
}

// Header progress for slow steps (reading, building the graph). busy(null) hides it.
// While a file loads, the worker's graph-building progress fills the bar, after the text.
let busyText = null;
function busy(text, fraction) {
  busyText = text;
  showBusy(text, fraction);
}
function showBusy(text, fraction) {
  $("busy").hidden = !text;
  $("busy-text").textContent = text ? text + (fraction != null ? ` ${Math.round(fraction * 100)}%` : " …") : "";
  if (fraction != null) $("busy-bar").value = fraction; else $("busy-bar").removeAttribute("value");
}

const fmtTime = (ms) => new Date(ms).toISOString().replace("T", " ").slice(0, 19) + "Z";
const fmtDur = (sec) => {
  const m = Math.round(sec / 60), d = Math.floor(m / 1440), h = Math.floor((m % 1440) / 60), mm = m % 60;
  return d ? `${d} d ${h} h ${mm} min` : h ? `${h} h ${mm} min` : `${mm} min`;
};
const fmtNum = (v, d = 1) => Number(v).toLocaleString(undefined, { maximumFractionDigits: d });

// ---------------------------------------------------------------- state
let A = null;         // open strikes: {meta, t0, t (s since t0), lat, lon}
let V = null;         // the persistence map's box: {frontier [gap min, km]*, xMin, xMax (km), yMin, yMax (min)}
let inView = null;    // strikes in the shown time range and map area (only shown, never used)
let sel = null;       // selection response (x km, y min)
let pick = null;      // isolated feature index (into sel.features) or null
let sim = null;      // similar-cluster set for the picked cluster (from /similar)
let tLo = 0, tHi = 1; // shown fraction of the time range
let vLo = 0, vHi = 1; // part of the time range the slider spans (scroll on it to zoom)

// ---------------------------------------------------------------- canvases
function fitCanvas(c) {
  const r = c.getBoundingClientRect(), dpr = devicePixelRatio || 1;
  const w = Math.max(1, Math.round(r.width * dpr)), h = Math.max(1, Math.round(r.height * dpr));
  if (c.width !== w || c.height !== h) { c.width = w; c.height = h; }
  return { w, h, dpr, ctx: c.getContext("2d") };
}

// Persistence map geometry: normalized [0,1]^2, origin bottom-left, with margins.
const PM = { l: 46, r: 10, t: 10, b: 36 };
function persFrame(c) {
  const f = fitCanvas(c), m = {};
  for (const k in PM) m[k] = PM[k] * f.dpr;
  f.pw = f.w - m.l - m.r; f.ph = f.h - m.t - m.b; f.m = m;
  f.X = (x) => m.l + x * f.pw;
  f.Y = (y) => m.t + (1 - y) * f.ph;
  return f;
}

// Log axes over the dataset's box: km and minutes <-> [0, 1] (clipped; 0 and below map to 0).
const nx = (km) => Math.min(1, Math.max(0, Math.log(km / V.xMin) / Math.log(V.xMax / V.xMin)));
const ny = (min) => Math.min(1, Math.max(0, Math.log(min / V.yMin) / Math.log(V.yMax / V.yMin)));
const kmAt = (u) => V.xMin * Math.pow(V.xMax / V.xMin, u);
const minAt = (u) => V.yMin * Math.pow(V.yMax / V.yMin, u);
const fmtGap = (min) => (min < 1 / 60 ? `${fmtNum(min * 60000, min * 60000 < 1 ? 3 : 0)} ms`
  : min < 1 ? `${fmtNum(min * 60, 1)} s` : min < 120 ? `${fmtNum(min)} min` : `${fmtNum(min / 60)} h`);
const fmtKm = (km) => (km < 1 ? `${fmtNum(km * 1000, km < 0.001 ? 3 : 0)} m` : `${fmtNum(km, km < 10 ? 2 : 1)} km`);
// A cluster within a selection: its first strike and size (stable across re-selections).
const keyOf = (f) => `${f.first}:${f.size}`;
const hexRgb = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));

// Rows [ylo, yhi (min), xlo, xhi (km)]* on the map's axes ([0, 1]), non-empty, by ylo.
function mapRows(R) {
  const out = [];
  for (let k = 0; k < R.length; k += 4) {
    const r = [ny(R[k]), ny(R[k + 1]), nx(R[k + 2]), nx(R[k + 3])];
    if (r[0] < r[1] && r[2] < r[3]) out.push(r);
  }
  return out.sort((p, q) => p[0] - q[0]);
}

// Fill and outline a region (rows on the map's axes): shaded inside, with its edge.
function drawRegion(f, ctx, rows, fill, alpha, edge, width) {
  if (!rows.length) return;
  const area = new Path2D();
  for (const [y0, y1, x0, x1] of rows) {
    // Snap to whole pixels so abutting rows don't leave hairline gaps.
    const L = Math.floor(f.X(x0)), R = Math.ceil(f.X(x1)), T = Math.floor(f.Y(y1)), B = Math.ceil(f.Y(y0));
    area.rect(L, T, R - L, B - T);
  }
  ctx.save();
  ctx.fillStyle = fill; ctx.globalAlpha = alpha; ctx.fill(area);
  ctx.restore();
  // Edge: the boundary of the union, found as the outline of a filled mask.
  const mask = document.createElement("canvas"); mask.width = f.w; mask.height = f.h;
  const mc = mask.getContext("2d"); mc.fillStyle = "#000"; mc.fill(area);
  const ring = document.createElement("canvas"); ring.width = f.w; ring.height = f.h;
  const rc = ring.getContext("2d"), r = width * f.dpr;
  for (const [dx, dy] of [[-r, 0], [r, 0], [0, -r], [0, r], [-r, -r], [r, r], [-r, r], [r, -r]]) rc.drawImage(mask, dx, dy);
  rc.globalCompositeOperation = "destination-out"; rc.drawImage(mask, 0, 0);
  rc.globalCompositeOperation = "source-in"; rc.fillStyle = edge; rc.fillRect(0, 0, f.w, f.h);
  ctx.drawImage(ring, 0, 0);
}

let picked = null; // {key, rows} the picked cluster's region (computed when picked)

function drawPers() {
  if (!A || !V) return;
  const c = $("pers"), f = persFrame(c), ctx = f.ctx;
  ctx.clearRect(0, 0, f.w, f.h);
  ctx.fillStyle = css("--surface-1"); ctx.fillRect(f.m.l, f.m.t, f.pw, f.ph);
  // Frontier: up and right of it, all the strikes are in one cluster.
  if (V.frontier.length) {
    const F = V.frontier, line = new Path2D(), beyond = new Path2D();
    line.moveTo(f.X(1), f.Y(ny(F[0])));
    beyond.moveTo(f.X(1), f.Y(ny(F[0])));
    for (let k = 0; k < F.length; k += 2) {
      const y = ny(F[k]), x = nx(F[k + 1]), yNext = k + 2 < F.length ? ny(F[k + 2]) : 1;
      for (const p of [line, beyond]) { p.lineTo(f.X(x), f.Y(y)); p.lineTo(f.X(x), f.Y(yNext)); }
    }
    beyond.lineTo(f.X(1), f.Y(1)); beyond.closePath();
    ctx.save();
    ctx.fillStyle = css("--text-muted"); ctx.globalAlpha = 0.12; ctx.fill(beyond);
    ctx.globalAlpha = 1;
    ctx.setLineDash([5 * f.dpr, 4 * f.dpr]);
    ctx.strokeStyle = css("--text-secondary"); ctx.lineWidth = 1.5 * f.dpr;
    ctx.stroke(line);
    ctx.restore();
  }
  if (sel && pick != null) {
    const ft = sel.features[pick], col = pickColor(pick);
    // Similar region: where a cluster within the change tolerance exists.
    if (sim && sim.key === keyOf(ft) && sim.rows) drawRegion(f, ctx, mapRows(sim.rows), col, 0.18, col, 1.5);
    // The picked cluster's own region.
    if (picked && picked.key === keyOf(ft)) drawRegion(f, ctx, mapRows(picked.rows), col, 0.45, col, 2.5);
  }
  // Axes (log): labels at 1, 2, 5 x powers of ten, skipping any that would overlap.
  ctx.strokeStyle = css("--border"); ctx.lineWidth = f.dpr;
  ctx.strokeRect(f.m.l, f.m.t, f.pw, f.ph);
  ctx.fillStyle = css("--text-secondary");
  ctx.font = `${11 * f.dpr}px system-ui, sans-serif`;
  const ticks = (lo, hi) => {
    const out = [];
    for (let e = Math.floor(Math.log10(lo)); e <= Math.ceil(Math.log10(hi)); e++) {
      for (const m of [1, 2, 5]) { const v = m * Math.pow(10, e); if (v >= lo && v <= hi) out.push(v); }
    }
    return out;
  };
  // Prefer powers of ten: place those first, then 2s and 5s where they fit.
  const place = (vals, pos, size, gap) => {
    const taken = [];
    const order = [...vals].sort((a, b) => (String(a).startsWith("1") ? 0 : 1) - (String(b).startsWith("1") ? 0 : 1));
    const out = [];
    for (const v of order) {
      const p = pos(v), h = size(v) / 2 + gap;
      if (taken.some(([a, b]) => p + h > a && p - h < b)) continue;
      taken.push([p - h, p + h]); out.push(v);
    }
    return out;
  };
  ctx.textAlign = "center"; ctx.textBaseline = "top";
  const xs = place(ticks(V.xMin, V.xMax), (v) => f.X(nx(v)), (v) => ctx.measureText(fmtKm(v)).width, 6 * f.dpr);
  for (const v of xs) ctx.fillText(fmtKm(v), f.X(nx(v)), f.m.t + f.ph + 4 * f.dpr);
  ctx.fillText("distance", f.X(0.5), f.m.t + f.ph + 18 * f.dpr);
  ctx.textAlign = "right"; ctx.textBaseline = "middle";
  const ys = place(ticks(V.yMin, V.yMax), (v) => f.Y(ny(v)), () => 11 * f.dpr, 3 * f.dpr);
  for (const v of ys) ctx.fillText(fmtGap(v), f.m.l - 4 * f.dpr, f.Y(ny(v)));
  ctx.save(); ctx.translate(10 * f.dpr, f.Y(0.5)); ctx.rotate(-Math.PI / 2);
  ctx.textAlign = "center"; ctx.fillText("time gap", 0, 0); ctx.restore();
  if (sel) { // selected point
    ctx.strokeStyle = css("--text-primary"); ctx.lineWidth = 1.5 * f.dpr;
    const px = f.X(nx(sel.x)), py = f.Y(ny(sel.y)), r = 6 * f.dpr;
    ctx.beginPath(); ctx.arc(px, py, r, 0, 2 * Math.PI); ctx.stroke();
    ctx.beginPath(); ctx.moveTo(px - 2 * r, py); ctx.lineTo(px - r, py); ctx.moveTo(px + r, py); ctx.lineTo(px + 2 * r, py);
    ctx.moveTo(px, py - 2 * r); ctx.lineTo(px, py - r); ctx.moveTo(px, py + r); ctx.lineTo(px, py + 2 * r); ctx.stroke();
  }
}

function persPoint(ev) {
  const c = $("pers"), f = persFrame(c), r = c.getBoundingClientRect();
  const x = ((ev.clientX - r.left) * f.dpr - f.m.l) / f.pw;
  const y = 1 - ((ev.clientY - r.top) * f.dpr - f.m.t) / f.ph;
  return { x, y, inside: x >= 0 && x < 1 && y >= 0 && y < 1, f };
}

$("pers").addEventListener("mousemove", (ev) => {
  const tip = $("pers-tip");
  if (!A || !V) return;
  const p = persPoint(ev);
  if (!p.inside) { tip.hidden = true; return; }
  tip.textContent = `${fmtKm(kmAt(p.x))} · ${fmtGap(minAt(p.y))}`;
  tip.hidden = false;
  const r = $("pers").getBoundingClientRect();
  tip.style.left = `${ev.clientX - r.left + 12}px`; tip.style.top = `${ev.clientY - r.top + 12}px`;
});
$("pers").addEventListener("mouseleave", () => { $("pers-tip").hidden = true; });
$("pers").addEventListener("click", async (ev) => {
  if (!A || !V) return;
  const p = persPoint(ev);
  if (!p.inside) return;
  await selectAt(kmAt(p.x), minAt(p.y));
});
// Every cluster at (km, min); keeps the picked cluster if it is still there.
async function selectAt(km, min, keep = null) {
  $("sel-head").textContent = "Finding clusters…";
  try {
    // New thresholds: similar regions still being computed for the old ones are moot, and
    // would hold this up; drop them first.
    const moving = !sel || sel.x !== km || sel.y !== min;
    if (moving) { stabSeq++; simSeq++; await restartEngine("Superseded"); }
    const before = sel;
    sel = await call("select", km, min);
    if (!before || before.x !== sel.x || before.y !== sel.y) simCache.clear();
    const again = keep == null ? -1 : sel.features.findIndex((f) => keyOf(f) === keep);
    pick = again >= 0 ? again : null;
    sim = null; picked = null; stab = [];
    renderFeatures(); drawPers(); drawMap(); loadSimilar(); loadStability();
  } catch (e) { $("sel-head").textContent = e.message; }
}

// ---------------------------------------------------------------- features list
const NAMED = 15; // fixed-order categorical slots (stability is computed for these); others neutral
function featureColor(idx) {
  return idx != null && idx < NAMED ? palette()[idx] : null;
}
// Colour of the picked cluster: its slot colour, or the dedicated selection ink if grey.
const pickColor = (idx) => featureColor(idx) || css("--selected");
// Shade for strikes that are only in similar clusters: darker (lighter for the ink).
function extraShade(col) {
  const mix = (a, b, t) => "#" + hexRgb(a).map((v, k) => Math.round(v + (hexRgb(b)[k] - v) * t)
    .toString(16).padStart(2, "0")).join("");
  return col === css("--selected") ? mix(col, css("--surface-1"), 0.5) : mix(col, dark() ? "#ffffff" : "#000000", 0.4);
}

// Change tolerance in %, kept below 100: then similar clusters' regions never overlap,
// which is what makes the stability area exact. Stability is the share of the map's
// (log-axis) box covered by the similar clusters' regions.
const TOL_MAX = 99;
const tolPct = () => Math.min(TOL_MAX, Math.max(0, +$("tol").value || 0));

// Stability of list row idx: from the coloured clusters' run, or the picked cluster's
// similar-cluster query. "" for rows that get none.
function stabilityText(f, idx) {
  let v = stab[idx];
  if (v == null && idx === pick && sim && sim.key === keyOf(f)) v = sim.error ? { error: sim.error } : sim.stability;
  if (v == null) return idx < NAMED || idx === pick ? "stability …" : "";
  if (v.error) return v.error;
  const pct = v * 100;
  return `stability <b>${pct >= 1 ? fmtNum(pct) : fmtNum(pct, 2)}%</b>`;
}

// Similar-cluster queries run one at a time in the worker, in the order sent; progress
// reports refer to the oldest one still running. Each list row shows its query's progress.
const simRunning = []; // registry ids of the clusters whose queries are queued, oldest first
const simProg = new Map(); // registry id -> {pass, fraction}
async function callSimilar(f, s, tol, union = false) {
  simRunning.push(keyOf(f));
  renderProgress();
  try { return await call("similar", f.component, s.x, s.y, tol, union); }
  finally {
    const k = simRunning.indexOf(keyOf(f));
    if (k >= 0) simRunning.splice(k, 1);
    simProg.delete(keyOf(f));
    renderProgress();
  }
}
// One similar-region query gives both a cluster's similar region and its stability, so the
// list and the picked cluster share results, by cluster and tolerance. Kept only while the
// thresholds stay the same (cleared by selectAt when they change): picking a cluster or
// changing the view there reuses them. Always with the similar clusters' strikes, for the
// map.
const simCache = new Map();
const simKey = (f, tol) => `${keyOf(f)}|${tol}`;
function cachedSimilar(f, s, tol) {
  const key = simKey(f, tol);
  let r = simCache.get(key); // the request itself, so one already running is shared too
  if (!r) {
    r = callSimilar(f, s, tol, true);
    simCache.set(key, r);
    r.catch(() => { if (simCache.get(key) === r) simCache.delete(key); }); // failed or stopped: ask again
  }
  return r;
}
function showSimProgress(pass, fraction) {
  if (!simRunning.length) return;
  simProg.set(simRunning[0], { pass, fraction });
  renderProgress();
}
// Progress bars in the list rows (updated in place: no full re-render).
function renderProgress() {
  for (const el of document.querySelectorAll("#features .simprog")) {
    const id = el.dataset.cluster, at = simRunning.indexOf(id), p = simProg.get(id);
    el.hidden = at < 0;
    if (at < 0) continue;
    const bar = el.querySelector("progress"), txt = el.querySelector("span");
    if (at > 0 || !p) { bar.removeAttribute("value"); txt.textContent = at > 0 ? "similar clusters: waiting" : "similar clusters…"; }
    else { bar.value = p.fraction; txt.textContent = `similar clusters: pass ${p.pass + 1}`; }
  }
}

// Stability of the coloured clusters, top of the list down, one worker call at a time so
// the picked cluster's query can run in between. Restarted when the point or tolerance
// changes.
let stab = [], stabSeq = 0;
async function loadStability() {
  const seq = ++stabSeq;
  stab = [];
  if (!sel) return;
  const tol = tolPct() / 100, s = sel;
  for (let idx = 0; idx < Math.min(NAMED, s.features.length); idx++) {
    let v;
    try {
      const r = await cachedSimilar(s.features[idx], s, tol);
      v = r.stability;
    } catch (e) { v = { error: e.message }; }
    if (seq !== stabSeq) return; // a newer point or tolerance superseded this run
    stab[idx] = v;
    drawPers();
    renderFeatures();
  }
}

// The picked cluster's region, then its similar region (both exact). A similar-region
// result has the region too: when one is known or running, nothing else is asked for;
// otherwise the region is asked for first, to show while the similar region is computed.
let simSeq = 0;
async function loadSimilar() {
  if (!sel || pick == null) return;
  const f = sel.features[pick], seq = ++simSeq, tol = tolPct() / 100, key = keyOf(f);
  try {
    if ((!picked || picked.key !== key) && !simCache.has(simKey(f, tol))) {
      const r = await call("region", f.component, sel.x, sel.y);
      if (seq !== simSeq) return;
      picked = { key, rows: r.rows };
      drawPers();
    }
    const r = await cachedSimilar(f, sel, tol); // with its strikes, for the map
    if (seq !== simSeq) return; // a newer request superseded this one
    if (!picked || picked.key !== key) picked = { key, rows: r.region };
    sim = Object.assign(r, { key });
  } catch (e) {
    if (seq !== simSeq) return;
    sim = { key, error: e.message };
  }
  renderFeatures(); drawPers(); drawMap(); // drawMap also draws the slider's cluster bands
}
let tolTimer = null;
$("tol").addEventListener("change", () => { // tidy what was typed once editing ends
  if (+$("tol").value !== tolPct()) $("tol").value = tolPct();
});
$("tol").addEventListener("input", () => {
  clearTimeout(tolTimer);
  tolTimer = setTimeout(() => {
    sim = null; stab = []; renderFeatures(); drawPers(); loadSimilar(); loadStability();
  }, 250);
});

// Save a cluster (plus strikes of similar clusters, tagged with their similarity) as
// GeoJSON: ask for a title, then a file location, then build it in the worker.
let saveJob = null; // {f, x, y, tol} of the cluster the save dialog is for
function saveCluster(f) {
  const tol = tolPct() / 100;
  saveJob = { f, x: sel.x, y: sel.y, tol };
  const at = `${fmtNum(sel.x)} km, ${fmtNum(sel.y)} min`;
  const src = A.meta.source?.name || A.meta.file;
  $("s-title").value = `${src}: cluster of ${fmtNum(f.size, 0)} strikes at ${at}, change ≤ ${fmtNum(tol * 100)}%`;
  $("s-info").textContent = `${f.start.slice(0, 19).replace("T", " ")} → ${f.end.slice(0, 19).replace("T", " ")}. ` +
    `Includes the extra strikes of clusters within ${fmtNum(tol * 100)}% change.`;
  $("s-status").textContent = ""; $("s-error").textContent = "";
  $("s-save").disabled = false; $("s-bar").hidden = true;
  $("dlg-save").showModal();
  $("s-title").select();
}
$("s-cancel").onclick = () => $("dlg-save").close();
$("form-save").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const job = saveJob, title = $("s-title").value.trim();
  if (!job || !title) return;
  const num = (v) => +v.toFixed(1);
  const name = `cluster-${job.f.size}strikes-${num(job.x)}km-${num(job.y)}min-` +
    `${num(job.tol * 100)}pct.geojson`;
  // The file picker must open straight from the click, so choose the file first.
  let handle = null;
  if (window.showSaveFilePicker) {
    try {
      handle = await showSaveFilePicker({ suggestedName: name,
        types: [{ description: "GeoJSON", accept: { "application/geo+json": [".geojson"] } }] });
    } catch (e) { if (e.name === "AbortError") return; throw e; }
  }
  $("s-save").disabled = true; $("s-error").textContent = "";
  $("s-status").textContent = "Building GeoJSON…";
  $("s-bar").value = 0; $("s-bar").hidden = false;
  try {
    const box = { distance_km: [V.xMin, V.xMax], gap_min: [V.yMin, V.yMax] };
    const json = await call("clusterGeojson", job.f.component, job.x, job.y, job.tol, title, box);
    $("s-status").textContent = "Writing file…"; $("s-bar").removeAttribute("value");
    if (handle) {
      const w = await handle.createWritable();
      await w.write(json); await w.close();
    } else { // browsers without a save picker: an ordinary download
      const a = document.createElement("a");
      a.href = URL.createObjectURL(new Blob([json], { type: "application/geo+json" }));
      a.download = name; a.click(); setTimeout(() => URL.revokeObjectURL(a.href), 10000);
    }
    $("dlg-save").close();
  } catch (e) {
    $("s-error").textContent = `Could not save the cluster: ${e.message}`;
    $("s-status").textContent = "";
  } finally { $("s-save").disabled = false; $("s-bar").hidden = true; }
});

function renderFeatures() {
  const ul = $("features"); ul.innerHTML = "";
  if (!sel) return;
  $("sel-head").innerHTML = `At <b>${fmtKm(sel.x)}</b>, <b>${fmtGap(sel.y)}</b>: ` +
    `${fmtNum(sel.features.length, 0)} clusters, ${fmtNum(sel.singletons, 0)} lone strikes`;
  const frag = document.createDocumentFragment();
  const rows = sel.features.map((f, idx) => [f, idx]).filter(([, idx]) => idx < 500 || idx === pick);
  rows.forEach(([f, idx]) => {
    const li = document.createElement("li");
    if (idx === pick) li.className = "on";
    const sw = document.createElement("span"); sw.className = "sw";
    sw.style.background = idx === pick ? pickColor(idx) : featureColor(idx) || css("--strike-dim");
    const main = document.createElement("div");
    const sec = (Date.parse(f.end) - Date.parse(f.start)) / 1000;
    const rate = sec > 0 ? ` · ${fmtNum(f.size / (sec / 60))} strikes/min` : "";
    const st = stabilityText(f, idx);
    main.innerHTML = `<b>${fmtNum(f.size, 0)} strikes</b>` +
      `<div class="small muted">${sec < 60 ? `${fmtNum(sec)} s` : fmtDur(sec)}${rate}</div>` +
      (st ? `<div class="small muted">${st}</div>` : "") +
      `<div class="simprog small muted" data-cluster="${keyOf(f)}" hidden><progress max="1"></progress> <span></span> ` +
      `<button class="small stop" title="Stop the similar-region queries under way (the selection stays)">Stop</button></div>`;
    main.querySelector(".stop").addEventListener("click", (ev) => { ev.stopPropagation(); stopEngine(); });
    const save = document.createElement("button");
    save.className = "small"; save.textContent = "Save";
    save.title = "Save this cluster as GeoJSON, with the extra strikes of clusters within the change tolerance";
    save.addEventListener("click", (ev) => { ev.stopPropagation(); saveCluster(f); });
    save.addEventListener("dblclick", (ev) => ev.stopPropagation());
    li.append(sw, main, save);
    li.addEventListener("click", (ev) => {
      if (ev.detail > 1) return; // second click of a double-click: handled below
      pick = pick === idx ? null : idx;
      sim = null;
      renderFeatures(); drawPers(); drawMap(); loadSimilar();
    });
    li.addEventListener("dblclick", () => {
      if (pick !== idx) { pick = idx; sim = null; renderFeatures(); drawPers(); loadSimilar(); }
      zoomToPick();
    });
    frag.append(li);
  });
  if (sel.features.length > 500) {
    const li = document.createElement("li");
    li.className = "muted small"; li.textContent = `…and ${fmtNum(sel.features.length - 500, 0)} smaller clusters`;
    frag.append(li);
  }
  ul.append(frag);
  renderProgress();
}

// ---------------------------------------------------------------- strike map (Leaflet)
// Basemap: Esri light/dark gray canvas (no API key); strikes drawn on a canvas overlay that
// is re-projected on every move/zoom, which is far faster than one marker per strike.
let LM = null, tiles = null, overlay = null;
const TILE_URL = () =>
  `https://server.arcgisonline.com/ArcGIS/rest/services/Canvas/World_${dark() ? "Dark" : "Light"}_Gray_Base/MapServer/tile/{z}/{y}/{x}`;

function ensureMap() {
  if (LM) return;
  LM = L.map("map", { zoomAnimation: false, preferCanvas: true, worldCopyJump: true, doubleClickZoom: false });
  tiles = L.tileLayer(TILE_URL(), {
    maxZoom: 16,
    attribution: "Tiles &copy; Esri &mdash; Esri, DeLorme, NAVTEQ",
  }).addTo(LM);
  overlay = document.createElement("canvas");
  overlay.className = "strike-overlay";
  LM.getContainer().appendChild(overlay);
  LM.on("move zoom resize", drawMap);
  LM.on("moveend", scheduleView);
  LM.on("click", mapClick);
  LM.on("dblclick", mapDblClick);
  LM.setView([30, -50], 3);
}

function resetView() {
  ensureMap();
  let a = Infinity, b = -Infinity, c = Infinity, d = -Infinity;
  for (let i = 0; i < A.lat.length; i++) {
    a = Math.min(a, A.lon[i]); b = Math.max(b, A.lon[i]); c = Math.min(c, A.lat[i]); d = Math.max(d, A.lat[i]);
  }
  LM.invalidateSize();
  LM.fitBounds([[c, a], [d, b]], { padding: [16, 16] });
}

// Web Mercator by hand: world pixels at this zoom minus the container's top-left.
function projector(dpr = 1) {
  const z = LM.getZoom(), world = 256 * Math.pow(2, z);
  const o = LM.project(LM.containerPointToLatLng([0, 0]), z);
  return {
    X: (lon) => (((lon + 180) / 360) * world - o.x) * dpr,
    Y: (lat) => {
      const s = Math.sin((lat * Math.PI) / 180);
      return ((0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI)) * world - o.y) * dpr;
    },
  };
}

// Click a strike to pick the cluster it belongs to at the selected threshold point.
function mapDblClick(e) {
  mapClick(e);
  zoomToPick();
}

function mapClick(e) {
  if (!A || !sel) { $("sel-head").textContent = "Click the persistence map first, then a strike, to pick its cluster."; return; }
  const { X, Y } = projector(1), cx = e.containerPoint.x, cy = e.containerPoint.y;
  const T = A.t[A.t.length - 1], lo = tLo * T, hi = tHi * T;
  let best = -1, bd = 8 * 8; // within 8 px
  for (let i = 0; i < A.t.length; i++) {
    if (A.t[i] < lo || A.t[i] > hi) continue;
    const dx = X(A.lon[i]) - cx, dy = Y(A.lat[i]) - cy, d2 = dx * dx + dy * dy;
    if (d2 < bd) { bd = d2; best = i; }
  }
  if (best < 0) return;
  const idx = sel.features.findIndex((f) => f.component === sel.label[best]);
  if (idx >= 0 && idx === pick) return; // already picked (e.g. first click of a double-click)
  if (idx < 0) { $("sel-head").textContent = "That strike is isolated at this threshold (no cluster)."; return; }
  pick = idx; sim = null;
  renderFeatures(); drawPers(); drawMap(); loadSimilar();
  document.querySelector("#features li.on")?.scrollIntoView({ block: "nearest" });
}

// Picked cluster's time extent as [lo, hi] fractions of the analysis time range.
function pickExtent() {
  if (!A || !sel || pick == null) return null;
  const f = sel.features[pick], T = A.t[A.t.length - 1] || 1;
  const frac = (iso) => (Date.parse(iso) / 1000 - A.t0) / T;
  return [Math.max(0, frac(f.start)), Math.min(1, frac(f.end))];
}
// Time extent of the picked cluster's similar clusters (their union), or null.
function simExtent() {
  if (!A || !sel || pick == null || !sim || sim.key !== keyOf(sel.features[pick]) || !sim.union) return null;
  const T = A.t[A.t.length - 1] || 1;
  let lo = Infinity, hi = -Infinity;
  for (const v of sim.union) { const t = A.t[v]; if (t < lo) lo = t; if (t > hi) hi = t; }
  return lo <= hi ? [lo / T, hi / T] : null;
}
function drawPickBand() {
  // The cluster's extent, over a wider band in the similar-strike shade for its similar
  // clusters' extent.
  for (const [id, ext, col] of [["t-pick", pickExtent(), () => pickColor(pick)],
                                ["t-sim", simExtent(), () => extraShade(pickColor(pick))]]) {
    const band = $(id);
    band.hidden = !ext || ext[1] < vLo || ext[0] > vHi;
    if (band.hidden) continue;
    const a = clamp01(viewPos(ext[0])), b = clamp01(viewPos(ext[1]));
    band.style.left = `${a * 100}%`;
    band.style.width = `${(b - a) * 100}%`;
    band.style.background = col();
  }
}
// Double-click on a cluster: frame its strikes on the map and restrict the shown time
// range to its extent (zooming the slider in if the cluster is short).
function zoomToPick() {
  const ext = pickExtent();
  if (!ext) return;
  const pad = Math.max(1e-4, (ext[1] - ext[0]) * 0.02); // a sliver either side so edge strikes show
  const lo = ext[0] - pad, hi = ext[1] + pad, len = hi - lo;
  if (len < 0.2 * (vHi - vLo) || lo < vLo || hi > vHi) setView(lo - len, hi + len); // range takes ~1/3
  setRange(lo, hi);
  frameOnMap();
  scheduleView();
}
function frameOnMap() {
  if (!LM || !sel || pick == null) return;
  const comp = sel.features[pick].component;
  let a = Infinity, b = -Infinity, c = Infinity, d = -Infinity;
  for (let i = 0; i < sel.label.length; i++) {
    if (sel.label[i] !== comp) continue;
    a = Math.min(a, A.lon[i]); b = Math.max(b, A.lon[i]); c = Math.min(c, A.lat[i]); d = Math.max(d, A.lat[i]);
  }
  if (a > b) return;
  LM.fitBounds([[c, a], [d, b]], { padding: [24, 24], maxZoom: 12 });
}

function drawMap() {
  if (!A || !LM) return;
  drawPickBand();
  const size = LM.getSize(), dpr = devicePixelRatio || 1;
  const w = Math.round(size.x * dpr), h = Math.round(size.y * dpr);
  if (overlay.width !== w || overlay.height !== h) { overlay.width = w; overlay.height = h; }
  overlay.style.width = `${size.x}px`; overlay.style.height = `${size.y}px`;
  const ctx = overlay.getContext("2d");
  ctx.clearRect(0, 0, w, h);
  const { X, Y } = projector(dpr);

  const T = A.t[A.t.length - 1], lo = tLo * T, hi = tHi * T;
  const pal = palette(), dim = css("--strike-map");
  // component label -> colour, for the largest NAMED clusters
  const colourOf = new Map();
  let isoComp = null, isoCol = null, extraCol = null, union = null;
  if (sel) {
    sel.features.forEach((ft, idx) => { if (idx < NAMED) colourOf.set(ft.component, pal[idx]); });
    if (pick != null) {
      const ft = sel.features[pick];
      isoComp = ft.component; isoCol = pickColor(pick); extraCol = extraShade(isoCol);
      if (sim && sim.key === keyOf(ft) && sim.union) {
        union = new Uint8Array(A.t.length);
        for (const v of sim.union) union[v] = 1;
      }
    }
  }
  const selInk = css("--selected"), halo = css("--surface-1");
  const sz = Math.max(2, 2.5 * dpr);
  let shown = 0;
  const layers = [[], []]; // background, foreground
  for (let i = 0; i < A.t.length; i++) {
    if (A.t[i] < lo || A.t[i] > hi) continue;
    shown++;
    let col = dim, fg = false;
    if (sel) {
      const comp = sel.label[i];
      if (isoComp != null) {
        // Picked cluster in its colour; other strikes of similar clusters in a darker shade.
        if (comp === isoComp) { fg = true; col = isoCol; }
        else if (union && union[i]) { fg = true; col = extraCol; }
      } else if (colourOf.has(comp)) { col = colourOf.get(comp); fg = true; }
    }
    layers[fg ? 1 : 0].push(i, col);
  }
  for (const [layer, alpha] of [[layers[0], isoComp != null ? 0.18 : 0.85], [layers[1], 1]]) {
    ctx.globalAlpha = alpha;
    for (let k = 0; k < layer.length; k += 2) {
      const i = layer[k], px = X(A.lon[i]), py = Y(A.lat[i]);
      if (px < -sz || py < -sz || px > w + sz || py > h + sz) continue;
      if (layer[k + 1] === selInk) { // the selection ink gets a halo so it reads on any basemap
        ctx.fillStyle = halo; ctx.fillRect(px - sz / 2 - 1, py - sz / 2 - 1, sz + 2, sz + 2);
      }
      ctx.fillStyle = layer[k + 1];
      ctx.fillRect(px - sz / 2, py - sz / 2, sz, sz);
    }
  }
  ctx.globalAlpha = 1;
  $("t-lo-l").textContent = fmtTime(A.t0 * 1000 + lo * 1000);
  $("t-hi-l").textContent = fmtTime(A.t0 * 1000 + hi * 1000);
  $("t-count").textContent = `${fmtDur(hi - lo)} · ${fmtNum(shown, 0)} of ${fmtNum(A.t.length, 0)} strikes shown`;
}

// Time-range slider: drag either handle to resize, drag the middle to slide a fixed-length
// window, click the track to centre the window there. Arrow keys nudge (Shift: 10x).
// Position on the slider (0..1) of a fraction of the full time range, given the view.
const viewPos = (f) => (f - vLo) / (vHi - vLo);
const clamp01 = (v) => Math.min(1, Math.max(0, v));
function layoutRange() {
  const a = viewPos(tLo), b = viewPos(tHi);
  for (const [id, p] of [["t-lo", a], ["t-hi", b]]) {
    $(id).style.left = `${clamp01(p) * 100}%`;
    $(id).classList.toggle("out", p < 0 || p > 1); // beyond the zoomed view: parked at the end
  }
  $("t-sel").style.left = `${clamp01(a) * 100}%`;
  $("t-sel").style.width = `${(clamp01(b) - clamp01(a)) * 100}%`;
  $("t-more-l").hidden = vLo <= 0;
  $("t-more-r").hidden = vHi >= 1;
  for (const [id, v] of [["t-lo", tLo], ["t-hi", tHi]]) $(id).setAttribute("aria-valuenow", Math.round(v * 1000));
  if (A) drawPickBand();
}
const MIN_VIEW = 1e-5; // narrowest slider view, as a fraction of the time range
function setView(lo, hi) {
  let len = Math.min(1, Math.max(MIN_VIEW, hi - lo));
  lo = Math.min(1 - len, Math.max(0, lo));
  vLo = lo; vHi = lo + len;
  layoutRange();
}
function setRange(lo, hi, slide = false) {
  if (slide) { // keep the length: push back from the ends
    const len = hi - lo;
    if (lo < 0) { lo = 0; hi = len; }
    if (hi > 1) { hi = 1; lo = 1 - len; }
  }
  tLo = Math.max(0, lo); tHi = Math.min(1, Math.max(tLo, hi));
  layoutRange(); drawMap();
}
(function initRange() {
  const box = $("trange");
  // Pointer position as a fraction of the full time range (through the zoomed view).
  const frac = (ev) => { const r = box.getBoundingClientRect(); return vLo + (ev.clientX - r.left) / r.width * (vHi - vLo); };
  box.addEventListener("wheel", (ev) => { // zoom the view around the pointer
    if (!A) return;
    ev.preventDefault();
    const c = frac(ev), len = vHi - vLo;
    const next = Math.min(1, Math.max(MIN_VIEW, len * Math.exp(ev.deltaY * 0.0015)));
    setView(c - (c - vLo) * next / len, c - (c - vLo) * next / len + next);
  }, { passive: false });
  let drag = null;
  const start = (mode) => (ev) => {
    ev.preventDefault(); ev.stopPropagation();
    drag = { mode, x0: frac(ev), lo: tLo, hi: tHi };
    box.setPointerCapture(ev.pointerId);
  };
  $("t-lo").addEventListener("pointerdown", start("lo"));
  $("t-hi").addEventListener("pointerdown", start("hi"));
  $("t-sel").addEventListener("pointerdown", start("move"));
  box.addEventListener("pointerdown", (ev) => { // click on the bare track: centre the window
    if (drag) return;
    const len = tHi - tLo, c = frac(ev);
    setRange(c - len / 2, c + len / 2, true);
    start("move")(ev);
  });
  box.addEventListener("pointermove", (ev) => {
    if (!drag || !A) return;
    const dx = frac(ev) - drag.x0;
    if (drag.mode === "move") setRange(drag.lo + dx, drag.hi + dx, true);
    else if (drag.mode === "lo") setRange(Math.min(drag.lo + dx, tHi), tHi);
    else setRange(tLo, Math.max(drag.hi + dx, tLo));
  });
  const end = () => { if (drag) scheduleView(); drag = null; };
  box.addEventListener("pointerup", end); box.addEventListener("pointercancel", end);
  for (const id of ["t-lo", "t-hi"]) {
    $(id).addEventListener("keydown", (ev) => {
      const step = (ev.shiftKey ? 0.01 : 0.001) * (vHi - vLo) * (ev.key === "ArrowLeft" ? -1 : ev.key === "ArrowRight" ? 1 : 0);
      if (!step) return;
      ev.preventDefault();
      if (id === "t-lo") setRange(Math.min(tLo + step, tHi), tHi); else setRange(tLo, Math.max(tHi + step, tLo));
    });
    $(id).addEventListener("keyup", (ev) => { if (ev.key === "ArrowLeft" || ev.key === "ArrowRight") scheduleView(); });
  }
  layoutRange();
})();

// ---------------------------------------------------------------- load / view
let source = null;     // summary of the strike file loaded in the worker
let sourceFile = null; // the File itself
let working = false;

function header() {
  const m = A.meta;
  $("current").textContent = `${m.file} · ${m.first.slice(0, 16)} → ${m.last.slice(0, 16)} · ${fmtNum(m.strikes, 0)} strikes` +
    (inView != null ? ` · ${fmtNum(inView, 0)} in view` : "");
}

function showNew() {
  $("f-error").textContent = "";
  $("f-run").disabled = !$("f-file").files[0] || working;
  $("dlg-new").showModal();
}

async function openFile(file) {
  working = true; $("f-run").disabled = true; $("f-error").textContent = "";
  busy(`Reading ${file.name}`, null);
  try {
    if (simRunning.length) { // drop work on the old file rather than wait for it
      stabSeq++; simSeq++;
      engine.end("New file");
      simRunning.length = 0; simProg.clear();
      engine = startWorker((pass, fraction) => showSimProgress(pass, fraction));
    }
    restoreInfo = null;
    const r = await call("loadSource", file);
    restoreInfo = r.restore;
    sourceFile = file;
    A = r; V = r.box; inView = null;
    sel = null; pick = null; sim = null; picked = null; stab = []; stabSeq++; simSeq++; simCache.clear();
    // Start with the whole time range.
    tLo = 0; tHi = 1; vLo = 0; vHi = 1;
    layoutRange();
    if ($("dlg-new").open) $("dlg-new").close();
    $("empty").hidden = true; $("view").hidden = false;
    header();
    $("features").innerHTML = "";
    $("sel-head").textContent = "Click the persistence map to list the clusters there.";
    await new Promise((res) => requestAnimationFrame(res));
    resetView(); drawMap(); drawPers();
    applyView();
  } catch (e) { $("f-error").textContent = e.message; $("current").textContent = e.message; }
  finally { working = false; $("f-run").disabled = false; busy(null); }
}

// The view: strikes in the shown time range and the map's viewport. It only decides what
// the map shows: the clusters, regions and stability never depend on it. Counted when the
// time slider is let go of or the map stops moving.
let viewTimer = null;
function scheduleView() {
  clearTimeout(viewTimer);
  viewTimer = setTimeout(applyView, 250);
}
function applyView() {
  if (!A || !LM) return;
  const T = A.t[A.t.length - 1], lo = tLo * T, hi = tHi * T;
  const b = LM.getBounds(), s = Math.max(-90, b.getSouth()), nth = Math.min(90, b.getNorth());
  const whole = b.getEast() - b.getWest() >= 360;
  const wrap = (x) => ((x + 540) % 360) - 180;
  const w = wrap(b.getWest()), e = wrap(b.getEast());
  let k = 0;
  for (let i = 0; i < A.t.length; i++) {
    const x = A.lon[i], y = A.lat[i];
    if (A.t[i] < lo || A.t[i] > hi || y < s || y > nth) continue;
    if (whole || (w <= e ? x >= w && x <= e : x >= w || x <= e)) k++;
  }
  inView = k;
  header();
}

// Stop the similar-region queries under way (the selection stays; those not yet computed
// show as stopped).
async function stopEngine() {
  stabSeq++; simSeq++;
  const restarting = restartEngine("Stopped");
  if (sel) {
    for (let idx = 0; idx < Math.min(NAMED, sel.features.length); idx++) if (stab[idx] == null) stab[idx] = { error: "stopped" };
    if (pick != null && !sim) sim = { key: keyOf(sel.features[pick]), error: "stopped" };
  }
  renderFeatures(); drawPers();
  await restarting;
}

$("f-file").addEventListener("change", () => { $("f-run").disabled = !$("f-file").files[0] || working; });
$("f-cancel").onclick = () => $("dlg-new").close();
$("form-new").addEventListener("submit", (ev) => {
  ev.preventDefault();
  const f = $("f-file").files[0];
  if (f && !working) openFile(f);
});
$("btn-new").onclick = showNew;
addEventListener("resize", () => { drawPers(); if (LM) LM.invalidateSize(); drawMap(); });
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
  if (tiles) tiles.setUrl(TILE_URL()); drawPers(); drawMap();
});

// ?file=<url>: load a GeoJSON from a URL straight away (e.g. a hosted file).
(async function fromUrl() {
  const url = new URLSearchParams(location.search).get("file");
  if (!url) return;
  try {
    $("current").textContent = `Fetching ${url}…`;
    const r = await fetch(url);
    if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
    await openFile(new File([await r.blob()], url.split("/").pop()));
  } catch (e) { $("current").textContent = `Could not load ${url}: ${e.message}`; }
})();
