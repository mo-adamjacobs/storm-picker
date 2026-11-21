"""Glue between the browser worker (worker.js) and the analysis code, under Pyodide.

Holds the loaded strike file and the open analysis in memory. Returns JSON strings for
small results and NumPy arrays for big ones (the worker turns those into typed arrays).
"""
import json

import numpy as np

from lightning import analysis, data

SOURCE = None  # (name, metadata, t, lat, lon, props) of the loaded GeoJSON
OPEN = None    # analysis.Loaded


def _array(js):
    return np.array(js.to_py())


def set_source(name, times, lat, lon, meta_json, props=None):
    """props: each strike's original `properties`, as JSON text (kept for cluster export)."""
    global SOURCE
    SOURCE = None  # free the previous file first
    t, la, lo, order = data.validate(times.to_py(), _array(lat), _array(lon), return_order=True)
    meta = json.loads(meta_json)
    if props is not None:
        props = props.to_py()
        props = [props[k] for k in order]
    SOURCE = (name, meta, t, la, lo, props)
    return json.dumps({"name": name, "strikes": len(t), "metadata": meta,
                       "first": analysis.iso(t[0]), "last": analysis.iso(t[-1])})


def run(params_json, progress):
    params = json.loads(params_json)
    if SOURCE is None or SOURCE[0] != params["file"]:
        raise ValueError("Load the strike file first")
    _, meta, t, lat, lon, props = SOURCE
    summary, blob = analysis.run(params, t, lat, lon, meta, lambda s, f=None: progress(s, f), props)
    return json.dumps(summary), blob


def summary_of(blob):
    return json.dumps(analysis.read_summary(bytes(blob.to_py())))


def open_analysis(blob):
    global OPEN
    OPEN = None  # free the previous analysis first
    OPEN = analysis.Loaded(bytes(blob.to_py()))
    return json.dumps(OPEN.meta), OPEN.payload()


def _open():
    if OPEN is None:
        raise ValueError("No analysis is open")
    return OPEN


def select(x, y):
    r = _open().select(float(x), float(y))
    label = r.pop("label")
    return json.dumps(r), label


def similar(cluster, x, y, tol):
    r = _open().similar(int(cluster), float(x), float(y), float(tol))
    union, ids = r.pop("union"), r.pop("ids")
    return json.dumps(r), union, ids


def cluster_geojson(cluster, x, y, tol):
    return json.dumps(_open().cluster_geojson(int(cluster), float(x), float(y), float(tol)))
