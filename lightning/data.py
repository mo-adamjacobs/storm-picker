"""Strike validation. Observations only: nothing is filled in or synthesised.

The browser parses GeoJSON (fast native JSON.parse) and hands over times as ISO strings
plus coordinate arrays; validation and time parsing happen here so the rules are the
same everywhere. `load_geojson` is the same thing for a file on disk (tests, scripts).
"""
from __future__ import annotations

import json
from datetime import datetime
from pathlib import Path

import numpy as np


def parse_time(value, where):
    if not isinstance(value, str):
        raise ValueError(f"{where}: properties.time must be an ISO 8601 string")
    dt = datetime.fromisoformat(value.replace("Z", "+00:00"))
    if dt.tzinfo is None:
        raise ValueError(f"{where}: time needs Z or a UTC offset")
    return dt.timestamp()


def validate(times, lat, lon, return_order=False):
    """Times (ISO strings) and coordinates -> (t, lat, lon) sorted by time (stable).

    With return_order, also the permutation applied (to reorder other per-strike data)."""
    n = len(times)
    if n == 0:
        raise ValueError("No strikes found")
    lat = np.asarray(lat, np.float64)
    lon = np.asarray(lon, np.float64)
    if len(lat) != n or len(lon) != n:
        raise ValueError("Times and coordinates differ in length")
    bad = ~(np.isfinite(lat) & np.isfinite(lon) & (np.abs(lat) <= 90) & (np.abs(lon) <= 180))
    if bad.any():
        raise ValueError(f"Feature {int(np.flatnonzero(bad)[0]) + 1}: coordinates out of range")
    t = np.fromiter((parse_time(v, f"Feature {k + 1}") for k, v in enumerate(times)), np.float64, n)
    order = np.argsort(t, kind="stable")
    if return_order:
        return t[order], lat[order], lon[order], order
    return t[order], lat[order], lon[order]


def load_geojson(path: Path):
    """Return (metadata, t, lat, lon) from a GeoJSON FeatureCollection of Points."""
    obj = json.loads(Path(path).read_text())
    if obj.get("type") != "FeatureCollection":
        raise ValueError("Expected a GeoJSON FeatureCollection")
    feats = obj.get("features") or []
    for k, f in enumerate(feats):
        if (f.get("geometry") or {}).get("type") != "Point":
            raise ValueError(f"Feature {k + 1}: only Point geometries are supported")
    times = [(f.get("properties") or {}).get("time") for f in feats]
    lon = [f["geometry"]["coordinates"][0] for f in feats]
    lat = [f["geometry"]["coordinates"][1] for f in feats]
    return (obj.get("metadata") or {}), *validate(times, lat, lon)
