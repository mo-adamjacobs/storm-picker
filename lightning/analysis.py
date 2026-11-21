"""Run, store and query exact 2-parameter cluster analyses of a strike window.

Pipeline: strikes -> proximity graph (distance km, gap min) -> exact H0 edge reduction
-> every cluster's exact region in (distance, gap) space.

Pure: arrays in, one self-contained .npz (arrays + JSON summary) out. The browser
worker keeps those bytes in its private storage; nothing here touches the filesystem.
"""
from __future__ import annotations

import io
import json
import time
import uuid
from datetime import datetime, timezone

import numpy as np
from scipy.sparse import coo_matrix
from scipy.sparse.csgraph import connected_components

from .graph import thinned_edges
from .regions import cluster_regions, overlaps, strike_tags

FORMAT = 4


def iso(ts):
    return datetime.fromtimestamp(ts, timezone.utc).isoformat().replace("+00:00", "Z")


def run(params, t, lat, lon, source, progress=lambda stage, frac=None: None, props=None):
    """Analyse strikes (t sorted, seconds) in params' window.

    params: file, start, end (ISO), max_km, max_min. props: optional per-strike JSON text
    of the original GeoJSON properties, stored so exported clusters keep them.
    Returns (summary, npz bytes).
    """
    t0 = datetime.fromisoformat(params["start"].replace("Z", "+00:00")).timestamp()
    t1 = datetime.fromisoformat(params["end"].replace("Z", "+00:00")).timestamp()
    sel = (t >= t0) & (t <= t1)
    t, lat, lon = t[sel], lat[sel], lon[sel]
    if props is not None:
        props = [props[k] for k in np.flatnonzero(sel)]
    n = len(t)
    if n < 2:
        raise ValueError(f"Only {n} strikes in that time range")
    km, mins = float(params["max_km"]), float(params["max_min"])
    if km <= 0 or mins <= 0:
        raise ValueError("Maxima must be positive")
    started = time.time()

    # Nearby pairs, thinned to those that can change clusters as they are found, so the
    # full candidate set (hundreds of millions at large maxima) is never held at once.
    stage = "Finding nearby strike pairs"
    progress(stage, 0)
    i, j, d, g, n_candidates = thinned_edges(lat, lon, t, km, mins, progress=lambda f: progress(stage, f))
    d, g = d.astype(np.float64), g.astype(np.float64)

    stage = "Computing every cluster's region"
    progress(stage, 0)
    zob = strike_tags(n)
    regions = cluster_regions(n, i, j, d, g, km, zob=zob, progress=lambda f: progress(stage, f))

    progress("Saving")
    summary = {
        "id": datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S-") + uuid.uuid4().hex[:6],
        "format": FORMAT, "created": iso(time.time()), "params": params,
        "source": source, "strikes": n, "first": iso(t[0]), "last": iso(t[-1]),
        "candidate_edges": n_candidates, "kept_edges": len(i), "clusters": len(regions),
        "regions": len(regions.rows), "seconds": round(time.time() - started, 1),
    }
    extra = {}
    if props is not None:  # optional: analyses without it export only `time`
        enc = [p.encode() for p in props]
        extra["props"] = np.frombuffer(b"".join(enc), np.uint8)
        extra["props_off"] = np.concatenate([[0], np.cumsum([len(b) for b in enc])]).astype(np.int64)
    buf = io.BytesIO()
    np.savez_compressed(
        buf, meta=np.frombuffer(json.dumps(summary).encode(), np.uint8),
        t=t, lat=lat, lon=lon, zob=zob, ei=i, ej=j, ed=d, eg=g,
        c_hash=regions.hash, c_size=regions.size, rows=regions.rows, row_off=regions.off, **extra)
    return summary, buf.getvalue()


def read_summary(data: bytes):
    """The JSON summary stored inside an analysis .npz (used to validate imports)."""
    with np.load(io.BytesIO(data)) as z:
        if "meta" not in z.files:
            raise ValueError("Not a lightning analysis file")
        meta = json.loads(bytes(z["meta"]).decode())
    if meta.get("format") != FORMAT:
        raise ValueError("This analysis was saved by a different version; run it again")
    return meta


class Loaded:
    """An analysis held in memory for interactive queries."""

    def __init__(self, data: bytes):
        self.meta = read_summary(data)
        z = np.load(io.BytesIO(data))
        self.t, self.lat, self.lon, self.zob = z["t"], z["lat"], z["lon"], z["zob"]
        self.ei, self.ej, self.ed, self.eg = z["ei"], z["ej"], z["ed"], z["eg"]
        self.rows, self.off = z["rows"], z["row_off"]
        self.size = z["c_size"]
        self.hash = z["c_hash"]
        self.props = (z["props"], z["props_off"]) if "props" in z.files else None
        # (hash, size) -> cluster id by binary search, instead of a huge Python dict.
        self._by_hash = np.lexsort((self.size, self.hash))
        self._hash_sorted = self.hash[self._by_hash]
        self._size_sorted = self.size[self._by_hash]
        p = self.meta["params"]
        self.km, self.mins = float(p["max_km"]), float(p["max_min"])
        self.n = len(self.t)
        # Components with every kept edge present: no cluster ever spans two of these.
        _, self.full = connected_components(
            coo_matrix((np.ones(len(self.ei)), (self.ei, self.ej)), shape=(self.n, self.n)), directed=False)

    def lookup(self, hashes, sizes):
        """Cluster ids for (hash, size) pairs; -1 where there is no such cluster."""
        hashes = np.asarray(hashes, np.int64); sizes = np.asarray(sizes, np.int64)
        hs, ss = self._hash_sorted, self._size_sorted
        lo = np.searchsorted(hs, hashes, "left")
        out = np.full(len(hashes), -1, np.int64)
        for q in range(len(hashes)):  # hash ties are vanishingly rare; scan them
            k = lo[q]
            while k < len(hs) and hs[k] == hashes[q]:
                if ss[k] == sizes[q]:
                    out[q] = self._by_hash[k]
                    break
                k += 1
        return out

    def payload(self):
        """What the page draws: strikes and every region, as compact numeric arrays.

        Regions are normalized to the [0, 1]^2 box; open-ended rows stop at its edge.
        """
        box = np.clip(self.rows / np.array([self.mins, self.mins, self.km, self.km]), 0.0, 1.0)
        return {"t": (self.t - self.t[0]).astype(np.float64), "t0": float(self.t[0]),
                "lat": self.lat.astype(np.float32), "lon": self.lon.astype(np.float32),
                "rows": box.astype(np.float32).ravel(), "row_off": self.off.astype(np.int32),
                "size": self.size.astype(np.int32)}

    def _components(self, x, y):
        n = self.n
        present = (self.ed <= x * self.km) & (self.eg <= y * self.mins)
        ncomp, label = connected_components(
            coo_matrix((np.ones(present.sum()), (self.ei[present], self.ej[present])), shape=(n, n)),
            directed=False)
        h = np.zeros(ncomp, np.int64)
        np.bitwise_xor.at(h, label, self.zob)
        return ncomp, label, np.bincount(label, minlength=ncomp), h

    def select(self, x, y):
        """Exact clusters at normalized point (x, y), each with its region id and its span
        along the diagonal through (x, y): how far both thresholds can shrink before it
        splits and grow before it merges."""
        ncomp, label, sizes, h = self._components(x, y)
        tmin = np.full(ncomp, np.inf); np.minimum.at(tmin, label, self.t)
        tmax = np.full(ncomp, -np.inf); np.maximum.at(tmax, label, self.t)
        feats, missing = [], 0
        comps = np.flatnonzero(sizes >= 2)
        cids = self.lookup(h[comps], sizes[comps])
        for c, cid in zip(comps, cids):
            if cid < 0:  # would mean a bug: every non-singleton cluster has a region
                missing += 1
                continue
            lo, hi = self._diagonal([cid], x, y)
            feats.append({"component": int(c), "cluster": int(cid), "size": int(sizes[c]),
                          "start": iso(tmin[c]), "end": iso(tmax[c]), "below": lo, "above": hi})
        feats.sort(key=lambda f: (-f["size"], f["start"]))
        return {"x": x, "y": y, "components": int(ncomp), "singletons": int((sizes == 1).sum()),
                "label": label.astype(np.int32), "features": feats, "missing": missing}

    def _members(self, cid, x, y):
        """Boolean mask of the strikes in cluster `cid`, which must exist at (x, y)."""
        ncomp, label, sizes, h = self._components(x, y)
        hit = np.flatnonzero((h == self.hash[cid]) & (sizes == self.size[cid]))
        if not len(hit):
            raise ValueError("That cluster does not exist at the selected point")
        return label == hit[0]

    def similar(self, cid, x, y, tol):
        """Clusters D whose change from cluster C (the cluster `cid`, which exists at (x, y))
        is at most `tol`, where change = |C xor D| / |C and D|: strikes in only one of the
        two, per shared strike. That is 1/J - 1 for Jaccard similarity J = |C and D| / |C or D|.
        Exact; replays the sweep within C's full-box component."""
        members = self._members(cid, x, y)
        key = (int(self.hash[cid]), int(self.size[cid]))
        comp = np.flatnonzero(self.full == self.full[np.flatnonzero(members)[0]])
        local = np.full(self.n, -1); local[comp] = np.arange(len(comp))
        e = (local[self.ei] >= 0)
        c_size = key[1]
        ov, union = overlaps(len(comp), local[self.ei[e]], local[self.ej[e]], self.ed[e], self.eg[e],
                             self.km, members[comp], self.zob[comp], c_size, tol,
                             self._hash_sorted, self._size_sorted)
        # `ov` already holds only clusters within the tolerance.
        khash = np.fromiter((k[0] for k in ov), np.int64, len(ov))
        ksize = np.fromiter((k[1] for k in ov), np.int64, len(ov))
        o = np.fromiter(ov.values(), np.float64, len(ov))
        found = self.lookup(khash, ksize)
        keep = found >= 0
        ids = found[keep]
        change = ((c_size + ksize - 2 * o) / o)[keep]
        lo, hi = self._diagonal(ids, x, y)
        sz = self.size[ids]
        union |= members[comp]
        return {"cluster": int(cid), "tol": tol, "ids": ids.astype(np.int32),
                "union": comp[union].astype(np.int32),
                "max_change": float(change.max()) if len(change) else 0.0, "count": len(ids),
                "min_size": int(sz.min()), "max_size": int(sz.max()), "below": lo, "above": hi}

    def cluster_geojson(self, cid, x, y, tol):
        """GeoJSON FeatureCollection of cluster C (`cid`, existing at (x, y)) plus the extra
        strikes of clusters within change `tol` of it. Each extra strike gets
        `in_peripheral_cluster_with_similarity`: the Jaccard similarity to C of the most
        similar such cluster containing it. Original properties are kept when stored."""
        members = self._members(cid, x, y)
        sim = self.similar(cid, x, y, tol)
        union = np.zeros(self.n, bool); union[sim["union"]] = True
        union |= members
        best = np.zeros(self.n)
        # Each similar cluster D is a component at the low corner of any of its rows. D lies
        # inside the union, so components of the union-induced graph find it exactly.
        idx = np.flatnonzero(union)
        local = np.full(self.n, -1); local[idx] = np.arange(len(idx))
        li, lj = local[self.ei], local[self.ej]
        e = (li >= 0) & (lj >= 0)
        li, lj, ed, eg, zob = li[e], lj[e], self.ed[e], self.eg[e], self.zob[idx]
        c_local = members[idx]
        for d in sim["ids"]:
            if d == cid:
                continue
            ylo, _, xlo, _ = self.rows[self.off[d]]
            p = (ed <= xlo) & (eg <= ylo)
            nc, lab = connected_components(
                coo_matrix((np.ones(p.sum()), (li[p], lj[p])), shape=(len(idx), len(idx))), directed=False)
            h = np.zeros(nc, np.int64); np.bitwise_xor.at(h, lab, zob)
            hit = np.flatnonzero((h == self.hash[d]) & (np.bincount(lab, minlength=nc) == self.size[d]))
            if not len(hit):  # would mean a bug: every similar cluster has a region
                continue
            dm = lab == hit[0]
            jac = (dm & c_local).sum() / (dm | c_local).sum()
            best[idx[dm]] = np.maximum(best[idx[dm]], jac)
        feats = []
        for k in idx:  # strikes are stored in time order
            if self.props is not None:
                b, o = self.props
                props = json.loads(bytes(b[o[k]:o[k + 1]]).decode())
            else:
                props = {"time": iso(self.t[k])}
            if not members[k]:
                props["in_peripheral_cluster_with_similarity"] = round(float(best[k]), 6)
            feats.append({"type": "Feature", "properties": props,
                          "geometry": {"type": "Point", "coordinates": [float(self.lon[k]), float(self.lat[k])]}})
        meta = dict(self.meta.get("source") or {})
        meta["cluster"] = {
            "analysis": self.meta["id"], "file": self.meta["params"]["file"],
            "distance_km": x * self.km, "gap_min": y * self.mins, "change_tolerance": tol,
            "strikes": int(members.sum()), "peripheral_strikes": int(union.sum() - members.sum()),
            "similar_clusters": int(sim["count"]), "properties_preserved": self.props is not None}
        return {"type": "FeatureCollection", "metadata": meta, "features": feats}

    def _diagonal(self, cids, x, y):
        """Normalized extent (down, up) along the line (x+s, y+s) of the union of
        the given clusters' regions (one cluster, or a set of similar clusters)."""
        R = np.concatenate([self.rows[self.off[c]:self.off[c + 1]] for c in cids])
        X, Y = x * self.km, y * self.mins
        # Row (ylo, yhi, xlo, xhi) contains (X + s*km, Y + s*min) for s in [a, b).
        a = np.maximum((R[:, 2] - X) / self.km, (R[:, 0] - Y) / self.mins)
        b = np.minimum((R[:, 3] - X) / self.km, (R[:, 1] - Y) / self.mins)
        ok = a < b
        a, b = a[ok], b[ok]
        o = np.argsort(a)
        a, b = a[o], b[o]
        # Merge sorted pieces into blocks; keep the block containing s = 0.
        lo = hi = None
        for p, q in zip(a, b):
            if hi is None or p > hi + 1e-12:
                if lo is not None and lo <= 1e-12 and hi >= -1e-12:
                    break  # the block holding 0 is complete
                lo, hi = p, q
            else:
                hi = max(hi, q)
        if lo is None or not (lo <= 1e-12 and hi >= -1e-12):
            lo = hi = 0.0
        lim = min(1 - x, 1 - y)  # the computed box ends here along the diagonal
        return float(-lo), float(min(hi, lim))
