//! Single-linkage merge tree over distance, updated by edge insertion.
//!
//! Leaves 0..n-1 are strikes (height 0); each internal node merges two children at its
//! height. A node's cluster is a component for x in [height, parent height).
//! Each node also carries its size, a count of marked strikes, and a minimum of a
//! per-strike value (see `lower_leaf`). A node's fields are kept together (one cache line):
//! insertion walks paths of nodes, and that is where the time goes.

pub const ROOT: i64 = -1;
pub const FREED: i64 = -2;

const NONE: u32 = u32::MAX;

#[derive(Clone, Copy)]
struct Node {
    height: f64,
    low: f64,
    parent: i32,
    kids: [u32; 2],
    size: u32,
    cnt: u32,
}

pub struct Tree {
    pub max_dist: f64,
    nodes: Vec<Node>,
    free: Vec<usize>,
    // Planned insertion (set by `prepare`, consumed by `apply`).
    a: usize,
    b: usize,
    d: f64,
    lca: i64,
    p: Vec<usize>,
    q: Vec<usize>,
    eq: Vec<usize>,
    pub chain: Vec<usize>,
}

impl Tree {
    /// n leaves, each with its count of marked strikes (0 if `cnt` is None).
    pub fn new(n: usize, max_dist: f64, cnt: Option<Vec<u32>>) -> Self {
        let leaf = Node {
            height: 0.0,
            low: f64::INFINITY,
            parent: ROOT as i32,
            kids: [NONE; 2],
            size: 1,
            cnt: 0,
        };
        let mut nodes = vec![leaf; n];
        if let Some(c) = cnt {
            for (nd, c) in nodes.iter_mut().zip(c) {
                nd.cnt = c;
            }
        }
        Tree {
            max_dist,
            nodes,
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

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn parent(&self, w: usize) -> i64 {
        self.nodes[w].parent as i64
    }

    pub fn height(&self, w: usize) -> f64 {
        self.nodes[w].height
    }

    pub fn kids(&self, w: usize) -> [usize; 2] {
        let [a, b] = self.nodes[w].kids;
        [a as usize, b as usize]
    }

    pub fn size(&self, w: usize) -> u32 {
        self.nodes[w].size
    }

    /// Marked strikes under w.
    pub fn cnt(&self, w: usize) -> u32 {
        self.nodes[w].cnt
    }

    /// Least per-strike value under w.
    pub fn low(&self, w: usize) -> f64 {
        self.nodes[w].low
    }

    pub fn interval(&self, v: usize) -> (f64, f64) {
        let p = self.nodes[v].parent;
        (
            self.nodes[v].height,
            if p >= 0 {
                self.nodes[p as usize].height
            } else {
                self.max_dist
            },
        )
    }

    fn new_node(&mut self) -> usize {
        if let Some(w) = self.free.pop() {
            return w;
        }
        self.nodes.push(Node {
            height: 0.0,
            low: f64::INFINITY,
            parent: ROOT as i32,
            kids: [NONE; 2],
            size: 0,
            cnt: 0,
        });
        self.nodes.len() - 1
    }

    /// Highest ancestor of v (or v) with height <= d.
    fn top_below(&self, mut v: usize, d: f64) -> usize {
        loop {
            let p = self.nodes[v].parent;
            if p >= 0 && self.nodes[p as usize].height <= d {
                v = p as usize;
            } else {
                return v;
            }
        }
    }

    /// Plan adding edge (u, v) at distance d. Returns false if nothing would change.
    /// After `true`, `touched()` lists every node whose cluster will change.
    pub fn prepare(&mut self, u: usize, v: usize, d: f64) -> bool {
        let a = self.top_below(u, d);
        let b = self.top_below(v, d);
        if a == b {
            return false;
        }
        self.a = a;
        self.b = b;
        self.d = d;
        self.p.clear();
        self.q.clear();
        let par = |t: &Self, w: i64| t.nodes[w as usize].parent as i64;
        let ht = |t: &Self, w: i64| t.nodes[w as usize].height;
        // Walk both root paths upward in height order until they meet (or both end).
        let (mut x, mut y) = (par(self, a as i64), par(self, b as i64));
        while x != y {
            if x >= 0 && y >= 0 && ht(self, x) == ht(self, y) {
                // Equal heights: one may be the other's ancestor via an equal-height chain.
                let mut met = false;
                for (lo, hi, side) in [(x, y, 0), (y, x, 1)] {
                    self.eq.clear();
                    let hl = ht(self, lo);
                    let mut w = lo;
                    while w >= 0 && w != hi && ht(self, w) == hl {
                        self.eq.push(w as usize);
                        w = par(self, w);
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
                    x = par(self, x);
                }
            } else if y < 0 || (x >= 0 && ht(self, x) < ht(self, y)) {
                self.p.push(x as usize);
                x = par(self, x);
            } else {
                self.q.push(y as usize);
                y = par(self, y);
            }
        }
        self.lca = x;
        true
    }

    pub fn touched(&self) -> impl Iterator<Item = usize> + '_ {
        [self.a, self.b]
            .into_iter()
            .chain(self.p.iter().copied())
            .chain(self.q.iter().copied())
            .chain((self.lca >= 0).then_some(self.lca as usize))
    }

    /// Carry out the planned insertion; `self.chain` then holds the changed nodes,
    /// bottom-up (its last node is the top of the changed path).
    pub fn apply(&mut self) {
        let (a, b, d, lca) = (self.a, self.b, self.d, self.lca);
        let top_parent = if lca >= 0 {
            self.nodes[lca as usize].parent
        } else {
            ROOT as i32
        };
        let p = std::mem::take(&mut self.p);
        let q = std::mem::take(&mut self.q);
        let mut chain = std::mem::take(&mut self.chain);
        chain.clear();
        let mut cur = self.new_node();
        self.nodes[cur].height = d;
        self.nodes[cur].kids = [a as u32, b as u32];
        self.nodes[a].parent = cur as i32;
        self.nodes[b].parent = cur as i32;
        chain.push(cur);
        let mut prev = [a, b];
        let (mut ip, mut iq) = (0, 0);
        while ip < p.len() || iq < q.len() {
            let take_p = iq >= q.len()
                || (ip < p.len() && self.nodes[p[ip]].height <= self.nodes[q[iq]].height);
            let (w, side) = if take_p {
                ip += 1;
                (p[ip - 1], 0)
            } else {
                iq += 1;
                (q[iq - 1], 1)
            };
            let [k0, k1] = self.nodes[w].kids;
            let sib = if k0 as usize == prev[side] { k1 } else { k0 };
            prev[side] = w;
            self.nodes[w].kids = [cur as u32, sib];
            self.nodes[cur].parent = w as i32;
            cur = w;
            chain.push(w);
        }
        if lca >= 0 {
            let l = lca as usize;
            self.nodes[cur].parent = top_parent;
            if top_parent >= 0 {
                let tp = top_parent as usize;
                if self.nodes[tp].kids[0] as usize == l {
                    self.nodes[tp].kids[0] = cur as u32;
                } else {
                    self.nodes[tp].kids[1] = cur as u32;
                }
            }
            self.nodes[l].kids = [NONE; 2];
            self.nodes[l].parent = FREED as i32;
            self.free.push(l);
        } else {
            self.nodes[cur].parent = ROOT as i32;
        }
        for &w in &chain {
            let [c0, c1] = self.nodes[w].kids;
            let (k0, k1) = (self.nodes[c0 as usize], self.nodes[c1 as usize]);
            let nd = &mut self.nodes[w];
            nd.size = k0.size + k1.size;
            nd.cnt = k0.cnt + k1.cnt;
            nd.low = k0.low.min(k1.low);
        }
        self.p = p;
        self.q = q;
        self.chain = chain;
    }

    /// Lower leaf u's value to v (if smaller); every node whose minimum changed is
    /// passed to `changed` (u first, then its ancestors).
    pub fn lower_leaf(&mut self, u: usize, v: f64, mut changed: impl FnMut(usize)) {
        let mut w = u as i64;
        while w >= 0 && self.nodes[w as usize].low > v {
            self.nodes[w as usize].low = v;
            changed(w as usize);
            w = self.nodes[w as usize].parent as i64;
        }
    }

    /// Raise leaf u's value to v (if larger), and its ancestors' minimum to match.
    pub fn raise_leaf(&mut self, u: usize, v: f64) {
        if self.nodes[u].low >= v {
            return;
        }
        self.nodes[u].low = v;
        let mut w = self.nodes[u].parent as i64;
        while w >= 0 {
            let [a, b] = self.nodes[w as usize].kids;
            let m = self.nodes[a as usize].low.min(self.nodes[b as usize].low);
            if m == self.nodes[w as usize].low {
                break;
            }
            self.nodes[w as usize].low = m;
            w = self.nodes[w as usize].parent as i64;
        }
    }

    /// Insert edge (u, v) at distance d; true if anything changed.
    pub fn insert(&mut self, u: usize, v: usize, d: f64) -> bool {
        if self.prepare(u, v, d) {
            self.apply();
            true
        } else {
            false
        }
    }
}
