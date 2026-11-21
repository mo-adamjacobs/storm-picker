//! Exact 2-parameter single-linkage region sweep (native and Pyodide).
//!
//! A cluster is a connected component of the strike graph at (distance x, gap y).
//! Sweeping the gap upward, the single-linkage dendrogram over distance is updated one
//! edge at a time; each cluster's region is recorded as rows [y_lo, y_hi) x [x_lo, x_hi).
//! Clusters are identified by (XOR of per-strike random tags, size).
//!
//! This is a direct translation of the reference implementation in
//! `lightning/regions.py`; tests check the two against each other and brute force.
//! Arrays come in through the buffer protocol and go back as raw bytes (native endian),
//! which the Python wrapper reads with numpy.frombuffer.

use pyo3::buffer::PyBuffer;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

// ---------------------------------------------------------------- hashing
// Keys are (random 64-bit set hash, size): already well mixed, so hash them cheaply.
#[derive(Default)]
struct KeyHasher(u64);
impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0.rotate_left(8) ^ *b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }
    fn write_i64(&mut self, v: i64) {
        self.0 = (self.0.rotate_left(29) ^ v as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}
type Map<K, V> = HashMap<K, V, BuildHasherDefault<KeyHasher>>;
type Key = (i64, i64);

const ROOT: i64 = -1;
const FREED: i64 = -2;

// ---------------------------------------------------------------- dendrogram
/// Single-linkage merge tree over distance, updated by edge insertion.
/// Leaves 0..n-1 are strikes (height 0); each internal node merges two children at its
/// height. A node's cluster is a component for x in [height, parent height).
struct Tree {
    max_dist: f64,
    parent: Vec<i64>,
    height: Vec<f64>,
    kids: Vec<[usize; 2]>,
    h: Vec<i64>,
    size: Vec<i64>,
    cnt: Option<Vec<i64>>, // per-node count of marked strikes (overlap queries)
    free: Vec<usize>,
    // Planned insertion (set by `prepare`, consumed by `apply`).
    a: usize,
    b: usize,
    d: f64,
    lca: i64,
    p: Vec<usize>,
    q: Vec<usize>,
    eq: Vec<usize>,
    chain: Vec<usize>,
}

impl Tree {
    fn new(tags: &[i64], max_dist: f64, cnt: Option<Vec<i64>>) -> Self {
        let n = tags.len();
        Tree {
            max_dist,
            parent: vec![ROOT; n],
            height: vec![0.0; n],
            kids: vec![[usize::MAX; 2]; n],
            h: tags.to_vec(),
            size: vec![1; n],
            cnt,
            free: Vec::new(),
            a: 0,
            b: 0,
            d: 0.0,
            lca: ROOT,
            p: Vec::new(),
            q: Vec::new(),
            eq: Vec::new(),
            chain: Vec::new(),
        }
    }

    fn len(&self) -> usize {
        self.parent.len()
    }

    fn key(&self, w: usize) -> Key {
        (self.h[w], self.size[w])
    }

    fn interval(&self, v: usize) -> (f64, f64) {
        let p = self.parent[v];
        (self.height[v], if p >= 0 { self.height[p as usize] } else { self.max_dist })
    }

    fn new_node(&mut self) -> usize {
        if let Some(w) = self.free.pop() {
            return w;
        }
        self.parent.push(ROOT);
        self.height.push(0.0);
        self.kids.push([usize::MAX; 2]);
        self.h.push(0);
        self.size.push(0);
        if let Some(c) = self.cnt.as_mut() {
            c.push(0);
        }
        self.parent.len() - 1
    }

    /// Highest ancestor of v (or v) with height <= d.
    fn top_below(&self, mut v: usize, d: f64) -> usize {
        loop {
            let p = self.parent[v];
            if p >= 0 && self.height[p as usize] <= d {
                v = p as usize;
            } else {
                return v;
            }
        }
    }

    /// Plan adding edge (u, v) at distance d. Returns false if nothing would change.
    /// After `true`, `touched()` lists every node whose cluster will change.
    fn prepare(&mut self, u: usize, v: usize, d: f64) -> bool {
        let a = self.top_below(u, d);
        let b = self.top_below(v, d);
        if a == b {
            return false; // u and v already joined at distance <= d
        }
        self.a = a;
        self.b = b;
        self.d = d;
        self.p.clear();
        self.q.clear();
        // Walk both root paths upward in height order until they meet (or both end).
        let (mut x, mut y) = (self.parent[a], self.parent[b]);
        while x != y {
            if x >= 0 && y >= 0 && self.height[x as usize] == self.height[y as usize] {
                // Equal heights: one may be the other's ancestor via an equal-height chain.
                let mut met = false;
                for (lo, hi, side) in [(x, y, 0), (y, x, 1)] {
                    self.eq.clear();
                    let hl = self.height[lo as usize];
                    let mut w = lo;
                    while w >= 0 && w != hi && self.height[w as usize] == hl {
                        self.eq.push(w as usize);
                        w = self.parent[w as usize];
                    }
                    if w == hi {
                        if side == 0 {
                            self.p.extend_from_slice(&self.eq);
                        } else {
                            self.q.extend_from_slice(&self.eq);
                        }
                        x = hi;
                        y = hi;
                        met = true;
                        break;
                    }
                }
                if !met {
                    self.p.push(x as usize);
                    x = self.parent[x as usize];
                }
            } else if y < 0 || (x >= 0 && self.height[x as usize] < self.height[y as usize]) {
                self.p.push(x as usize);
                x = self.parent[x as usize];
            } else {
                self.q.push(y as usize);
                y = self.parent[y as usize];
            }
        }
        self.lca = x; // -1 when u and v were in different trees
        true
    }

    fn touched(&self) -> impl Iterator<Item = usize> + '_ {
        [self.a, self.b]
            .into_iter()
            .chain(self.p.iter().copied())
            .chain(self.q.iter().copied())
            .chain((self.lca >= 0).then_some(self.lca as usize))
    }

    /// Carry out the planned insertion; `self.chain` then holds the changed nodes.
    fn apply(&mut self) {
        let (a, b, d, lca) = (self.a, self.b, self.d, self.lca);
        let top_parent = if lca >= 0 { self.parent[lca as usize] } else { ROOT };
        let p = std::mem::take(&mut self.p);
        let q = std::mem::take(&mut self.q);
        let mut chain = std::mem::take(&mut self.chain);
        chain.clear();
        // New merge of a and b at d, then zip the two paths (stable by height, P first on
        // ties): each path node keeps its height and off-path child, and takes the
        // growing merged cluster.
        let mut cur = self.new_node();
        self.height[cur] = d;
        self.kids[cur] = [a, b];
        self.parent[a] = cur as i64;
        self.parent[b] = cur as i64;
        chain.push(cur);
        let mut prev = [a, b];
        let (mut ip, mut iq) = (0, 0);
        while ip < p.len() || iq < q.len() {
            let take_p = iq >= q.len() || (ip < p.len() && self.height[p[ip]] <= self.height[q[iq]]);
            let (w, side) = if take_p {
                ip += 1;
                (p[ip - 1], 0)
            } else {
                iq += 1;
                (q[iq - 1], 1)
            };
            let [k0, k1] = self.kids[w];
            let sib = if k0 == prev[side] { k1 } else { k0 };
            prev[side] = w;
            self.kids[w] = [cur, sib];
            self.parent[cur] = w as i64;
            cur = w;
            chain.push(w);
        }
        if lca >= 0 {
            // The old merge of the two sides is now redundant.
            let l = lca as usize;
            self.parent[cur] = top_parent;
            if top_parent >= 0 {
                let tp = top_parent as usize;
                if self.kids[tp][0] == l {
                    self.kids[tp][0] = cur;
                } else {
                    self.kids[tp][1] = cur;
                }
            }
            self.kids[l] = [usize::MAX; 2];
            self.parent[l] = FREED;
            self.free.push(l);
        } else {
            self.parent[cur] = ROOT;
        }
        // Recompute set hash, size (and marked count) bottom-up along the new path.
        for &w in &chain {
            let [c0, c1] = self.kids[w];
            self.h[w] = self.h[c0] ^ self.h[c1];
            self.size[w] = self.size[c0] + self.size[c1];
            if let Some(c) = self.cnt.as_mut() {
                c[w] = c[c0] + c[c1];
            }
        }
        self.p = p;
        self.q = q;
        self.chain = chain;
    }
}

/// Edge indices sorted by gap (stable).
fn gap_order(gap: &[f64]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..gap.len()).collect();
    order.sort_by(|&x, &y| gap[x].partial_cmp(&gap[y]).unwrap());
    order
}

// ---------------------------------------------------------------- regions
struct Regions {
    hash: Vec<i64>,
    size: Vec<i64>,
    rows: Vec<f64>, // 4 per row: y_lo, y_hi, x_lo, x_hi; grouped by cluster
    off: Vec<i64>,
}

fn regions_impl(
    ei: &[i32], ej: &[i32], dist: &[f64], gap: &[f64], max_dist: f64, min_size: i64,
    tags: &[i64], mut progress: impl FnMut(f64) -> PyResult<()>,
) -> PyResult<Regions> {
    let mut t = Tree::new(tags, max_dist, None);
    let order = gap_order(gap);
    let mut open: Map<Key, (f64, f64, f64)> = Map::default(); // key -> (y_lo, x_lo, x_hi)
    let mut ids: Map<Key, usize> = Map::default();
    let (mut c_hash, mut c_size) = (Vec::new(), Vec::new());
    let (mut r_id, mut r_val): (Vec<usize>, Vec<f64>) = (Vec::new(), Vec::new());
    let mut close = |key: Key, ylo: f64, yhi: f64, xlo: f64, xhi: f64| {
        let c = *ids.entry(key).or_insert_with(|| {
            c_hash.push(key.0);
            c_size.push(key.1);
            c_size.len() - 1
        });
        r_id.push(c);
        r_val.extend_from_slice(&[ylo, yhi, xlo, xhi]);
    };
    let state = |t: &Tree, w: usize| -> Option<(Key, (f64, f64))> {
        if t.parent[w] == FREED || t.size[w] < min_size {
            return None;
        }
        let (lo, hi) = t.interval(w);
        if lo < hi { Some((t.key(w), (lo, hi))) } else { None }
    };

    let mut stamp: Vec<usize> = Vec::new(); // node -> level+1 when last seen
    let mut nodes: Vec<usize> = Vec::new();
    let mut before: Vec<(Key, (f64, f64))> = Vec::new();
    let mut after: Map<Key, (f64, f64)> = Map::default();
    let mut after_order: Vec<(Key, (f64, f64))> = Vec::new();
    let n_levels = {
        let mut c = 0;
        for k in 0..order.len() {
            if k == 0 || gap[order[k]] != gap[order[k - 1]] {
                c += 1;
            }
        }
        c
    };
    let (mut level, mut s) = (0usize, 0usize);
    while s < order.len() {
        let y = gap[order[s]];
        let mut e_end = s;
        while e_end < order.len() && gap[order[e_end]] == y {
            e_end += 1;
        }
        if level % 500 == 0 {
            progress(level as f64 / n_levels.max(1) as f64)?;
        }
        let tag = level + 1;
        nodes.clear();
        before.clear();
        for &e in &order[s..e_end] {
            if !t.prepare(ei[e] as usize, ej[e] as usize, dist[e]) {
                continue;
            }
            let touched: Vec<usize> = t.touched().collect();
            for w in touched {
                if stamp.len() <= w {
                    stamp.resize(t.len().max(w + 1), 0);
                }
                if stamp[w] != tag {
                    // First sight this level: remember the old cluster.
                    stamp[w] = tag;
                    nodes.push(w);
                    if let Some(st) = state(&t, w) {
                        before.push(st);
                    }
                }
            }
            t.apply();
            if stamp.len() < t.len() {
                stamp.resize(t.len(), 0);
            }
            for i in 0..t.chain.len() {
                let w = t.chain[i];
                if stamp[w] != tag {
                    stamp[w] = tag;
                    nodes.push(w);
                }
            }
        }
        after.clear();
        after_order.clear();
        for &w in &nodes {
            if let Some((k, iv)) = state(&t, w) {
                after.insert(k, iv);
                after_order.push((k, iv));
            }
        }
        for &(key, _) in &before {
            // Close rows whose cluster changed or vanished.
            if let Some(&(ylo, xlo, xhi)) = open.get(&key) {
                if after.get(&key) != Some(&(xlo, xhi)) {
                    open.remove(&key);
                    close(key, ylo, y, xlo, xhi);
                }
            }
        }
        for &(key, (xlo, xhi)) in &after_order {
            open.entry(key).or_insert((y, xlo, xhi));
        }
        level += 1;
        s = e_end;
    }
    // Close every row still open (open to the top). Sorted for determinism.
    let mut rest: Vec<(Key, (f64, f64, f64))> = open.into_iter().collect();
    rest.sort_by(|p, q| p.0.cmp(&q.0));
    for (key, (ylo, xlo, xhi)) in rest {
        close(key, ylo, f64::INFINITY, xlo, xhi);
    }
    drop(close);
    // Group rows by cluster (counting sort keeps each cluster's rows in closing order).
    let nc = c_size.len();
    let mut off = vec![0i64; nc + 1];
    for &c in &r_id {
        off[c + 1] += 1;
    }
    for c in 0..nc {
        off[c + 1] += off[c];
    }
    let mut fill: Vec<i64> = off[..nc].to_vec();
    let mut rows = vec![0.0f64; r_val.len()];
    for (r, &c) in r_id.iter().enumerate() {
        let dst = fill[c] as usize;
        fill[c] += 1;
        rows[4 * dst..4 * dst + 4].copy_from_slice(&r_val[4 * r..4 * r + 4]);
    }
    progress(1.0)?;
    Ok(Regions { hash: c_hash, size: c_size, rows, off })
}

// ---------------------------------------------------------------- overlaps
struct Overlaps {
    hash: Vec<i64>,
    size: Vec<i64>,
    count: Vec<i64>,
    union: Vec<u8>,
}

/// Clusters D whose change from the marked cluster C, |C xor D| / |C and D|, is at most
/// `tol` and which are real regions (present in the sorted (hash, size) table): their key
/// and |C and D|. Also the union of their strikes.
fn overlaps_impl(
    ei: &[i32], ej: &[i32], dist: &[f64], gap: &[f64], max_dist: f64, tags: &[i64],
    marked: &[u8], c_size: i64, tol: f64, sorted_hash: &[i64], sorted_size: &[i64],
) -> Overlaps {
    let n = tags.len();
    let cnt: Vec<i64> = marked.iter().map(|&m| (m != 0) as i64).collect();
    let mut t = Tree::new(tags, max_dist, Some(cnt));
    let order = gap_order(gap);
    let mut out: Map<Key, i64> = Map::default();
    let mut union = vec![0u8; n];
    let mut done: Vec<bool> = Vec::new(); // every leaf under the node is already in union
    let mut stack: Vec<usize> = Vec::new();
    let exists = |key: Key| -> bool {
        let (mut lo, mut hi) = (0usize, sorted_hash.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if (sorted_hash[mid], sorted_size[mid]) < key { lo = mid + 1 } else { hi = mid }
        }
        lo < sorted_hash.len() && (sorted_hash[lo], sorted_size[lo]) == key
    };
    for &e in &order {
        if !t.prepare(ei[e] as usize, ej[e] as usize, dist[e]) {
            continue;
        }
        t.apply();
        if done.len() < t.len() {
            done.resize(t.len(), false);
        }
        for i in 0..t.chain.len() {
            done[t.chain[i]] = false; // its subtree just changed
        }
        for i in 0..t.chain.len() {
            let w = t.chain[i];
            let c = t.cnt.as_ref().unwrap()[w];
            if c == 0 {
                continue;
            }
            let key = t.key(w);
            let change = (c_size + key.1 - 2 * c) as f64 / c as f64;
            if change <= tol + 1e-12 && exists(key) {
                out.insert(key, c); // only clusters within the tolerance are reported
                // Collect w's strikes; every node visited ends up fully collected.
                stack.clear();
                stack.push(w);
                while let Some(v) = stack.pop() {
                    if v < n {
                        union[v] = 1;
                    } else if !done[v] {
                        done[v] = true;
                        stack.extend_from_slice(&t.kids[v]);
                    }
                }
            }
        }
    }
    let mut hash = Vec::with_capacity(out.len());
    let mut size = Vec::with_capacity(out.len());
    let mut count = Vec::with_capacity(out.len());
    let mut items: Vec<(Key, i64)> = out.into_iter().collect();
    items.sort_by(|p, q| p.0.cmp(&q.0));
    for ((h, s), c) in items {
        hash.push(h);
        size.push(s);
        count.push(c);
    }
    Overlaps { hash, size, count, union }
}

// ---------------------------------------------------------------- edge reduction
/// Union-find with path halving.
fn find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

/// Kruskal reconstruction tree of a forest: bottleneck (minimax) distance queries.
struct Krt {
    n: usize,
    weight: Vec<f64>,
    depth: Vec<u32>,
    up: Vec<Vec<u32>>, // up[l][v]: 2^l-th ancestor (roots point to themselves)
}

impl Krt {
    fn new(n: usize, forest: &[(u32, u32, f64)]) -> Self {
        let mut order: Vec<usize> = (0..forest.len()).collect();
        order.sort_by(|&x, &y| forest[x].2.partial_cmp(&forest[y].2).unwrap());
        let mut uf: Vec<usize> = (0..n).collect();
        let mut top: Vec<u32> = (0..n as u32).collect(); // union-find root -> its tree node
        let mut parent: Vec<u32> = (0..n as u32).collect(); // node -> parent (self at roots)
        let mut weight = vec![0.0f64; n];
        for &k in &order {
            let (u, v, d) = forest[k];
            let (a, b) = (find(&mut uf, u as usize), find(&mut uf, v as usize));
            if a == b {
                continue;
            }
            let node = parent.len() as u32;
            parent.push(node);
            weight.push(d);
            parent[top[a] as usize] = node;
            parent[top[b] as usize] = node;
            uf[b] = a;
            top[a] = node;
        }
        let m = parent.len();
        // Parents always have larger ids, so one reverse sweep gives depths.
        let mut depth = vec![0u32; m];
        for v in (0..m).rev() {
            let p = parent[v] as usize;
            if p != v {
                depth[v] = depth[p] + 1;
            }
        }
        let max_depth = depth.iter().copied().max().unwrap_or(0);
        let levels = (32 - max_depth.leading_zeros()).max(1) as usize;
        let mut up = vec![parent];
        for l in 1..levels {
            let prev = &up[l - 1];
            let next: Vec<u32> = (0..m).map(|v| prev[prev[v] as usize]).collect();
            up.push(next);
        }
        Krt { n, weight, depth, up }
    }

    /// Bottleneck distance between leaves u and v; infinity if not connected.
    fn minimax(&self, u: usize, v: usize) -> f64 {
        let (mut u, mut v) = (u, v);
        if self.depth[u] < self.depth[v] {
            std::mem::swap(&mut u, &mut v);
        }
        let mut diff = self.depth[u] - self.depth[v];
        let mut l = 0;
        while diff > 0 {
            if diff & 1 == 1 {
                u = self.up[l][u] as usize;
            }
            diff >>= 1;
            l += 1;
        }
        if u == v {
            return if u >= self.n { self.weight[u] } else { f64::INFINITY };
        }
        for l in (0..self.up.len()).rev() {
            let (pu, pv) = (self.up[l][u] as usize, self.up[l][v] as usize);
            if pu != pv {
                u = pu;
                v = pv;
            }
        }
        let (pu, pv) = (self.up[0][u] as usize, self.up[0][v] as usize);
        if pu == pv && pu != u { self.weight[pu] } else { f64::INFINITY }
    }
}

/// Minimum spanning forest (by distance) of the given edges.
fn spanning_forest(n: usize, mut edges: Vec<(u32, u32, f64)>) -> Vec<(u32, u32, f64)> {
    edges.sort_by(|x, y| x.2.partial_cmp(&y.2).unwrap());
    let mut uf: Vec<usize> = (0..n).collect();
    let mut out = Vec::with_capacity(n);
    for (u, v, d) in edges {
        let (a, b) = (find(&mut uf, u as usize), find(&mut uf, v as usize));
        if a != b {
            uf[b] = a;
            out.push((u, v, d));
        }
    }
    out
}

/// Indices of edges that can change connectivity at some (distance, gap) threshold.
/// Edge e is redundant if its endpoints are already joined by edges with no larger
/// distance and no larger gap. Edges are swept by gap (then distance) in batches; each
/// batch is tested against the distance-spanning forest of all earlier batches.
/// Same algorithm as `reduce_h0` in lightning/graph.py.
fn reduce_impl(
    n: usize, ei: &[i32], ej: &[i32], dist: &[f64], gap: &[f64], batch: usize,
    mut progress: impl FnMut(f64) -> PyResult<()>,
) -> PyResult<Vec<i64>> {
    let mut order: Vec<usize> = (0..gap.len()).collect();
    order.sort_by(|&x, &y| {
        gap[x].partial_cmp(&gap[y]).unwrap().then(dist[x].partial_cmp(&dist[y]).unwrap())
    });
    let mut forest: Vec<(u32, u32, f64)> = Vec::new();
    let mut kept: Vec<i64> = Vec::new();
    let total = order.len().max(1);
    for (b, chunk) in order.chunks(batch.max(1)).enumerate() {
        let krt = Krt::new(n, &forest);
        let start = kept.len();
        for &e in chunk {
            // Forest edges come from earlier batches, so their gaps are <= this edge's.
            if !(krt.minimax(ei[e] as usize, ej[e] as usize) <= dist[e]) {
                kept.push(e as i64);
            }
        }
        let mut edges = std::mem::take(&mut forest);
        edges.extend(kept[start..].iter().map(|&e| (ei[e as usize] as u32, ej[e as usize] as u32, dist[e as usize])));
        forest = spanning_forest(n, edges);
        progress(((b + 1) * batch).min(total) as f64 / total as f64)?;
    }
    kept.sort_unstable();
    Ok(kept)
}

// ---------------------------------------------------------------- Python bindings
fn bytes_of<'py, T: Copy>(py: Python<'py>, v: &[T]) -> Bound<'py, PyBytes> {
    // Plain numeric data: view the slice as bytes (native endian) for numpy.frombuffer.
    let raw = unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) };
    PyBytes::new(py, raw)
}

/// cluster_regions(ei, ej, dist, gap, max_dist, min_size, tags, progress=None)
/// -> (hash bytes[i64], size bytes[i64], rows bytes[f64 x4], off bytes[i64])
#[pyfunction]
#[pyo3(signature = (ei, ej, dist, gap, max_dist, min_size, tags, progress=None))]
fn cluster_regions<'py>(
    py: Python<'py>, ei: PyBuffer<i32>, ej: PyBuffer<i32>, dist: PyBuffer<f64>, gap: PyBuffer<f64>,
    max_dist: f64, min_size: i64, tags: PyBuffer<i64>, progress: Option<Bound<'py, PyAny>>,
) -> PyResult<(Bound<'py, PyBytes>, Bound<'py, PyBytes>, Bound<'py, PyBytes>, Bound<'py, PyBytes>)> {
    let (ei, ej) = (ei.to_vec(py)?, ej.to_vec(py)?);
    let (dist, gap, tags) = (dist.to_vec(py)?, gap.to_vec(py)?, tags.to_vec(py)?);
    let r = regions_impl(&ei, &ej, &dist, &gap, max_dist, min_size, &tags, |f| {
        if let Some(cb) = &progress {
            cb.call1((f,))?;
        }
        Ok(())
    })?;
    Ok((bytes_of(py, &r.hash), bytes_of(py, &r.size), bytes_of(py, &r.rows), bytes_of(py, &r.off)))
}

/// overlaps(ei, ej, dist, gap, max_dist, tags, marked, c_size, tol, sorted_hash, sorted_size)
/// -> (hash bytes[i64], size bytes[i64], count bytes[i64], union bytes[u8])
#[pyfunction]
#[allow(clippy::too_many_arguments)]
fn overlaps<'py>(
    py: Python<'py>, ei: PyBuffer<i32>, ej: PyBuffer<i32>, dist: PyBuffer<f64>, gap: PyBuffer<f64>,
    max_dist: f64, tags: PyBuffer<i64>, marked: PyBuffer<u8>, c_size: i64, tol: f64,
    sorted_hash: PyBuffer<i64>, sorted_size: PyBuffer<i64>,
) -> PyResult<(Bound<'py, PyBytes>, Bound<'py, PyBytes>, Bound<'py, PyBytes>, Bound<'py, PyBytes>)> {
    let o = overlaps_impl(
        &ei.to_vec(py)?, &ej.to_vec(py)?, &dist.to_vec(py)?, &gap.to_vec(py)?, max_dist,
        &tags.to_vec(py)?, &marked.to_vec(py)?, c_size, tol, &sorted_hash.to_vec(py)?,
        &sorted_size.to_vec(py)?,
    );
    Ok((bytes_of(py, &o.hash), bytes_of(py, &o.size), bytes_of(py, &o.count), bytes_of(py, &o.union)))
}

/// reduce(n, ei, ej, dist, gap, batch, progress=None) -> kept edge indices bytes[i64]
#[pyfunction]
#[pyo3(signature = (n, ei, ej, dist, gap, batch, progress=None))]
#[allow(clippy::too_many_arguments)]
fn reduce<'py>(
    py: Python<'py>, n: usize, ei: PyBuffer<i32>, ej: PyBuffer<i32>, dist: PyBuffer<f64>,
    gap: PyBuffer<f64>, batch: usize, progress: Option<Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyBytes>> {
    let kept = reduce_impl(
        n, &ei.to_vec(py)?, &ej.to_vec(py)?, &dist.to_vec(py)?, &gap.to_vec(py)?, batch,
        |f| {
            if let Some(cb) = &progress {
                cb.call1((f,))?;
            }
            Ok(())
        },
    )?;
    Ok(bytes_of(py, &kept))
}

#[pymodule]
fn lightning_sweep(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(reduce, m)?)?;
    m.add_function(wrap_pyfunction!(cluster_regions, m)?)?;
    m.add_function(wrap_pyfunction!(overlaps, m)?)?;
    Ok(())
}
