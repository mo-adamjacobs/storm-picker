//! Distances between strikes.
//!
//! Every distance that decides a result (edge lengths, thresholds, which edges are kept)
//! is the WGS84 geodesic distance, from GeographicLib (Karney's algorithm).
//!
//! The unit sphere (same latitude / longitude) is only used for cheap *bounds*, to skip
//! work that provably cannot matter. With geodetic coordinates the ellipsoid's metric is
//! ds² = M(φ)² dφ² + (N(φ) cos φ)² dλ², the unit sphere's is dθ² = dφ² + cos²φ dλ², so
//! their ratio lies between the smallest and largest radius of curvature:
//! a(1 - e²) = 6335.439 km (meridional, at the equator) and a / √(1 - e²) = 6399.594 km
//! (at the poles). Hence for central angle θ: 6335.439 θ ≤ geodesic ≤ 6399.594 θ.

use geographiclib_rs::{Geodesic, InverseGeodesic};

/// Below a(1 - e²) for WGS84, with margin.
const LO_KM: f64 = 6335.0;
/// Above a / sqrt(1 - e²) for WGS84, with margin.
const HI_KM: f64 = 6400.0;

pub struct Geo {
    g: Geodesic,
}

impl Default for Geo {
    fn default() -> Self {
        Geo {
            g: Geodesic::wgs84(),
        }
    }
}

impl Geo {
    /// WGS84 geodesic distance in km.
    pub fn km(&self, lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
        let s: f64 = self.g.inverse(lat1, lon1, lat2, lon2);
        s / 1000.0
    }
}

pub fn unit(lat: f64, lon: f64) -> [f64; 3] {
    let (la, lo) = (lat.to_radians(), lon.to_radians());
    [la.cos() * lo.cos(), la.cos() * lo.sin(), la.sin()]
}

pub fn chord(p: &[f64; 3], q: &[f64; 3]) -> f64 {
    let (a, b, c) = (p[0] - q[0], p[1] - q[1], p[2] - q[2]);
    (a * a + b * b + c * c).sqrt()
}

/// Central angle for a chord on the unit sphere.
pub fn angle(chord: f64) -> f64 {
    2.0 * (chord * 0.5).min(1.0).asin()
}

/// Lower bound on the geodesic (km) for central angle θ (radians).
pub fn lower(theta: f64) -> f64 {
    (theta * LO_KM * (1.0 - 1e-9) - 1e-9).max(0.0)
}

/// Upper bound on the geodesic (km) for central angle θ (radians).
pub fn upper(theta: f64) -> f64 {
    theta * HI_KM * (1.0 + 1e-9) + 1e-9
}

pub fn chord2(p: &[f64; 3], q: &[f64; 3]) -> f64 {
    let (a, b, c) = (p[0] - q[0], p[1] - q[1], p[2] - q[2]);
    a * a + b * b + c * c
}

/// Squared chord c² beyond which lower(θ) > km is certain (∞ if never).
pub fn chord2_beyond(km: f64) -> f64 {
    let th = (km + 1e-9) / (LO_KM * (1.0 - 1e-9));
    if th >= std::f64::consts::PI {
        return f64::INFINITY;
    }
    let c = 2.0 * (th * 0.5).sin() * (1.0 + 1e-12);
    c * c
}

/// Squared chord c² up to which upper(θ) ≤ km is certain (negative if never).
pub fn chord2_within(km: f64) -> f64 {
    let th = (km - 1e-9) / (HI_KM * (1.0 + 1e-9));
    if th <= 0.0 {
        return -1.0;
    }
    let c = 2.0 * (th.min(std::f64::consts::PI) * 0.5).sin() * (1.0 - 1e-12);
    c * c
}

/// Chord thresholds relative to a reference pair (a, b) with chord `cab` and angle `th`,
/// for a third strike c: by convexity of the angle in the chord, |cb| ≤ |ab| implies
/// θcb ≤ θab · |cb| / |ab|, and |cb| ≥ |ab| implies θcb ≥ θab · |cb| / |ab|.
/// Returns squared chords (sure, never): if chord²(c, b) ≤ sure then surely
/// geodesic(c, b) ≤ km; if chord²(c, b) > never then surely geodesic(c, b) > km.
pub fn relative_chord2(cab2: f64, th: f64, km: f64) -> (f64, f64) {
    if th <= 0.0 {
        return (-1.0, f64::INFINITY);
    }
    let sure = ((km - 1e-9) / (th * HI_KM * (1.0 + 1e-9))).min(1.0) * (1.0 - 1e-12);
    let never = ((km + 1e-9) / (th * LO_KM * (1.0 - 1e-9))).max(1.0) * (1.0 + 1e-12);
    (
        if sure > 0.0 { sure * sure * cab2 } else { -1.0 },
        never * never * cab2,
    )
}

/// Smallest and largest squared chord from p to any point of the axis-aligned box
/// [lo, hi].
pub fn box_chords2(p: &[f64; 3], lo: &[f64; 3], hi: &[f64; 3]) -> (f64, f64) {
    let (mut near, mut far) = (0.0, 0.0);
    for k in 0..3 {
        let d = if p[k] < lo[k] {
            lo[k] - p[k]
        } else if p[k] > hi[k] {
            p[k] - hi[k]
        } else {
            0.0
        };
        near += d * d;
        let f = (p[k] - lo[k]).abs().max((p[k] - hi[k]).abs());
        far += f * f;
    }
    (near, far)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sphere_bounds_hold() {
        let geo = Geo::default();
        let mut s = 12345u64;
        let mut rnd = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f64 / (1u64 << 53) as f64
        };
        for _ in 0..20000 {
            let (la1, lo1) = (rnd() * 178.0 - 89.0, rnd() * 360.0 - 180.0);
            let scale = [0.001, 0.1, 1.0, 10.0][(rnd() * 4.0) as usize];
            let la2 = (la1 + (rnd() - 0.5) * scale).clamp(-89.9, 89.9);
            let lo2 = lo1 + (rnd() - 0.5) * scale;
            let d = geo.km(la1, lo1, la2, lo2);
            let th = angle(chord(&unit(la1, lo1), &unit(la2, lo2)));
            assert!(
                lower(th) <= d && d <= upper(th),
                "{la1} {lo1} {la2} {lo2}: {} {d} {}",
                lower(th),
                upper(th)
            );
            // The chord forms of the same bounds.
            let c2 = chord2(&unit(la1, lo1), &unit(la2, lo2));
            let k = d * (0.5 + rnd());
            if c2 > chord2_beyond(k) {
                assert!(d > k);
            }
            if c2 <= chord2_within(k) {
                assert!(d <= k);
            }
            // Relative to a reference pair from the first point.
            let la3 = (la1 + (rnd() - 0.5) * scale).clamp(-89.9, 89.9);
            let lo3 = lo1 + (rnd() - 0.5) * scale;
            let (p1, p3) = (unit(la1, lo1), unit(la3, lo3));
            let (cab2, th3) = (chord2(&p1, &p3), angle(chord(&p1, &p3)));
            let (sure, never) = relative_chord2(cab2, th3, k);
            let c2 = chord2(&unit(la2, lo2), &p3);
            let d23 = geo.km(la2, lo2, la3, lo3);
            if c2 <= sure {
                assert!(d23 <= k, "sure");
            }
            if c2 > never {
                assert!(d23 > k, "never");
            }
        }
    }
}
