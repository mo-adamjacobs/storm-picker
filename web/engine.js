// Thin wrapper over engine.wasm (the Rust engine in engine/, built by build.sh).
// Inputs are copied into WebAssembly memory; results are copied out of numbered slots.
// Works in a worker and in Node (for timing).

export async function loadEngine(bytesOrResponse, onProgress = () => {}) {
  const imports = { env: { progress: (f) => onProgress(f) } };
  const { instance } = bytesOrResponse instanceof Response
    ? await WebAssembly.instantiateStreaming(bytesOrResponse, imports)
    : await WebAssembly.instantiate(bytesOrResponse, imports);
  const w = instance.exports;
  const mem = () => w.memory.buffer; // re-read after every call: memory may grow

  function put(arr) { // typed array -> pointer (caller frees)
    const p = w.alloc(arr.byteLength);
    new Uint8Array(mem(), p, arr.byteLength).set(new Uint8Array(arr.buffer, arr.byteOffset, arr.byteLength));
    return [p, arr.byteLength];
  }
  function slot(k, Type) { // copy of output slot k
    const p = w.out_ptr(k), n = w.out_len(k);
    return new Type(mem().slice(p, p + n));
  }
  return {
    // Strikes: t seconds (ascending), lat, lon degrees (Float64Array each).
    init(t, lat, lon) {
      const bufs = [t, lat, lon].map(put);
      try { w.init(t.length, bufs[0][0], bufs[1][0], bufs[2][0]); }
      finally { for (const [p, n] of bufs) w.dealloc(p, n); }
    },
    // The same strikes with a graph saved by `graph()` (same `graphFormat`), instead of
    // building it. Returns false (nothing loaded: call init) if it is not well formed.
    initWithGraph(t, lat, lon, g) {
      const bufs = [t, lat, lon, g.ea, g.eb, g.ed, g.frontier].map(put);
      try {
        return w.init_with_graph(t.length, bufs[0][0], bufs[1][0], bufs[2][0],
          g.ea.length, bufs[3][0], bufs[4][0], bufs[5][0], g.frontier.length / 2, bufs[6][0]) === 1;
      } finally { for (const [p, n] of bufs) w.dealloc(p, n); }
    },
    // The loaded graph, to save: {ea, eb (Uint32Array), ed, frontier (Float64Array)}.
    graph() {
      w.graph();
      return { ea: slot(0, Uint32Array), eb: slot(1, Uint32Array), ed: slot(2, Float64Array), frontier: slot(3, Float64Array) };
    },
    graphFormat: () => w.graph_format(),
    // View: strikes with t in [t0, t1] (s) inside the lat/lon box (lon0 > lon1 wraps).
    // Returns the frontier corners [gap min, km]* and the box (log axes).
    view(t0, t1, lat0, lat1, lon0, lon1) {
      w.view(t0, t1, lat0, lat1, lon0, lon1);
      const s = slot(1, Float64Array);
      return { frontier: slot(0, Float64Array), strikes: s[0], xMin: s[1], xMax: s[2], yMin: s[3], yMax: s[4] };
    },
    // Clusters containing a strike in view at (km, min): [{label, size, first, last}]
    // (largest first), and a label per strike (0xffffffff: in none of them).
    select(km, min) {
      w.select(km, min);
      const c = slot(1, Uint32Array), s = slot(2, Float64Array), clusters = [];
      for (let k = 0; k < c.length; k += 4) clusters.push({ label: c[k], size: c[k + 1], first: c[k + 2], last: c[k + 3] });
      return { label: slot(0, Uint32Array), clusters, x: s[0], y: s[1], singletons: s[2] };
    },
    // Exact region of the selection's cluster `label`: rows [ylo, yhi (min), xlo, xhi (km)]*.
    region(label) {
      w.region(label);
      return slot(0, Float64Array);
    },
    // Similar region of the selection's cluster `label` (onProgress gets pass + fraction
    // meanwhile); with `union`, also the strikes in similar clusters and their best
    // similarity.
    similar(label, tol, union = false) {
      w.similar(label, tol, union ? 1 : 0);
      const s = slot(1, Float64Array);
      return { rows: slot(0, Float64Array), stability: s[0], passes: s[1],
               union: slot(2, Uint32Array), best: slot(3, Float64Array) };
    },
  };
}
