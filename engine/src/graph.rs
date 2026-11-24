//! The strike graph: built once, for every threshold, as sparse as it can be.
//!
//! Strikes are indexed in time order. A pair (a, b), a < b, has a distance (WGS84 km) and
//! a time gap. At threshold (x, y) the pairs with distance ≤ x and gap ≤ y are present; a
//! cluster is a connected component.
//!
//! **Which edges are stored.** Order all pairs by (gap, distance, a, b). A pair is stored
//! unless its strikes are already joined by stored pairs earlier in that order with no
//! larger distance (so no larger gap either). Then wherever the pair is present, so is a
//! path joining its strikes, and the components at every threshold are exactly those of
//! all pairs. Every stored edge is needed: without it its strikes are apart just below its
//! own threshold. (With ties the order decides which of two equivalent edges is kept.)
//! There is no distance or gap limit: the graph is complete for every threshold.
//!
//! **How they are found.** Pairs are taken in gap bands, each `RATIO` times the last.
//! Before each band, the components of the edges stored so far are labelled at log-spaced
//! distance levels, for every strike and every KD-tree node that lies in one component.
//! A pair whose strikes share a label at a level no larger than their distance is already
//! joined (by edges of smaller gap and distance) and is skipped; so is every pair between
//! two tree nodes with the same label at a level no larger than any distance between them.
//! The rest are taken in order and stored if the single-linkage tree of the stored edges
//! (over distance) does not already join them at their distance.
//!
//! Strikes are grouped in time blocks, each with a KD-tree over unit vectors (sphere bounds,
//! see geo.rs, only ever used to skip). Wide bands are searched tree against tree; narrow
//! bands (few strikes per strike within the band's gaps) by scanning strikes in time order.
//! Every distance that decides anything is an exact geodesic.
//!
//! **The frontier** comes out of the same sweep: once the stored edges join all strikes,
//! the root of their single-linkage tree is at the least distance doing so at the
//! current gap.

use crate::geo::{self, Geo};
use crate::tree::{ROOT, Tree};

/// Version of what is stored (see `from_parts`): bump whenever the edges a build keeps for
/// the same strikes could differ, so graphs saved by an older engine are not reused.
pub const FORMAT: u32 = 1;

/// Strikes per time block (each with its own KD-tree).
pub const BLOCK: usize = 1024;
const LEAF: usize = 8;
/// Each gap band and distance level is this factor beyond the last.
const RATIO: f64 = 1.5;
/// Scan a band strike by strike when, on average, each strike has at most this many
/// strikes within the band's gaps; search tree against tree otherwise.
pub const DIRECT: f64 = 64.0;
/// Beyond the longest WGS84 geodesic (20003.93 km).
const ANY_KM: f64 = 20100.0;
const MIXED: u32 = u32::MAX;

struct Node {
    lo: [f64; 3],
    hi: [f64; 3],
    /// Time range of the node's strikes.
    t0: f64,
    t1: f64,
    start: u32,
    end: u32,
    kids: Option<(u32, u32)>,
}

struct Block {
    nodes: Vec<Node>,
    perm: Vec<u32>,
}

impl Block {
    fn new(xyz: &[[f64; 3]], t: &[f64], start: usize, end: usize) -> Self {
        let mut b = Block {
            nodes: Vec::new(),
            perm: (start as u32..end as u32).collect(),
        };
        b.build(xyz, t, 0, end - start);
        b
    }

    fn build(&mut self, xyz: &[[f64; 3]], t: &[f64], s: usize, e: usize) -> u32 {
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        let (mut t0, mut t1) = (f64::INFINITY, f64::NEG_INFINITY);
        for &p in &self.perm[s..e] {
            let q = &xyz[p as usize];
            for k in 0..3 {
                lo[k] = lo[k].min(q[k]);
                hi[k] = hi[k].max(q[k]);
            }
            t0 = t0.min(t[p as usize]);
            t1 = t1.max(t[p as usize]);
        }
        let id = self.nodes.len() as u32;
        self.nodes.push(Node {
            lo,
            hi,
            t0,
            t1,
            start: s as u32,
            end: e as u32,
            kids: None,
        });
        if e - s > LEAF {
            let axis = (0..3)
                .max_by(|&x, &y| (hi[x] - lo[x]).total_cmp(&(hi[y] - lo[y])))
                .unwrap();
            let mid = (s + e) / 2;
            self.perm[s..e].select_nth_unstable_by(mid - s, |&p, &q| {
                xyz[p as usize][axis].total_cmp(&xyz[q as usize][axis])
            });
            let l = self.build(xyz, t, s, mid);
            let r = self.build(xyz, t, mid, e);
            self.nodes[id as usize].kids = Some((l, r));
        }
        id
    }
}

fn find(uf: &mut [u32], mut x: u32) -> u32 {
    while uf[x as usize] != x {
        uf[x as usize] = uf[uf[x as usize] as usize];
        x = uf[x as usize];
    }
    x
}

pub struct Graph {
    pub n: usize,
    /// Seconds since the first strike, ascending.
    pub t: Vec<f64>,
    pub lat: Vec<f64>,
    pub lon: Vec<f64>,
    /// Stored edges: endpoints (a < b) and distance (km), in (gap, distance, a, b) order.
    pub ea: Vec<u32>,
    pub eb: Vec<u32>,
    pub ed: Vec<f64>,
    /// Edge ids in (distance, gap) order.
    pub by_km: Vec<u32>,
    /// Each strike's edges (ids, in gap order): `inc[off[u]..off[u + 1]]`.
    off: Vec<u32>,
    inc: Vec<u32>,
    /// Work done building (profiling): exact geodesics; tree node pairs visited, pairs
    /// looked at, candidates left for the single-linkage tree.
    pub geodesics: u64,
    pub stats: [u64; 3],
    /// The frontier, found during the build: corners (gap s, km) such that from each gap
    /// on (until the next corner) all strikes are in one cluster from that distance on.
    /// Gaps ascending, distances descending.
    pub frontier: Vec<(f64, f64)>,
}

impl Graph {
    /// `t` must be ascending.
    pub fn new(t: Vec<f64>, lat: Vec<f64>, lon: Vec<f64>) -> Self {
        Self::build(t, lat, lon, BLOCK, DIRECT, |_| {})
    }

    pub fn with_block(t: Vec<f64>, lat: Vec<f64>, lon: Vec<f64>, block: usize) -> Self {
        Self::build(t, lat, lon, block, DIRECT, |_| {})
    }

    /// Build the graph (see the module notes). `direct`: see `DIRECT`. `progress(fraction)`.
    pub fn build(
        t: Vec<f64>,
        lat: Vec<f64>,
        lon: Vec<f64>,
        block: usize,
        direct: f64,
        mut progress: impl FnMut(f64),
    ) -> Self {
        let n = t.len();
        let mut g = Graph {
            n,
            t,
            lat,
            lon,
            ea: Vec::new(),
            eb: Vec::new(),
            ed: Vec::new(),
            by_km: Vec::new(),
            off: vec![0; n + 1],
            inc: Vec::new(),
            geodesics: 0,
            stats: [0; 3],
            frontier: Vec::new(),
        };
        if n >= 2 {
            g.sweep(block, direct, &mut progress);
        }
        g.index();
        progress(1.0);
        g
    }

    /// A graph saved from an earlier build of the same strikes: its edges (`ea`, `eb`, `ed`,
    /// in build order) and frontier. None if they are not a well-formed graph of n strikes
    /// in that order (edges a < b < n, gaps then distances ascending, frontier corners with
    /// gaps ascending and distances descending).
    pub fn from_parts(
        t: Vec<f64>,
        lat: Vec<f64>,
        lon: Vec<f64>,
        ea: Vec<u32>,
        eb: Vec<u32>,
        ed: Vec<f64>,
        frontier: Vec<(f64, f64)>,
    ) -> Option<Self> {
        let n = t.len();
        if lat.len() != n || lon.len() != n || eb.len() != ea.len() || ed.len() != ea.len() {
            return None;
        }
        let mut last = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for e in 0..ea.len() {
            let (a, b, d) = (ea[e] as usize, eb[e] as usize, ed[e]);
            if a >= b || b >= n || d.is_nan() || d < 0.0 {
                return None;
            }
            let key = (t[b] - t[a], d);
            if key.0 < last.0 || (key.0 == last.0 && key.1 < last.1) {
                return None;
            }
            last = key;
        }
        if frontier.windows(2).any(|w| w[0].0 >= w[1].0 || w[0].1 <= w[1].1) {
            return None;
        }
        let mut g = Graph {
            n,
            t,
            lat,
            lon,
            ea,
            eb,
            ed,
            by_km: Vec::new(),
            off: vec![0; n + 1],
            inc: Vec::new(),
            geodesics: 0,
            stats: [0; 3],
            frontier,
        };
        g.index();
        Some(g)
    }

    pub fn edges(&self) -> usize {
        self.ea.len()
    }

    /// Strike u's edges (ids, in gap order).
    pub fn edges_of(&self, u: usize) -> &[u32] {
        &self.inc[self.off[u] as usize..self.off[u + 1] as usize]
    }

    /// The other end of edge e from strike u.
    pub fn other(&self, e: u32, u: usize) -> usize {
        let (a, b) = (self.ea[e as usize] as usize, self.eb[e as usize] as usize);
        if a == u { b } else { a }
    }

    pub fn gap(&self, e: u32) -> f64 {
        self.t[self.eb[e as usize] as usize] - self.t[self.ea[e as usize] as usize]
    }

    fn sweep(&mut self, block: usize, direct: f64, progress: &mut impl FnMut(f64)) {
        let n = self.n;
        let (t, lat, lon) = (&self.t, &self.lat, &self.lon);
        let geo = Geo::default();
        let xyz: Vec<[f64; 3]> = (0..n).map(|i| geo::unit(lat[i], lon[i])).collect();
        let blocks: Vec<Block> = (0..n)
            .step_by(block)
            .map(|s| Block::new(&xyz, t, s, (s + block).min(n)))
            .collect();
        // Distance levels and gap band ends.
        let mut xs = Vec::new();
        let mut x = 0.01;
        while x < ANY_KM {
            xs.push(x);
            x *= RATIO;
        }
        xs.push(ANY_KM);
        let l = xs.len();
        let span = t[n - 1] - t[0];
        let mut ys = vec![0.0];
        let mut y = 0.05;
        while y < span {
            ys.push(y);
            y *= RATIO;
        }
        if span > 0.0 {
            ys.push(span);
        }
        // Levels 0..level_at(d) have distance ≤ d.
        let level_at = |d: f64| xs.partition_point(|&v| v <= d);
        // Components of the stored edges at each level, and those as of the band's start.
        let mut uf: Vec<Vec<u32>> = (0..l).map(|_| (0..n as u32).collect()).collect();
        let mut comp = vec![0u32; n * l];
        let mut tree = Tree::new(n, f64::INFINITY, None);
        let mut cand: Vec<(f64, f64, u32, u32)> = Vec::new();
        let mut stack: Vec<(u32, u32, u32, u32)> = Vec::new();
        let (mut geodesics, mut stats) = (0u64, [0u64; 3]);
        // Components of all stored edges, and the root of the single-linkage tree once
        // there is one: its height is the frontier's distance at the current gap.
        let (mut parts, mut root) = (n, 0usize);
        let mut frontier: Vec<(f64, f64)> = Vec::new();
        let bands = ys.len();
        for (k, &yhi) in ys.iter().enumerate() {
            let ylo = if k == 0 { -1.0 } else { ys[k - 1] };
            for j in 0..l {
                for u in 0..n {
                    comp[u * l + j] = find(&mut uf[j], u as u32);
                }
            }
            // Per tree node and level: the label all its strikes share, or MIXED.
            let labs: Vec<Vec<u32>> = blocks
                .iter()
                .map(|b| {
                    let m = b.nodes.len();
                    let mut lab = vec![MIXED; m * l];
                    for id in (0..m).rev() {
                        let nd = &b.nodes[id];
                        for j in 0..l {
                            lab[id * l + j] = match nd.kids {
                                Some((p, q)) => {
                                    let (p, q) = (lab[p as usize * l + j], lab[q as usize * l + j]);
                                    if p == q { p } else { MIXED }
                                }
                                None => {
                                    let ps = &b.perm[nd.start as usize..nd.end as usize];
                                    let c0 = comp[ps[0] as usize * l + j];
                                    if ps.iter().all(|&p| comp[p as usize * l + j] == c0) {
                                        c0
                                    } else {
                                        MIXED
                                    }
                                }
                            };
                        }
                    }
                    lab
                })
                .collect();
            // A pair within the band, unless its strikes are already joined.
            let consider = |a: usize, b: usize, cand: &mut Vec<_>, geodesics: &mut u64| {
                let gap = t[b] - t[a];
                if gap <= ylo || gap > yhi {
                    return;
                }
                let c2 = geo::chord2(&xyz[a], &xyz[b]);
                let lv = level_at(geo::lower(geo::angle(c2.sqrt())));
                if lv > 0 && comp[a * l + lv - 1] == comp[b * l + lv - 1] {
                    return;
                }
                *geodesics += 1;
                let d = geo.km(lat[a], lon[a], lat[b], lon[b]);
                let lv = level_at(d);
                if lv > 0 && comp[a * l + lv - 1] == comp[b * l + lv - 1] {
                    return;
                }
                cand.push((gap, d, a as u32, b as u32));
            };
            cand.clear();
            // Average strikes per strike within the band's gaps (two pointers).
            let (mut s, mut e, mut within) = (0usize, 0usize, 0usize);
            for a in 0..n {
                s = s.max(a + 1);
                e = e.max(a + 1);
                while s < n && t[s] - t[a] <= ylo {
                    s += 1;
                }
                while e < n && t[e] - t[a] <= yhi {
                    e += 1;
                }
                within += e - s.min(e);
            }
            if (within as f64) <= direct * n as f64 {
                let (mut s, mut e) = (0usize, 0usize);
                for a in 0..n {
                    s = s.max(a + 1);
                    e = e.max(a + 1);
                    while s < n && t[s] - t[a] <= ylo {
                        s += 1;
                    }
                    while e < n && t[e] - t[a] <= yhi {
                        e += 1;
                    }
                    for b in s..e {
                        stats[1] += 1;
                        consider(a, b, &mut cand, &mut geodesics);
                    }
                }
            } else {
                for qi in 0..blocks.len() {
                    let q0 = &blocks[qi].nodes[0];
                    for (ci, cb) in blocks.iter().enumerate().skip(qi) {
                        let c0 = &cb.nodes[0];
                        if c0.t0 - q0.t1 > yhi {
                            break;
                        }
                        if c0.t1 - q0.t0 > ylo {
                            stack.push((qi as u32, 0, ci as u32, 0));
                        }
                    }
                    while let Some((qi, qn, ci, cn)) = stack.pop() {
                        let (qb, cb) = (&blocks[qi as usize], &blocks[ci as usize]);
                        let (q, c) = (&qb.nodes[qn as usize], &cb.nodes[cn as usize]);
                        stats[0] += 1;
                        let (gmin, gmax) = (c.t0 - q.t1, c.t1 - q.t0);
                        if gmax <= ylo || gmin > yhi || gmax < 0.0 {
                            continue;
                        }
                        let mut near2 = 0.0;
                        for k in 0..3 {
                            let d = (c.lo[k] - q.hi[k]).max(q.lo[k] - c.hi[k]).max(0.0);
                            near2 += d * d;
                        }
                        let lv = level_at(geo::lower(geo::angle(near2.sqrt())));
                        if lv > 0 {
                            let lq = labs[qi as usize][qn as usize * l + lv - 1];
                            if lq != MIXED && lq == labs[ci as usize][cn as usize * l + lv - 1] {
                                continue; // every pair between them is already joined
                            }
                        }
                        match (q.kids, c.kids) {
                            (None, None) => {
                                for &a in &qb.perm[q.start as usize..q.end as usize] {
                                    for &b in &cb.perm[c.start as usize..c.end as usize] {
                                        if b > a {
                                            stats[1] += 1;
                                            consider(a as usize, b as usize, &mut cand, &mut geodesics);
                                        }
                                    }
                                }
                            }
                            (Some((x, y)), None) => {
                                stack.push((qi, x, ci, cn));
                                stack.push((qi, y, ci, cn));
                            }
                            (None, Some((x, y))) => {
                                stack.push((qi, qn, ci, x));
                                stack.push((qi, qn, ci, y));
                            }
                            (Some((x, y)), Some((u, v))) => {
                                if q.end - q.start >= c.end - c.start {
                                    stack.push((qi, x, ci, cn));
                                    stack.push((qi, y, ci, cn));
                                } else {
                                    stack.push((qi, qn, ci, u));
                                    stack.push((qi, qn, ci, v));
                                }
                            }
                        }
                    }
                    progress((k as f64 + (qi + 1) as f64 / blocks.len() as f64) / bands as f64);
                }
            }
            stats[2] += cand.len() as u64;
            cand.sort_by(|p, q| {
                p.0.total_cmp(&q.0)
                    .then(p.1.total_cmp(&q.1))
                    .then(p.2.cmp(&q.2))
                    .then(p.3.cmp(&q.3))
            });
            for (i, &(gap, d, a, b)) in cand.iter().enumerate() {
                'store: {
                    // Already joined by edges stored so far at a level ≤ d: no need for
                    // the tree.
                    let lv = level_at(d);
                    if lv > 0 && find(&mut uf[lv - 1], a) == find(&mut uf[lv - 1], b) {
                        break 'store;
                    }
                    if !tree.insert(a as usize, b as usize, d) {
                        break 'store;
                    }
                    let top = *tree.chain.last().unwrap();
                    if tree.parent(top) == ROOT {
                        root = top;
                    }
                    self.ea.push(a);
                    self.eb.push(b);
                    self.ed.push(d);
                    let from = xs.partition_point(|&v| v < d);
                    for (j, level) in uf[from..].iter_mut().enumerate() {
                        let (ra, rb) = (find(level, a), find(level, b));
                        if ra != rb {
                            level[ra as usize] = rb;
                            if from + j == l - 1 {
                                parts -= 1; // the top level holds every distance
                            }
                        }
                    }
                }
                // After the last edge at this gap: a frontier corner if all are joined
                // at a shorter distance than before.
                if parts == 1 && cand.get(i + 1).is_none_or(|c| c.0 != gap) {
                    let x = tree.height(root);
                    if frontier.last().is_none_or(|&(_, x1)| x < x1) {
                        frontier.push((gap, x));
                    }
                }
            }
            progress((k + 1) as f64 / bands as f64);
        }
        self.geodesics = geodesics;
        self.stats = stats;
        self.frontier = frontier;
    }

    /// Per-strike edge lists and the distance order.
    fn index(&mut self) {
        let n = self.n;
        let mut off = vec![0u32; n + 1];
        for (&a, &b) in self.ea.iter().zip(&self.eb) {
            off[a as usize + 1] += 1;
            off[b as usize + 1] += 1;
        }
        for u in 0..n {
            off[u + 1] += off[u];
        }
        let mut fill = off.clone();
        let mut inc = vec![0u32; off[n] as usize];
        for (e, (&a, &b)) in self.ea.iter().zip(&self.eb).enumerate() {
            for u in [a, b] {
                inc[fill[u as usize] as usize] = e as u32;
                fill[u as usize] += 1;
            }
        }
        self.off = off;
        self.inc = inc;
        let mut by_km: Vec<u32> = (0..self.ea.len() as u32).collect();
        by_km.sort_by(|&p, &q| self.ed[p as usize].total_cmp(&self.ed[q as usize]).then(p.cmp(&q)));
        self.by_km = by_km;
    }
}
