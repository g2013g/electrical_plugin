//! Splitting a wire's drawn path at crossings, and folding its runs back.
//!
//! A wire that carries the gap flag is drawn as **several polylines** — one
//! per uninterrupted stretch — sharing one `el_id`, the same fold ladders use.
//! These functions are the whole of the maths:
//!
//! - [`split_runs`] turns one path plus a list of crossing points into the
//!   runs to draw, cutting a half-gap each side of every crossing;
//! - [`fold_runs`] reverses it: the run polylines as drawn, back into one
//!   path, so consumers that think in whole wires (the label midpoint, a
//!   re-seat) keep working on a gapped wire.
//!
//! Host-free and pure, like the rest of `el-core`. The crossing *detection*
//! itself lives in [`crate::geom`] (`proper_crossing`) — it is what keeps a
//! junction (an endpoint touch) from ever being gapped.

use crate::geom::{dist, polyline_length, Pt, EPS};

/// The points where path `a` properly crosses any of `others`.
///
/// Junctions are excluded by construction — `proper_crossing` requires both
/// segments to pass *through* each other — so a wire that ends on another wire
/// is never cut here. Duplicate points (two segments of `b` meeting at the
/// same spot) are dropped.
pub fn crossing_points(a: &[Pt], others: &[&[Pt]]) -> Vec<Pt> {
    let mut out: Vec<Pt> = Vec::new();
    for b in others {
        for p in crate::geom::polyline_crossings(a, b) {
            if out.iter().any(|q| dist(*q, p) < 1e-6) {
                continue;
            }
            out.push(p);
        }
    }
    out
}

/// Where `p` sits along the polyline by arc length, if it lies on it.
fn arc_position(points: &[Pt], p: Pt) -> Option<f64> {
    let mut walked = 0.0;
    for w in points.windows(2) {
        let t = crate::geom::dist_point_to_segment(p, w[0], w[1]);
        if t < 1e-6 {
            let seg = dist(w[0], w[1]);
            if seg < EPS {
                return Some(walked);
            }
            return Some(walked + dist(w[0], p));
        }
        walked += dist(w[0], w[1]);
    }
    None
}

/// Split one path into the runs to draw, cutting `gap_mm` total around every
/// point in `cuts`.
///
/// The cut is centred: `gap_mm / 2` is removed on each side of the crossing,
/// so the *other* wire passes through the middle of the hole. Cuts that land
/// inside an existing hole merge with it, a cut nearer than `gap_mm / 2` to an
/// end clamps to the end, and a stretch that a cut consumes entirely is simply
/// not drawn. With no surviving cuts the whole path comes back as one run.
pub fn split_runs(points: &[Pt], cuts: &[Pt], gap_mm: f64) -> Vec<Vec<Pt>> {
    if points.len() < 2 {
        return Vec::new();
    }
    let half = (gap_mm / 2.0).max(0.0);

    // Arc-length positions of every cut, expanded into blocked intervals.
    let mut blocked: Vec<(f64, f64)> = Vec::new();
    for c in cuts {
        let Some(at) = arc_position(points, *c) else {
            continue;
        };
        blocked.push((at - half, at + half));
    }
    blocked.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));

    // Merge overlaps so two crossings one millimetre apart read as one hole.
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (lo, hi) in blocked {
        match merged.last_mut() {
            Some(last) if lo <= last.1 => last.1 = last.1.max(hi),
            _ => merged.push((lo, hi)),
        }
    }

    let total = polyline_length(points);
    // A block that reaches past both ends erases the wire.
    let kept: Vec<(f64, f64)> = merged
        .into_iter()
        .filter(|(lo, hi)| *lo < total - EPS && *hi > EPS)
        .collect();
    if kept.is_empty() {
        return vec![points.to_vec()];
    }

    // Walk the complement: free stretches between the blocked intervals.
    let point_at = |target: f64| -> Pt {
        let mut walked = 0.0;
        for w in points.windows(2) {
            let seg = dist(w[0], w[1]);
            if walked + seg >= target - EPS && seg > EPS {
                let local = ((target - walked) / seg).clamp(0.0, 1.0);
                return crate::geom::add(w[0], crate::geom::scale(crate::geom::sub(w[1], w[0]), local));
            }
            walked += seg;
        }
        *points.last().unwrap()
    };
    // Arc position of every polyline vertex, so interior bends can be kept.
    let vertex_arcs: Vec<f64> = {
        let mut arcs = vec![0.0];
        for w in points.windows(2) {
            let last = *arcs.last().unwrap();
            arcs.push(last + dist(w[0], w[1]));
        }
        arcs
    };

    let mut runs: Vec<Vec<Pt>> = Vec::new();
    let mut start = 0.0;
    for (lo, hi) in &kept {
        let lo = lo.clamp(0.0, total);
        let hi = hi.clamp(0.0, total);
        if lo - start > EPS {
            let mut run = vec![point_at(start)];
            // Interior vertices of the free stretch stay in the run, so bends
            // between crossings survive.
            for (arc, p) in vertex_arcs.iter().zip(points.iter()) {
                if *arc > start + EPS && *arc < lo - EPS {
                    run.push(*p);
                }
            }
            run.push(point_at(lo));
            runs.push(run);
        }
        start = hi;
    }
    if total - start > EPS {
        let mut run = vec![point_at(start)];
        for (arc, p) in vertex_arcs.iter().zip(points.iter()) {
            if *arc > start + EPS && *arc < total - EPS {
                run.push(*p);
            }
        }
        run.push(point_at(total));
        runs.push(run);
    }
    runs
}

/// Rebuild one path from the runs a gapped wire was drawn as.
///
/// Runs are chained by nearest endpoints, and the chain *starts* at the
/// lexicographically smallest endpoint of any run, oriented outward — so the
/// result is canonical: independent of the order the document lists the run
/// entities in, and of which end of the wire is "from". Consumers here (the
/// label midpoint, a re-seat that matches ends against pins) do not need a
/// particular direction. Run interiors are concatenated with shared joints
/// de-duplicated.
pub fn fold_runs(runs: &[Vec<Pt>]) -> Vec<Pt> {
    let mut remaining: Vec<Vec<Pt>> = runs.iter().filter(|r| !r.is_empty()).cloned().collect();
    if remaining.is_empty() {
        return Vec::new();
    }

    // Canonical chain head: the smallest endpoint of any run, front-facing.
    let mut head = (0usize, true);
    let mut head_pt = [f64::INFINITY, f64::INFINITY];
    for (i, r) in remaining.iter().enumerate() {
        if r[0] < head_pt {
            head_pt = r[0];
            head = (i, true);
        }
        let last = r[r.len() - 1];
        if last < head_pt {
            head_pt = last;
            head = (i, false);
        }
    }
    let mut path = remaining.remove(head.0);
    if !head.1 {
        path.reverse();
    }

    while !remaining.is_empty() {
        let tail = *path.last().unwrap();
        // The nearest run endpoint to the tail, of either orientation.
        let (best_i, best_start, _best_d) = remaining
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let d_front = dist(tail, r[0]);
                let d_back = dist(tail, r[r.len() - 1]);
                if d_front <= d_back {
                    (i, true, d_front)
                } else {
                    (i, false, d_back)
                }
            })
            .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap();
        let run = remaining.remove(best_i);
        let mut part: Vec<Pt> = if best_start {
            run.clone()
        } else {
            run.iter().rev().copied().collect()
        };
        // De-duplicate the joint.
        if dist(tail, part[0]) < 1e-6 {
            part.remove(0);
        }
        path.extend(part);
    }
    path
}

/// Where wire `a` should be cut given the other wires' paths.
///
/// `tol` is how far a crossing may sit from the path and still count; the
/// crossings come from segment intersections, so they land on the path to
/// within floating-point noise and the tolerance only guards rounding.
pub fn cuts_for(a: &[Pt], others: &[&[Pt]], tol: f64) -> Vec<Pt> {
    crossing_points(a, others)
        .into_iter()
        .filter(|p| {
            (0..a.len() - 1).any(|i| {
                crate::geom::dist_point_to_segment(*p, a[i], a[i + 1]) <= tol
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::dist;

    fn approx(a: Pt, b: Pt) -> bool {
        dist(a, b) < 1e-6
    }

    #[test]
    fn no_cuts_returns_the_whole_path() {
        let path = vec![[0.0, 0.0], [10.0, 0.0]];
        let runs = split_runs(&path, &[], 1.6);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0], path);
    }

    #[test]
    fn one_cut_makes_two_runs_with_a_centred_gap() {
        let path = vec![[0.0, 0.0], [10.0, 0.0]];
        let runs = split_runs(&path, &[[5.0, 0.0]], 1.6);
        assert_eq!(runs.len(), 2);
        // 5 ± 0.8
        assert!(approx(runs[0][0], [0.0, 0.0]));
        assert!(approx(runs[0][1], [4.2, 0.0]));
        assert!(approx(runs[1][0], [5.8, 0.0]));
        assert!(approx(runs[1][1], [10.0, 0.0]));
    }

    #[test]
    fn junction_touches_never_become_cuts() {
        // `proper_crossing` already refuses an endpoint touch; this pins that
        // `crossing_points` inherits the refusal.
        let horiz = vec![[0.0, 0.0], [10.0, 0.0]];
        let tee = vec![[5.0, 0.0], [5.0, 5.0]];
        let cuts = crossing_points(&horiz, &[&tee]);
        assert!(cuts.is_empty(), "a T-junction is a connection, not a crossing");
    }

    #[test]
    fn two_crossings_make_three_runs() {
        let path = vec![[0.0, 0.0], [30.0, 0.0]];
        let v1 = vec![[10.0, -5.0], [10.0, 5.0]];
        let v2 = vec![[20.0, -5.0], [20.0, 5.0]];
        let cuts = crossing_points(&path, &[&v1, &v2]);
        assert_eq!(cuts.len(), 2);
        let runs = split_runs(&path, &cuts, 1.6);
        assert_eq!(runs.len(), 3);
        assert!(approx(runs[1][0], [10.8, 0.0]));
        assert!(approx(runs[1][1], [19.2, 0.0]));
    }

    #[test]
    fn nearby_cuts_merge_into_one_hole() {
        let path = vec![[0.0, 0.0], [10.0, 0.0]];
        // Two crossings 0.6 mm apart, gap 1.6 → the holes overlap.
        let cuts = vec![[4.7, 0.0], [5.3, 0.0]];
        let runs = split_runs(&path, &cuts, 1.6);
        assert_eq!(runs.len(), 2);
        assert!(approx(runs[0][1], [3.9, 0.0]));
        assert!(approx(runs[1][0], [6.1, 0.0]));
    }

    #[test]
    fn a_cut_at_an_end_clamps_without_creating_a_degenerate_run() {
        let path = vec![[0.0, 0.0], [10.0, 0.0]];
        // Half the 1.6 gap reaches 0.8 from the end: the head run is gone.
        let runs = split_runs(&path, &[[0.2, 0.0]], 1.6);
        assert_eq!(runs.len(), 1);
        assert!(approx(runs[0][0], [1.0, 0.0]));
        assert!(approx(runs[0][1], [10.0, 0.0]));
    }

    #[test]
    fn a_cut_wider_than_the_path_erases_it() {
        let path = vec![[0.0, 0.0], [10.0, 0.0]];
        let runs = split_runs(&path, &[[3.0, 0.0], [7.0, 0.0]], 10.0);
        // The whole path sits inside one merged hole… except the ends: the
        // complement keeps whatever survives the clamp.
        assert!(runs.iter().all(|r| r.len() >= 2));
        for run in &runs {
            for p in run {
                assert!(p[0] <= 0.0 + 1e-6 || p[0] >= 10.0 - 1e-6, "only the ends survive: {p:?}");
            }
        }
    }

    #[test]
    fn bends_between_crossings_survive() {
        // An L whose corner sits between two crossings keeps its corner in the
        // middle run.
        let path = vec![[0.0, 0.0], [20.0, 0.0], [20.0, 20.0]];
        let v1 = vec![[5.0, -5.0], [5.0, 5.0]];
        let v2 = vec![[15.0, 15.0], [25.0, 15.0]];
        let cuts = crossing_points(&path, &[&v1, &v2]);
        assert_eq!(cuts.len(), 2);
        let runs = split_runs(&path, &cuts, 1.6);
        assert_eq!(runs.len(), 3);
        let mid = &runs[1];
        assert!(mid.iter().any(|p| approx(*p, [20.0, 0.0])), "corner kept: {mid:?}");
    }

    #[test]
    fn fold_runs_reverses_split() {
        let path = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 20.0]];
        let runs = split_runs(&path, &[[10.0, 0.0]], 0.0);
        // Zero gap → the runs join at the corner.
        let folded = fold_runs(&runs);
        assert_eq!(folded.len(), 3);
        assert!(approx(folded[0], [0.0, 0.0]));
        assert!(approx(folded[1], [10.0, 0.0]));
        assert!(approx(folded[2], [10.0, 20.0]));
    }

    #[test]
    fn fold_runs_is_independent_of_input_order() {
        let a = vec![[0.0, 0.0], [4.2, 0.0]];
        let b = vec![[5.8, 0.0], [10.0, 0.0]];
        let folded = fold_runs(&[b, a]);
        assert_eq!(folded.len(), 4);
        assert!(approx(folded[0], [0.0, 0.0]));
        assert!(approx(folded[3], [10.0, 0.0]));
    }

    #[test]
    fn fold_runs_flips_a_backwards_run() {
        let a = vec![[0.0, 0.0], [4.2, 0.0]];
        let b = vec![[10.0, 0.0], [5.8, 0.0]]; // drawn tail-first
        let folded = fold_runs(&[a, b]);
        assert!(approx(folded[2], [5.8, 0.0]));
        assert!(approx(folded[3], [10.0, 0.0]));
    }

    #[test]
    fn fold_runs_handles_a_single_run_and_empty_input() {
        assert!(fold_runs(&[]).is_empty());
        let one = vec![vec![[1.0, 1.0], [2.0, 2.0]]];
        assert_eq!(fold_runs(&one), one[0]);
    }

    #[test]
    fn cuts_for_ignores_off_path_noise() {
        let path = vec![[0.0, 0.0], [10.0, 0.0]];
        let other = vec![[5.0, -5.0], [5.0, 5.0]];
        let cuts = cuts_for(&path, &[&other], 1e-6);
        assert_eq!(cuts.len(), 1);
        // A point nowhere near the path is filtered even if handed in directly
        // through crossing_points is not — cuts_for guards the tolerance.
        let bogus = vec![[5.0, 3.0]];
        assert!(cuts_for(&path, &[&bogus], 1e-6).is_empty());
    }
}
