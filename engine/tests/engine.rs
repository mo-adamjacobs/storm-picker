//! The lazy engine against brute force over every pair of strikes (exact WGS84).

mod common;

use std::collections::{HashMap, HashSet};

use common::*;
use storm_engine::engine::Engine;
use storm_engine::graph::Graph;

type Key = (u64, u32);

/// Random tags per strike (to identify strike sets by XOR).
fn tags(n: usize) -> Vec<u64> {
    let mut r = Rng(0x5eed);
    (0..n)
        .map(|_| (r.f() * 2f64.powi(52)) as u64 ^ ((r.f() * 2f64.powi(52)) as u64) << 12)
        .collect()
}

fn key_of(m: &[usize], tg: &[u64]) -> Key {
    (m.iter().fold(0u64, |h, &v| h ^ tg[v]), m.len() as u32)
}

/// (seed, strikes, lat, lon): mid-latitudes, near a pole, across the antimeridian, sparse.
fn engine_cases() -> Vec<(u64, usize, f64, f64)> {
    vec![
        (1, 60, 35.0, -60.0),
        (2, 70, 10.0, 20.0),
        (3, 50, 85.0, 40.0),
        (4, 60, -20.0, 179.9),
        (101, 60, -50.0, -120.0),
    ]
}

/// A view: the middle of the time range, and a lat/lon box covering part of the area
/// (across the antimeridian when lon0 is near it).
fn view_of(t: &[f64], lat0: f64, lon0: f64) -> (f64, f64, f64, f64, f64, f64) {
    let span = t[t.len() - 1];
    let (lo0, lo1) = (lon0 - 0.25, lon0 + 0.5);
    let wrap = |v: f64| {
        if v > 180.0 {
            v - 360.0
        } else if v < -180.0 {
            v + 360.0
        } else {
            v
        }
    };
    (
        0.25 * span,
        0.7 * span,
        lat0 - 0.3,
        lat0 + 0.5,
        wrap(lo0),
        wrap(lo1),
    )
}

fn in_view(v: (f64, f64, f64, f64, f64, f64), t: f64, lat: f64, lon: f64) -> bool {
    let (t0, t1, la0, la1, lo0, lo1) = v;
    let inlon = if lo0 <= lo1 {
        lon >= lo0 && lon <= lo1
    } else {
        lon >= lo0 || lon <= lo1
    };
    t >= t0 && t <= t1 && lat >= la0 && lat <= la1 && inlon
}

/// Every cluster (2+ strikes) at any threshold within (bx km, by s), by Kruskal over the
/// distance at each gap: key -> members.
fn all_clusters(b: &Brute, bx: f64, by: f64, tg: &[u64]) -> HashMap<Key, Vec<usize>> {
    let mut out = HashMap::new();
    let mut gaps: Vec<f64> = b
        .pairs
        .iter()
        .filter(|p| p.2 <= bx && p.3 <= by)
        .map(|p| p.3)
        .collect();
    gaps.sort_by(|a, c| a.total_cmp(c));
    gaps.dedup();
    for &y in &gaps {
        let mut ps: Vec<&(usize, usize, f64, f64)> =
            b.pairs.iter().filter(|p| p.2 <= bx && p.3 <= y).collect();
        ps.sort_by(|p, q| p.2.total_cmp(&q.2));
        let mut comp: Vec<usize> = (0..b.n).collect();
        let mut members: Vec<Vec<usize>> = (0..b.n).map(|v| vec![v]).collect();
        let mut k = 0;
        while k < ps.len() {
            let d = ps[k].2;
            let mut changed = Vec::new();
            while k < ps.len() && ps[k].2 == d {
                let (i, j) = (comp[ps[k].0], comp[ps[k].1]);
                k += 1;
                if i != j {
                    let (big, small) = if members[i].len() >= members[j].len() {
                        (i, j)
                    } else {
                        (j, i)
                    };
                    let moved = std::mem::take(&mut members[small]);
                    for &v in &moved {
                        comp[v] = big;
                    }
                    members[big].extend(moved);
                    changed.push(big);
                }
            }
            for c in changed {
                let c = comp[members[c].first().copied().unwrap_or(c)];
                let mut m = members[c].clone();
                m.sort_unstable();
                out.insert(key_of(&m, tg), m);
            }
        }
    }
    out
}

fn present(b: &Brute, x: f64, y: f64, tg: &[u64]) -> HashSet<Key> {
    let lab = b.labels(x, y);
    let mut sets: HashMap<usize, Vec<usize>> = HashMap::new();
    for v in 0..b.n {
        sets.entry(lab[v]).or_default().push(v);
    }
    sets.into_values()
        .filter(|m| m.len() >= 2)
        .map(|m| key_of(&m, tg))
        .collect()
}

#[test]
fn frontier_matches_brute_force() {
    for (seed, n, la, lo) in engine_cases() {
        let (t, lat, lon) = strikes(seed, n, la, lo);
        let v = view_of(&t, la, lo);
        let ids: Vec<usize> = (0..n)
            .filter(|&i| in_view(v, t[i], lat[i], lon[i]))
            .collect();
        let b = Brute::new(&t, &lat, &lon);
        let mut e = Engine::new(Graph::new(t.clone(), lat.clone(), lon.clone()));
        let view = e.set_view(v.0, v.1, v.2, v.3, v.4, v.5);
        assert_eq!(
            view.strikes.iter().map(|&s| s as usize).collect::<Vec<_>>(),
            ids,
            "seed {seed}: view strikes"
        );
        // Axis floors: the 1st percentile of the edges' positive distances, the 5th of
        // their gaps.
        let q = |mut v: Vec<f64>, q: f64| {
            v.retain(|&x| x > 0.0);
            v.sort_by(|a, b| a.total_cmp(b));
            v[(q * (v.len() - 1) as f64) as usize]
        };
        let min = minimal(&b, &t);
        let pos_d = q(min.iter().map(|e| e.2).collect(), 0.01);
        let pos_g = q(min.iter().map(|e| t[e.1 as usize] - t[e.0 as usize]).collect(), 0.05);
        assert_eq!(e.x_min, pos_d, "seed {seed}: distance floor");
        assert_eq!(e.y_min, pos_g, "seed {seed}: gap floor");
        // At every gap, the least distance connecting all the strikes.
        for &y in &b.gaps(f64::INFINITY) {
            let mut ps: Vec<f64> = b.pairs.iter().filter(|p| p.3 <= y).map(|p| p.2).collect();
            ps.sort_by(|a, c| a.total_cmp(c));
            let conn = ps
                .iter()
                .copied()
                .find(|&x| b.labels(x, y).iter().all(|&l| l == 0));
            let got = e.frontier.iter().rev().find(|f| f.0 <= y).map(|f| f.1);
            assert_eq!(got, conn, "seed {seed}: frontier at gap {y}");
        }
        eprintln!("seed {seed}: frontier {:?}", e.frontier);
    }
}

#[test]
fn clusters_and_regions_match_brute_force() {
    for (seed, n, la, lo) in engine_cases() {
        let (t, lat, lon) = strikes(seed, n, la, lo);
        let v = view_of(&t, la, lo);
        let b = Brute::new(&t, &lat, &lon);
        let tg = tags(n);
        let mut e = Engine::new(Graph::new(t.clone(), lat.clone(), lon.clone()));
        e.set_view(v.0, v.1, v.2, v.3, v.4, v.5);
        let in_v: Vec<bool> = (0..n).map(|i| in_view(v, t[i], lat[i], lon[i])).collect();
        let (bx, by) = e.bx;
        let mut r = Rng(seed + 5);
        let (dx, gy) = (b.dists(bx), b.gaps(by));
        let (xs, ys) = (sample(&dx, 12, &mut r), sample(&gy, 12, &mut r));
        for &x in &xs {
            for &y in &ys {
                let lab = b.labels(x, y);
                let touching: HashSet<usize> =
                    (0..n).filter(|&i| in_v[i]).map(|i| lab[i]).collect();
                let mut want: Vec<Vec<usize>> = Vec::new();
                let mut singles = 0;
                for &c in &touching {
                    let m: Vec<usize> = (0..n).filter(|&i| lab[i] == c).collect();
                    if m.len() >= 2 {
                        want.push(m)
                    } else {
                        singles += 1
                    }
                }
                let sel = e.select(x, y);
                let got: HashSet<Key> = sel
                    .clusters
                    .iter()
                    .map(|c| {
                        key_of(
                            &(0..n)
                                .filter(|&i| sel.label[i] == c.label)
                                .collect::<Vec<_>>(),
                            &tg,
                        )
                    })
                    .collect();
                let want_keys: HashSet<Key> = want.iter().map(|m| key_of(m, &tg)).collect();
                assert_eq!(
                    got, want_keys,
                    "seed {seed}: clusters touching the view at ({x}, {y})"
                );
                assert_eq!(sel.singletons, singles, "seed {seed}: lone strikes in view");
                for i in 0..n {
                    assert_eq!(
                        sel.label[i] != u32::MAX,
                        touching.contains(&lab[i]),
                        "seed {seed}: strike {i} labelled"
                    );
                }
            }
        }
        // Every cluster's region (computed on demand) is exact over the box.
        let (xs, ys) = (sample(&dx, 25, &mut r), sample(&gy, 25, &mut r));
        let mut regions: Vec<(Key, Vec<f64>)> = Vec::new();
        for _ in 0..4 {
            let (x, y) = (
                dx[(r.f() * dx.len() as f64) as usize],
                gy[(r.f() * gy.len() as f64) as usize],
            );
            let sel = e.select(x, y);
            let picks: Vec<(u32, Vec<usize>)> = sel
                .clusters
                .iter()
                .take(6)
                .map(|c| {
                    (
                        c.label,
                        sel.members[c.label as usize]
                            .iter()
                            .map(|&v| v as usize)
                            .collect(),
                    )
                })
                .collect();
            for (label, m) in picks {
                regions.push((key_of(&m, &tg), e.region_of(label)));
            }
        }
        for &x in &xs {
            for &y in &ys {
                let p = present(&b, x, y, &tg);
                for (key, rows) in &regions {
                    assert_eq!(
                        in_rows(rows, x, y / 60.0),
                        p.contains(key),
                        "seed {seed}: region of {} at ({x}, {y}): {rows:?}",
                        key.1
                    );
                }
            }
        }
        eprintln!(
            "seed {seed}: {} regions checked, box ({bx:.1} km, {:.1} min)",
            regions.len(),
            by / 60.0
        );
    }
}

#[test]
fn similar_matches_brute_force() {
    for (seed, n, la, lo) in [
        (1, 40, 35.0, -60.0),
        (3, 40, 85.0, 40.0),
        (4, 45, -20.0, 179.9),
        (101, 45, -50.0, -120.0),
        (7, 50, 40.0, 10.0),
    ] {
        let (t, lat, lon) = strikes(seed, n, la, lo);
        let v = view_of(&t, la, lo);
        let b = Brute::new(&t, &lat, &lon);
        let tg = tags(n);
        let mut e = Engine::new(Graph::new(t.clone(), lat.clone(), lon.clone()));
        e.set_view(v.0, v.1, v.2, v.3, v.4, v.5);
        let (bx, by) = e.bx;
        let all = all_clusters(&b, bx, by, &tg);
        let (dx, gy) = (b.dists(bx), b.gaps(by));
        let mut r = Rng(seed + 3);
        let mut checked = 0;
        for _ in 0..6 {
            let (x, y) = (
                dx[(r.f() * dx.len() as f64) as usize],
                gy[(r.f() * gy.len() as f64) as usize],
            );
            let sel = e.select(x, y);
            let picks: Vec<(u32, Vec<usize>)> = sel
                .clusters
                .iter()
                .take(3)
                .map(|c| {
                    (
                        c.label,
                        sel.members[c.label as usize]
                            .iter()
                            .map(|&v| v as usize)
                            .collect(),
                    )
                })
                .collect();
            for (label, cm) in picks {
                let csize = cm.len() as u32;
                let cset: HashSet<usize> = cm.iter().copied().collect();
                let change = |m: &[usize]| -> Option<f64> {
                    let o = m.iter().filter(|v| cset.contains(v)).count() as u32;
                    (o > 0).then(|| (csize + m.len() as u32 - 2 * o) as f64 / o as f64)
                };
                for tol in [0.0, 0.25, 0.6, 0.95] {
                    let s = e.similar(label, tol, true, |_| {});
                    // The region: wherever some cluster is within the tolerance.
                    for _ in 0..60 {
                        let (x, y) = (
                            dx[(r.f() * dx.len() as f64) as usize],
                            gy[(r.f() * gy.len() as f64) as usize],
                        );
                        let lab = b.labels(x, y);
                        let mut sets: HashMap<usize, Vec<usize>> = HashMap::new();
                        for v in 0..n {
                            sets.entry(lab[v]).or_default().push(v);
                        }
                        let want = sets
                            .values()
                            .any(|m| m.len() >= 2 && change(m).is_some_and(|c| c <= tol + 1e-12));
                        assert_eq!(
                            in_rows(&s.rows, x, y / 60.0),
                            want,
                            "seed {seed}: cluster of {csize}, tol {tol}, at ({x}, {y})"
                        );
                    }
                    // Union and best similarity, against every similar cluster in the box.
                    let mut best = vec![0.0f64; n];
                    for m in all.values() {
                        if let Some(c) = change(m).filter(|&c| c <= tol + 1e-12) {
                            let o = m.iter().filter(|v| cset.contains(v)).count() as f64;
                            let jac = o / (csize as f64 + m.len() as f64 - o);
                            let _ = c;
                            for &v in m {
                                best[v] = best[v].max(jac);
                            }
                        }
                    }
                    let want_union: Vec<u32> =
                        (0..n as u32).filter(|&v| best[v as usize] > 0.0).collect();
                    assert_eq!(s.union, want_union, "seed {seed}: union");
                    for (k, &v) in s.union.iter().enumerate() {
                        assert!(
                            (s.best[k] - best[v as usize]).abs() < 1e-12,
                            "seed {seed}: best"
                        );
                    }
                    checked += 1;
                }
            }
        }
        eprintln!("seed {seed}: {checked} similar queries");
    }
}

/// The graph is shared and grown as queries need it: results must not depend on what was
/// computed before.
#[test]
fn results_do_not_depend_on_history() {
    for (seed, n, la, lo) in engine_cases() {
        let (t, lat, lon) = strikes(seed, n, la, lo);
        let span = t[n - 1];
        let v = view_of(&t, la, lo);
        let mut fresh = Engine::new(Graph::new(t.clone(), lat.clone(), lon.clone()));
        let mut used = Engine::new(Graph::new(t.clone(), lat.clone(), lon.clone()));
        // Use one engine heavily first: other views, clicks, regions, similar regions.
        let mut r = Rng(seed + 17);
        for (a, z) in [(0.0, 0.3), (0.5, 1.0), (0.0, 1.0)] {
            used.set_view(a * span, z * span, -90.0, 90.0, -180.0, 180.0);
            let (bx, by) = used.bx;
            for _ in 0..3 {
                let sel = used.select(bx * r.f() * 0.5, by * r.f() * 0.5);
                if let Some(c) = sel.clusters.first().map(|c| c.label) {
                    used.region_of(c);
                    used.similar(c, 0.3, false, |_| {});
                }
            }
        }
        for e in [&mut fresh, &mut used] {
            e.set_view(v.0, v.1, v.2, v.3, v.4, v.5);
        }
        let (bx, by) = fresh.bx;
        for _ in 0..5 {
            let (x, y) = (bx * r.f() * 0.6, by * r.f() * 0.6);
            let labels: Vec<u32> = fresh
                .select(x, y)
                .clusters
                .iter()
                .take(4)
                .map(|c| c.label)
                .collect();
            used.select(x, y);
            for l in labels {
                assert_eq!(fresh.region_of(l), used.region_of(l), "seed {seed}: region");
                let (a, b) = (
                    fresh.similar(l, 0.3, false, |_| {}),
                    used.similar(l, 0.3, false, |_| {}),
                );
                assert_eq!(a.rows, b.rows, "seed {seed}: similar region");
            }
        }
    }
}
