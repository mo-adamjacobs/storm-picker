//! Single-linkage merge tree over distance, updated by edge insertion (carried over from
//! the previous sweep code).
//!
//! Leaves 0..n-1 are strikes (height 0); each internal node merges two children at its
//! height. A node's cluster is a component for x in [height, parent height).
//! Each node carries the XOR of its strikes' random tags and its size: together they
//! identify the strike set.

pub const ROOT: i64 = -1;
pub const FREED: i64 = -2;

pub type Key = (u64, u32);

pub struct Tree {
    pub max_dist: f64,
    pub parent: Vec<i64>,
    pub height: Vec<f64>,
    pub kids: Vec<[usize; 2]>,
    pub h: Vec<u64>,
    pub size: Vec<u32>,
    /// Optional per-node count of marked strikes.
    pub cnt: Option<Vec<u32>>,
    /// Optional per-node minimum of a per-strike value (see `lower_leaf`).
    pub low: Option<Vec<f64>>,
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
    pub fn new(tags: &[u64], max_dist: f64, cnt: Option<Vec<u32>>) -> Self {
        let n = tags.len();
        Tree {
            max_dist,
            parent: vec![ROOT; n],
            height: vec![0.0; n],
            kids: vec![[usize::MAX; 2]; n],
            h: tags.to_vec(),
            size: vec![1; n],
            cnt,
            low: None,
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
        self.parent.len()
    }

    pub fn key(&self, w: usize) -> Key {
        (self.h[w], self.size[w])
    }

    pub fn interval(&self, v: usize) -> (f64, f64) {
        let p = self.parent[v];
        (
            self.height[v],
            if p >= 0 {
                self.height[p as usize]
            } else {
                self.max_dist
            },
        )
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
        if let Some(l) = self.low.as_mut() {
            l.push(f64::INFINITY);
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
            self.parent[lca as usize]
        } else {
            ROOT
        };
        let p = std::mem::take(&mut self.p);
        let q = std::mem::take(&mut self.q);
        let mut chain = std::mem::take(&mut self.chain);
        chain.clear();
        let mut cur = self.new_node();
        self.height[cur] = d;
        self.kids[cur] = [a, b];
        self.parent[a] = cur as i64;
        self.parent[b] = cur as i64;
        chain.push(cur);
        let mut prev = [a, b];
        let (mut ip, mut iq) = (0, 0);
        while ip < p.len() || iq < q.len() {
            let take_p =
                iq >= q.len() || (ip < p.len() && self.height[p[ip]] <= self.height[q[iq]]);
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
        for &w in &chain {
            let [c0, c1] = self.kids[w];
            self.h[w] = self.h[c0] ^ self.h[c1];
            self.size[w] = self.size[c0] + self.size[c1];
            if let Some(c) = self.cnt.as_mut() {
                c[w] = c[c0] + c[c1];
            }
            if let Some(l) = self.low.as_mut() {
                l[w] = l[c0].min(l[c1]);
            }
        }
        self.p = p;
        self.q = q;
        self.chain = chain;
    }

    /// Keep a per-node minimum of a per-strike value, all starting at infinity.
    pub fn with_low(mut self) -> Self {
        self.low = Some(vec![f64::INFINITY; self.parent.len()]);
        self
    }

    /// Lower leaf u's value to v (if smaller); every node whose minimum changed is
    /// passed to `changed` (u first, then its ancestors).
    pub fn lower_leaf(&mut self, u: usize, v: f64, mut changed: impl FnMut(usize)) {
        let low = self.low.as_mut().expect("with_low");
        let mut w = u as i64;
        while w >= 0 && low[w as usize] > v {
            low[w as usize] = v;
            changed(w as usize);
            w = self.parent[w as usize];
        }
    }

    /// Raise leaf u's value to v (if larger), and its ancestors' minimum to match.
    pub fn raise_leaf(&mut self, u: usize, v: f64) {
        let low = self.low.as_mut().expect("with_low");
        if low[u] >= v {
            return;
        }
        low[u] = v;
        let mut w = self.parent[u];
        while w >= 0 {
            let [a, b] = self.kids[w as usize];
            let m = low[a].min(low[b]);
            if m == low[w as usize] {
                break;
            }
            low[w as usize] = m;
            w = self.parent[w as usize];
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
