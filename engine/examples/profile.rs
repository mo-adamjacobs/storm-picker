//! Graph build profile: `cargo run --release --example profile -- strikes.bin [hours]`
//! (strikes.bin as for the bench example; hours: only the strikes in the first that many).
use std::time::Instant;
use storm_engine::graph::Graph;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let raw = std::fs::read(&a[1]).unwrap();
    let hours: f64 = a.get(2).map_or(f64::INFINITY, |s| s.parse().unwrap());
    let v: Vec<f64> = raw
        .chunks(8)
        .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
        .collect();
    let (mut t, mut lat, mut lon) = (Vec::new(), Vec::new(), Vec::new());
    for c in v.chunks(3).take_while(|c| c[0] <= hours * 3600.0) {
        t.push(c[0]);
        lat.push(c[1]);
        lon.push(c[2]);
    }
    let n = t.len();
    let s = Instant::now();
    let g = Graph::new(t, lat, lon);
    let per = |v: u64| v as f64 / n as f64;
    println!(
        "{n} strikes: {:.2}s; edges {} ({:.1}/strike); node pairs {:.1}/strike, pairs looked at {:.1}/strike, geodesics {:.1}/strike, candidates {:.1}/strike",
        s.elapsed().as_secs_f64(),
        g.edges(),
        per(g.edges() as u64),
        per(g.stats[0]),
        per(g.stats[1]),
        per(g.geodesics),
        per(g.stats[2]),
    );
    if let Ok(st) = std::fs::read_to_string("/proc/self/status") {
        if let Some(l) = st.lines().find(|l| l.starts_with("VmHWM")) {
            println!("peak memory: {}", l.trim_start_matches("VmHWM:").trim());
        }
    }
}
