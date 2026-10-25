//! 2D geometry predicates for schematic connectivity.
//!
//! Host-free and pure, so the rules that decide "does this wire cross that one"
//! and "where does a wire number sit" are testable without a CAD host.
//!
//! The distinction the whole gap feature rests on is **crossing versus
//! touching**. Two wires meeting at an endpoint are *connected* — a junction.
//! Two wires whose bodies intersect in their interiors are *crossing without
//! connecting*, which is what a gap renders. Conflating them would either gap
//! real junctions or connect real crossings, so [`proper_crossing`] excludes
//! endpoint touches and collinear overlap by construction.

use std::f64::consts::PI;

/// A 2D point, `[x, y]`.
pub type Pt = [f64; 2];

/// Tolerance for treating a coordinate as zero, in mm.
pub const EPS: f64 = 1e-9;

/// Snap tolerance for [`snap_to_ortho`], in degrees.
pub const ORTHO_TOL_DEG: f64 = 5.0;

pub fn sub(a: Pt, b: Pt) -> Pt {
    [a[0] - b[0], a[1] - b[1]]
}

pub fn add(a: Pt, b: Pt) -> Pt {
    [a[0] + b[0], a[1] + b[1]]
}

pub fn scale(a: Pt, k: f64) -> Pt {
    [a[0] * k, a[1] * k]
}

/// The 2D cross product's z component — the sign tells which side of `a` the
/// point `b` lies on.
pub fn cross(a: Pt, b: Pt) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

pub fn dot(a: Pt, b: Pt) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

pub fn dist(a: Pt, b: Pt) -> f64 {
    let d = sub(a, b);
    (d[0] * d[0] + d[1] * d[1]).sqrt()
}

pub fn dist_sq(a: Pt, b: Pt) -> f64 {
    let d = sub(a, b);
    d[0] * d[0] + d[1] * d[1]
}

/// Distance from `p` to the *segment* `a`–`b` (not the infinite line).
pub fn dist_point_to_segment(p: Pt, a: Pt, b: Pt) -> f64 {
    let ab = sub(b, a);
    let len_sq = dot(ab, ab);
    if len_sq < EPS {
        return dist(p, a); // degenerate segment
    }
    let t = (dot(sub(p, a), ab) / len_sq).clamp(0.0, 1.0);
    dist(p, add(a, scale(ab, t)))
}

/// Is `p` on the segment `a`–`b`, within `tol`?
pub fn point_on_segment(p: Pt, a: Pt, b: Pt, tol: f64) -> bool {
    dist_point_to_segment(p, a, b) <= tol
}

/// Where two segments meet, if they do at all.
///
/// Returns a point for a proper crossing, an endpoint touch, or a collinear
/// overlap; `None` only for parallel non-collinear segments. Use
/// [`proper_crossing`] when the question is specifically about a crossing that
/// should be gapped.
pub fn segment_intersection(a1: Pt, a2: Pt, b1: Pt, b2: Pt) -> Option<Pt> {
    let r = sub(a2, a1);
    let s = sub(b2, b1);
    let denom = cross(r, s);
    let qp = sub(b1, a1);

    if denom.abs() < EPS {
        // Parallel. Collinear overlap is a shared path, not a meeting point;
        // report its midpoint so callers still see "they interact".
        if cross(qp, r).abs() > EPS {
            return None;
        }
        let rr = dot(r, r);
        if rr < EPS {
            // `a` is degenerate; fall back to testing the single point.
            return point_on_segment(a1, b1, b2, 1e-6).then_some(a1);
        }
        let t0 = dot(qp, r) / rr;
        let t1 = t0 + dot(s, r) / rr;
        let (lo, hi) = if t0 <= t1 { (t0, t1) } else { (t1, t0) };
        let lo = lo.max(0.0);
        let hi = hi.min(1.0);
        if lo > hi {
            return None; // collinear but disjoint
        }
        let mid = (lo + hi) / 2.0;
        return Some(add(a1, scale(r, mid)));
    }

    let t = cross(qp, s) / denom;
    let u = cross(qp, r) / denom;
    if (-EPS..=1.0 + EPS).contains(&t) && (-EPS..=1.0 + EPS).contains(&u) {
        Some(add(a1, scale(r, t)))
    } else {
        None
    }
}

/// The point where two segments cross *through* each other, or `None`.
///
/// Excludes anything that is really a junction or a shared path:
/// - a touch at either segment's endpoint (a T-junction or a corner),
/// - collinear overlap (two wires drawn along the same line),
/// - a degenerate zero-length segment.
pub fn proper_crossing(a1: Pt, a2: Pt, b1: Pt, b2: Pt) -> Option<Pt> {
    let r = sub(a2, a1);
    let s = sub(b2, b1);
    if dot(r, r) < EPS || dot(s, s) < EPS {
        return None; // a zero-length "segment" has no interior
    }
    let denom = cross(r, s);
    if denom.abs() < EPS {
        return None; // parallel: collinear overlap is not a crossing
    }
    let qp = sub(b1, a1);
    let t = cross(qp, s) / denom;
    let u = cross(qp, r) / denom;

    // `t`/`u` must be strictly interior to both segments.
    let t_in = t > EPS && t < 1.0 - EPS;
    let u_in = u > EPS && u < 1.0 - EPS;
    if t_in && u_in {
        Some(add(a1, scale(r, t)))
    } else {
        None
    }
}

/// Every point where polyline `a` properly crosses polyline `b`, in order along
/// `a`.
pub fn polyline_crossings(a: &[Pt], b: &[Pt]) -> Vec<Pt> {
    let mut out = Vec::new();
    if a.len() < 2 || b.len() < 2 {
        return out;
    }
    for i in 0..a.len() - 1 {
        for j in 0..b.len() - 1 {
            if let Some(p) = proper_crossing(a[i], a[i + 1], b[j], b[j + 1]) {
                out.push(p);
            }
        }
    }
    out
}

/// Do two polylines share an endpoint (within `tol`)? That is a junction, and
/// it is the condition under which two wires *are* connected.
pub fn polylines_touch_at_endpoint(a: &[Pt], b: &[Pt], tol: f64) -> bool {
    let ends = |p: &[Pt]| -> Vec<Pt> {
        match (p.first(), p.last()) {
            (Some(f), Some(l)) => vec![*f, *l],
            _ => vec![],
        }
    };
    let (ae, be) = (ends(a), ends(b));
    if ae.len() < 2 || be.len() < 2 {
        return false;
    }
    // A shared *corner* counts too: an interior vertex of one may sit on the
    // other's endpoint.
    let any_of = |pts: &[Pt], targets: &[Pt]| -> bool {
        pts.iter()
            .any(|p| targets.iter().any(|t| dist(*p, *t) <= tol))
    };
    any_of(&ae, &be) || any_of(&be, &ae)
}

pub fn polyline_length(pts: &[Pt]) -> f64 {
    pts.windows(2).map(|w| dist(w[0], w[1])).sum()
}

/// The point a fraction `t` of the way along a polyline *by arc length*.
///
/// Used for placing a wire number at the wire's visual middle, which is why it
/// is arc-length and not vertex-index based: a wire with a long first segment
/// and a short second would otherwise get a label near its corner.
pub fn polyline_point_at(pts: &[Pt], t: f64) -> Option<Pt> {
    if pts.is_empty() {
        return None;
    }
    if pts.len() == 1 {
        return Some(pts[0]);
    }
    let total = polyline_length(pts);
    if total < EPS {
        return Some(pts[0]);
    }
    let target = (t.clamp(0.0, 1.0)) * total;
    let mut walked = 0.0;
    for w in pts.windows(2) {
        let seg = dist(w[0], w[1]);
        if walked + seg >= target {
            let local = if seg < EPS { 0.0 } else { (target - walked) / seg };
            return Some(add(w[0], scale(sub(w[1], w[0]), local)));
        }
        walked += seg;
    }
    pts.last().copied()
}

/// The visual middle of a polyline.
pub fn polyline_midpoint(pts: &[Pt]) -> Option<Pt> {
    polyline_point_at(pts, 0.5)
}

/// `[min_x, min_y, max_x, max_y]`.
pub fn bbox(pts: &[Pt]) -> [f64; 4] {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in pts {
        b[0] = b[0].min(p[0]);
        b[1] = b[1].min(p[1]);
        b[2] = b[2].max(p[0]);
        b[3] = b[3].max(p[1]);
    }
    b
}

pub fn bbox_overlap(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

/// Snap the second point onto the dominant axis of the first, when within
/// `tol_deg` of it — the behaviour that keeps a hand-drawn schematic orthogonal
/// without forbidding an angled run.
pub fn snap_to_ortho(from: Pt, to: Pt, tol_deg: f64) -> Pt {
    let d = sub(to, from);
    if dist_sq(from, to) < EPS {
        return to;
    }
    // Fold the direction into 0..180 first: a line pointing west is the same
    // axis as one pointing east, so the axis test must not depend on which way
    // the user happened to drag.
    let a = normalize_deg(d[1].atan2(d[0]).to_degrees()).abs();
    let to_horizontal = a.min(180.0 - a);
    let to_vertical = (a - 90.0).abs();

    if to_horizontal <= tol_deg && to_horizontal <= to_vertical {
        [to[0], from[1]]
    } else if to_vertical <= tol_deg {
        [from[0], to[1]]
    } else {
        to
    }
}

/// The compass direction of a segment, for a report or a port name.
pub fn direction_of(from: Pt, to: Pt) -> &'static str {
    let d = sub(to, from);
    if d[0].abs() < 1e-6 && d[1].abs() < 1e-6 {
        return "zero";
    }
    let deg = d[1].atan2(d[0]).to_degrees();
    let octant = ((deg + 22.5).rem_euclid(360.0) / 45.0) as i32 % 8;
    match octant {
        0 => "east",
        1 => "north-east",
        2 => "north",
        3 => "north-west",
        4 => "west",
        5 => "south-west",
        6 => "south",
        _ => "south-east",
    }
}

/// The angle of a segment in degrees, in `(-180, 180]`.
pub fn angle_deg(from: Pt, to: Pt) -> f64 {
    let d = sub(to, from);
    d[1].atan2(d[0]).to_degrees()
}

/// Normalise an angle into `(-180, 180]`.
pub fn normalize_deg(deg: f64) -> f64 {
    let mut d = deg % 360.0;
    if d <= -180.0 {
        d += 360.0;
    } else if d > 180.0 {
        d -= 360.0;
    }
    d
}

/// Radians to degrees, for callers that only have the former.
pub fn to_deg(rad: f64) -> f64 {
    rad * 180.0 / PI
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proper_crossing_finds_an_x() {
        // The classic case a gap exists for.
        let p = proper_crossing([0.0, 0.0], [10.0, 0.0], [5.0, -5.0], [5.0, 5.0]).unwrap();
        assert!((p[0] - 5.0).abs() < 1e-9);
        assert!(p[1].abs() < 1e-9);
    }

    #[test]
    fn endpoint_touches_are_junctions_not_crossings() {
        // A T-junction: `b` ends on `a`'s interior. Wires here are connected,
        // so this must NOT be gapped.
        assert!(proper_crossing([0.0, 0.0], [10.0, 0.0], [5.0, 0.0], [5.0, 5.0]).is_none());
        // A corner: shared endpoints.
        assert!(proper_crossing([0.0, 0.0], [10.0, 0.0], [10.0, 0.0], [10.0, 10.0]).is_none());
    }

    #[test]
    fn collinear_overlap_is_not_a_crossing() {
        assert!(proper_crossing([0.0, 0.0], [10.0, 0.0], [5.0, 0.0], [15.0, 0.0]).is_none());
    }

    #[test]
    fn parallel_segments_never_cross() {
        assert!(proper_crossing([0.0, 0.0], [10.0, 0.0], [0.0, 5.0], [10.0, 5.0]).is_none());
    }

    #[test]
    fn a_degenerate_segment_has_no_interior() {
        assert!(proper_crossing([5.0, 0.0], [5.0, 0.0], [0.0, 0.0], [10.0, 10.0]).is_none());
    }

    #[test]
    fn segments_that_miss_do_not_cross() {
        assert!(proper_crossing([0.0, 0.0], [1.0, 0.0], [5.0, -5.0], [5.0, 5.0]).is_none());
    }

    #[test]
    fn segment_intersection_reports_endpoint_and_collinear_meetings() {
        // Unlike `proper_crossing`, this one does report a touch.
        assert!(segment_intersection([0.0, 0.0], [10.0, 0.0], [10.0, 0.0], [10.0, 5.0]).is_some());
        assert!(segment_intersection([0.0, 0.0], [10.0, 0.0], [5.0, 0.0], [15.0, 0.0]).is_some());
        assert!(segment_intersection([0.0, 0.0], [10.0, 0.0], [0.0, 5.0], [10.0, 5.0]).is_none());
        // Collinear but disjoint.
        assert!(segment_intersection([0.0, 0.0], [1.0, 0.0], [5.0, 0.0], [6.0, 0.0]).is_none());
    }

    #[test]
    fn polyline_crossings_are_ordered_along_the_first() {
        // A horizontal run crossing two verticals, left to right.
        let horiz = [[0.0, 0.0], [30.0, 0.0]];
        let v1 = [[10.0, -5.0], [10.0, 5.0]];
        let v2 = [[20.0, -5.0], [20.0, 5.0]];
        let mut all = polyline_crossings(&horiz, &v1);
        all.extend(polyline_crossings(&horiz, &v2));
        all.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap());
        assert_eq!(all.len(), 2);
        assert!((all[0][0] - 10.0).abs() < 1e-9);
        assert!((all[1][0] - 20.0).abs() < 1e-9);
    }

    #[test]
    fn polyline_crossings_handles_multi_segment_paths() {
        // An L-shaped run: only the horizontal leg crosses.
        let l = [[0.0, 10.0], [0.0, 0.0], [30.0, 0.0]];
        let v = [[15.0, -5.0], [15.0, 5.0]];
        let x = polyline_crossings(&l, &v);
        assert_eq!(x.len(), 1);
        assert!((x[0][0] - 15.0).abs() < 1e-9 && x[0][1].abs() < 1e-9);
    }

    #[test]
    fn endpoint_touch_detection_covers_shared_corners() {
        let a = [[0.0, 0.0], [10.0, 0.0]];
        let b = [[10.0, 0.0], [10.0, 10.0]];
        assert!(polylines_touch_at_endpoint(&a, &b, 0.5));
        let c = [[20.0, 0.0], [30.0, 0.0]];
        assert!(!polylines_touch_at_endpoint(&a, &c, 0.5));
    }

    #[test]
    fn point_to_segment_distance_clamps_at_the_ends() {
        let a = [0.0, 0.0];
        let b = [10.0, 0.0];
        // Perpendicular foot inside the segment.
        assert!((dist_point_to_segment([5.0, 3.0], a, b) - 3.0).abs() < 1e-9);
        // Beyond the end: distance is to the endpoint, not the infinite line.
        assert!((dist_point_to_segment([15.0, 0.0], a, b) - 5.0).abs() < 1e-9);
        assert!((dist_point_to_segment([-4.0, 0.0], a, b) - 4.0).abs() < 1e-9);
    }

    #[test]
    fn degenerate_segment_distance_is_a_point_distance() {
        assert!((dist_point_to_segment([3.0, 4.0], [0.0, 0.0], [0.0, 0.0]) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn polyline_length_sums_segments() {
        assert!((polyline_length(&[[0.0, 0.0], [3.0, 4.0], [3.0, 0.0]]) - 9.0).abs() < 1e-9);
    }

    #[test]
    fn polyline_point_at_is_arc_length_not_vertex_index() {
        // A long first segment then a short one: the midpoint by arc length is
        // NOT the shared vertex.
        let pts = [[0.0, 0.0], [10.0, 0.0], [10.0, 1.0]];
        let m = polyline_point_at(&pts, 0.5).unwrap();
        assert!((m[0] - 5.5).abs() < 1e-9, "got {m:?}");
        assert!(m[1].abs() < 1e-9);
    }

    #[test]
    fn polyline_point_at_clamps_and_handles_degenerate_input() {
        let pts = [[0.0, 0.0], [10.0, 0.0]];
        assert_eq!(polyline_point_at(&pts, 0.0).unwrap(), [0.0, 0.0]);
        assert_eq!(polyline_point_at(&pts, 1.0).unwrap(), [10.0, 0.0]);
        assert_eq!(polyline_point_at(&pts, 99.0).unwrap(), [10.0, 0.0]);
        assert_eq!(polyline_point_at(&pts, -5.0).unwrap(), [0.0, 0.0]);
        assert_eq!(polyline_point_at(&[[1.0, 2.0]], 0.5).unwrap(), [1.0, 2.0]);
        assert!(polyline_point_at(&[], 0.5).is_none());
        // A zero-length polyline must not divide by zero.
        assert_eq!(polyline_point_at(&[[1.0, 1.0], [1.0, 1.0]], 0.5).unwrap(), [1.0, 1.0]);
    }

    #[test]
    fn bbox_and_overlap() {
        let a = bbox(&[[0.0, 0.0], [10.0, 5.0]]);
        assert_eq!(a, [0.0, 0.0, 10.0, 5.0]);
        let b = bbox(&[[5.0, 2.0], [20.0, 8.0]]);
        assert!(bbox_overlap(a, b));
        let c = bbox(&[[50.0, 50.0], [60.0, 60.0]]);
        assert!(!bbox_overlap(a, c));
        // Touching edges count as overlapping, which is what a coarse
        // pre-filter wants.
        let d = bbox(&[[10.0, 5.0], [20.0, 8.0]]);
        assert!(bbox_overlap(a, d));
    }

    #[test]
    fn ortho_snap_pulls_near_axis_points_onto_the_axis() {
        let from = [0.0, 0.0];
        // Within 5 degrees of horizontal.
        assert_eq!(snap_to_ortho(from, [10.0, 0.5], 5.0), [10.0, 0.0]);
        // Within 5 degrees of vertical.
        assert_eq!(snap_to_ortho(from, [0.5, 10.0], 5.0), [0.0, 10.0]);
        // A genuine diagonal is left alone.
        assert_eq!(snap_to_ortho(from, [10.0, 10.0], 5.0), [10.0, 10.0]);
    }

    #[test]
    fn ortho_snap_works_in_every_quadrant() {
        // Each case is a few degrees off an axis, in each direction. Getting
        // this right matters because the drag direction must not change which
        // axis a line snaps to.
        let from = [5.0, 5.0];
        // North-of-east and south-of-east.
        assert_eq!(snap_to_ortho(from, [15.0, 5.4], 5.0), [15.0, 5.0]);
        assert_eq!(snap_to_ortho(from, [15.0, 4.6], 5.0), [15.0, 5.0]);
        // North-of-west and south-of-west.
        assert_eq!(snap_to_ortho(from, [-5.0, 5.4], 5.0), [-5.0, 5.0]);
        assert_eq!(snap_to_ortho(from, [-5.4, 4.6], 5.0), [-5.4, 5.0]);
        // East-of-north and west-of-north.
        assert_eq!(snap_to_ortho(from, [5.4, 15.0], 5.0), [5.0, 15.0]);
        assert_eq!(snap_to_ortho(from, [4.6, 15.0], 5.0), [5.0, 15.0]);
        // East-of-south and west-of-south.
        assert_eq!(snap_to_ortho(from, [5.4, -5.0], 5.0), [5.0, -5.0]);
        assert_eq!(snap_to_ortho(from, [4.6, -5.0], 5.0), [5.0, -5.0]);
    }

    #[test]
    fn ortho_snap_leaves_a_true_diagonal_alone() {
        // A 44-degree run is not near either axis, so snapping it would be
        // wrong — the clamp is a minimum-distance test, not a nearest-axis one.
        assert_eq!(snap_to_ortho([5.0, 5.0], [-5.4, -5.0], 5.0), [-5.4, -5.0]);
    }

    #[test]
    fn ortho_snap_tolerates_a_zero_length_move() {
        let p = [3.0, 4.0];
        assert_eq!(snap_to_ortho([3.0, 4.0], p, 5.0), p);
    }

    #[test]
    fn angle_helpers() {
        assert!((angle_deg([0.0, 0.0], [1.0, 0.0])).abs() < 1e-9);
        assert!((angle_deg([0.0, 0.0], [0.0, 1.0]) - 90.0).abs() < 1e-9);
        assert!((normalize_deg(370.0) - 10.0).abs() < 1e-9);
        assert!((normalize_deg(-190.0) - 170.0).abs() < 1e-9);
        assert!((normalize_deg(180.0) - 180.0).abs() < 1e-9);
    }

    #[test]
    fn direction_names_the_eight_compass_points() {
        assert_eq!(direction_of([0.0, 0.0], [1.0, 0.0]), "east");
        assert_eq!(direction_of([0.0, 0.0], [0.0, 1.0]), "north");
        assert_eq!(direction_of([0.0, 0.0], [-1.0, 0.0]), "west");
        assert_eq!(direction_of([0.0, 0.0], [0.0, -1.0]), "south");
        assert_eq!(direction_of([1.0, 1.0], [1.0, 1.0]), "zero");
    }
}
