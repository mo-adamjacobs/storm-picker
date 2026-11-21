# Lightning persistence

Explore lightning strikes as clusters across two scales at once: **distance** (km) and
**time gap** (minutes). Two strikes are linked when they are within both thresholds; a
cluster is a connected group of linked strikes. For a window of strikes, the explorer
computes **every** cluster that exists at any pair of thresholds up to your chosen
maxima, and exactly where in (distance, time gap) space each one exists.

Everything runs in your browser (Python via [Pyodide](https://pyodide.org), with the
heavy sweep in Rust compiled to WebAssembly). Strike files are read locally and never
uploaded; saved analyses stay in your browser unless you export them.

## Using it

1. **New analysis** → choose a GeoJSON strike file (format below), set the time window
   and the maximum distance and time gap, then **Run**. Progress shows in the header.
2. **Persistence map** (left): the shading is the exact number of clusters (2+ strikes)
   at each distance / time-gap threshold. Click a point to list the clusters there.
3. **Clusters** (right): the clusters at that point, largest first, with their time span
   and how far along the diagonal through the point the same strikes stay a cluster.
   Pick one to outline its region on the persistence map and highlight it on the map.
   - **Treat as the same cluster if change ≤ x %** also shades every cluster that
     differs from the picked one by at most x %, where change = strikes in only one
     of the two clusters ÷ strikes they share (that is 1/J − 1, for Jaccard similarity
     J = shared ÷ in either). Their extra strikes show in a darker shade.
   - **Save** on a cluster downloads it as GeoJSON, plus the extra strikes of the
     clusters within the change tolerance. Each extra strike gets
     `in_peripheral_cluster_with_similarity`: the Jaccard similarity to the saved cluster
     of the most similar of those clusters containing it. Strikes keep their original
     properties, and `metadata.cluster` records the thresholds and tolerance used.
4. **Strike map** (centre): click a strike to pick its cluster; double-click (here or
   in the list) to limit the time range to that cluster. The slider below shows a time
   window: drag the ends to resize it, or the middle to slide it.
5. **Open…** lists analyses saved in this browser, with **Export** (an `.npz` file) and
   **Import**.

Large maxima are supported (e.g. 300 km / 60 min over 233k strikes), but take minutes
and need a desktop browser with a few GB of memory available.

## Input: GeoJSON strike files

A GeoJSON `FeatureCollection` of `Point` features, one per strike:

```json
{
  "type": "FeatureCollection",
  "metadata": {
    "name": "WWLLN strikes, Hurricane Gonzalo, 2014-10-11 to 2014-10-20",
    "source": "NASA GHRC DAAC, HS3 WWLLN Storms V1, doi:10.5067/HS3/WWLN/DATA201"
  },
  "features": [
    {
      "type": "Feature",
      "geometry": { "type": "Point", "coordinates": [-66.3616, 18.1424] },
      "properties": { "time": "2014-10-11T00:00:00.952317Z", "stations": 6 }
    }
  ]
}
```

- `geometry.coordinates` is `[longitude, latitude]` in degrees (GeoJSON order;
  longitude −180…180, latitude −90…90). Any third coordinate is ignored.
- `properties.time` is required: an ISO 8601 string **with a timezone**, either `Z` or
  an offset such as `+00:00`. Fractional seconds are kept.
- Other properties are not used in the analysis, but are stored with it and written back
  out when you save a cluster. (Analyses run before this was added save only `time`.)
- `metadata` is optional and not part of the GeoJSON standard; if present, its `name`
  is shown when the file is loaded and the whole object is stored with each analysis.
- Features need not be in time order. Every feature must be a valid `Point` with a
  time; the file is rejected with the offending feature number otherwise. Nothing is
  filled in or invented.

Distances are great-circle on a sphere of radius 6371.0088 km.

## Data

Development used the [HS3 WWLLN Storms V1](https://www.earthdata.nasa.gov/data/catalog/ghrc-daac-hs3wwlln-1)
dataset (World Wide Lightning Location Network strikes for storms studied by NASA's
Hurricane and Severe Storm Sentinel campaign), converted to the format above. The data
are not included in this repository. NASA describes the dataset as "openly shared,
without restriction" under the EOSDIS Data Use and Citation Guidance; WWLLN is operated
by the University of Washington, so check before redistributing raw strikes. Cite it as:

> Holzworth, R. (2025). *Hurricane and Severe Storm Sentinel (HS3) World Wide Lightning
> Location Network (WWLLN) Storms V1* [Data set]. NASA Global Hydrometeorology Resource
> Center Distributed Active Archive Center. https://doi.org/10.5067/HS3/WWLN/DATA201

## How it works

1. **Nearby pairs.** All strike pairs within the maximum distance and time gap, found
   with a KD-tree in time chunks.
2. **Edge reduction.** Pairs that can never change which strikes are connected (their
   strikes are already joined by pairs with no larger distance and no larger gap) are
   dropped as each chunk is found, then once more over the survivors. This is exact.
3. **Region sweep.** Sweeping the time gap upward, the single-linkage tree over distance
   is updated one edge at a time; every cluster that ever exists is recorded with the
   rectangles of (distance, gap) where it is a connected component. Clusters are
   identified by a 64-bit hash of their strike set plus their size.
4. **Queries.** Clicking a point finds the exact clusters there; "similar clusters"
   replays the sweep within the picked cluster's component, counting shared strikes.

The edge reduction, the region sweep and the similar-cluster replay run in Rust
(`sweep/`); pair finding and point queries use NumPy/SciPy. `lightning/regions.py` and
`lightning/graph.py` keep the Rust algorithms in pure Python as a reference, used
automatically if the Rust module is unavailable.

## Development

Requires [uv](https://docs.astral.sh/uv/) and a Rust toolchain ([rustup](https://rustup.rs)).

```sh
uv sync                       # builds the Rust module natively (via maturin)
uv run lightning-explorer     # serves the page at http://127.0.0.1:8765
```

The page needs the Rust module built for Pyodide to run at full speed; without it, the
worker falls back to the Python reference (same results, much slower) and says so in the
header. To build it locally:

```sh
uv tool install pyodide-cli --with pyodide-build
pyodide xbuildenv install 314.0.7          # the version worker.js loads
pyodide xbuildenv install-emscripten
rustup toolchain install "$(pyodide config get rust_toolchain)" --target wasm32-unknown-emscripten
./sweep/build-pyodide.sh                   # writes lightning/static/wheels/
```

The GitHub Pages workflow (`.github/workflows/pages.yml`) does the same on every push to
`main` and publishes `lightning/static/`, the Python modules (under `py/`) and the wheel.

### Layout

| Path | What |
|---|---|
| `lightning/static/` | The page (`index.html`, `app.js`, `style.css`) and the Pyodide worker (`worker.js`) |
| `lightning/browser.py` | Glue between the worker and the analysis code |
| `lightning/analysis.py` | Run an analysis, store it as one `.npz`, answer clicks |
| `lightning/graph.py` | Nearby pairs and edge reduction |
| `lightning/regions.py` | Region sweep and overlap queries (dispatches to Rust) |
| `lightning/data.py` | Strike validation |
| `lightning/server.py` | Local static server for development |
| `sweep/` | The Rust module (`lightning_sweep`, built with maturin) |
