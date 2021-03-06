///! Utilities to flatten cubic bezier curve segments, implmeneted both with callback and
///! iterator based APIs.
///!
///! The algorithm implemented here is based on:
///! http://cis.usouthal.edu/~hain/general/Publications/Bezier/Bezier%20Offset%20Curves.pdf
///! It produces a better approximations than the usual recursive subdivision approach (or
///! in other words, it generates less points for a given tolerance threshold).

use super::{Point, CubicBezierSegment};
use up_to_two::UpToTwo;

use std::f32;
use std::mem::swap;

/// An iterator over a cubic bezier segment that yields line segments approximating the
/// curve for a given approximation threshold.
///
/// The iterator starts at the first point *after* the origin of the curve and ends at the
/// destination.
pub struct CubicFlatteningIter {
    remaining_curve: CubicBezierSegment,
    // current portion of the curve, does not have inflections.
    current_curve: Option<CubicBezierSegment>,
    next_inflection: Option<f32>,
    following_inflection: Option<f32>,
    tolerance: f32,
}

impl CubicFlatteningIter {
    /// Creates an iterator that yields points along a cubic bezier segment, useful to build a
    /// flattened approximation of the curve given a certain tolerance.
    pub fn new(bezier: CubicBezierSegment, tolerance: f32) -> Self {
        let inflections = find_cubic_bezier_inflection_points(&bezier);

        let mut iter = CubicFlatteningIter {
            remaining_curve: bezier,
            current_curve: None,
            next_inflection: inflections.get(0).cloned(),
            following_inflection: inflections.get(1).cloned(),
            tolerance: tolerance,
        };

        if let Some(&t1) = inflections.get(0) {
            let (before, after) = bezier.split(t1);
            iter.current_curve = Some(before);
            iter.remaining_curve = after;
            if let Some(&t2) = inflections.get(1) {
                // Adjust the second inflection since we removed the part before the
                // first inflection from the bezier curve.
                let t2 = (t2 - t1) / (1.0 - t1);
                iter.following_inflection = Some(t2)
            }

            return iter;
        }

        iter.current_curve = Some(bezier);

        return iter;
    }
}

impl Iterator for CubicFlatteningIter {
    type Item = Point;
    fn next(&mut self) -> Option<Point> {

        if self.current_curve.is_none() {
            if self.next_inflection.is_some() {
                if let Some(t2) = self.following_inflection {
                    // No need to re-map t2 in the curve because we already did iter_points
                    // in the iterator's new function.
                    let (before, after) = self.remaining_curve.split(t2);
                    self.current_curve = Some(before);
                    self.remaining_curve = after;
                } else {
                    // the last chunk doesn't have inflection points, use it.
                    self.current_curve = Some(self.remaining_curve);
                }

                // Pop the inflection stack.
                self.next_inflection = self.following_inflection;
                self.following_inflection = None;
            }
        }

        if let Some(sub_curve) = self.current_curve {
            // We are iterating over a sub-curve that does not have inflections.
            let t = no_inflection_flattening_step(&sub_curve, self.tolerance);
            if t >= 1.0 {
                let to = sub_curve.to;
                self.current_curve = None;
                return Some(to);
            }

            let next_curve = sub_curve.after_split(t);
            self.current_curve = Some(next_curve);
            return Some(next_curve.from);
        }

        return None;
    }
}

pub fn flatten_cubic_bezier<F: FnMut(Point)>(
    mut bezier: CubicBezierSegment,
    tolerance: f32,
    call_back: &mut F,
) {
    let inflections = find_cubic_bezier_inflection_points(&bezier);

    if let Some(&t1) = inflections.get(0) {
        let (before, after) = bezier.split(t1);
        flatten_cubic_no_inflection(before, tolerance, call_back);
        bezier = after;

        if let Some(&t2) = inflections.get(1) {
            // Adjust the second inflection since we removed the part before the
            // first inflection from the bezier curve.
            let t2 = (t2 - t1) / (1.0 - t1);

            let (before, after) = bezier.split(t2);
            flatten_cubic_no_inflection(before, tolerance, call_back);
            bezier = after;
        }
    }

    flatten_cubic_no_inflection(bezier, tolerance, call_back);
}

// The algorithm implemented here is based on:
// http://cis.usouthal.edu/~hain/general/Publications/Bezier/Bezier%20Offset%20Curves.pdf
//
// The basic premise is that for a small t the third order term in the
// equation of a cubic bezier curve is insignificantly small. This can
// then be approximated by a quadratic equation for which the maximum
// difference from a linear approximation can be much more easily determined.
fn flatten_cubic_no_inflection<F: FnMut(Point)>(
    mut bezier: CubicBezierSegment,
    tolerance: f32,
    call_back: &mut F,
) {
    let end = bezier.to;

    let mut t = 0.0;
    while t < 1.0 {
        t = no_inflection_flattening_step(&bezier, tolerance);

        if t == 1.0 {
            break;
        }
        bezier = bezier.after_split(t);
        call_back(bezier.from);
    }

    call_back(end);
}

fn no_inflection_flattening_step(bezier: &CubicBezierSegment, tolerance: f32) -> f32 {
    let v1 = bezier.ctrl1 - bezier.from;
    let v2 = bezier.ctrl2 - bezier.from;

    // To remove divisions and check for divide-by-zero, this is optimized from:
    // Float s2 = (v2.x * v1.y - v2.y * v1.x) / hypot(v1.x, v1.y);
    // t = 2 * Float(sqrt(tolerance / (3. * abs(s2))));
    let v1_cross_v2 = v2.x * v1.y - v2.y * v1.x;
    let h = v1.x.hypot(v1.y);
    if v1_cross_v2 * h == 0.0 {
        return 1.0;
    }
    let s2inv = h / v1_cross_v2;

    let t = 2.0 * (tolerance * s2inv.abs() / 3.0).sqrt();

    // TODO: We start having floating point precision issues if this constant
    // is closer to 1.0 with a small enough tolerance threshold.
    if t >= 0.995 {
        return 1.0;
    }

    return t;
}

// Find the inflection points of a cubic bezier curve.
pub fn find_cubic_bezier_inflection_points(bezier: &CubicBezierSegment) -> UpToTwo<f32> {
    // Find inflection points.
    // See www.faculty.idc.ac.il/arik/quality/appendixa.html for an explanation
    // of this approach.
    let pa = bezier.ctrl1 - bezier.from;
    let pb = bezier.ctrl2 - (bezier.ctrl1.to_vector() * 2.0) + bezier.from.to_vector();
    let pc = bezier.to - (bezier.ctrl2.to_vector() * 3.0) + (bezier.ctrl1.to_vector() * 3.0) - bezier.from.to_vector();

    let a = pb.x * pc.y - pb.y * pc.x;
    let b = pa.x * pc.y - pa.y * pc.x;
    let c = pa.x * pb.y - pa.y * pb.x;

    let mut ret = UpToTwo::new();

    if a == 0.0 {
        // Not a quadratic equation.
        if b == 0.0 {
            // Instead of a linear acceleration change we have a constant
            // acceleration change. This means the equation has no solution
            // and there are no inflection points, unless the constant is 0.
            // In that case the curve is a straight line, essentially that means
            // the easiest way to deal with is is by saying there's an inflection
            // point at t == 0. The inflection point approximation range found will
            // automatically extend into infinity.
            if c == 0.0 {
                ret.push(0.0);
            }
        } else {
            ret.push(-c / b);
        }

        return ret;
    }

    fn in_range(t: f32) -> bool { t >= 0.0 && t < 1.0 }

    let discriminant = b * b - 4.0 * a * c;

    if discriminant < 0.0 {
        return ret;
    }

    if discriminant == 0.0 {
        let t = -b / (2.0 * a);

        if in_range(t) {
            ret.push(t);
        }

        return ret;
    }

    let discriminant_sqrt = discriminant.sqrt();
    let q = if b < 0.0 { b - discriminant_sqrt } else { b + discriminant_sqrt } * -0.5;

    let mut first_inflection = q / a;
    let mut second_inflection = c / q;
    if first_inflection > second_inflection {
        swap(&mut first_inflection, &mut second_inflection);
    }

    if in_range(first_inflection) {
        ret.push(first_inflection);
    }

    if in_range(second_inflection) {
        ret.push(second_inflection);
    }

    ret
}

#[cfg(test)]
fn print_arrays(a: &[Point], b: &[Point]) {
    println!("left:  {:?}", a);
    println!("right: {:?}", b);
}

#[cfg(test)]
fn assert_approx_eq(a: &[Point], b: &[Point]) {
    if a.len() != b.len() {
        print_arrays(a, b);
        panic!("Lenths differ ({} != {})", a.len(), b.len());
    }
    for i in 0..a.len() {
        if (a[i].x - b[i].x).abs() > 0.0000001 || (a[i].y - b[i].y).abs() > 0.0000001 {
            print_arrays(a, b);
            panic!("The arrays are not equal");
        }
    }
}

#[test]
fn test_iterator_builder_1() {
    let tolerance = 0.01;
    let c1 = CubicBezierSegment {
        from: Point::new(0.0, 0.0),
        ctrl1: Point::new(1.0, 0.0),
        ctrl2: Point::new(1.0, 1.0),
        to: Point::new(0.0, 1.0),
    };
    let iter_points: Vec<Point> = c1.flattening_iter(tolerance).collect();
    let mut builder_points = Vec::new();
    c1.flattened_for_each(tolerance, &mut |p| { builder_points.push(p); });

    assert!(iter_points.len() > 2);
    assert_approx_eq(&iter_points[..], &builder_points[..]);
}

#[test]
fn test_iterator_builder_2() {
    let tolerance = 0.01;
    let c1 = CubicBezierSegment {
        from: Point::new(0.0, 0.0),
        ctrl1: Point::new(1.0, 0.0),
        ctrl2: Point::new(0.0, 1.0),
        to: Point::new(1.0, 1.0),
    };
    let iter_points: Vec<Point> = c1.flattening_iter(tolerance).collect();
    let mut builder_points = Vec::new();
    c1.flattened_for_each(tolerance, &mut |p| { builder_points.push(p); });

    assert!(iter_points.len() > 2);
    assert_approx_eq(&iter_points[..], &builder_points[..]);
}

#[test]
fn test_iterator_builder_3() {
    let tolerance = 0.01;
    let c1 = CubicBezierSegment {
        from: Point::new(141.0, 135.0),
        ctrl1: Point::new(141.0, 130.0),
        ctrl2: Point::new(140.0, 130.0),
        to: Point::new(131.0, 130.0),
    };
    let iter_points: Vec<Point> = c1.flattening_iter(tolerance).collect();
    let mut builder_points = Vec::new();
    c1.flattened_for_each(tolerance, &mut |p| { builder_points.push(p); });

    assert!(iter_points.len() > 2);
    assert_approx_eq(&iter_points[..], &builder_points[..]);
}

#[test]
fn test_issue_19() {
    let tolerance = 0.15;
    let c1 = CubicBezierSegment {
        from: Point::new(11.71726, 9.07143),
        ctrl1: Point::new(1.889879, 13.22917),
        ctrl2: Point::new(18.142855, 19.27679),
        to: Point::new(18.142855, 19.27679),
    };
    let iter_points: Vec<Point> = c1.flattening_iter(tolerance).collect();
    let mut builder_points = Vec::new();
    c1.flattened_for_each(tolerance, &mut |p| { builder_points.push(p); });

    assert_approx_eq(&iter_points[..], &builder_points[..]);

    assert!(iter_points.len() > 1);
}
