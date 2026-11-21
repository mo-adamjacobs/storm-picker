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
const RAMP = ["#cde2fb", "#b7d3f6", "#9ec5f4", "#86b6ef", "#6da7ec", "#5598e7", "#3987e5",
              "#2a78d6", "#256abf", "#1c5cab", "#184f95", "#104281", "#0d366b"];
const palette = () => SERIES[dark() ? "dark" : "light"];

// ---------------------------------------------------------------- worker RPC
// All analysis runs in a Web Worker (Python via Pyodide); nothing is sent to a server.
const worker = new Worker("worker.js", { type: "module" });
const pending = new Map();
let nextId = 1;
function call(cmd, ...args) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    worker.postMessage({ id, cmd, args });
  });
}
worker.onmessage = (ev) => {
  const m = ev.data;
  if (m.type === "saveProgress") { $("s-bar").value = m.fraction; return; }
  if (m.type === "openProgress") { $("open-bar").value = m.fraction; return; }
  if (m.type === "progress") return showJob({ status: "running", stage: m.stage, fraction: m.fraction });
  if (m.type === "status") { $("py-status").textContent = m.text; $("py-status").hidden = !m.text; return; }
  if (m.type === "warning") { console.warn(m.text); $("py-warning").textContent = m.text; $("py-warning").hidden = false; return; }
  const p = pending.get(m.id);
  if (!p) return;
  pending.delete(m.id);
  if (m.error) p.reject(new Error(m.error)); else p.resolve(m.result);
};
worker.onerror = (e) => { $("py-status").textContent = `Python worker failed: ${e.message}`; $("py-status").hidden = false; };

const fmtTime = (ms) => new Date(ms).toISOString().replace("T", " ").slice(0, 19) + "Z";
const fmtDur = (sec) => {
  const m = Math.round(sec / 60), d = Math.floor(m / 1440), h = Math.floor((m % 1440) / 60), mm = m % 60;
  return d ? `${d} d ${h} h ${mm} min` : h ? `${h} h ${mm} min` : `${mm} min`;
};
const fmtBytes = (b) => (b >= 1e9 ? `${fmtNum(b / 1e9)} GB` : b >= 1e6 ? `${fmtNum(b / 1e6)} MB` : `${fmtNum(b / 1e3, 0)} kB`);
const fmtNum = (v, d = 1) => Number(v).toLocaleString(undefined, { maximumFractionDigits: d });

// ---------------------------------------------------------------- state
let A = null;         // loaded analysis payload
let sel = null;       // selection response
let pick = null;      // isolated feature index (into sel.features) or null
let countImg = null;  // persistence-map count image cache
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

// Rows of one cluster: [ylo, yhi, xlo, xhi] in the normalized box, sorted by ylo.
function clusterRows(c) {
  const out = [];
  for (let k = A.row_off[c]; k < A.row_off[c + 1]; k++) out.push(A.rows.slice(4 * k, 4 * k + 4));
  return out.sort((p, q) => p[0] - q[0]);
}

// Exact cluster count (clusters of >= 2 strikes) at every pixel centre:
// each row is a rectangle, summed with a 2D difference array.
function buildCounts(W, H) {
  const D = new Float64Array((W + 1) * (H + 1));
  const ix = (v) => Math.min(W, Math.max(0, Math.ceil(v * W - 0.5)));
  const iy = (v) => Math.min(H, Math.max(0, Math.ceil(v * H - 0.5)));
  const R = A.rows;
  for (let k = 0; k < R.length; k += 4) {
    const j0 = iy(R[k]), j1 = iy(R[k + 1]), i0 = ix(R[k + 2]), i1 = ix(R[k + 3]);
    if (i0 >= i1 || j0 >= j1) continue;
    D[j0 * (W + 1) + i0]++; D[j0 * (W + 1) + i1]--; D[j1 * (W + 1) + i0]--; D[j1 * (W + 1) + i1]++;
  }
  const cnt = new Float64Array(W * H);
  for (let j = 0; j < H; j++) {
    let run = 0;
    for (let i = 0; i < W; i++) {
      run += D[j * (W + 1) + i];
      cnt[j * W + i] = run + (j ? cnt[(j - 1) * W + i] : 0);
    }
  }
  return cnt;
}

const hexRgb = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));

function drawPers() {
  if (!A) return;
  const c = $("pers"), f = persFrame(c), ctx = f.ctx;
  const W = Math.max(1, Math.round(f.pw)), H = Math.max(1, Math.round(f.ph));
  if (!countImg || countImg.W !== W || countImg.H !== H || countImg.dark !== dark()) {
    const cnt = buildCounts(W, H);
    // Log scale stretched over the 2nd-98th percentile of non-zero pixels, so the ramp
    // isn't spent on a few extreme pixels near the origin.
    const nz = Array.from(cnt).filter((v) => v > 0).sort((a, b) => a - b);
    const q = (f) => (nz.length ? nz[Math.min(nz.length - 1, Math.floor(f * nz.length))] : 1);
    const min = Math.max(1, q(0.02)), max = Math.max(min + 1, q(0.98));
    const lmin = Math.log(min), span = Math.log(max) - lmin || 1;
    const img = new ImageData(W, H);
    const surf = hexRgb(css("--surface-1"));
    const rgbs = RAMP.map(hexRgb);
    for (let j = 0; j < H; j++) for (let i = 0; i < W; i++) {
      const v = cnt[j * W + i], p = ((H - 1 - j) * W + i) * 4;
      // Continuous: interpolate between the ramp's stops instead of snapping to 13 bands.
      let rgb = surf;
      if (v > 0) {
        const u = Math.min(1, Math.max(0, (Math.log(v) - lmin) / span)) * (RAMP.length - 1);
        const k = Math.min(RAMP.length - 2, Math.floor(u)), w = u - k;
        rgb = [0, 1, 2].map((c) => rgbs[k][c] * (1 - w) + rgbs[k + 1][c] * w);
      }
      img.data[p] = rgb[0]; img.data[p + 1] = rgb[1]; img.data[p + 2] = rgb[2]; img.data[p + 3] = 255;
    }
    countImg = { W, H, cnt, max, img, dark: dark() };
    drawLegend(min, max);
  }
  ctx.clearRect(0, 0, f.w, f.h);
  ctx.putImageData(countImg.img, Math.round(f.m.l), Math.round(f.m.t));
  // Axes
  const km = A.meta.params.max_km, mins = A.meta.params.max_min;
  ctx.strokeStyle = css("--border"); ctx.lineWidth = f.dpr;
  ctx.strokeRect(f.m.l, f.m.t, f.pw, f.ph);
  ctx.fillStyle = css("--text-secondary");
  ctx.font = `${11 * f.dpr}px system-ui, sans-serif`;
  ctx.textAlign = "center"; ctx.textBaseline = "top";
  for (let k = 0; k <= 4; k++) ctx.fillText(fmtNum(km * k / 4), f.X(k / 4), f.m.t + f.ph + 4 * f.dpr);
  ctx.fillText("distance (km)", f.X(0.5), f.m.t + f.ph + 18 * f.dpr);
  ctx.textAlign = "right"; ctx.textBaseline = "middle";
  for (let k = 0; k <= 4; k++) ctx.fillText(fmtNum(mins * k / 4), f.m.l - 4 * f.dpr, f.Y(k / 4));
  ctx.save(); ctx.translate(12 * f.dpr, f.Y(0.5)); ctx.rotate(-Math.PI / 2);
  ctx.textAlign = "center"; ctx.fillText("time gap (min)", 0, 0); ctx.restore();
  // Clusters within the change tolerance of the picked one: light fill and a thin ring.
  if (sel && pick != null) {
    const ft = sel.features[pick], col = pickColor(pick);
    const mask = document.createElement("canvas"); mask.width = f.w; mask.height = f.h;
    const mc = mask.getContext("2d");
    mc.fillStyle = "#000";
    for (const c of (sim && sim.cluster === ft.cluster ? sim.ids : [])) {
      for (let k = A.row_off[c]; k < A.row_off[c + 1]; k++) {
        const y0 = A.rows[4 * k], y1 = A.rows[4 * k + 1], x0 = A.rows[4 * k + 2], x1 = A.rows[4 * k + 3];
        // Snap to whole pixels so abutting rows don't leave hairline gaps in the mask.
        const L = Math.floor(f.X(x0)), R = Math.ceil(f.X(x1)), T = Math.floor(f.Y(y1)), B = Math.ceil(f.Y(y0));
        mc.fillRect(L, T, R - L, B - T);
      }
    }
    const ring = document.createElement("canvas"); ring.width = f.w; ring.height = f.h;
    const rc = ring.getContext("2d"), r = 1.5 * f.dpr;
    for (const [dx, dy] of [[-r, 0], [r, 0], [0, -r], [0, r], [-r, -r], [r, r], [-r, r], [r, -r]]) rc.drawImage(mask, dx, dy);
    rc.globalCompositeOperation = "destination-out"; rc.drawImage(mask, 0, 0);
    rc.globalCompositeOperation = "source-in"; rc.fillStyle = col; rc.fillRect(0, 0, f.w, f.h);
    ctx.save();
    ctx.globalAlpha = 0.3; ctx.drawImage(mask, 0, 0);  // dim the feature's area slightly
    ctx.globalCompositeOperation = "source-over"; ctx.globalAlpha = 1;
    ctx.drawImage(ring, 0, 0);
    ctx.restore();
  }
  // Isolated cluster's exact region: union of its rows, outlined.
  if (sel && pick != null) {
    const rows = clusterRows(sel.features[pick].cluster);
    ctx.fillStyle = css("--surface-1"); ctx.globalAlpha = 0.55;
    for (const [y0, y1, x0, x1] of rows) ctx.fillRect(f.X(x0), f.Y(y1), f.X(x1) - f.X(x0), f.Y(y0) - f.Y(y1));
    ctx.globalAlpha = 1;
    // Outline: sides of each row, plus horizontal steps where consecutive rows meet.
    const path = new Path2D();
    const seg = (xa, ya, xb, yb) => { path.moveTo(f.X(xa), f.Y(ya)); path.lineTo(f.X(xb), f.Y(yb)); };
    rows.forEach(([y0, y1, x0, x1], k) => {
      seg(x0, y0, x0, y1); seg(x1, y0, x1, y1);
      const prev = rows[k - 1], next = rows[k + 1];
      if (prev && Math.abs(prev[1] - y0) < 1e-9) { seg(prev[2], y0, x0, y0); seg(prev[3], y0, x1, y0); }
      else seg(x0, y0, x1, y0);
      if (!next || Math.abs(next[0] - y1) > 1e-9) seg(x0, y1, x1, y1);
    });
    ctx.strokeStyle = css("--surface-1"); ctx.lineWidth = 5 * f.dpr; ctx.stroke(path);
    ctx.strokeStyle = pickColor(pick); ctx.lineWidth = 2.5 * f.dpr; ctx.stroke(path);
  }
  if (sel) { // selected point
    ctx.strokeStyle = css("--text-primary"); ctx.lineWidth = 1.5 * f.dpr;
    const px = f.X(sel.x), py = f.Y(sel.y), r = 6 * f.dpr;
    ctx.beginPath(); ctx.arc(px, py, r, 0, 2 * Math.PI); ctx.stroke();
    ctx.beginPath(); ctx.moveTo(px - 2 * r, py); ctx.lineTo(px - r, py); ctx.moveTo(px + r, py); ctx.lineTo(px + 2 * r, py);
    ctx.moveTo(px, py - 2 * r); ctx.lineTo(px, py - r); ctx.moveTo(px, py + r); ctx.lineTo(px, py + 2 * r); ctx.stroke();
  }
}

function drawLegend(min, max) {
  const el = $("pers-legend");
  el.innerHTML = "";
  const c = document.createElement("canvas"); c.width = 140; c.height = 1;
  const x = c.getContext("2d"), grad = x.createLinearGradient(0, 0, 140, 0);
  RAMP.forEach((h, i) => grad.addColorStop(i / (RAMP.length - 1), h));
  x.fillStyle = grad; x.fillRect(0, 0, 140, 1);
  el.append(`clusters: ≤${fmtNum(min, 0)}`, c, `≥${fmtNum(max, 0)} (log scale)`);
}

function persPoint(ev) {
  const c = $("pers"), f = persFrame(c), r = c.getBoundingClientRect();
  const x = ((ev.clientX - r.left) * f.dpr - f.m.l) / f.pw;
  const y = 1 - ((ev.clientY - r.top) * f.dpr - f.m.t) / f.ph;
  return { x, y, inside: x >= 0 && x < 1 && y >= 0 && y < 1, f };
}

$("pers").addEventListener("mousemove", (ev) => {
  const tip = $("pers-tip");
  if (!A || !countImg) return;
  const p = persPoint(ev);
  if (!p.inside) { tip.hidden = true; return; }
  const i = Math.floor(p.x * countImg.W), j = Math.floor(p.y * countImg.H);
  const n = countImg.cnt[j * countImg.W + i];
  tip.textContent = `${fmtNum(p.x * A.meta.params.max_km)} km · ${fmtNum(p.y * A.meta.params.max_min)} min · ${fmtNum(n, 0)} features`;
  tip.hidden = false;
  const r = $("pers").getBoundingClientRect();
  tip.style.left = `${ev.clientX - r.left + 12}px`; tip.style.top = `${ev.clientY - r.top + 12}px`;
});
$("pers").addEventListener("mouseleave", () => { $("pers-tip").hidden = true; });
$("pers").addEventListener("click", async (ev) => {
  if (!A) return;
  const p = persPoint(ev);
  if (!p.inside) return;
  $("sel-head").textContent = "Selecting…";
  try {
    sel = await call("select", p.x, p.y);
    pick = null; sim = null; stab = [];
    renderFeatures(); drawPers(); drawMap(); writeHash(); loadStability();
  } catch (e) { $("sel-head").textContent = e.message; }
});

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
// which is what makes the stability area exact.
const TOL_MAX = 99;
const tolPct = () => Math.min(TOL_MAX, Math.max(0, +$("tol").value || 0));

// Stability of list row idx: from the coloured clusters' run, or the picked cluster's
// similar-cluster query. "" for rows that get none.
function stabilityText(f, idx) {
  let v = stab[idx];
  if (v == null && idx === pick && sim && sim.cluster === f.cluster) v = sim.error ? { error: sim.error } : sim.stability;
  if (v == null) return idx < NAMED || idx === pick ? "stability …" : "";
  if (v.error) return v.error;
  const pct = v * 100;
  return `stability <b>${pct >= 1 ? fmtNum(pct) : fmtNum(pct, 2)}%</b>`;
}

// Stability of the coloured clusters, largest first, one worker call at a time so the
// picked cluster's query can run in between. Restarted when the point or tolerance changes.
let stab = [], stabSeq = 0;
async function loadStability() {
  const seq = ++stabSeq;
  stab = [];
  if (!sel) return;
  const tol = tolPct() / 100, s = sel;
  for (let idx = 0; idx < Math.min(NAMED, s.features.length); idx++) {
    let v;
    try { v = await call("stability", s.features[idx].cluster, s.x, s.y, tol); }
    catch (e) { v = { error: e.message }; }
    if (seq !== stabSeq) return; // a newer point or tolerance superseded this run
    stab[idx] = v;
    renderFeatures();
  }
}

// Fetch the clusters within the change tolerance of the picked one (exact, server-side).
let simSeq = 0;
async function loadSimilar() {
  if (!sel || pick == null) return;
  const f = sel.features[pick], seq = ++simSeq, tol = tolPct() / 100;
  try {
    const r = await call("similar", f.cluster, sel.x, sel.y, tol);
    if (seq !== simSeq) return; // a newer request superseded this one
    sim = r;
  } catch (e) {
    if (seq !== simSeq) return;
    sim = { cluster: f.cluster, error: e.message };
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
    sim = null; stab = []; renderFeatures(); drawPers(); writeHash(); loadSimilar(); loadStability();
  }, 250);
});

// Save a cluster (plus strikes of similar clusters, tagged with their similarity) as
// GeoJSON: ask for a title, then a file location, then build it in the worker.
let saveJob = null; // {f, x, y, tol} of the cluster the save dialog is for
function saveCluster(f) {
  const p = A.meta.params, tol = tolPct() / 100;
  saveJob = { f, x: sel.x, y: sel.y, tol };
  const at = `${fmtNum(sel.x * p.max_km)} km, ${fmtNum(sel.y * p.max_min)} min`;
  const src = A.meta.source?.name || p.file;
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
  const job = saveJob, title = $("s-title").value.trim(), p = A.meta.params;
  if (!job || !title) return;
  const num = (v) => +v.toFixed(1);
  const name = `cluster-${job.f.size}strikes-${num(job.x * p.max_km)}km-${num(job.y * p.max_min)}min-` +
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
    const json = await call("clusterGeojson", job.f.cluster, job.x, job.y, job.tol, title);
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
  const p = A.meta.params;
  $("sel-head").innerHTML =
    `At <b>${fmtNum(sel.x * p.max_km)} km</b>, <b>${fmtNum(sel.y * p.max_min)} min</b>` +
    (sel.missing ? ` <b>${sel.missing} clusters had no stored region (bug).</b>` : "");
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
      (st ? `<div class="small muted">${st}</div>` : "");
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
      renderFeatures(); drawPers(); drawMap(); writeHash(); loadSimilar();
    });
    li.addEventListener("dblclick", () => {
      if (pick !== idx) { pick = idx; sim = null; renderFeatures(); drawPers(); writeHash(); loadSimilar(); }
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
  renderFeatures(); drawPers(); drawMap(); writeHash(); loadSimilar();
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
  if (!A || !sel || pick == null || !sim || sim.cluster !== sel.features[pick].cluster || !sim.union) return null;
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
      if (sim && sim.cluster === ft.cluster && sim.union) {
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
  const end = () => { drag = null; };
  box.addEventListener("pointerup", end); box.addEventListener("pointercancel", end);
  for (const id of ["t-lo", "t-hi"]) {
    $(id).addEventListener("keydown", (ev) => {
      const step = (ev.shiftKey ? 0.01 : 0.001) * (vHi - vLo) * (ev.key === "ArrowLeft" ? -1 : ev.key === "ArrowRight" ? 1 : 0);
      if (!step) return;
      ev.preventDefault();
      if (id === "t-lo") setRange(Math.min(tLo + step, tHi), tHi); else setRange(tLo, Math.max(tHi + step, tLo));
    });
  }
  layoutRange();
})();

// ---------------------------------------------------------------- open / new
async function openAnalysis(id) {
  $("current").textContent = "Loading…";
  // Indeterminate until the worker reports (it may still be starting Python).
  const bar = $("open-bar"); bar.removeAttribute("value"); bar.hidden = false;
  try { A = await call("open", id); }
  catch (e) {
    bar.hidden = true;
    A = null; sel = null; pick = null; sim = null; stab = []; stabSeq++;
    $("view").hidden = true; $("empty").hidden = false;
    $("current").textContent = /NotFound|could not be found/i.test(e.message)
      ? "That analysis isn't saved in this browser (links from the old server version won't work)."
      : `Could not open the analysis: ${e.message}`;
    history.replaceState(null, "", location.pathname);
    return false;
  }
  bar.hidden = true;
  sel = null; pick = null; countImg = null; tLo = 0; tHi = 1; vLo = 0; vHi = 1; stab = []; stabSeq++;
  sim = null;
  layoutRange();
  $("empty").hidden = true; $("view").hidden = false;
  const m = A.meta, p = m.params;
  $("current").textContent = `${p.file} · ${m.first.slice(0, 16)} → ${m.last.slice(0, 16)} · ` +
    `${fmtNum(m.strikes, 0)} strikes · ≤${fmtNum(p.max_km)} km, ≤${fmtNum(p.max_min)} min`;
  $("features").innerHTML = "";
  $("sel-head").textContent = "Click the persistence map to list the clusters there.";
  await new Promise((r) => requestAnimationFrame(r));
  resetView(); drawPers(); drawMap(); writeHash();
  if (lastJob) showJob(lastJob); // hides "Open result" for the analysis now open
  return true;
}

// Saved analyses live in this browser's private storage; Export/Import move them as files.
async function showSaved() {
  const ul = $("saved"); ul.innerHTML = "<li class='muted'>Loading…</li>";
  if (!$("dlg-open").open) $("dlg-open").showModal();
  const list = await call("list");
  ul.innerHTML = list.length ? "" : "<li class='muted'>No saved analyses in this browser yet.</li>";
  for (const m of list) {
    const p = m.params, li = document.createElement("li");
    li.innerHTML = `<div class="info"><b>${p.file}</b><div class="small muted">` +
      `${m.first.slice(0, 16)} → ${m.last.slice(0, 16)} · ${fmtNum(m.strikes, 0)} strikes · ` +
      `≤${fmtNum(p.max_km)} km · ≤${fmtNum(p.max_min)} min<br>` +
      `${fmtNum(m.clusters, 0)} clusters · ${fmtNum(m.kept_edges, 0)} of ${fmtNum(m.candidate_edges, 0)} edges kept · ` +
      `${fmtNum(m.seconds, 0)} s · ${fmtBytes(m.bytes)} · saved ${m.created.slice(0, 16).replace("T", " ")}</div></div>`;
    const open = document.createElement("button"); open.textContent = "Open";
    open.onclick = () => { $("dlg-open").close(); openAnalysis(m.id); };
    const exp = document.createElement("button"); exp.textContent = "Export";
    exp.onclick = async () => {
      const bytes = await call("exportFile", m.id);
      const a = document.createElement("a");
      a.href = URL.createObjectURL(new Blob([bytes], { type: "application/octet-stream" }));
      a.download = `lightning-${m.id}.npz`;
      a.click(); setTimeout(() => URL.revokeObjectURL(a.href), 10000);
    };
    const del = document.createElement("button"); del.textContent = "Delete";
    del.onclick = async () => {
      if (del.dataset.armed) { await call("remove", m.id); li.remove(); }
      else { del.dataset.armed = 1; del.textContent = "Confirm delete"; }
    };
    li.append(open, exp, del); ul.append(li);
  }
}
$("o-import").addEventListener("change", async () => {
  const file = $("o-import").files[0];
  if (!file) return;
  try { await call("importFile", file); await showSaved(); }
  catch (e) { $("saved").insertAdjacentHTML("afterbegin", `<li class="error small"></li>`); $("saved").firstChild.textContent = e.message; }
  $("o-import").value = "";
});

const toLocalInput = (iso) => iso.slice(0, 19);
const withSecs = (v) => (v.length === 16 ? v + ":00" : v.slice(0, 19)); // browsers drop ":00"
let source = null; // the strike file loaded in the worker
async function loadSource(file) {
  $("f-info").textContent = `Reading ${file.name}…`;
  $("f-run").disabled = true;
  try {
    source = await call("loadSource", file);
    $("f-info").textContent = `${fmtNum(source.strikes, 0)} strikes, ${source.first} → ${source.last}` +
      (source.metadata?.name ? ` · ${source.metadata.name}` : "");
    $("f-start").value = toLocalInput(source.first); $("f-end").value = toLocalInput(source.last);
  } catch (e) { source = null; $("f-info").textContent = e.message; }
  $("f-run").disabled = !source || running;
}

function showNew() {
  $("f-error").textContent = "";
  if (source) $("f-info").textContent = `Loaded: ${source.name} · ${fmtNum(source.strikes, 0)} strikes, ${source.first} → ${source.last}`;
  $("f-run").disabled = !source || running;
  $("dlg-new").showModal();
  // Persistent storage keeps saved analyses from being evicted under storage pressure.
  navigator.storage?.persist?.();
}

// Job progress: shown in the header always, and in the dialog while it is open.
let lastJob = null, running = false;
function showJob(j) {
  lastJob = j;
  running = j.status === "running";
  const text = running
    ? j.stage + (j.fraction != null ? ` ${Math.round(j.fraction * 100)}%` : " …")
    : j.status === "error" ? `Analysis failed: ${j.error}` : j.status === "done" ? "Analysis finished" : "";
  for (const [bar, label] of [[$("job-bar"), $("job-stage")], [$("hjob-bar"), $("hjob-text")]]) {
    label.textContent = text;
    bar.hidden = !running;
    if (j.fraction != null) bar.value = j.fraction; else bar.removeAttribute("value");
  }
  $("job").hidden = !running;
  $("f-run").disabled = running || !source;
  if (j.status === "error") $("f-error").textContent = j.error;
  const hj = $("hjob");
  hj.hidden = j.status === "idle" || (j.status === "done" && j.id === A?.meta.id);
  hj.classList.toggle("err", j.status === "error");
  $("hjob-open").hidden = !(j.status === "done" && j.id && j.id !== A?.meta.id);
  $("hjob-open").onclick = () => openAnalysis(j.id);
}

$("f-file").addEventListener("change", () => { const f = $("f-file").files[0]; if (f) loadSource(f); });
$("f-cancel").onclick = () => $("dlg-new").close();
$("form-new").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  if (!source) return;
  $("f-error").textContent = "";
  showJob({ status: "running", stage: "Starting", fraction: null });
  try {
    const summary = await call("run", {
      file: source.name, // The inputs hold whole seconds; make the end inclusive of its last second.
      start: withSecs($("f-start").value) + "Z", end: withSecs($("f-end").value) + ".999999Z",
      max_km: +$("f-km").value, max_min: +$("f-min").value,
    });
    showJob({ status: "done", id: summary.id });
    if ($("dlg-new").open) { $("dlg-new").close(); openAnalysis(summary.id); }
  } catch (e) { showJob({ status: "error", error: e.message }); }
});
$("btn-new").onclick = showNew;
$("btn-open").onclick = () => showSaved().catch((e) => { $("saved").textContent = e.message; });
$("o-close").onclick = () => $("dlg-open").close();
addEventListener("resize", () => { drawPers(); if (LM) LM.invalidateSize(); drawMap(); });
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
  countImg = null; if (tiles) tiles.setUrl(TILE_URL()); drawPers(); drawMap();
});

// ---------------------------------------------------------------- URL hash state
// #a=<analysis id>&x=<0..1>&y=<0..1>&f=<feature list index>  (bookmarkable views)
function writeHash() {
  if (!A) return;
  const h = new URLSearchParams({ a: A.meta.id });
  if (sel) { h.set("x", sel.x.toFixed(4)); h.set("y", sel.y.toFixed(4)); }
  if (pick != null) h.set("f", pick);
  h.set("tol", $("tol").value);
  history.replaceState(null, "", "#" + h);
}
async function readHash() {
  const h = new URLSearchParams(location.hash.slice(1));
  if (!h.get("a")) return;
  try {
    if (!(await openAnalysis(h.get("a")))) return;
    if (h.has("x") && h.has("y")) {
      sel = await call("select", +h.get("x"), +h.get("y"));
      pick = h.has("f") && +h.get("f") < sel.features.length ? +h.get("f") : null;
      if (h.has("tol")) $("tol").value = Math.min(TOL_MAX, Math.max(0, +h.get("tol") || 0));
      await new Promise((r) => requestAnimationFrame(r));
      renderFeatures(); drawPers(); drawMap(); loadSimilar(); loadStability();
    }
  } catch (e) { $("current").textContent = e.message; }
}
readHash();
