//! The graph (graph.rs) against brute force over every pair of strikes.

mod common;

use common::*;
use storm_engine::graph::{DIRECT, Graph};

fn all_cases() -> Vec<(u64, usize, f64, f64)> {
    let mut all = cases();
    all.extend([
        (11, 200, 40.0, -100.0),
        (12, 300, 0.0, 0.0),
        (13, 250, -60.0, 120.0),
    ]);
    all.extend((100..112).map(|s| {
        (
            s,
            150 + 20 * (s as usize - 100),
            -70.0 + 12.0 * (s - 100) as f64,
            -179.0 + 30.0 * (s - 100) as f64,
        )
    }));
    all
}

fn stored(g: &Graph) -> Vec<(u32, u32, f64)> {
    (0..g.edges())
        .map(|e| (g.ea[e], g.eb[e], g.ed[e]))
        .collect()
}

/// Exactly the minimal set, whatever the block size and however each band is searched
/// (tree against tree or strike by strike).
#[test]
fn stored_edges_are_the_minimal_set() {
    for (seed, n, la, lo) in all_cases() {
        let (t, lat, lon) = strikes(seed, n, la, lo);
        let b = Brute::new(&t, &lat, &lon);
        let want = minimal(&b, &t);
        for (block, direct) in [(256, DIRECT), (16, DIRECT), (16, 0.0), (16, f64::INFINITY)] {
            let g = Graph::build(t.clone(), lat.clone(), lon.clone(), block, direct, |_| {});
            assert_eq!(
                stored(&g),
                want,
                "seed {seed}, block {block}, direct {direct}"
            );
        }
        eprintln!("seed {seed}: {} pairs, {} edges", b.pairs.len(), want.len());
    }
}

/// The stored edges give the components of all pairs at every threshold (checked
/// independently of how they were chosen), and each strike lists its own edges.
#[test]
fn stored_edges_give_every_threshold_exactly() {
    for (seed, n, la, lo) in all_cases() {
        let (t, lat, lon) = strikes(seed, n, la, lo);
        let b = Brute::new(&t, &lat, &lon);
        let g = Graph::with_block(t.clone(), lat, lon, 16);
        for u in 0..n {
            for &e in g.edges_of(u) {
                assert!(g.ea[e as usize] as usize == u || g.eb[e as usize] as usize == u);
            }
        }
        let listed: usize = (0..n).map(|u| g.edges_of(u).len()).sum();
        assert_eq!(listed, 2 * g.edges(), "seed {seed}: each edge listed at both ends");
        let mut r = Rng(seed + 9);
        let (dx, gy) = (b.dists(f64::INFINITY), b.gaps(f64::INFINITY));
        let (xs, ys) = (sample(&dx, 10, &mut r), sample(&gy, 10, &mut r));
        for &x in &xs {
            for &y in &ys {
                let mut uf: Vec<usize> = (0..n).collect();
                fn find(uf: &mut [usize], mut v: usize) -> usize {
                    while uf[v] != v {
                        uf[v] = uf[uf[v]];
                        v = uf[v];
                    }
                    v
                }
                for e in 0..g.edges() {
                    if g.ed[e] <= x && g.gap(e as u32) <= y {
                        let (p, q) = (find(&mut uf, g.ea[e] as usize), find(&mut uf, g.eb[e] as usize));
                        uf[p.max(q)] = p.min(q);
                    }
                }
                let got: Vec<usize> = (0..n).map(|v| find(&mut uf, v)).collect();
                assert_eq!(got, b.labels(x, y), "seed {seed}: components at ({x}, {y})");
            }
        }
    }
}

/// A graph saved and loaded again (`from_parts`) is the graph built; malformed parts are
/// refused rather than used.
#[test]
fn saved_graph_loads_back() {
    for (seed, n, la, lo) in cases() {
        let (t, lat, lon) = strikes(seed, n, la, lo);
        let g = Graph::new(t.clone(), lat.clone(), lon.clone());
        let load = |ea: Vec<u32>, eb: Vec<u32>, ed: Vec<f64>, fr: Vec<(f64, f64)>| {
            Graph::from_parts(t.clone(), lat.clone(), lon.clone(), ea, eb, ed, fr)
        };
        let h = load(g.ea.clone(), g.eb.clone(), g.ed.clone(), g.frontier.clone())
            .expect("a built graph loads");
        assert_eq!((&h.ea, &h.eb, &h.ed, &h.frontier), (&g.ea, &g.eb, &g.ed, &g.frontier));
        assert_eq!(h.by_km, g.by_km, "seed {seed}: distance order");
        for u in 0..n {
            assert_eq!(h.edges_of(u), g.edges_of(u), "seed {seed}: edges of {u}");
        }
        let mut bad = g.eb.clone();
        bad[0] = n as u32;
        assert!(load(g.ea.clone(), bad, g.ed.clone(), g.frontier.clone()).is_none());
        // Out of order: the edges reversed.
        let rev = |v: &Vec<u32>| v.iter().rev().copied().collect::<Vec<u32>>();
        let gaps: Vec<f64> = (0..g.edges() as u32).map(|e| g.gap(e)).collect();
        if gaps.first() != gaps.last() {
            let ed: Vec<f64> = g.ed.iter().rev().copied().collect();
            assert!(load(rev(&g.ea), rev(&g.eb), ed, g.frontier.clone()).is_none());
        }
        assert!(load(g.ea.clone(), g.eb.clone(), g.ed[1..].to_vec(), g.frontier.clone()).is_none());
    }
}
