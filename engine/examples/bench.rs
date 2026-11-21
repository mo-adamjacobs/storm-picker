//! Timing on real data: `cargo run --release --example bench -- strikes.bin [hours]`
//! (strikes.bin: little-endian f64 triples [seconds, lat, lon], ascending time).
use std::time::Instant;

use storm_engine::engine::Engine;
use storm_engine::graph::Graph;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let raw = std::fs::read(&a[1]).unwrap();
    let hours: f64 = a.get(2).map_or(1.0, |s| s.parse().unwrap());
    let v: Vec<f64> = raw
        .chunks(8)
        .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
        .collect();
    let (mut t, mut lat, mut lon) = (Vec::new(), Vec::new(), Vec::new());
    for c in v.chunks(3) {
        t.push(c[0]);
        lat.push(c[1]);
        lon.push(c[2]);
    }
    let s = Instant::now();
    let mut e = Engine::new(Graph::new(t, lat, lon));
    println!("index {:.2}s", s.elapsed().as_secs_f64());
    let starts: Vec<f64> = a.get(3).map_or(vec![0.0, 120.0], |s| {
        s.split(',').map(|v| v.parse().unwrap()).collect()
    });
    let clicks = a.get(4).is_none_or(|s| s != "noclick");
    for start_h in starts {
        let s = Instant::now();
        let v = e.set_view(
            start_h * 3600.0,
            (start_h + hours) * 3600.0,
            -90.0,
            90.0,
            -180.0,
            180.0,
        );
        let ns = v.strikes.len();
        let (xmin, ymin, fr) = (e.x_min, e.y_min, e.frontier.len());
        let (bx, by) = e.bx;
        println!(
            "view from hour {start_h} ({hours} h): {:.2}s, {ns} strikes, x {xmin:.3}..{bx:.0} km, y {:.1}s..{:.0} min, {fr} frontier corners",
            s.elapsed().as_secs_f64(),
            ymin,
            by / 60.0
        );
        for f in [0.2, 0.4, 0.6, 0.8].into_iter().filter(|_| clicks) {
            // Log-spaced point across the box.
            let x = xmin * (bx / xmin).powf(f);
            let y = ymin * (by / ymin).powf(f);
            let s = Instant::now();
            let sel = e.select(x, y);
            let (nc, big, label) = (
                sel.clusters.len(),
                sel.clusters.first().map_or(0, |c| c.size),
                sel.clusters.first().map(|c| c.label),
            );
            let reached = sel.label.iter().filter(|&&l| l != u32::MAX).count();
            let ts = s.elapsed().as_secs_f64();
            let s = Instant::now();
            let rows = label.map_or(0, |l| e.region_of(l).len() / 4);
            let tr = s.elapsed().as_secs_f64();
            let s = Instant::now();
            let sim = label.map(|l| e.similar(l, 0.2, false, |_| {}));
            println!(
                "  click ({x:.2} km, {:.2} min): select {ts:.3}s ({nc} clusters, largest {big}, {reached} strikes reached); its region {tr:.3}s ({rows} rows); similar {:.3}s ({} passes, stability {:.1}%)",
                y / 60.0,
                s.elapsed().as_secs_f64(),
                sim.as_ref().map_or(0, |s| s.passes),
                sim.map_or(0.0, |s| s.stability * 100.0)
            );
        }
    }
}
