//! Queries, all answered from the one graph of graph.rs (built once, exact at every
//! threshold): nothing here measures a distance.
//!
//! - **The box**: the dataset's *frontier* (the least thresholds at which all its strikes
//!   are in one cluster), with a margin, bounds the persistence map; the 1st percentile of
//!   the edges' positive distances and the 5th of their gaps set its lower (log-axis)
//!   limits. Found with
//!   the graph.
//! - **View**: the strikes in a time range and map viewport (which clusters are listed).
//! - **Clusters at (x, y)**: every cluster of the whole dataset containing a strike in
//!   view, found by following edges present at (x, y) out from the view's strikes.
//! - **A cluster's region**: from the edges between its strikes and those leaving it.
//! - **Similar region**: where the cluster holding most of C is within the change
//!   tolerance; a single-linkage sweep over a set grown from C (see `similar`). The similar
//!   clusters themselves are never listed.
//!
//! Units: distance km, gap seconds (regions are reported in minutes). A region is rows
//! [y_lo, y_hi, x_lo, x_hi] (min, min, km, km): x in [x_lo, x_hi), y in [y_lo, y_hi). An
//! infinite bound means "still there at the edge of the box".

use std::collections::HashSet;

use crate::graph::Graph;
use crate::tree::{FREED, ROOT, Tree};

pub struct Cluster {
    pub label: u32,
    pub size: u32,
    pub first: u32,
    pub last: u32,
}

pub struct Selection {
    pub x: f64,
    pub y_s: f64,
    /// Cluster label per strike (u32::MAX: in no cluster touching the view).
    pub label: Vec<u32>,
    /// Members of each label (sorted strike indices).
    pub members: Vec<Vec<u32>>,
    /// Clusters (2+ strikes), largest first.
    pub clusters: Vec<Cluster>,
    /// Strikes in view that are alone at this threshold.
    pub singletons: u32,
}

pub struct View {
    pub strikes: Vec<u32>,
}

pub struct Similar {
    /// The similar region (rows as for a cluster's region; disjoint).
    pub rows: Vec<f64>,
    /// Area of the similar region in the box, as a fraction of it (log axes).
    pub stability: f64,
    /// Passes the sweep took (each grows the swept set).
    pub passes: u32,
    /// If asked for: strikes in any similar cluster, with the best Jaccard similarity to C
    /// of a similar cluster holding each.
    pub union: Vec<u32>,
    pub best: Vec<f64>,
}

pub struct Engine {
    pub g: Graph,
    /// Frontier corners (gap s, km): from each gap on (until the next corner), all strikes
    /// are in one cluster from that distance on. Gaps ascending, distances descending.
    pub frontier: Vec<(f64, f64)>,
    /// The box's lower (log-axis) limits: the 1st percentile of the edges' positive
    /// distances (km) and the 5th of their gaps (s) (see `floor`).
    pub x_min: f64,
    pub y_min: f64,
    /// Box queries are computed in (km, s): the frontier's extent with a margin.
    pub bx: (f64, f64),
    mark: Vec<u32>,
    stamp: u32,
    /// Scratch: position of each strike in the set being worked on.
    local: Vec<u32>,
    pub view: Option<View>,
    pub last: Option<Selection>,
}

/// Value at gap g of a staircase [(gap, km)] (gaps up, km down): the least km at a gap
/// ≤ g, or infinity before the first step.
fn stair_at(stairs: &[(f64, f64)], g: f64) -> f64 {
    let k = stairs.partition_point(|s| s.0 <= g);
    if k == 0 {
        f64::INFINITY
    } else {
        stairs[k - 1].1
    }
}

/// Keep only the steps of a staircase that matter: sorted by gap, each strictly nearer
/// than all before it.
fn tidy(stairs: &mut Vec<(f64, f64)>) {
    stairs.sort_by(|p, q| p.0.total_cmp(&q.0).then(p.1.total_cmp(&q.1)));
    let mut best = f64::INFINITY;
    stairs.retain(|&(_, d)| {
        let keep = d < best;
        best = best.min(d);
        keep
    });
}

fn find(uf: &mut [u32], mut x: u32) -> u32 {
    while uf[x as usize] != x {
        uf[x as usize] = uf[uf[x as usize] as usize];
        x = uf[x as usize];
    }
    x
}

/// The box's lower (log-axis) limit on one axis: quantile `q` of the edges' positive
/// values (infinite if there are none).
pub fn floor(values: impl Iterator<Item = f64>, q: f64) -> f64 {
    let mut v: Vec<f64> = values.filter(|&x| x > 0.0).collect();
    if v.is_empty() {
        return f64::INFINITY;
    }
    let k = (q * (v.len() - 1) as f64) as usize;
    *v.select_nth_unstable_by(k, |a, b| a.total_cmp(b)).1
}

/// Quantiles of edge distances and gaps at which the box starts (the rare shortest ones
/// would otherwise stretch the log axes over decades of nothing). Gaps are higher: a few
/// percent of edges join strikes microseconds apart (repeat detections of one flash).
pub const FLOOR_KM: f64 = 0.01;
pub const FLOOR_GAP: f64 = 0.05;

/// Margin around the frontier box (factor on each axis maximum).
const MARGIN: f64 = 1.5;

impl Engine {
    pub fn new(g: Graph) -> Self {
        let n = g.n;
        let all: Vec<u32> = (0..n as u32).collect();
        let frontier = g.frontier.clone();
        let mut x_min = floor(g.ed.iter().copied(), FLOOR_KM);
        let mut y_min = floor((0..g.edges() as u32).map(|e| g.gap(e)), FLOOR_GAP);
        if !x_min.is_finite() {
            x_min = 0.01;
        }
        if !y_min.is_finite() {
            y_min = 1.0;
        }
        // Box: the frontier's extent with a margin (at least a decade above the minima).
        let fx = frontier.first().map_or(0.0, |f| f.1);
        let fy = frontier.last().map_or(0.0, |f| f.0);
        let bx = (
            (fx * MARGIN).max(x_min * 10.0),
            (fy * MARGIN).max(y_min * 10.0),
        );
        Engine {
            g,
            frontier,
            x_min,
            y_min,
            bx,
            mark: vec![0; n],
            stamp: 0,
            local: vec![0; n],
            view: Some(View { strikes: all }),
            last: None,
        }
    }

    fn next_stamp(&mut self) -> u32 {
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == 0 {
            self.mark.iter_mut().for_each(|m| *m = 0);
            self.stamp = 1;
        }
        self.stamp
    }

    /// Set the view: strikes with t in [t0, t1] (s) inside the lat/lon box (lon0 > lon1
    /// wraps across the antimeridian). Clicks list the clusters touching it.
    pub fn set_view(
        &mut self,
        t0: f64,
        t1: f64,
        lat0: f64,
        lat1: f64,
        lon0: f64,
        lon1: f64,
    ) -> &View {
        let g = &self.g;
        let inlon = |x: f64| {
            if lon0 <= lon1 {
                x >= lon0 && x <= lon1
            } else {
                x >= lon0 || x <= lon1
            }
        };
        let a = g.t.partition_point(|&v| v < t0);
        let b = g.t.partition_point(|&v| v <= t1);
        let strikes: Vec<u32> = (a..b)
            .filter(|&i| g.lat[i] >= lat0 && g.lat[i] <= lat1 && inlon(g.lon[i]))
            .map(|i| i as u32)
            .collect();
        self.view = Some(View { strikes });
        self.last = None;
        self.view.as_ref().unwrap()
    }

    /// Clusters at (x km, y seconds) that contain a strike in view.
    pub fn select(&mut self, x: f64, y: f64) -> &Selection {
        let (x, y) = (x.min(self.bx.0), y.min(self.bx.1));
        let seeds = self.view.as_ref().map_or(Vec::new(), |v| v.strikes.clone());
        let g = &self.g;
        let mut label = vec![u32::MAX; g.n];
        let mut members: Vec<Vec<u32>> = Vec::new();
        for &s in &seeds {
            if label[s as usize] != u32::MAX {
                continue;
            }
            let c = members.len() as u32;
            label[s as usize] = c;
            let mut m = vec![s];
            let mut k = 0;
            while k < m.len() {
                let u = m[k] as usize;
                k += 1;
                for &e in g.edges_of(u) {
                    if g.ed[e as usize] <= x && g.gap(e) <= y {
                        let v = g.other(e, u);
                        if label[v] == u32::MAX {
                            label[v] = c;
                            m.push(v as u32);
                        }
                    }
                }
            }
            m.sort_unstable();
            members.push(m);
        }
        let mut clusters = Vec::new();
        let mut singletons = 0;
        for (c, m) in members.iter().enumerate() {
            if m.len() < 2 {
                singletons += 1;
            } else {
                clusters.push(Cluster {
                    label: c as u32,
                    size: m.len() as u32,
                    first: m[0],
                    last: m[m.len() - 1],
                });
            }
        }
        clusters.sort_by(|p, q| q.size.cmp(&p.size).then(p.first.cmp(&q.first)));
        self.last = Some(Selection {
            x,
            y_s: y,
            label,
            members,
            clusters,
            singletons,
        });
        self.last.as_ref().unwrap()
    }

    /// Exact region of the last selection's cluster with this label.
    pub fn region_of(&mut self, label: u32) -> Vec<f64> {
        let members = self.last.as_ref().expect("no selection").members[label as usize].clone();
        self.region(&members)
    }

    /// Exact region of the cluster with these members (sorted strike indices).
    ///
    /// For gap ≤ y it is a component for x in [a(y), b(y)): b(y) is the shortest edge
    /// within gap y from it to another strike, a(y) the distance at which its own strikes
    /// connect (the root of their single-linkage tree), from the edges between them. Only
    /// those below b(y) matter (a connection at or beyond it is never seen while it is a
    /// cluster). Both are exact because the graph is: where the cluster is a component, the
    /// paths standing in for its strikes' left-out pairs stay inside it, and wherever it
    /// joins another strike some edge leaving it is present.
    pub fn region(&mut self, members: &[u32]) -> Vec<f64> {
        let (bx, by) = self.bx;
        let st = self.next_stamp();
        for (k, &u) in members.iter().enumerate() {
            self.mark[u as usize] = st;
            self.local[u as usize] = k as u32;
        }
        let g = &self.g;
        let mut stairs: Vec<(f64, f64)> = Vec::new(); // (gap, km): b(y) from each gap on
        let mut inner: Vec<(f64, f64, u32, u32)> = Vec::new(); // (gap, dist, u, v)
        for &u in members {
            for &e in g.edges_of(u as usize) {
                let (d, gap) = (g.ed[e as usize], g.gap(e));
                if gap > by || d > bx {
                    continue;
                }
                let v = g.other(e, u as usize);
                if self.mark[v] != st {
                    stairs.push((gap, d));
                } else if (u as usize) < v {
                    inner.push((gap, d, self.local[u as usize], self.local[v]));
                }
            }
        }
        tidy(&mut stairs);
        inner.retain(|&(gap, d, _, _)| d < stair_at(&stairs, gap));
        let outer = stairs;
        inner.sort_by(|p, q| p.0.total_cmp(&q.0));
        let m = members.len();
        let mut tree = Tree::new(&vec![0u64; m], f64::INFINITY, None);
        let mut uf: Vec<u32> = (0..m as u32).collect();
        let (mut parts, mut root, mut b) = (m, 0usize, f64::INFINITY);
        let mut rows = Vec::new();
        let mut open: Option<(f64, f64, f64)> = None; // (y_lo, a, b)
        let (mut i, mut o) = (0, 0);
        while i < inner.len() || o < outer.len() {
            let y = match (inner.get(i), outer.get(o)) {
                (Some(p), Some(q)) => p.0.min(q.0),
                (Some(p), None) => p.0,
                (None, Some(q)) => q.0,
                (None, None) => unreachable!(),
            };
            while i < inner.len() && inner[i].0 == y {
                let (_, d, u, v) = inner[i];
                i += 1;
                let (ru, rv) = (find(&mut uf, u), find(&mut uf, v));
                if ru != rv {
                    uf[ru as usize] = rv;
                    parts -= 1;
                }
                if tree.insert(u as usize, v as usize, d) {
                    let top = *tree.chain.last().unwrap();
                    if tree.parent[top] == ROOT {
                        root = top;
                    }
                }
            }
            while o < outer.len() && outer[o].0 == y {
                b = b.min(outer[o].1);
                o += 1;
            }
            let a = if parts == 1 {
                tree.height[root]
            } else {
                f64::INFINITY
            };
            let now = (a < b).then_some((a, b));
            if open.map(|(_, a0, b0)| (a0, b0)) != now {
                if let Some((y0, a0, b0)) = open.take() {
                    rows.extend_from_slice(&[y0 / 60.0, y / 60.0, a0, b0]);
                }
                open = now.map(|(a, b)| (y, a, b));
            }
        }
        if let Some((y0, a0, b0)) = open {
            rows.extend_from_slice(&[y0 / 60.0, f64::INFINITY, a0, b0]);
        }
        rows
    }

    /// The similar region of cluster C (the last selection's cluster with this label):
    /// the thresholds where some cluster D has change |C xor D| / |C and D| ≤ tol.
    ///
    /// With tol < 1 such a D holds more than half of C, so at each threshold there is at
    /// most one candidate: the cluster M holding a majority of C. As thresholds grow, M
    /// only grows, and D = M qualifies when its strikes outside C, g, and of C, h, satisfy
    /// g ≤ (1 + tol) h - |C|. So the sweep follows M's chain of merges and never lists D.
    ///
    /// The sweep runs over a set W grown from C, with the edges between W's strikes and,
    /// for each, the edges leaving W. A D has at most tol·|C| strikes outside C, and at D's
    /// threshold the W-cluster of any of its C strikes lies inside D. So whenever a node
    /// holding C's strikes and at most tol·|C| others has an edge leaving W while it
    /// exists, the strike at its other end (and its cluster there) joins W and the sweep
    /// is repeated; when none does, the result is exact.
    /// `progress(pass + fraction)` reports each pass's progress.
    pub fn similar(
        &mut self,
        label: u32,
        tol: f64,
        want_union: bool,
        mut progress: impl FnMut(f64),
    ) -> Similar {
        let c_members = self.last.as_ref().expect("no selection").members[label as usize].clone();
        let c_size = c_members.len() as u32;
        let cap = (tol * c_size as f64 + 1e-9).floor() as u32; // strikes outside C
        let (bx, by) = self.bx;
        let mut w = c_members.clone();
        let mut pass = 0u32;
        let out = loop {
            let st = self.next_stamp();
            for (k, &u) in w.iter().enumerate() {
                self.mark[u as usize] = st;
                self.local[u as usize] = k as u32;
            }
            let g = &self.g;
            let mut inner: Vec<(f64, f64, u32, u32)> = Vec::new();
            let mut events: Vec<Vec<(f64, f64, u32)>> = Vec::with_capacity(w.len());
            for (k, &u) in w.iter().enumerate() {
                let mut ev = Vec::new();
                for &e in g.edges_of(u as usize) {
                    let (d, gap) = (g.ed[e as usize], g.gap(e));
                    if gap > by || d > bx {
                        continue;
                    }
                    let v = g.other(e, u as usize);
                    if self.mark[v] != st {
                        ev.push((gap, d, v as u32));
                    } else if (u as usize) < v {
                        inner.push((gap, d, k as u32, self.local[v]));
                    }
                }
                ev.sort_by(|p, q| p.0.total_cmp(&q.0));
                events.push(ev);
            }
            let cst = self.next_stamp();
            for &u in &c_members {
                self.mark[u as usize] = cst;
            }
            let marked: Vec<u32> = w
                .iter()
                .map(|&u| (self.mark[u as usize] == cst) as u32)
                .collect();
            let p = pass as f64;
            match majority_sweep(
                &marked,
                c_size,
                cap,
                tol,
                want_union,
                &mut inner,
                &events,
                &|_| bx,
                |f| progress(p + f),
            ) {
                Ok(out) => break out,
                Err(add) => {
                    // Each strike that must join comes with its cluster at the threshold
                    // where it is needed (up to the cap): adding more than needed never
                    // changes the result, and saves passes.
                    let st = self.next_stamp();
                    for &u in &w {
                        self.mark[u as usize] = st;
                    }
                    let g = &self.g;
                    let mut grow: Vec<u32> = Vec::new();
                    for (v, x, y) in add {
                        if self.mark[v as usize] == st {
                            continue;
                        }
                        let start = grow.len();
                        self.mark[v as usize] = st;
                        grow.push(v);
                        let mut k = start;
                        while k < grow.len() && grow.len() - start <= cap as usize {
                            let u = grow[k] as usize;
                            k += 1;
                            for &e in g.edges_of(u) {
                                if g.ed[e as usize] <= x.min(bx) && g.gap(e) <= y {
                                    let q = g.other(e, u);
                                    if self.mark[q] != st {
                                        self.mark[q] = st;
                                        grow.push(q as u32);
                                    }
                                }
                            }
                        }
                    }
                    w.extend(&grow);
                    w.sort_unstable();
                    pass += 1;
                }
            }
        };
        // Stability: the region's share of the box on log axes.
        let (x0, y0) = (self.x_min, self.y_min / 60.0);
        let (x1, y1) = (bx, by / 60.0);
        let lx = |v: f64| (v.clamp(x0, x1) / x0).ln();
        let ly = |v: f64| (v.clamp(y0, y1) / y0).ln();
        let area: f64 = out
            .rows
            .chunks(4)
            .map(|r| (ly(r[1]) - ly(r[0])).max(0.0) * (lx(r[3]) - lx(r[2])).max(0.0))
            .sum();
        let total = lx(x1) * ly(y1);
        let union: Vec<u32> = out.union.iter().map(|&k| w[k as usize]).collect();
        Similar {
            rows: out.rows,
            stability: if total > 0.0 { area / total } else { 0.0 },
            passes: pass + 1,
            union,
            best: out.best,
        }
    }
}

struct SweepOut {
    rows: Vec<f64>,
    union: Vec<u32>,
    best: Vec<f64>,
}

/// Single-linkage sweep over the gap on a set W (inner edges (gap, dist, u, v); for each
/// strike, its nearest-outside staircase (gap, dist, strike) in increasing gap), following
/// the chain of nodes holding a majority of the marked set C and recording where they are
/// within change `tol`. Fails with the outside strikes to add (each with the threshold
/// (km, s) where it is needed) if a node holding C's strikes and at most `cap` others
/// reaches outside W while it exists.
#[allow(clippy::too_many_arguments)]
fn majority_sweep(
    marked: &[u32],
    c_size: u32,
    cap: u32,
    tol: f64,
    want_union: bool,
    edges: &mut [(f64, f64, u32, u32)],
    events: &[Vec<(f64, f64, u32)>],
    reach: &dyn Fn(f64) -> f64,
    mut progress: impl FnMut(f64),
) -> Result<SweepOut, Vec<(u32, f64, f64)>> {
    let n = marked.len();
    edges.sort_by(|p, q| p.0.total_cmp(&q.0));
    let mut outer: Vec<(f64, f64, u32, u32)> = Vec::new(); // (gap, dist, local strike, outside strike)
    for (u, ev) in events.iter().enumerate() {
        outer.extend(ev.iter().map(|&(g, d, x)| (g, d, u as u32, x)));
    }
    outer.sort_by(|p, q| p.0.total_cmp(&q.0));
    let mut t = Tree::new(&vec![0u64; n], f64::INFINITY, Some(marked.to_vec())).with_low();
    let half = c_size as f64 / 2.0;
    let mut add: Vec<(u32, f64, f64)> = Vec::new();
    // Outside strikes already in `add` (each is listed once, where first found).
    let mut queued: HashSet<u32> = HashSet::new();
    let mut rows: Vec<f64> = Vec::new();
    let mut state: Vec<(f64, f64)> = Vec::new(); // similar x-intervals at the current gap
    let mut since = 0.0; // gap from which `state` holds
    let mut in_union = vec![false; n];
    let mut best = vec![0.0f64; n];
    let mut cover: Vec<f64> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut raise: Vec<(usize, f64)> = Vec::new(); // leaves whose minimum to raise
    let mut stamp: Vec<usize> = Vec::new();
    let mut nodes: Vec<usize> = Vec::new();
    let (mut level, mut s, mut o) = (0usize, 0usize, 0usize);
    let total = (edges.len() + outer.len()).max(1);
    let step = (total / 100).max(1);
    let mut next_report = 0;
    fn note(stamp: &mut Vec<usize>, nodes: &mut Vec<usize>, len: usize, tag: usize, w: usize) {
        if stamp.len() < len.max(w + 1) {
            stamp.resize(len.max(w + 1), 0);
        }
        if stamp[w] != tag {
            stamp[w] = tag;
            nodes.push(w);
        }
    }
    while s < edges.len() || o < outer.len() {
        let y = match (edges.get(s), outer.get(o)) {
            (Some(p), Some(q)) => p.0.min(q.0),
            (Some(p), None) => p.0,
            (None, Some(q)) => q.0,
            (None, None) => unreachable!(),
        };
        if s + o >= next_report {
            progress((s + o) as f64 / total as f64);
            next_report = s + o + step;
        }
        let tag = level + 1;
        level += 1;
        nodes.clear();
        let top_x = reach(y); // thresholds at this gap are only known below this
        while s < edges.len() && edges[s].0 == y {
            let (_, d, u, v) = edges[s];
            s += 1;
            if !t.prepare(u as usize, v as usize, d) {
                continue;
            }
            let touched: Vec<usize> = t.touched().collect();
            for w in touched {
                note(&mut stamp, &mut nodes, t.len(), tag, w);
            }
            t.apply();
            if cover.len() < t.len() {
                cover.resize(t.len(), -1.0);
            }
            for i in 0..t.chain.len() {
                let w = t.chain[i];
                cover[w] = -1.0; // its subtree just changed
                note(&mut stamp, &mut nodes, t.len(), tag, w);
            }
        }
        // Nearest outside strikes now within the gap: lower the nodes' minimum distance.
        while o < outer.len() && outer[o].0 == y {
            let (_, d, u, x) = outer[o];
            o += 1;
            if queued.contains(&x) {
                continue; // already joining W: this edge can show nothing new
            }
            let mut changed = Vec::new();
            t.lower_leaf(u as usize, d, |w| changed.push(w));
            for w in changed {
                note(&mut stamp, &mut nodes, t.len(), tag, w);
            }
        }
        // Does any node holding C's strikes, small enough to be (part of) a D, have an
        // outside strike within reach while it exists? Then W must grow.
        let mut majority: Option<usize> = None; // a changed node holding most of C
        for &w in &nodes {
            if t.parent[w] == FREED {
                continue;
            }
            let c = t.cnt.as_ref().unwrap()[w];
            if c as f64 > half {
                majority = Some(w);
            }
            if c == 0 || t.size[w] - c > cap {
                continue;
            }
            let (lo, hi) = t.interval(w);
            let hi = hi.min(top_x);
            let low = t.low.as_ref().unwrap();
            if lo < hi && low[w] < hi {
                stack.clear();
                stack.push(w);
                while let Some(v) = stack.pop() {
                    if low[v] >= hi {
                        continue;
                    }
                    if v < n {
                        // Queue what this strike reaches; then it only needs looking at
                        // again for an edge to a strike not yet queued.
                        let mut left = f64::INFINITY;
                        for &(g, d, x) in &events[v] {
                            if g > y {
                                break;
                            }
                            if d < hi && queued.insert(x) {
                                add.push((x, hi, y)); // its cluster over all of w's range
                            }
                            if !queued.contains(&x) {
                                left = left.min(d);
                            }
                        }
                        raise.push((v, left));
                    } else {
                        stack.extend_from_slice(&t.kids[v]);
                    }
                }
                for &(v, left) in &raise {
                    t.raise_leaf(v, left);
                }
                raise.clear();
            }
        }
        let Some(start) = majority else {
            continue; // M's chain unchanged (nodes holding most of C only change when touched)
        };
        if !add.is_empty() {
            continue; // growing W: finish the pass to collect every strike to add
        }
        // M's chain: nodes holding most of C overlap, so lie on one path. Go down from a
        // changed one to the lowest, then up while the outsiders allow a D.
        let cnt = t.cnt.as_ref().unwrap();
        let mut v = start;
        while v >= n {
            let [a, b] = t.kids[v];
            v = if cnt[a] as f64 > half {
                a
            } else if cnt[b] as f64 > half {
                b
            } else {
                break;
            };
        }
        let mut now: Vec<(f64, f64)> = Vec::new();
        {
            let mut w = v;
            loop {
                let (c, size) = (cnt[w], t.size[w]);
                let out_c = size - c;
                if out_c > cap {
                    break; // only more outsiders further up
                }
                let (lo, hi) = t.interval(w);
                let hi = hi.min(top_x);
                let qualifies = lo < hi
                    && size >= 2
                    && out_c as f64 <= (1.0 + tol) * c as f64 - c_size as f64 + 1e-9;
                if qualifies {
                    match now.last_mut() {
                        Some(last) if last.1 == lo => last.1 = hi,
                        _ => now.push((lo, hi)),
                    }
                }
                if qualifies && want_union {
                    let jac = c as f64 / (c_size + out_c) as f64;
                    stack.clear();
                    stack.push(w);
                    while let Some(v) = stack.pop() {
                        if v < n {
                            in_union[v] = true;
                            best[v] = best[v].max(jac);
                        } else if cover[v] < jac {
                            cover[v] = jac;
                            stack.extend_from_slice(&t.kids[v]);
                        }
                    }
                }
                if t.parent[w] < 0 {
                    break;
                }
                w = t.parent[w] as usize;
            }
        }
        if now != state {
            for &(a, b) in &state {
                rows.extend_from_slice(&[since / 60.0, y / 60.0, a, b]);
            }
            state = now;
            since = y;
        }
    }
    progress(1.0);
    if !add.is_empty() {
        return Err(add);
    }
    for &(a, b) in &state {
        rows.extend_from_slice(&[since / 60.0, f64::INFINITY, a, b]);
    }
    let union: Vec<u32> = (0..n as u32).filter(|&v| in_union[v as usize]).collect();
    let best = union.iter().map(|&v| best[v as usize]).collect();
    Ok(SweepOut { rows, union, best })
}
