//! Storm picker engine: exact 2-parameter (distance, time gap) clustering of lightning
//! strikes, compiled to plain WebAssembly (no bindings generator).
//!
//! The interface is C-like: JS copies input arrays into memory from `alloc`, calls a
//! function, then reads its results from numbered output slots (`out_ptr` / `out_len`,
//! in bytes). Pointers into memory are invalidated by any later call (memory may grow).
//!
//! See graph.rs (the one sparse graph, built when strikes are loaded), engine.rs (the
//! queries, all answered from it) and tree.rs (single-linkage trees).

pub mod engine;
pub mod geo;
pub mod graph;
pub mod tree;

use std::cell::RefCell;

use engine::Engine;
use graph::Graph;

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    #[link_name = "progress"]
    fn js_progress(fraction: f64);
}

fn progress(f: f64) {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        js_progress(f)
    };
    #[cfg(not(target_arch = "wasm32"))]
    let _ = f;
}

thread_local! {
    static ENGINE: RefCell<Option<Engine>> = const { RefCell::new(None) };
    static OUT: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
}

fn bytes<T: Copy>(v: &[T]) -> Vec<u8> {
    // Plain numeric data, viewed as bytes (little endian on wasm32).
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
        .to_vec()
}

fn set_out(slots: Vec<Vec<u8>>) {
    OUT.with(|o| *o.borrow_mut() = slots);
}

fn with<R>(f: impl FnOnce(&mut Engine) -> R) -> R {
    ENGINE.with(|e| f(e.borrow_mut().as_mut().expect("no strikes loaded")))
}

#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    let mut v = vec![0u8; len.max(1)];
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// # Safety
/// `p` and `len` must come from one `alloc` call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dealloc(p: *mut u8, len: usize) {
    drop(unsafe { Vec::from_raw_parts(p, len.max(1), len.max(1)) });
}

#[unsafe(no_mangle)]
pub extern "C" fn out_ptr(slot: usize) -> *const u8 {
    OUT.with(|o| {
        o.borrow()
            .get(slot)
            .map_or(std::ptr::null(), |v| v.as_ptr())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn out_len(slot: usize) -> usize {
    OUT.with(|o| o.borrow().get(slot).map_or(0, |v| v.len()))
}

/// Load strikes: n times (seconds, ascending), latitudes and longitudes (degrees), f64,
/// and build the graph (progress reported as a fraction).
///
/// # Safety
/// Each pointer must point to n f64 values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn init(n: usize, t: *const f64, lat: *const f64, lon: *const f64) {
    let v = |p: *const f64| unsafe { std::slice::from_raw_parts(p, n) }.to_vec();
    let g = Graph::build(v(t), v(lat), v(lon), graph::BLOCK, graph::DIRECT, progress);
    ENGINE.with(|e| *e.borrow_mut() = Some(Engine::new(g)));
    set_out(Vec::new());
}

/// Version of the graph `graph` outputs: a graph saved under another version must not be
/// passed to `init_with_graph`.
#[unsafe(no_mangle)]
pub extern "C" fn graph_format() -> u32 {
    graph::FORMAT
}

/// Load strikes (as for `init`) with a graph saved from `graph` for the same strikes,
/// instead of building it: m edges (ea, eb u32; ed f64 km) and nf frontier corners (fr:
/// f64 pairs [gap s, km]). Returns 1 if used, 0 if not a well-formed graph (then nothing is
/// loaded: call `init`).
///
/// # Safety
/// t, lat, lon must point to n f64 values; ea, eb to m u32 values, ed to m f64 values; fr
/// to 2 nf f64 values.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn init_with_graph(
    n: usize,
    t: *const f64,
    lat: *const f64,
    lon: *const f64,
    m: usize,
    ea: *const u32,
    eb: *const u32,
    ed: *const f64,
    nf: usize,
    fr: *const f64,
) -> u32 {
    let v = |p: *const f64, k: usize| unsafe { std::slice::from_raw_parts(p, k) }.to_vec();
    let u = |p: *const u32| unsafe { std::slice::from_raw_parts(p, m) }.to_vec();
    let corners: Vec<(f64, f64)> = v(fr, 2 * nf).chunks(2).map(|c| (c[0], c[1])).collect();
    let g = Graph::from_parts(v(t, n), v(lat, n), v(lon, n), u(ea), u(eb), v(ed, m), corners);
    set_out(Vec::new());
    match g {
        Some(g) => {
            ENGINE.with(|e| *e.borrow_mut() = Some(Engine::new(g)));
            1
        }
        None => 0,
    }
}

/// The loaded graph, to save. Outputs: 0 ea u32, 1 eb u32, 2 ed f64 (km), 3 frontier
/// f64 pairs [gap s, km]*.
#[unsafe(no_mangle)]
pub extern "C" fn graph() {
    with(|e| {
        let g = &e.g;
        let fr: Vec<f64> = g.frontier.iter().flat_map(|&(y, x)| [y, x]).collect();
        set_out(vec![bytes(&g.ea), bytes(&g.eb), bytes(&g.ed), bytes(&fr)]);
    });
}

/// Set the view: strikes with t in [t0, t1] (s) inside the lat/lon box (lon0 > lon1
/// wraps); clicks list the clusters touching it. Outputs: 0 the dataset's frontier corners
/// f64 pairs [gap min, km]*, 1 stats f64 [strikes in view, x min km, x max km, y min min,
/// y max min] (the box, log axes: the same for every view).
#[unsafe(no_mangle)]
pub extern "C" fn view(t0: f64, t1: f64, lat0: f64, lat1: f64, lon0: f64, lon1: f64) {
    with(|e| {
        e.set_view(t0, t1, lat0, lat1, lon0, lon1);
        let fr: Vec<f64> = e.frontier.iter().flat_map(|&(g, x)| [g / 60.0, x]).collect();
        let stats = [
            e.view.as_ref().unwrap().strikes.len() as f64,
            e.x_min,
            e.bx.0,
            e.y_min / 60.0,
            e.bx.1 / 60.0,
        ];
        set_out(vec![bytes(&fr), bytes(&stats)]);
    });
}

/// Clusters containing a strike in view at (km, minutes). Outputs: 0 label per strike u32
/// (u32::MAX: none), 1 clusters u32 x4 [label, size, first strike, last strike],
/// 2 stats f64 [x km, y min, singletons].
#[unsafe(no_mangle)]
pub extern "C" fn select(km: f64, min: f64) {
    with(|e| {
        e.select(km, min * 60.0);
        let s = e.last.as_ref().unwrap();
        let cl: Vec<u32> = s
            .clusters
            .iter()
            .flat_map(|c| [c.label, c.size, c.first, c.last])
            .collect();
        let stats = [s.x, s.y_s / 60.0, s.singletons as f64];
        set_out(vec![bytes(&s.label), bytes(&cl), bytes(&stats)]);
    });
}

/// Exact region of the last selection's cluster with this label. Output: 0 rows f64
/// [y_lo, y_hi (min), x_lo, x_hi (km)]*.
#[unsafe(no_mangle)]
pub extern "C" fn region(label: u32) {
    with(|e| {
        let rows = e.region_of(label);
        set_out(vec![bytes(&rows)]);
    });
}

/// Similar region of the last selection's cluster with this label (progress reported as
/// pass + fraction). Outputs: 0 rows f64 (as for region), 1 stats f64 [stability (log-area
/// share of the box), passes], and if `union`: 2 strikes in similar clusters u32, 3 best
/// Jaccard similarity of each f64.
#[unsafe(no_mangle)]
pub extern "C" fn similar(label: u32, tol: f64, union: u32) {
    with(|e| {
        let s = e.similar(label, tol, union != 0, progress);
        let stats = [s.stability, s.passes as f64];
        set_out(vec![
            bytes(&s.rows),
            bytes(&stats),
            bytes(&s.union),
            bytes(&s.best),
        ]);
    });
}
