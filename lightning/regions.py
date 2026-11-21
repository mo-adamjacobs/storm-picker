"""Exact 2D single-linkage hierarchy: every strike set that is a cluster somewhere.

A cluster is a connected component at (distance x, gap y). Sweeping the gap upward,
the single-linkage dendrogram over distance is updated one edge at a time; each
cluster's (x, y) region is recorded as rows [y_lo, y_hi) x [x_lo, x_hi).
Clusters are identified by (XOR of per-strike random tags, size).

The sweep runs in Rust (the `lightning_sweep` extension, sweep/ in the repo) when it is
installed, natively or in Pyodide. The pure-Python code here is the reference
implementation: select it with backend="python"; tests check the two agree.
"""
from array import array

import numpy as np

try:
    import lightning_sweep as _native
except ImportError:  # not built/installed: use the Python reference
    _native = None


def _backend(backend):
    b = backend or ("native" if _native is not None else "python")
    if b == "native" and _native is None:
        raise RuntimeError("lightning_sweep (Rust extension) is not installed")
    return b


def strike_tags(n, seed=0):
    return np.random.default_rng(seed).integers(1, 2**63, n, dtype=np.int64)


class Dendrogram:
    """Single-linkage merge tree over distance, updated by edge insertion.

    Leaves 0..n-1 are strikes (height 0). Each internal node merges two children at
    its height. A node's cluster is a component for x in [height, parent height).
    """

    def __init__(self, n, tags, max_dist):
        self.n, self.max_dist = n, max_dist
        self.parent = [-1] * n
        self.height = [0.0] * n
        self.kids = [None] * n
        self.h = [int(v) for v in tags]
        self.size = [1] * n
        self.free = []
        self.cnt = None  # optional per-node count of marked strikes (see overlaps)

    def key(self, w):
        return (self.h[w], self.size[w])

    def interval(self, v):
        p = self.parent[v]
        return self.height[v], (self.height[p] if p >= 0 else self.max_dist)

    def _new(self):
        if self.free:
            return self.free.pop()
        self.parent.append(-1); self.height.append(0.0); self.kids.append(None)
        self.h.append(0); self.size.append(0)
        if self.cnt is not None:
            self.cnt.append(0)
        return len(self.parent) - 1

    def _top_below(self, v, d):
        """Highest ancestor of leaf v (or v) with height <= d."""
        p = self.parent[v]
        while p >= 0 and self.height[p] <= d:
            v, p = p, self.parent[p]
        return v

    def insert(self, u, v, d, touched):
        """Add edge (u, v) at distance d. Calls touched(w) on every node whose cluster
        will change, before mutating it; returns the nodes holding changed clusters after."""
        a, b = self._top_below(u, d), self._top_below(v, d)
        if a == b:
            return []  # u and v already joined at distance <= d: nothing changes
        # Walk both root paths upward in height order until they meet (or both end).
        P, Q, x, y = [], [], self.parent[a], self.parent[b]
        while x != y:
            if x >= 0 and y >= 0 and self.height[x] == self.height[y]:
                # Equal heights: one may be the other's ancestor via an equal-height chain.
                for lo, hi, path in ((x, y, P), (y, x, Q)):
                    chain, w = [], lo
                    while w >= 0 and w != hi and self.height[w] == self.height[lo]:
                        chain.append(w); w = self.parent[w]
                    if w == hi:
                        path.extend(chain)
                        x = y = hi
                        break
                else:
                    P.append(x); x = self.parent[x]
            elif y < 0 or (x >= 0 and self.height[x] < self.height[y]):
                P.append(x); x = self.parent[x]
            else:
                Q.append(y); y = self.parent[y]
        lca = x  # -1 when u and v were in different trees
        for w in [a, b] + P + Q + ([lca] if lca >= 0 else []):
            touched(w)
        top_parent = self.parent[lca] if lca >= 0 else -1
        # New merge of a and b at d, then zip the two paths: each path node w keeps
        # its height and its off-path child, and takes the growing merged cluster.
        cur = self._new()
        self.height[cur], self.kids[cur] = d, [a, b]
        self.parent[a] = self.parent[b] = cur
        side = {w: 0 for w in P}
        side.update({w: 1 for w in Q})
        prev = {0: a, 1: b}  # last on-path node of each side (to find off-path child)
        chain = [cur]
        for w in sorted(P + Q, key=lambda w: self.height[w]):
            k0, k1 = self.kids[w]
            sib = k1 if k0 == prev[side[w]] else k0
            prev[side[w]] = w
            self.kids[w] = [cur, sib]
            self.parent[cur] = w
            cur = w
            chain.append(w)
        if lca >= 0:  # the old merge of the two sides is now redundant
            self.parent[cur] = top_parent
            if top_parent >= 0:
                ks = self.kids[top_parent]
                ks[ks.index(lca)] = cur
            self.kids[lca] = None
            self.parent[lca] = -2  # freed
            self.free.append(lca)
        else:
            self.parent[cur] = -1
        for w in chain:  # recompute set hash and size bottom-up along the new path
            c0, c1 = self.kids[w]
            self.h[w] = self.h[c0] ^ self.h[c1]
            self.size[w] = self.size[c0] + self.size[c1]
            if self.cnt is not None:
                self.cnt[w] = self.cnt[c0] + self.cnt[c1]
        return chain


class Regions:
    """Every cluster's region, as compact arrays.

    Cluster c has set hash `hash[c]` and `size[c]` strikes; its rows are
    `rows[off[c]:off[c + 1]]`, each (y_lo, y_hi, x_lo, x_hi): the cluster is a component
    for x in [x_lo, x_hi), y in [y_lo, y_hi). y_hi may be inf (open to the top).
    """

    def __init__(self, hash_, size, rows, off):
        self.hash, self.size, self.rows, self.off = hash_, size, rows, off

    def __len__(self):
        return len(self.size)

    def as_dict(self):
        """{(hash, size): (size, [rows])} (small inputs only: tests)."""
        return {(int(h), int(z)): (int(z), [tuple(r) for r in self.rows[self.off[c]:self.off[c + 1]]])
                for c, (h, z) in enumerate(zip(self.hash, self.size))}


def _regions_python(n, i, j, dist, gap, max_dist, min_size=2, zob=None, progress=None):
    """Every cluster's exact region (a Regions). Sweeps every distinct gap.

    Rows are appended to flat numeric arrays as they close, so memory stays close to
    the size of the result itself (millions of clusters fit in a browser).
    """
    if zob is None:
        zob = strike_tags(n)
    T = Dendrogram(n, zob, max_dist)
    i, j, dist = i.tolist(), j.tolist(), dist.tolist()
    order = np.argsort(gap, kind="stable")
    levels, starts = np.unique(gap[order], return_index=True)
    bounds = list(starts) + [len(order)]
    order = order.tolist()
    open_rows = {}
    ids = {}                                   # (hash, size) -> cluster index
    c_hash, c_size = array("q"), array("q")
    r_id, r_val = array("q"), array("d")       # one cluster id and 4 values per row

    def close(key, ylo, yhi, xlo, xhi):
        c = ids.get(key)
        if c is None:
            c = ids[key] = len(c_size)
            c_hash.append(key[0]); c_size.append(key[1])
        r_id.append(c)
        r_val.extend((ylo, yhi, xlo, xhi))

    def state(w):  # (key, interval) of a node, or None if it is not a cluster region
        if T.parent[w] == -2 or T.size[w] < min_size:
            return None
        lo, hi = T.interval(w)
        return ((T.h[w], T.size[w]), (lo, hi)) if lo < hi else None

    for k, y in enumerate(levels):
        if progress and k % 500 == 0:
            progress(k / len(levels))
        nodes = set()
        before = {}

        def touched(w):
            if w not in nodes:  # first sight this level: remember the old cluster
                nodes.add(w)
                s = state(w)
                if s:
                    before[s[0]] = s[1]
        for e in order[bounds[k]:bounds[k + 1]]:
            nodes.update(T.insert(i[e], j[e], dist[e], touched))
        after = dict(filter(None, (state(w) for w in nodes)))
        for key in before:  # close rows whose cluster changed or vanished
            if key in open_rows and after.get(key) != open_rows[key][1:]:
                ylo, xlo, xhi = open_rows.pop(key)
                close(key, ylo, float(y), xlo, xhi)
        for key, (xlo, xhi) in after.items():
            if key not in open_rows:
                open_rows[key] = (float(y), xlo, xhi)
    for key, (ylo, xlo, xhi) in open_rows.items():
        close(key, ylo, np.inf, xlo, xhi)
    del ids, open_rows, T
    # Group rows by cluster.
    rid = np.frombuffer(r_id, np.int64)
    rows = np.frombuffer(r_val, np.float64).reshape(-1, 4)
    o = np.argsort(rid, kind="stable")
    rows = rows[o]
    off = np.searchsorted(rid[o], np.arange(len(c_size) + 1))
    return Regions(np.frombuffer(c_hash, np.int64).copy(), np.frombuffer(c_size, np.int64).copy(), rows, off)


def _overlaps_python(n, i, j, dist, gap, max_dist, marked, zob, similar=None):
    """{cluster key: number of marked strikes in it}, for every cluster that ever exists.

    Replays the same sweep with one extra per-node count (marked strikes), summed at
    merges like size. Exact; used to compare one chosen cluster against all others.

    If `similar(key, overlap)` is given, also returns a boolean mask of the union of all
    strikes in clusters it accepts, collected as each such cluster forms.
    """
    T = Dendrogram(n, zob, max_dist)
    T.cnt = [1 if m else 0 for m in marked]
    order = np.argsort(gap, kind="stable").tolist()
    i, j, dist = i.tolist(), j.tolist(), dist.tolist()
    out = {}
    union = np.zeros(n, bool)
    done = {}  # node -> True if every leaf under it is already in `union` (reset on change)

    def collect(w):
        # Every node visited here ends up with all its leaves in `union`: mark them all.
        stack = [w]
        while stack:
            v = stack.pop()
            if v < n:
                union[v] = True
            elif not done.get(v):
                done[v] = True
                stack.extend(T.kids[v])

    for e in order:
        chain = T.insert(i[e], j[e], dist[e], lambda w: None)
        for w in chain:
            done.pop(w, None)  # its subtree just changed
        for w in chain:
            if T.cnt[w]:
                k = T.key(w)
                out[k] = T.cnt[w]
                if similar is not None and similar(k, T.cnt[w]):
                    collect(w)
    return (out, union) if similar is not None else out


def cluster_regions(n, i, j, dist, gap, max_dist, min_size=2, zob=None, progress=None, backend=None):
    """Every cluster's exact region, as a Regions. Sweeps every distinct gap."""
    if zob is None:
        zob = strike_tags(n)
    if _backend(backend) == "python":
        return _regions_python(n, i, j, dist, gap, max_dist, min_size, zob, progress)
    h, z, rows, off = _native.cluster_regions(
        np.ascontiguousarray(i, np.int32), np.ascontiguousarray(j, np.int32),
        np.ascontiguousarray(dist, np.float64), np.ascontiguousarray(gap, np.float64),
        float(max_dist), int(min_size), np.ascontiguousarray(zob, np.int64), progress)
    return Regions(np.frombuffer(h, np.int64), np.frombuffer(z, np.int64),
                   np.frombuffer(rows, np.float64).reshape(-1, 4), np.frombuffer(off, np.int64))


def overlaps(n, i, j, dist, gap, max_dist, marked, zob, c_size, tol, sorted_hash, sorted_size,
             backend=None):
    """Clusters similar to one cluster C (its strikes `marked`, |C| = c_size).

    Returns ({(hash, size): |C and D|}, union mask) for the clusters D whose change
    |C xor D| / |C and D| is at most `tol` and which are real regions (present in the
    (hash, size) table, sorted by hash then size); the mask is the union of their strikes.
    """
    sorted_hash = np.ascontiguousarray(sorted_hash, np.int64)
    sorted_size = np.ascontiguousarray(sorted_size, np.int64)
    if _backend(backend) == "python":
        def exists(k):
            lo = np.searchsorted(sorted_hash, k[0], "left")
            hi = np.searchsorted(sorted_hash, k[0], "right")
            return bool((sorted_size[lo:hi] == k[1]).any())
        accept = lambda k, o: (c_size + k[1] - 2 * o) / o <= tol + 1e-12 and exists(k)
        ov, union = _overlaps_python(n, i, j, dist, gap, max_dist, marked, zob, similar=accept)
        return {k: o for k, o in ov.items() if accept(k, o)}, union
    h, z, cnt, union = _native.overlaps(
        np.ascontiguousarray(i, np.int32), np.ascontiguousarray(j, np.int32),
        np.ascontiguousarray(dist, np.float64), np.ascontiguousarray(gap, np.float64),
        float(max_dist), np.ascontiguousarray(zob, np.int64),
        np.ascontiguousarray(marked, np.uint8), int(c_size), float(tol), sorted_hash, sorted_size)
    h, z, cnt = (np.frombuffer(b, np.int64) for b in (h, z, cnt))
    return ({(int(a), int(b)): int(c) for a, b, c in zip(h, z, cnt)},
            np.frombuffer(union, np.uint8).astype(bool))
