// Web Worker: runs the Python analysis under Pyodide and keeps analyses in the browser's
// private file storage (OPFS). The page talks to it with {id, cmd, args} messages.
// Loaded as a module worker: new Worker("worker.js", { type: "module" }).

const PYODIDE = "https://cdn.jsdelivr.net/pyodide/v314.0.7/full/"; // CI reads this version to build the Rust wheel (.github/workflows/pages.yml)

const PY_FILES = ["__init__.py", "data.py", "graph.py", "regions.py", "analysis.py", "browser.py"];
let pyodide = null, B = null; // B: the lightning.browser module

const status = (text) => self.postMessage({ type: "status", text });
const progress = (stage, fraction) =>
  self.postMessage({ type: "progress", stage, fraction: fraction == null ? null : Number(fraction) });

// No top-level await: self.onmessage (below) must be attached as soon as the module runs,
// or messages the page sends during Pyodide's download are dropped and never answered.
const ready = (async () => {
  status("Starting Python…");
  const { loadPyodide } = await import(PYODIDE + "pyodide.mjs");
  pyodide = await loadPyodide({ indexURL: PYODIDE });
  status("Loading NumPy and SciPy…");
  await pyodide.loadPackage(["numpy", "scipy"]);
  // The Rust sweep (sweep/ in the repo, built by build-pyodide.sh). Without it the
  // Python reference implementation runs instead: same results, much slower.
  try {
    const manifest = await (await fetch("wheels/manifest.json", { cache: "no-store" })).json();
    await pyodide.loadPackage(new URL(`wheels/${manifest.lightning_sweep}`, self.location).href);
  } catch (e) {
    self.postMessage({ type: "warning", text: `Fast Rust module not loaded (${e.message}); using slower Python.` });
  }
  pyodide.FS.mkdirTree("/home/pyodide/lightning");
  for (const f of PY_FILES) {
    const r = await fetch(`py/${f}`, { cache: "no-store" });
    if (!r.ok) throw new Error(`Could not load py/${f}`);
    pyodide.FS.writeFile(`/home/pyodide/lightning/${f}`, await r.text());
  }
  pyodide.runPython("import sys; sys.path.insert(0, '/home/pyodide')");
  B = pyodide.pyimport("lightning.browser");
  status("");
})();

// ---------------------------------------------------------------- helpers
// NumPy array (PyProxy) -> a standalone JS typed array of the same dtype.
function typed(arr) {
  const buf = arr.getBuffer();
  try { return buf.data.slice(); } finally { buf.release(); arr.destroy(); }
}
// Python tuple (PyProxy) -> JS array of its items (still proxies), freeing the tuple.
function items(tup) {
  const out = [];
  for (let k = 0; k < tup.length; k++) out.push(tup.get(k));
  tup.destroy();
  return out;
}

async function store() {
  const root = await navigator.storage.getDirectory();
  return root.getDirectoryHandle("analyses", { create: true });
}
async function writeFile(name, bytes) {
  const fh = await (await store()).getFileHandle(name, { create: true });
  const h = await fh.createSyncAccessHandle();
  try { h.truncate(0); h.write(bytes, { at: 0 }); h.flush(); } finally { h.close(); }
}
async function readFile(name) {
  const fh = await (await store()).getFileHandle(name);
  return new Uint8Array(await (await fh.getFile()).arrayBuffer());
}
async function save(summary, bytes) {
  await writeFile(`${summary.id}.npz`, bytes);
  await writeFile(`${summary.id}.json`, new TextEncoder().encode(JSON.stringify(summary)));
}

// ---------------------------------------------------------------- commands
const commands = {
  // Parse a GeoJSON File in JS (fast), validate and keep it in Python.
  async loadSource(file) {
    status(`Reading ${file.name}…`);
    const obj = JSON.parse(await file.text());
    if (obj.type !== "FeatureCollection") throw new Error("Expected a GeoJSON FeatureCollection");
    const feats = obj.features || [];
    const n = feats.length, times = new Array(n), props = new Array(n);
    const lat = new Float64Array(n), lon = new Float64Array(n);
    for (let k = 0; k < n; k++) {
      const f = feats[k], g = f.geometry || {};
      if (g.type !== "Point") throw new Error(`Feature ${k + 1}: only Point geometries are supported`);
      lon[k] = g.coordinates[0]; lat[k] = g.coordinates[1];
      times[k] = (f.properties || {}).time;
      props[k] = JSON.stringify(f.properties || {}); // kept so saved clusters keep them
    }
    status("Checking strikes…");
    try {
      return JSON.parse(B.set_source(file.name, times, lat, lon, JSON.stringify(obj.metadata || {}), props));
    } finally { status(""); }
  },

  async run(params) {
    const [summaryJson, blob] = items(B.run(JSON.stringify(params), progress));
    const summary = JSON.parse(summaryJson);
    const bytes = typed(blob); // Python bytes -> Uint8Array
    progress("Saving", null);
    await save(summary, bytes);
    return summary;
  },

  async list() {
    const d = await store(), out = [];
    for await (const [name, h] of d.entries()) {
      if (!name.endsWith(".json")) continue;
      try {
        const m = JSON.parse(await (await h.getFile()).text());
        m.bytes = (await (await d.getFileHandle(`${m.id}.npz`)).getFile()).size;
        out.push(m);
      } catch { /* incomplete entry: skip */ }
    }
    return out.sort((a, b) => (a.id < b.id ? 1 : -1));
  },

  async open(id) {
    // Stages: read from storage 0-5%, load in Python 5-95%, hand over arrays 95-100%.
    const report = (f) => self.postMessage({ type: "openProgress", fraction: Number(f) });
    report(0);
    const bytes = await readFile(`${id}.npz`);
    report(0.05);
    const [metaJson, payload] = items(B.open_analysis(bytes, (f) => report(0.05 + 0.9 * f)));
    report(0.95);
    const out = { meta: JSON.parse(metaJson) };
    for (const k of ["t", "lat", "lon", "rows", "row_off", "size"]) out[k] = typed(payload.get(k));
    out.t0 = payload.get("t0");
    payload.destroy();
    return out;
  },

  async select(x, y) {
    const [json, label] = items(B.select(x, y));
    return Object.assign(JSON.parse(json), { label: typed(label) });
  },

  async similar(cluster, x, y, tol) {
    const [json, union, ids] = items(B.similar(cluster, x, y, tol));
    return Object.assign(JSON.parse(json), { union: typed(union), ids: typed(ids) });
  },

  async stability(cluster, x, y, tol) {
    return B.stability(cluster, x, y, tol);
  },

  async clusterGeojson(cluster, x, y, tol, title) {
    const report = (f) => self.postMessage({ type: "saveProgress", fraction: Number(f) });
    return B.cluster_geojson(cluster, x, y, tol, title, report);
  },

  async remove(id) {
    const d = await store();
    for (const ext of [".npz", ".json"]) await d.removeEntry(id + ext).catch(() => {});
  },

  async exportFile(id) {
    return readFile(`${id}.npz`);
  },

  async importFile(file) {
    const bytes = new Uint8Array(await file.arrayBuffer());
    const summary = JSON.parse(B.summary_of(bytes)); // validates the file
    await save(summary, bytes);
    return summary;
  },
};

self.onmessage = async (ev) => {
  const { id, cmd, args } = ev.data;
  try {
    await ready;
    const result = await commands[cmd](...(args || []));
    // Hand big typed arrays over without copying.
    const transfer = [];
    if (result && typeof result === "object") {
      for (const v of Object.values(result)) if (ArrayBuffer.isView(v)) transfer.push(v.buffer);
      if (ArrayBuffer.isView(result)) transfer.push(result.buffer);
    }
    self.postMessage({ id, result }, transfer);
  } catch (e) {
    let msg = e && e.message ? e.message : String(e);
    // Python errors arrive as a traceback; show only the last line.
    if (e && e.type) msg = msg.trim().split("\n").pop();
    self.postMessage({ id, error: msg });
  }
};
