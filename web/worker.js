// Web Worker: holds the loaded strikes and the Rust engine (engine.wasm), and answers the
// page's {id, cmd, args} messages. Loading a file builds the engine's graph (exact at
// every threshold); the view's frontier, the clusters at a clicked threshold, their
// regions and similar clusters are then all read from it when asked for. Nothing is saved.
// Loaded as a module worker: new Worker("worker.js", { type: "module" }).


const post = (m, transfer) => self.postMessage(m, transfer || []);
const status = (text) => post({ type: "status", text });
// Engine progress: building the graph (a fraction), or `similar` (pass + fraction).
let building = false, shown = -1;
const progress = (f) => {
  if (!building) return post({ type: "simProgress", pass: Math.floor(f), fraction: f - Math.floor(f) });
  const pct = Math.floor(f * 100);
  if (pct !== shown) { shown = pct; post({ type: "buildProgress", fraction: f }); }
};

// No top-level await: self.onmessage (below) must be attached as soon as the module runs.
let E = null;
const ready = (async () => {
  status("Loading engine…");
  // Always this build's engine: a cached older engine.js or engine.wasm next to a newer
  // worker would not match it.
  const { loadEngine } = await import(`./engine.js?v=${Date.now()}`);
  E = await loadEngine(await fetch("engine.wasm", { cache: "no-cache" }), progress);
  status("");
})();

// ---------------------------------------------------------------- saved graphs
// The graph of each recently loaded file is kept in IndexedDB, by file name, size, content
// hash and graph format, and used instead of building it when the same file is loaded
// again. Only the most recent few are kept; anything going wrong just means building.
const DB = "storm-picker", GRAPHS = "graphs", SAVED = "saved", KEEP = 3;
function openDb() {
  return new Promise((res, rej) => {
    const r = indexedDB.open(DB, 1);
    r.onupgradeneeded = () => { r.result.createObjectStore(GRAPHS); r.result.createObjectStore(SAVED); };
    r.onsuccess = () => res(r.result);
    r.onerror = () => rej(r.error);
  });
}
const done = (q) => new Promise((res, rej) => { q.onsuccess = () => res(q.result); q.onerror = () => rej(q.error); });
async function savedGraph(key) {
  try {
    const db = await openDb();
    try { return (await done(db.transaction(GRAPHS).objectStore(GRAPHS).get(key))) || null; }
    finally { db.close(); }
  } catch { return null; }
}
async function saveGraph(key, g) {
  try {
    const db = await openDb();
    try {
      const tx = db.transaction([GRAPHS, SAVED], "readwrite");
      tx.objectStore(GRAPHS).put(g, key);
      tx.objectStore(SAVED).put(Date.now(), key);
      const keys = await done(tx.objectStore(SAVED).getAllKeys());
      const times = await done(tx.objectStore(SAVED).getAll());
      const old = keys.map((k, i) => [times[i], k]).sort((a, b) => b[0] - a[0]).slice(KEEP);
      for (const [, k] of old) { tx.objectStore(GRAPHS).delete(k); tx.objectStore(SAVED).delete(k); }
      await new Promise((res, rej) => { tx.oncomplete = res; tx.onerror = () => rej(tx.error); tx.onabort = () => rej(tx.error); });
      post({ type: "graphSaved", key });
    } finally { db.close(); }
  } catch { /* not saved (no space, private mode): built again next time */ }
}
async function graphKey(file, bytes) {
  if (typeof E.graphFormat !== "function") return null; // an engine that cannot save graphs
  const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  const hex = Array.from(hash, (b) => b.toString(16).padStart(2, "0")).join("");
  return `${file.name}|${file.size}|${hex}|graph${E.graphFormat()}`;
}

let SRC = null;     // loaded file: {name, meta, t0, t (s since t0), lat, lon, props}, time-sorted
let lastSel = null; // {x, y, label} of the engine's current selection (null after a view change)

// ---------------------------------------------------------------- input
// ISO 8601 with a UTC offset or Z -> epoch seconds, keeping sub-millisecond digits.
const ISO = /^(\d{4})-(\d\d)-(\d\d)[T ](\d\d):(\d\d)(?::(\d\d)(\.\d+)?)?(Z|[+-]\d\d(?::?\d\d)?)$/i;
function parseTime(v, k) {
  const m = typeof v === "string" && ISO.exec(v.trim());
  if (!m) throw new Error(`Feature ${k + 1}: properties.time must be ISO 8601 with Z or a UTC offset (got ${JSON.stringify(v)})`);
  let s = Date.UTC(+m[1], +m[2] - 1, +m[3], +m[4], +m[5], +(m[6] || 0)) / 1000 + (m[7] ? +m[7] : 0);
  if (m[8].toUpperCase() !== "Z") {
    const sign = m[8][0] === "-" ? -1 : 1, d = m[8].slice(1).replace(":", "");
    s -= sign * (+d.slice(0, 2) * 3600 + +(d.slice(2) || 0) * 60);
  }
  return s;
}
const iso = (sec) => new Date(sec * 1000).toISOString().replace(".000Z", "Z");

function select(x, y) {
  const r = E.select(x, y);
  lastSel = { x: r.x, y: r.y, label: r.label };
  return r;
}
// The engine's selection must be at (x, y) for region and similar-cluster queries.
function ensureSelected(x, y) {
  if (!lastSel || lastSel.x !== x || lastSel.y !== y) select(x, y);
}

// ---------------------------------------------------------------- loading
// Parse a GeoJSON File and validate it: its strikes in time order (s since the first),
// and the key its graph is saved under.
async function readFile(file) {
  const bytes = await file.arrayBuffer();
  const key = await graphKey(file, bytes);
  const obj = JSON.parse(new TextDecoder().decode(bytes));
  if (obj.type !== "FeatureCollection") throw new Error("Expected a GeoJSON FeatureCollection");
  const feats = obj.features || [];
  const n = feats.length;
  if (n < 2) throw new Error(`Only ${n} strikes found`);
  const t = new Float64Array(n), lat = new Float64Array(n), lon = new Float64Array(n), props = new Array(n);
  for (let k = 0; k < n; k++) {
    const f = feats[k], g = f.geometry || {};
    if (g.type !== "Point") throw new Error(`Feature ${k + 1}: only Point geometries are supported`);
    const [x, y] = g.coordinates || [];
    if (!(Math.abs(y) <= 90 && Math.abs(x) <= 180)) throw new Error(`Feature ${k + 1}: coordinates out of range`);
    lon[k] = x; lat[k] = y;
    t[k] = parseTime((f.properties || {}).time, k);
    props[k] = f.properties || {}; // kept so saved clusters keep them
  }
  const order = Array.from(t.keys()).sort((a, b) => t[a] - t[b] || a - b);
  const t0 = t[order[0]];
  const src = { file, name: file.name, meta: obj.metadata || {}, t0, t: Float64Array.from(order, (i) => t[i] - t0),
                lat: Float64Array.from(order, (i) => lat[i]), lon: Float64Array.from(order, (i) => lon[i]),
                props: order.map((i) => props[i]) };
  return { src, key };
}

// The engine's graph for SRC: the saved one if there is one, else built (and saved).
async function loadGraph(key) {
  const saved = key && await savedGraph(key);
  if (saved && E.initWithGraph(SRC.t, SRC.lat, SRC.lon, saved)) { post({ type: "graphSaved", key }); return; }
  building = true; shown = -1;
  post({ type: "buildProgress", fraction: 0 });
  try { E.init(SRC.t, SRC.lat, SRC.lon); } finally { building = false; }
  if (key) saveGraph(key, E.graph()); // in the background
}

// ---------------------------------------------------------------- commands
const commands = {
  // Load a GeoJSON File: its strikes, and the engine's graph of them. Returns what the page
  // shows, and what `restore` needs to start a worker again without reading the file.
  async loadSource(file) {
    SRC = null; lastSel = null;
    const { src, key } = await readFile(file);
    SRC = src;
    await loadGraph(key);
    const n = SRC.t.length;
    // The persistence map's box and frontier (the dataset's); every strike is in the
    // engine's view, so clicks list every cluster at a threshold.
    const v = E.view(0, SRC.t[n - 1], -90, 90, -180, 180);
    return {
      box: { frontier: v.frontier, xMin: v.xMin, xMax: v.xMax, yMin: v.yMin, yMax: v.yMax },
      meta: { file: SRC.name, source: SRC.meta, strikes: n, first: iso(SRC.t0), last: iso(SRC.t0 + SRC.t[n - 1]) },
      t0: SRC.t0, t: SRC.t.slice(), lat: Float32Array.from(SRC.lat), lon: Float32Array.from(SRC.lon),
      restore: { key, meta: SRC.meta, lat: SRC.lat.slice(), lon: SRC.lon.slice() },
    };
  },

  // Start again (a new worker, after the last one was stopped) on a file already loaded:
  // its strikes from the page and its saved graph, without reading the file (its
  // properties are read again only if a cluster is saved).
  async restore(file, r, t0, t) {
    lastSel = null;
    SRC = { file, name: file.name, meta: r.meta, t0, t, lat: r.lat, lon: r.lon, props: null };
    await loadGraph(r.key);
    return true;
  },

  async select(x, y) {
    const r = select(x, y);
    const features = r.clusters.map((c) => ({
      component: c.label, size: c.size, first: c.first,
      start: iso(SRC.t0 + SRC.t[c.first]), end: iso(SRC.t0 + SRC.t[c.last]),
    }));
    return { x: r.x, y: r.y, singletons: r.singletons, label: r.label, features };
  },

  // Exact region of the cluster `component` at (x, y): rows [ylo, yhi (min), xlo, xhi (km)]*.
  async region(component, x, y) {
    ensureSelected(x, y);
    return { rows: E.region(component) };
  },

  // Similar region of the cluster `component` at (x, y) (change ≤ tol), with the strikes
  // in similar clusters (to show on the map), and the cluster's own region (cheap), so
  // one result has all the persistence map needs.
  async similar(component, x, y, tol, union) {
    ensureSelected(x, y);
    const r = E.similar(component, tol, !!union);
    return { rows: r.rows, union: r.union, stability: r.stability, passes: r.passes,
             region: E.region(component) };
  },

  // GeoJSON of cluster `component` at (x, y) plus the extra strikes of clusters within
  // change tol, each tagged with the best similarity of such a cluster holding it.
  async clusterGeojson(component, x, y, tol, title, box) {
    if (!SRC.props) SRC.props = (await readFile(SRC.file)).src.props; // after a restore
    ensureSelected(x, y);
    const label = lastSel.label;
    const r = E.similar(component, tol, true);
    const best = new Map();
    r.union.forEach((v, k) => best.set(v, r.best[k]));
    const feats = [];
    let members = 0;
    for (let v = 0; v < label.length; v++) {
      const inC = label[v] === component;
      if (!inC && !best.has(v)) continue;
      const props = { ...SRC.props[v] };
      if (inC) members++;
      else props.in_peripheral_cluster_with_similarity = Math.round(best.get(v) * 1e6) / 1e6;
      feats.push({ type: "Feature", properties: props, geometry: { type: "Point", coordinates: [SRC.lon[v], SRC.lat[v]] } });
    }
    const meta = { ...SRC.meta, name: title || SRC.meta.name };
    meta.cluster = {
      file: SRC.name, distance_km: x, gap_min: y, change_tolerance: tol,
      strikes: members, peripheral_strikes: feats.length - members,
      stability: r.stability,
      stability_box: box, // log axes: distance_km and gap_min ranges
    };
    return JSON.stringify({ type: "FeatureCollection", metadata: meta, features: feats });
  },
};

self.onmessage = async (ev) => {
  const { id, cmd, args } = ev.data;
  try {
    await ready;
    const result = await commands[cmd](...(args || []));
    // Hand big typed arrays over without copying.
    const transfer = [];
    const scan = (o) => {
      if (!o || typeof o !== "object") return;
      if (ArrayBuffer.isView(o)) { if (!transfer.includes(o.buffer)) transfer.push(o.buffer); return; }
      for (const v of Object.values(o)) if (ArrayBuffer.isView(v) || (v && v.constructor === Object)) scan(v);
    };
    scan(result);
    post({ id, result }, transfer);
  } catch (e) {
    post({ id, error: e && e.message ? e.message : String(e) });
  }
};
