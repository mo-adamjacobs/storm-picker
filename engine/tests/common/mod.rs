//! Shared test helpers: random strikes and brute force over every pair (exact WGS84).
#![allow(dead_code)]

use std::collections::HashMap;

use storm_engine::geo::Geo;
use storm_engine::tree::Tree;

pub struct Rng(pub u64);
impl Rng {
    pub fn f(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Clumpy random strikes around (lat0, lon0), with some repeated places and times.
/// Seeds of 100 or more scatter strikes uniformly instead (sparser: fewer alternative paths).
pub fn strikes(seed: u64, n: usize, lat0: f64, lon0: f64) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let mut r = Rng(seed * 2654435761 + 1);
    let k = if seed >= 100 { n } else { 4 };
    let centres: Vec<(f64, f64, f64)> = (0..k)
        .map(|_| (r.f() - 0.5, r.f() - 0.5, r.f() * 3600.0))
        .collect();
    let mut pts: Vec<(f64, f64, f64)> = Vec::new();
    for i in 0..n {
        if i > 0 && r.f() < 0.08 {
            let (a, b, c) = pts[(r.f() * pts.len() as f64) as usize];
            // Same place, or same time.
            pts.push(if r.f() < 0.5 {
                (a, b, c + r.f() * 300.0)
            } else {
                (a + 0.1 * (r.f() - 0.5), b, c)
            });
            continue;
        }
        let (ca, co, ct) = centres[(r.f() * k as f64) as usize];
        pts.push((
            lat0 + ca + 0.3 * (r.f() - 0.5),
            lon0 + co + 0.3 * (r.f() - 0.5),
            ct + 900.0 * (r.f() - 0.5),
        ));
    }
    pts.sort_by(|p, q| p.2.total_cmp(&q.2));
    let t0 = pts[0].2;
    (
        pts.iter().map(|p| p.2 - t0).collect(),
        pts.iter().map(|p| p.0).collect(),
        pts.iter().map(|p| p.1).collect(),
    )
}

pub struct Brute {
    pub n: usize,
    /// (i, j, km, gap s), i < j
    pub pairs: Vec<(usize, usize, f64, f64)>,
}

impl Brute {
    pub fn new(t: &[f64], lat: &[f64], lon: &[f64]) -> Self {
        let geo = Geo::default();
        let n = t.len();
        let mut pairs = Vec::new();
        for i in 0..n {
            for j in i + 1..n {
                pairs.push((i, j, geo.km(lat[i], lon[i], lat[j], lon[j]), t[j] - t[i]));
            }
        }
        Brute { n, pairs }
    }

    /// Canonical component labels (smallest member) at (x km, y s).
    pub fn labels(&self, x: f64, y: f64) -> Vec<usize> {
        let mut uf: Vec<usize> = (0..self.n).collect();
        fn find(uf: &mut [usize], mut v: usize) -> usize {
            while uf[v] != v {
                uf[v] = uf[uf[v]];
                v = uf[v];
            }
            v
        }
        for &(i, j, d, g) in &self.pairs {
            if d <= x && g <= y {
                let (a, b) = (find(&mut uf, i), find(&mut uf, j));
                if a != b {
                    uf[a.max(b)] = a.min(b);
                }
            }
        }
        (0..self.n).map(|v| find(&mut uf, v)).collect()
    }

    pub fn dists(&self, max: f64) -> Vec<f64> {
        let mut v: Vec<f64> = self
            .pairs
            .iter()
            .map(|p| p.2)
            .filter(|&d| d <= max)
            .collect();
        v.push(0.0);
        v.sort_by(|a, b| a.total_cmp(b));
        v.dedup();
        v
    }

    pub fn gaps(&self, max: f64) -> Vec<f64> {
        let mut v: Vec<f64> = self
            .pairs
            .iter()
            .map(|p| p.3)
            .filter(|&g| g <= max)
            .collect();
        v.push(0.0);
        v.sort_by(|a, b| a.total_cmp(b));
        v.dedup();
        v
    }
}

pub fn canon(label: &[u32]) -> Vec<usize> {
    let mut first: HashMap<u32, usize> = HashMap::new();
    label
        .iter()
        .enumerate()
        .map(|(i, &l)| *first.entry(l).or_insert(i))
        .collect()
}

/// A few exact edge values plus values in between.
pub fn sample(v: &[f64], k: usize, r: &mut Rng) -> Vec<f64> {
    let mut out: Vec<f64> = (0..k)
        .map(|_| v[(r.f() * v.len() as f64) as usize])
        .collect();
    out.extend((0..k).map(|_| r.f() * v[v.len() - 1]));
    out.push(v[v.len() - 1]);
    out
}

pub fn in_rows(rows: &[f64], x: f64, y_min: f64) -> bool {
    rows.chunks(4)
        .any(|r| r[0] <= y_min && y_min < r[1] && r[2] <= x && x < r[3])
}

pub const BOX: (f64, f64) = (40.0, 1800.0);

pub fn cases() -> Vec<(u64, usize, f64, f64)> {
    // (seed, strikes, lat, lon): mid-latitudes, near a pole, across the antimeridian.
    vec![
        (1, 60, 35.0, -60.0),
        (2, 80, 10.0, 20.0),
        (3, 50, 85.0, 40.0),
        (4, 70, -20.0, 179.9),
        (5, 90, 50.0, 0.0),
    ]
}

/// The minimal set by definition: all pairs in (gap, distance, a, b) order, each kept
/// unless the kept pairs before it already join its strikes at its distance.
pub fn minimal(b: &Brute, t: &[f64]) -> Vec<(u32, u32, f64)> {
    let mut ps: Vec<(f64, f64, usize, usize)> =
        b.pairs.iter().map(|p| (t[p.1] - t[p.0], p.2, p.0, p.1)).collect();
    ps.sort_by(|p, q| {
        p.0.total_cmp(&q.0)
            .then(p.1.total_cmp(&q.1))
            .then(p.2.cmp(&q.2))
            .then(p.3.cmp(&q.3))
    });
    let mut tree = Tree::new(b.n, f64::INFINITY, None);
    ps.into_iter()
        .filter(|&(_, d, i, j)| tree.insert(i, j, d))
        .map(|(_, d, i, j)| (i as u32, j as u32, d))
        .collect()
}

