"""Distance / time-gap proximity graph and an exact H0-preserving edge reduction.

Edges carry two filtration values: geodesic distance (km) and absolute time gap
(minutes). Every vertex is born at (0, 0).
"""
from __future__ import annotations

import numpy as np
from scipy.sparse import coo_matrix
from scipy.sparse.csgraph import minimum_spanning_tree
from scipy.spatial import cKDTree

EARTH_RADIUS_KM = 6371.0088  # mean radius; great-circle on a sphere, not WGS84


def unit_vectors(lat, lon):
    la, lo = np.radians(lat), np.radians(lon)
    return np.c_[np.cos(la) * np.cos(lo), np.cos(la) * np.sin(lo), np.sin(la)]


def geodesic_km(xyz_a, xyz_b):
    chord = np.linalg.norm(xyz_a - xyz_b, axis=1)
    return 2 * EARTH_RADIUS_KM * np.arcsin(np.clip(chord / 2, 0, 1))


def candidate_edges(lat, lon, t, max_km, max_min, chunk=20_000, progress=None):
    """All pairs within max_km AND max_min. `t` (seconds) must be sorted ascending.

    Works in time chunks so the KD-tree query never holds the whole dataset's pairs
    twice. Returns (i, j, dist_km, gap_min) with i < j, as int32/float32 arrays.
    """
    n = len(t)
    xyz = unit_vectors(lat, lon)
    chord = 2 * np.sin(max_km / EARTH_RADIUS_KM / 2)
    gap_s = max_min * 60.0
    parts = []
    for start in range(0, n, chunk):
        stop = min(start + chunk, n)
        end = np.searchsorted(t, t[stop - 1] + gap_s, side="right")
        # Chebyshev box in (xyz / chord, t / gap): a superset of the exact condition.
        pts = np.c_[xyz[start:end] / chord, (t[start:end] - t[start]) / gap_s]
        p = cKDTree(pts).query_pairs(1.0, p=np.inf, output_type="ndarray")
        p = p[p.min(axis=1) < stop - start] + start  # each pair owned by one chunk
        a, b = p.min(axis=1), p.max(axis=1)
        dist = geodesic_km(xyz[a], xyz[b])
        gap = np.abs(t[b] - t[a]) / 60.0
        keep = (dist <= max_km) & (gap <= max_min)
        parts.append((a[keep].astype(np.int32), b[keep].astype(np.int32),
                      dist[keep].astype(np.float32), gap[keep].astype(np.float32)))
        if progress:
            progress(stop / n)
    if not parts:
        e = np.empty(0, np.int32), np.empty(0, np.float32)
        return e[0], e[0].copy(), e[1], e[1].copy()
    return tuple(np.concatenate(x) for x in zip(*parts))


def _chunk_pairs(xyz, t, start, stop, end, chord, gap_s, max_km, max_min):
    """Pairs (a, b), a in [start, stop), a < b < end, within max_km and max_min."""
    pts = np.c_[xyz[start:end] / chord, (t[start:end] - t[start]) / gap_s]
    # Chebyshev box in (xyz / chord, t / gap): a superset of the exact condition.
    sdm = cKDTree(pts[: stop - start]).sparse_distance_matrix(
        cKDTree(pts), 1.0, p=np.inf, output_type="ndarray")
    a, b = sdm["i"].astype(np.int64) + start, sdm["j"].astype(np.int64) + start
    own = a < b
    a, b = a[own], b[own]
    dist = geodesic_km(xyz[a], xyz[b])
    gap = np.abs(t[b] - t[a]) / 60.0
    keep = (dist <= max_km) & (gap <= max_min)
    return a[keep], b[keep], dist[keep].astype(np.float32), gap[keep].astype(np.float32)


def thinned_edges(lat, lon, t, max_km, max_min, budget=3_000_000, progress=None):
    """The edges that can change connectivity (as reduce_h0 would keep), without ever
    holding all candidate pairs: each time chunk's pairs are reduced as soon as they are
    found, then the survivors are reduced together. Exact: an edge redundant among its
    chunk's edges is redundant among all edges. `t` (seconds) must be sorted ascending.

    Chunks are sized from a sampled neighbour count so each holds about `budget` pairs.
    Returns (i, j, dist_km, gap_min, number of candidate pairs).
    """
    n = len(t)
    xyz = unit_vectors(lat, lon)
    chord = 2 * np.sin(max_km / EARTH_RADIUS_KM / 2)
    gap_s = max_min * 60.0
    # Sample how many neighbours a strike has, to size the chunks.
    pts = np.c_[xyz / chord, t / gap_s]
    sample = np.random.default_rng(0).choice(n, min(n, 300), replace=False)
    per = max(1.0, float(np.mean(cKDTree(pts).query_ball_point(pts[sample], 1.0, p=np.inf, return_length=True))))
    del pts
    chunk = int(np.clip(budget / per, 200, 50_000))
    parts, candidates = [], 0
    for start in range(0, n, chunk):
        stop = min(start + chunk, n)
        end = int(np.searchsorted(t, t[stop - 1] + gap_s, side="right"))
        a, b, d, g = _chunk_pairs(xyz, t, start, stop, end, chord, gap_s, max_km, max_min)
        candidates += len(a)
        if len(a):
            k = reduce_h0(end - start, (a - start).astype(np.int32), (b - start).astype(np.int32), d, g)
            parts.append((a[k].astype(np.int32), b[k].astype(np.int32), d[k], g[k]))
        if progress:
            progress(stop / n)
    if not parts:
        e = np.empty(0, np.int32), np.empty(0, np.float32)
        return e[0], e[0].copy(), e[1], e[1].copy(), 0
    i, j, d, g = (np.concatenate(x) for x in zip(*parts))
    k = reduce_h0(n, i, j, d, g)
    return i[k], j[k], d[k], g[k], candidates


def _minimax(n, fi, fj, fd, qi, qj):
    """Bottleneck (minimax) distance between query pairs in forest (fi, fj, fd).

    Built as a Kruskal reconstruction tree; inf where the pair is disconnected.
    """
    order = np.argsort(fd, kind="stable")
    parent = np.arange(n)
    node_parent = np.full(2 * n, -1, np.int64)
    node_w = np.zeros(2 * n)
    top = np.arange(n)  # union-find root -> its current reconstruction-tree node
    nxt = n

    def find(x):
        root = x
        while parent[root] != root:
            root = parent[root]
        while parent[x] != root:
            parent[x], x = root, parent[x]
        return root

    for k in order:
        a, b = find(fi[k]), find(fj[k])
        if a == b:
            continue
        node_parent[top[a]] = node_parent[top[b]] = nxt
        node_w[nxt] = fd[k]
        parent[b] = a
        top[a] = nxt
        nxt += 1
    m = nxt
    node_parent, node_w = node_parent[:m], node_w[:m]
    # Depth from roots (parents always have larger ids, so a reverse sweep works).
    depth = np.zeros(m, np.int64)
    for v in range(m - 1, -1, -1):
        p = node_parent[v]
        if p >= 0:
            depth[v] = depth[p] + 1
    levels = max(1, int(depth.max()).bit_length())
    up = np.empty((levels, m), np.int64)
    up[0] = np.where(node_parent >= 0, node_parent, np.arange(m))
    for l in range(1, levels):
        up[l] = up[l - 1][up[l - 1]]
    u, v = qi.astype(np.int64), qj.astype(np.int64)
    du, dv = depth[u], depth[v]
    swap = du < dv
    u, v = np.where(swap, v, u), np.where(swap, u, v)
    diff = np.abs(du - dv)
    for l in range(levels):
        bit = (diff >> l) & 1 == 1
        u[bit] = up[l][u[bit]]
    for l in range(levels - 1, -1, -1):
        move = up[l][u] != up[l][v]
        u[move], v[move] = up[l][u[move]], up[l][v[move]]
    same = u == v
    lca = np.where(same, u, up[0][u])
    # Two different roots also end with up[0] == self, so compare parents explicitly.
    connected = same | ((up[0][u] == up[0][v]) & (up[0][u] != u))
    connected &= lca >= n
    out = np.full(len(qi), np.inf)
    out[connected] = node_w[lca[connected]]
    return out


def _reduce_python(n, i, j, dist, gap, batch=None, progress=None):
    """Drop edges that can never change connectivity at any (distance, gap) threshold.

    Edge e is redundant if its endpoints are already joined by edges with smaller
    distance and smaller gap: then wherever e is present, so is that path. Edges
    are swept in increasing gap; each batch is tested against the distance-MST of
    all earlier batches. Exact for H0 (keeps a superset of the minimal edge set).
    Returns indices of the kept edges.
    """
    order = np.lexsort((dist, gap))
    batch = batch or max(n, 50_000)
    fi = np.empty(0, np.int32); fj = fi.copy(); fd = np.empty(0, np.float64)
    kept = []
    for s in range(0, len(order), batch):
        idx = order[s:s + batch]
        # Forest edges come from earlier batches, so their gaps are <= this edge's.
        mm = _minimax(n, fi, fj, fd, i[idx], j[idx])
        k = idx[~(mm <= dist[idx])]
        kept.append(k)
        # New forest = distance-MST of old forest + kept edges (+1 so 0 km survives).
        ai = np.concatenate([fi, i[k]]); aj = np.concatenate([fj, j[k]])
        ad = np.concatenate([fd, dist[k].astype(np.float64)])
        g = minimum_spanning_tree(coo_matrix((ad + 1.0, (ai, aj)), shape=(n, n))).tocoo()
        fi, fj, fd = g.row.astype(np.int32), g.col.astype(np.int32), g.data - 1.0
        if progress:
            progress(min(1.0, (s + batch) / len(order)))
    return np.sort(np.concatenate(kept)) if kept else np.empty(0, np.int64)


def reduce_h0(n, i, j, dist, gap, batch=None, progress=None, backend=None):
    """Indices of the edges that can change connectivity at some threshold (see
    _reduce_python for the algorithm). Runs in Rust when lightning_sweep is installed."""
    from .regions import _backend, _native
    batch = batch or max(n, 50_000)
    if _backend(backend) == "python":
        return _reduce_python(n, i, j, dist, gap, batch, progress)
    kept = _native.reduce(
        int(n), np.ascontiguousarray(i, np.int32), np.ascontiguousarray(j, np.int32),
        np.ascontiguousarray(dist, np.float64), np.ascontiguousarray(gap, np.float64),
        int(batch), progress)
    return np.frombuffer(kept, np.int64)
