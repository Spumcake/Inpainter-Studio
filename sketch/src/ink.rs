//! Outlines, erase differences, layer unions, and tessellation.
//! Derived ink is never stored on the stroke as authored data.

use crate::document::{Cap, CurvePoint, Dash, Dynamics, ErasePath, Point, Sample};
use perfect_freehand::{InputPoint, StrokeOptions, TaperOptions, TaperType, get_stroke};
use std::panic::AssertUnwindSafe;

pub type Shape = Vec<Vec<Point>>;
pub type Shapes = Vec<Shape>;

#[derive(Clone, Debug)]
pub struct Mesh {
    pub shapes: Shapes,
    pub triangles: Vec<[Point; 3]>,
    pub bounds: Option<(Point, Point)>,
    /// The convex hull of the ink. An affine map carries it onto the hull of
    /// the mapped ink, so it gives tight bounds under any rotation.
    pub hull: Vec<Point>,
}

/// The freehand ring before erases, in the same space as `samples`.
/// Stabilization moves the positions used to draw; the stored samples stay.
pub fn raw_outline(samples: &[Sample], diameter: f64, dynamics: &Dynamics, cap: Cap) -> Vec<Point> {
    let stabilized = stabilize(samples, dynamics);
    outline_polyline(&stabilized, diameter, dynamics, cap)
}

pub fn eraser_outline(path: &ErasePath) -> Vec<Point> {
    raw_outline(
        path.samples(),
        path.size() as f64,
        path.dynamics(),
        Cap::Round,
    )
}

/// A stroke's visible ink: its outline minus its erase outlines.
/// `None` means the boolean failed; the stroke draws nothing that frame.
pub fn stroke_visible(
    samples: &[Sample],
    dynamics: &Dynamics,
    erases: &[ErasePath],
    diameter: f64,
    cap: Cap,
    dash: Dash,
) -> Option<Shapes> {
    let stabilized = stabilize(samples, dynamics);
    let pieces = match dash {
        Dash::Solid => vec![stabilized],
        // Dashes are 3× the diameter and gaps are 2×, along the stabilized centerline.
        Dash::Dashed => split_dashes(&stabilized, diameter * 3.0, diameter * 2.0),
    };
    let clips: Vec<Vec<Point>> = erases.iter().map(eraser_outline).collect();
    let mut shapes = Vec::new();
    let mut failed = false;
    for piece in pieces {
        let outline = outline_polyline(&piece, diameter, dynamics, cap);
        match difference(&outline, &clips) {
            Ok(part) => shapes.extend(part),
            Err(()) => failed = true,
        }
    }
    if shapes.is_empty() && failed {
        None
    } else {
        Some(shapes)
    }
}

/// Walks the centerline, keeping runs of `dash_len` and skipping `gap_len`.
fn split_dashes(samples: &[Sample], dash_len: f64, gap_len: f64) -> Vec<Vec<Sample>> {
    if samples.len() < 2
        || !dash_len.is_finite()
        || dash_len <= 0.0
        || !gap_len.is_finite()
        || gap_len < 0.0
    {
        return vec![samples.to_vec()];
    }
    let mut dashes = Vec::new();
    let mut current = vec![samples[0].clone()];
    let mut into = 0.0;
    let mut in_dash = true;
    for pair in samples.windows(2) {
        let start = &pair[0];
        let end = &pair[1];
        let seg_len = ((end.x - start.x).powi(2) + (end.y - start.y).powi(2)).sqrt();
        if seg_len <= 1.0e-9 {
            continue;
        }
        let mut traveled = 0.0;
        while traveled < seg_len - 1.0e-9 {
            let limit = if in_dash { dash_len } else { gap_len };
            let room = (limit - into).max(1.0e-6);
            let left = seg_len - traveled;
            if left <= room + 1.0e-9 {
                if in_dash {
                    current.push(end.clone());
                }
                into += left;
                break;
            }
            traveled += room;
            let point = lerp_sample(start, end, traveled / seg_len);
            into = 0.0;
            if in_dash {
                current.push(point);
                if current.len() >= 2 {
                    dashes.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
                in_dash = false;
            } else {
                current = vec![point];
                in_dash = true;
            }
        }
    }
    if in_dash && current.len() >= 2 {
        dashes.push(current);
    }
    if dashes.is_empty() {
        vec![samples.to_vec()]
    } else {
        dashes
    }
}

fn lerp_sample(start: &Sample, end: &Sample, t: f64) -> Sample {
    let pressure = match (start.pressure, end.pressure) {
        (Some(left), Some(right)) => Some(left + (right - left) * t),
        (Some(pressure), None) | (None, Some(pressure)) => Some(pressure),
        (None, None) => None,
    };
    Sample {
        x: start.x + (end.x - start.x) * t,
        y: start.y + (end.y - start.y) * t,
        pressure,
    }
}

pub fn union_meshes(parts: &[Shapes]) -> Option<Mesh> {
    let mut acc: Shapes = Vec::new();
    for part in parts {
        if shapes_empty(part) {
            continue;
        }
        if shapes_empty(&acc) {
            acc = part.clone();
            continue;
        }
        acc = union_shapes(&acc, part)?;
    }
    mesh_from_shapes(acc).ok()
}

fn union_shapes(left: &Shapes, right: &Shapes) -> Option<Shapes> {
    use i_overlay::core::fill_rule::FillRule;
    use i_overlay::core::overlay_rule::OverlayRule;
    use i_overlay::float::single::SingleFloatOverlay;

    let left = left.clone();
    let right = right.clone();
    let overlaid = std::panic::catch_unwind(AssertUnwindSafe(|| {
        left.overlay_as::<i64>(&right, OverlayRule::Union, FillRule::NonZero)
    }));
    match overlaid {
        Ok(shapes) => Some(clean_shapes(shapes)),
        Err(_) => None,
    }
}

/// Resolves self-intersections with the non-zero rule, then tessellates.
/// A failed boolean draws nothing for that outline and keeps the source path.
pub fn resolved_mesh(shapes: Shapes) -> Result<Mesh, ()> {
    if shapes_empty(&shapes) {
        return mesh_from_shapes(Vec::new());
    }
    let shapes = boolean(OverlayKind::Simplify, &shapes, &[])?;
    mesh_from_shapes(shapes)
}

pub fn mesh_from_shapes(shapes: Shapes) -> Result<Mesh, ()> {
    let triangles = triangulate(&shapes)?;
    let bounds = bounds_of_shapes(&shapes);
    let hull = convex_hull(shapes.iter().flatten().flatten().copied());
    Ok(Mesh {
        shapes,
        triangles,
        bounds,
        hull,
    })
}

/// The convex hull of a set of points, counterclockwise, by the monotone chain.
pub fn convex_hull(points: impl IntoIterator<Item = Point>) -> Vec<Point> {
    let mut points: Vec<Point> = points
        .into_iter()
        .filter(|point| point[0].is_finite() && point[1].is_finite())
        .collect();
    points.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let turn = |o: Point, a: Point, b: Point| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut hull: Vec<Point> = Vec::with_capacity(points.len() * 2);
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &Point>> = if pass == 0 {
            Box::new(points.iter())
        } else {
            Box::new(points.iter().rev())
        };
        for point in iter {
            while hull.len() >= start + 2
                && turn(hull[hull.len() - 2], hull[hull.len() - 1], *point) <= 0.0
            {
                hull.pop();
            }
            hull.push(*point);
        }
        hull.pop();
    }
    hull
}

pub fn bounds_of_shapes(shapes: &Shapes) -> Option<(Point, Point)> {
    bounds_of(shapes.iter().flatten().flatten())
}

pub fn shapes_empty(shapes: &Shapes) -> bool {
    shapes
        .iter()
        .all(|shape| shape.iter().all(|ring| ring.len() < 3))
}

pub fn point_in_shapes(point: Point, shapes: &Shapes) -> bool {
    shapes.iter().any(|shape| {
        if shape.is_empty() || !point_in_ring(point, &shape[0]) {
            return false;
        }
        shape.iter().skip(1).all(|hole| !point_in_ring(point, hole))
    })
}

pub fn polygons_intersect(ink: &Shapes, clip: &[Point]) -> bool {
    let clip = clean_ring(clip);
    if clip.len() < 3 || shapes_empty(ink) {
        return false;
    }
    let clips = [clip];
    match boolean(OverlayKind::Intersect, ink, &clips) {
        Ok(shapes) => !shapes_empty(&shapes),
        Err(()) => false,
    }
}

pub fn bounds_overlap(a: (Point, Point), b: (Point, Point)) -> bool {
    a.0[0] <= b.1[0] && b.0[0] <= a.1[0] && a.0[1] <= b.1[1] && b.0[1] <= a.1[1]
}

pub fn bounds_of<'a>(points: impl IntoIterator<Item = &'a Point>) -> Option<(Point, Point)> {
    points
        .into_iter()
        .filter(|point| point[0].is_finite() && point[1].is_finite())
        .fold(None, |bounds, point| {
            Some(match bounds {
                None => (*point, *point),
                Some((min, max)) => (
                    [min[0].min(point[0]), min[1].min(point[1])],
                    [max[0].max(point[0]), max[1].max(point[1])],
                ),
            })
        })
}

pub fn curve_y(points: &[CurvePoint], x: f64) -> f64 {
    if points.is_empty() {
        return x.clamp(0.0, 1.0);
    }
    let x = x.clamp(points[0].x, points[points.len() - 1].x);
    if points.len() == 1 {
        return points[0].y.clamp(0.0, 1.0);
    }
    let mut index = 0;
    while index + 1 < points.len() && points[index + 1].x < x {
        index += 1;
    }
    if index + 1 >= points.len() {
        return points[points.len() - 1].y.clamp(0.0, 1.0);
    }
    let tangents = monotone_tangents(points);
    hermite(
        points[index],
        points[index + 1],
        tangents[index],
        tangents[index + 1],
        x,
    )
}

fn effective_pressure(sample: &Sample, dynamics: &Dynamics) -> f64 {
    match sample.pressure {
        Some(pressure) if dynamics.pressure_size => curve_y(&dynamics.curve, pressure),
        _ => 1.0,
    }
}

/// `n = round(stabilization / 100 × 16)`. Zero or one leaves positions alone.
fn stabilize(samples: &[Sample], dynamics: &Dynamics) -> Vec<Sample> {
    let n = (f64::from(dynamics.stabilization) / 100.0 * 16.0).round() as usize;
    if n <= 1 {
        return samples.to_vec();
    }
    samples
        .iter()
        .enumerate()
        .map(|(index, sample)| {
            let start = index.saturating_sub(n - 1);
            let window = &samples[start..=index];
            let (sum_x, sum_y) = window
                .iter()
                .fold((0.0, 0.0), |acc, item| (acc.0 + item.x, acc.1 + item.y));
            let count = window.len() as f64;
            Sample {
                x: sum_x / count,
                y: sum_y / count,
                pressure: sample.pressure,
            }
        })
        .collect()
}

fn outline_polyline(
    samples: &[Sample],
    diameter: f64,
    dynamics: &Dynamics,
    cap: Cap,
) -> Vec<Point> {
    if samples.is_empty() || !diameter.is_finite() || diameter <= 0.0 {
        return Vec::new();
    }
    get_stroke(
        &library_inputs(samples, dynamics),
        &stroke_options(diameter, dynamics, cap),
    )
}

/// Library size is half the diameter: with thinning 1, radius = size × pressure,
/// so the drawn diameter is the layer size times the effective pressure.
fn stroke_options(diameter: f64, dynamics: &Dynamics, cap: Cap) -> StrokeOptions {
    let round = cap == Cap::Round;
    let ends = TaperOptions {
        cap: Some(round),
        taper: Some(TaperType::Bool(false)),
        easing: None,
    };
    StrokeOptions {
        size: Some(diameter / 2.0),
        thinning: Some(1.0),
        smoothing: Some(f64::from(dynamics.smoothing) / 100.0),
        streamline: Some(f64::from(dynamics.streamline) / 100.0),
        simulate_pressure: Some(false),
        start: Some(ends.clone()),
        end: Some(ends),
        last: Some(true),
        ..StrokeOptions::default()
    }
}

/// The 0.1.1 port replaces every pressure after the first with 0.5 when it is
/// given exactly two points. A midpoint is added only for that call.
fn library_inputs(samples: &[Sample], dynamics: &Dynamics) -> Vec<InputPoint> {
    let mut expanded: Vec<(f64, f64, f64)> = samples
        .iter()
        .map(|sample| (sample.x, sample.y, effective_pressure(sample, dynamics)))
        .collect();
    if expanded.len() == 2 {
        let first = expanded[0];
        let second = expanded[1];
        expanded.insert(
            1,
            (
                (first.0 + second.0) / 2.0,
                (first.1 + second.1) / 2.0,
                (first.2 + second.2) / 2.0,
            ),
        );
    }
    expanded
        .into_iter()
        .map(|(x, y, pressure)| InputPoint::Struct {
            x,
            y,
            pressure: Some(pressure),
        })
        .collect()
}

fn monotone_tangents(points: &[CurvePoint]) -> Vec<f64> {
    let count = points.len();
    let mut secants = vec![0.0; count - 1];
    for index in 0..count - 1 {
        let dx = points[index + 1].x - points[index].x;
        secants[index] = if dx == 0.0 {
            0.0
        } else {
            (points[index + 1].y - points[index].y) / dx
        };
    }
    let mut slopes = vec![0.0; count];
    slopes[0] = secants[0];
    slopes[count - 1] = secants[count - 2];
    for index in 1..count - 1 {
        if secants[index - 1] * secants[index] <= 0.0 {
            slopes[index] = 0.0;
        } else {
            slopes[index] = (secants[index - 1] + secants[index]) / 2.0;
        }
    }
    for index in 0..count - 1 {
        if secants[index] == 0.0 {
            slopes[index] = 0.0;
            slopes[index + 1] = 0.0;
            continue;
        }
        let a = slopes[index] / secants[index];
        let b = slopes[index + 1] / secants[index];
        let sum = a * a + b * b;
        if sum > 9.0 {
            let scale = 3.0 / sum.sqrt();
            slopes[index] = scale * a * secants[index];
            slopes[index + 1] = scale * b * secants[index];
        }
    }
    slopes
}

fn hermite(start: CurvePoint, end: CurvePoint, slope0: f64, slope1: f64, x: f64) -> f64 {
    let span = end.x - start.x;
    if span == 0.0 {
        return start.y.clamp(0.0, 1.0);
    }
    let t = ((x - start.x) / span).clamp(0.0, 1.0);
    let t2 = t * t;
    let t3 = t2 * t;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    (h00 * start.y + h10 * span * slope0 + h01 * end.y + h11 * span * slope1).clamp(0.0, 1.0)
}

/// A self-crossing pen outline is resolved with the non-zero rule even when
/// nothing is erased, so tessellation only sees simple rings.
fn difference(subject: &[Point], clips: &[Vec<Point>]) -> Result<Shapes, ()> {
    let subject = clean_ring(subject);
    if subject.len() < 3 {
        return Ok(Vec::new());
    }
    let clips: Vec<Vec<Point>> = clips
        .iter()
        .map(|clip| clean_ring(clip))
        .filter(|clip| clip.len() >= 3)
        .collect();
    let subject_shapes = vec![vec![subject]];
    if clips.is_empty() {
        return boolean(OverlayKind::Simplify, &subject_shapes, &clips);
    }
    boolean(OverlayKind::Difference, &subject_shapes, &clips)
}

enum OverlayKind {
    Simplify,
    Difference,
    Intersect,
}

fn boolean(kind: OverlayKind, subject: &Shapes, clips: &[Vec<Point>]) -> Result<Shapes, ()> {
    use i_overlay::core::fill_rule::FillRule;
    use i_overlay::core::overlay_rule::OverlayRule;
    use i_overlay::float::simplify::SimplifyShape;
    use i_overlay::float::single::SingleFloatOverlay;

    let subject = subject.clone();
    let clips = clips.to_vec();
    let overlaid = std::panic::catch_unwind(AssertUnwindSafe(|| match kind {
        OverlayKind::Simplify => subject.simplify_shape_as::<i64>(FillRule::NonZero),
        OverlayKind::Difference => {
            subject.overlay_as::<i64>(&clips, OverlayRule::Difference, FillRule::NonZero)
        }
        OverlayKind::Intersect => {
            subject.overlay_as::<i64>(&clips, OverlayRule::Intersect, FillRule::NonZero)
        }
    }));
    match overlaid {
        Ok(shapes) => Ok(clean_shapes(shapes)),
        Err(_) => Err(()),
    }
}

fn clean_shapes(shapes: Shapes) -> Shapes {
    shapes
        .into_iter()
        .map(|shape| {
            shape
                .into_iter()
                .map(|ring| clean_ring(&ring))
                .filter(|ring| ring.len() >= 3)
                .collect()
        })
        .filter(|shape: &Vec<Vec<Point>>| !shape.is_empty())
        .collect()
}

fn triangulate(shapes: &Shapes) -> Result<Vec<[Point; 3]>, ()> {
    let mut triangles = Vec::new();
    for shape in shapes {
        if shape.is_empty() || shape[0].len() < 3 {
            continue;
        }
        let mut vertices = Vec::new();
        let mut hole_indices = Vec::new();
        for (index, ring) in shape.iter().enumerate() {
            if ring.len() < 3 {
                continue;
            }
            if index > 0 {
                hole_indices.push(vertices.len() / 2);
            }
            for point in ring {
                vertices.push(point[0]);
                vertices.push(point[1]);
            }
        }
        if vertices.len() < 6 {
            continue;
        }
        let indices = earcutr::earcut(&vertices, &hole_indices, 2).map_err(|_| ())?;
        for corner in indices.chunks_exact(3) {
            let mut triangle = [[0.0; 2]; 3];
            for (slot, index) in corner.iter().enumerate() {
                let offset = index * 2;
                if offset + 1 >= vertices.len() {
                    return Err(());
                }
                triangle[slot] = [vertices[offset], vertices[offset + 1]];
            }
            triangles.push(triangle);
        }
    }
    Ok(triangles)
}

fn clean_ring(points: &[Point]) -> Vec<Point> {
    let mut cleaned = Vec::new();
    for point in points {
        if !point[0].is_finite() || !point[1].is_finite() {
            continue;
        }
        if cleaned
            .last()
            .is_some_and(|previous: &Point| previous[0] == point[0] && previous[1] == point[1])
        {
            continue;
        }
        cleaned.push(*point);
    }
    if cleaned.len() >= 2 {
        let first = cleaned[0];
        let last = *cleaned.last().unwrap();
        if first[0] == last[0] && first[1] == last[1] {
            cleaned.pop();
        }
    }
    cleaned
}

pub fn point_in_ring(point: Point, ring: &[Point]) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut previous = ring.len() - 1;
    for current in 0..ring.len() {
        let a = ring[current];
        let b = ring[previous];
        if (a[1] > point[1]) != (b[1] > point[1]) {
            let x = (b[0] - a[0]) * (point[1] - a[1]) / (b[1] - a[1]) + a[0];
            if point[0] < x {
                inside = !inside;
            }
        }
        previous = current;
    }
    inside
}
