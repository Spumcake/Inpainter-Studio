//! The brush, the eraser, and the selection: what a stroke covers, how it
//! changes a layer's pixels, and the selection's coverage mask and outline.

use crate::picture::{Pixels, Rect};

/// A brush or eraser as the toolbar sets it. Size is in picture pixels;
/// hardness and opacity run from 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Brush {
    pub rgb: [u8; 3],
    pub size: f64,
    pub hardness: f64,
    pub opacity: f64,
    pub pressure: bool,
    pub erase: bool,
}

/// How much of each pixel one stroke covers, from 0 to 1: the most any of
/// its dabs covers there, so overlapping dabs never build past the brush's
/// opacity.
pub struct Coverage {
    pub rect: Rect,
    pub values: Vec<f32>,
}

/// The stroke's coverage: every pixel's distance to the nearest part of the
/// line through the points, against the brush's radius there, which pen
/// pressure scales. Measured against the line itself rather than round dabs
/// along it, so a hard edge is smooth, without the ridges dabs leave between
/// them, as the owner saw on 2026-10-30.
pub fn stroke_coverage(points: &[[f64; 3]], brush: &Brush) -> Option<Coverage> {
    let points: Vec<[f64; 3]> = points
        .iter()
        .copied()
        .filter(|point| point.iter().all(|value| value.is_finite()))
        .collect();
    if points.is_empty() || !brush.size.is_finite() || brush.size <= 0.0 {
        return None;
    }
    let radius_at = |pressure: f64| {
        let scale = if brush.pressure { pressure.clamp(0.0, 1.0) } else { 1.0 };
        ((brush.size * scale).max(1.0) / 2.0).max(0.5)
    };
    // Each piece of the line: its two ends, with the radius at each. A
    // stroke of one point is one piece of no length, a round dot.
    let pieces: Vec<([f64; 2], f64, [f64; 2], f64)> = if points.len() == 1 {
        let point = points[0];
        vec![([point[0], point[1]], radius_at(point[2]), [point[0], point[1]], radius_at(point[2]))]
    } else {
        points
            .windows(2)
            .map(|pair| ([pair[0][0], pair[0][1]], radius_at(pair[0][2]), [pair[1][0], pair[1][1]], radius_at(pair[1][2])))
            .collect()
    };
    let mut bounds = Rect::new(0, 0, 0, 0);
    for (a, ra, b, rb) in &pieces {
        let reach = ra.max(*rb) + 1.0;
        let x = (a[0].min(b[0]) - reach).floor() as i32;
        let y = (a[1].min(b[1]) - reach).floor() as i32;
        let right = (a[0].max(b[0]) + reach).ceil() as i32;
        let bottom = (a[1].max(b[1]) + reach).ceil() as i32;
        bounds = bounds.union(&Rect::new(x, y, right - x, bottom - y));
    }
    if bounds.is_empty() {
        return None;
    }
    let mut values = vec![0.0f32; bounds.w as usize * bounds.h as usize];
    let hardness = brush.hardness.clamp(0.0, 1.0);
    for (a, ra, b, rb) in &pieces {
        let reach = ra.max(*rb) + 1.0;
        let x0 = ((a[0].min(b[0]) - reach).floor() as i32).max(bounds.x);
        let y0 = ((a[1].min(b[1]) - reach).floor() as i32).max(bounds.y);
        let x1 = ((a[0].max(b[0]) + reach).ceil() as i32).min(bounds.right());
        let y1 = ((a[1].max(b[1]) + reach).ceil() as i32).min(bounds.bottom());
        let along = [b[0] - a[0], b[1] - a[1]];
        let length2 = along[0] * along[0] + along[1] * along[1];
        for y in y0..y1 {
            let py = f64::from(y) + 0.5;
            for x in x0..x1 {
                let px = f64::from(x) + 0.5;
                // The nearest place on this piece, and the radius there.
                let t = if length2 > 0.0 {
                    (((px - a[0]) * along[0] + (py - a[1]) * along[1]) / length2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let nearest = [a[0] + along[0] * t, a[1] + along[1] * t];
                let radius = ra + (rb - ra) * t;
                let distance = ((px - nearest[0]).powi(2) + (py - nearest[1]).powi(2)).sqrt();
                // The edge is smoothed over one pixel, whatever the hardness.
                let edge = (radius - distance + 0.5).clamp(0.0, 1.0);
                if edge <= 0.0 {
                    continue;
                }
                let inner = radius * hardness;
                let soft = if distance <= inner || radius - inner <= f64::EPSILON {
                    1.0
                } else {
                    let t = ((distance - inner) / (radius - inner)).clamp(0.0, 1.0);
                    1.0 - t * t * (3.0 - 2.0 * t)
                };
                let value = (edge.min(soft)) as f32;
                let cell = &mut values[(y - bounds.y) as usize * bounds.w as usize + (x - bounds.x) as usize];
                if value > *cell {
                    *cell = value;
                }
            }
        }
    }
    Some(Coverage { rect: bounds, values })
}

/// Paints or erases a stroke's coverage into a layer whose pixels sit at
/// `offset`, inside `clip` (the picture, in picture pixels) and the
/// selection when there is one. `keep_alpha` paints colour only, as the
/// transparency lock asks. Returns the part of the layer that changed, in
/// the layer's own pixels.
pub fn apply(
    pixels: &mut Pixels,
    offset: [i32; 2],
    coverage: &Coverage,
    clip: Rect,
    selection: Option<&Mask>,
    brush: &Brush,
    keep_alpha: bool,
) -> Option<Rect> {
    let layer = Rect::new(offset[0], offset[1], pixels.width as i32, pixels.height as i32);
    let area = coverage.rect.intersect(&layer).intersect(&clip);
    if area.is_empty() {
        return None;
    }
    let opacity = brush.opacity.clamp(0.0, 1.0) as f32;
    let color = brush.rgb.map(|value| f32::from(value) / 255.0);
    let mut changed = Rect::new(0, 0, 0, 0);
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let mut amount = coverage.values[(y - coverage.rect.y) as usize * coverage.rect.w as usize + (x - coverage.rect.x) as usize] * opacity;
            if let Some(mask) = selection {
                amount *= f32::from(mask.at(x, y)) / 255.0;
            }
            if amount <= 0.0 {
                continue;
            }
            let at = ((y - layer.y) as usize * pixels.width as usize + (x - layer.x) as usize) * 4;
            let before = [pixels.data[at], pixels.data[at + 1], pixels.data[at + 2], pixels.data[at + 3]];
            let below = f32::from(before[3]) / 255.0;
            let mut after = before;
            if brush.erase {
                after[3] = to_byte(below * (1.0 - amount));
            } else if keep_alpha {
                for channel in 0..3 {
                    let old = f32::from(before[channel]) / 255.0;
                    after[channel] = to_byte(old + (color[channel] - old) * amount);
                }
            } else {
                let alpha = amount + below * (1.0 - amount);
                for channel in 0..3 {
                    let old = f32::from(before[channel]) / 255.0;
                    after[channel] = to_byte((color[channel] * amount + old * below * (1.0 - amount)) / alpha);
                }
                after[3] = to_byte(alpha);
            }
            if after != before {
                pixels.data[at..at + 4].copy_from_slice(&after);
                changed = changed.union(&Rect::new(x - layer.x, y - layer.y, 1, 1));
            }
        }
    }
    (!changed.is_empty()).then_some(changed)
}

fn to_byte(value: f32) -> u8 {
    (value * 255.0).round().clamp(0.0, 255.0) as u8
}

/// How a new selection shape joins the one there is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combine {
    New,
    Add,
    Subtract,
    Intersect,
}

impl Combine {
    pub const ALL: [Combine; 4] = [Combine::New, Combine::Add, Combine::Subtract, Combine::Intersect];

    pub fn label(self) -> &'static str {
        match self {
            Combine::New => "New",
            Combine::Add => "Add",
            Combine::Subtract => "Subtract",
            Combine::Intersect => "Intersect",
        }
    }
}

/// The selection: how much of each picture pixel is selected, 0 to 255.
#[derive(Debug, Clone, PartialEq)]
pub struct Mask {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Mask {
    pub fn empty(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; width as usize * height as usize],
        }
    }

    pub fn at(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return 0;
        }
        self.data[y as usize * self.width as usize + x as usize]
    }

    pub fn is_clear(&self) -> bool {
        self.data.iter().all(|value| *value == 0)
    }

    /// A rectangle, corner to corner, of the pixels whose centres it holds.
    pub fn rect(width: u32, height: u32, from: [f64; 2], to: [f64; 2]) -> Self {
        let mut mask = Self::empty(width, height);
        let x0 = from[0].min(to[0]).round().clamp(0.0, f64::from(width)) as usize;
        let x1 = from[0].max(to[0]).round().clamp(0.0, f64::from(width)) as usize;
        let y0 = from[1].min(to[1]).round().clamp(0.0, f64::from(height)) as usize;
        let y1 = from[1].max(to[1]).round().clamp(0.0, f64::from(height)) as usize;
        for y in y0..y1 {
            mask.data[y * width as usize + x0..y * width as usize + x1].fill(255);
        }
        mask
    }

    /// A freehand outline, closed by its last point: the pixels whose
    /// centres it holds, by the even-odd rule.
    pub fn lasso(width: u32, height: u32, points: &[[f64; 2]]) -> Self {
        let mut mask = Self::empty(width, height);
        let points: Vec<[f64; 2]> = points.iter().copied().filter(|point| point[0].is_finite() && point[1].is_finite()).collect();
        if points.len() < 3 {
            return mask;
        }
        let top = points.iter().map(|point| point[1]).fold(f64::INFINITY, f64::min).floor().max(0.0) as usize;
        let bottom = points.iter().map(|point| point[1]).fold(f64::NEG_INFINITY, f64::max).ceil().min(f64::from(height)) as usize;
        let mut crossings = Vec::new();
        for y in top..bottom {
            let center = y as f64 + 0.5;
            crossings.clear();
            for index in 0..points.len() {
                let a = points[index];
                let b = points[(index + 1) % points.len()];
                if (a[1] <= center) != (b[1] <= center) {
                    crossings.push(a[0] + (center - a[1]) / (b[1] - a[1]) * (b[0] - a[0]));
                }
            }
            crossings.sort_by(|a, b| a.total_cmp(b));
            for pair in crossings.chunks(2) {
                if pair.len() < 2 {
                    break;
                }
                // Pixels whose centre lies between the two crossings.
                let x0 = (pair[0] - 0.5).ceil().clamp(0.0, f64::from(width)) as usize;
                let x1 = ((pair[1] - 0.5).ceil()).clamp(0.0, f64::from(width)) as usize;
                if x1 > x0 {
                    mask.data[y * width as usize + x0..y * width as usize + x1].fill(255);
                }
            }
        }
        mask
    }

    /// Softens the edge over about `radius` pixels: three box blurs, which
    /// come close to a Gaussian.
    pub fn feather(&mut self, radius: f64) {
        if !radius.is_finite() || radius < 0.5 {
            return;
        }
        let pass = ((radius / 3.0).ceil() as usize).max(1);
        let width = self.width as usize;
        let height = self.height as usize;
        let mut values: Vec<f32> = self.data.iter().map(|value| f32::from(*value)).collect();
        let mut scratch = vec![0.0f32; width.max(height)];
        for _ in 0..3 {
            for y in 0..height {
                box_line(&mut values[y * width..(y + 1) * width], 1, width, pass, &mut scratch);
            }
            for x in 0..width {
                box_line(&mut values[x..], width, height, pass, &mut scratch);
            }
        }
        for (byte, value) in self.data.iter_mut().zip(values) {
            *byte = value.round().clamp(0.0, 255.0) as u8;
        }
    }

    /// Joins `shape` into this selection.
    pub fn combine(&mut self, shape: &Mask, how: Combine) {
        for (value, other) in self.data.iter_mut().zip(shape.data.iter()) {
            *value = match how {
                Combine::New => *other,
                Combine::Add => (*value).max(*other),
                Combine::Subtract => (*value).min(255 - *other),
                Combine::Intersect => (*value).min(*other),
            };
        }
    }

    pub fn invert(&mut self) {
        for value in &mut self.data {
            *value = 255 - *value;
        }
    }

    /// A part of the mask, as crop keeps it.
    pub fn cut(&self, rect: Rect) -> Mask {
        let mut out = Mask::empty(rect.w as u32, rect.h as u32);
        for y in 0..rect.h {
            for x in 0..rect.w {
                out.data[y as usize * rect.w as usize + x as usize] = self.at(rect.x + x, rect.y + y);
            }
        }
        out
    }

    /// The edges between selected and unselected pixels, at half coverage,
    /// as closed outlines in picture pixels.
    pub fn outlines(&self) -> Vec<Vec<[f64; 2]>> {
        let inside = |x: i32, y: i32| self.at(x, y) >= 128;
        // Each edge runs clockwise around the selected pixels, keyed by
        // where it starts.
        let mut edges: std::collections::HashMap<(i32, i32), Vec<(i32, i32)>> = std::collections::HashMap::new();
        let mut count = 0usize;
        for y in 0..self.height as i32 {
            for x in 0..self.width as i32 {
                if !inside(x, y) {
                    continue;
                }
                let mut push = |from: (i32, i32), to: (i32, i32)| {
                    edges.entry(from).or_default().push(to);
                    count += 1;
                };
                if !inside(x, y - 1) {
                    push((x, y), (x + 1, y));
                }
                if !inside(x + 1, y) {
                    push((x + 1, y), (x + 1, y + 1));
                }
                if !inside(x, y + 1) {
                    push((x + 1, y + 1), (x, y + 1));
                }
                if !inside(x - 1, y) {
                    push((x, y + 1), (x, y));
                }
            }
        }
        let mut loops = Vec::new();
        while count > 0 {
            let Some((&start, _)) = edges.iter().find(|(_, ends)| !ends.is_empty()) else {
                break;
            };
            let mut points = vec![start];
            let mut at = start;
            loop {
                let Some(next) = edges.get_mut(&at).and_then(|ends| ends.pop()) else {
                    break;
                };
                count -= 1;
                if next == start {
                    break;
                }
                points.push(next);
                at = next;
            }
            loops.push(simplify(&points));
        }
        loops
    }
}

/// One row or column of a box blur, in place: `len` values `stride` apart.
fn box_line(values: &mut [f32], stride: usize, len: usize, radius: usize, scratch: &mut [f32]) {
    let get = |values: &[f32], index: isize| -> f32 {
        if index < 0 || index as usize >= len { 0.0 } else { values[index as usize * stride] }
    };
    let window = (2 * radius + 1) as f32;
    let mut sum = 0.0;
    for index in -(radius as isize)..=(radius as isize) {
        sum += get(values, index);
    }
    for index in 0..len {
        scratch[index] = sum / window;
        sum += get(values, index as isize + radius as isize + 1);
        sum -= get(values, index as isize - radius as isize);
    }
    for index in 0..len {
        values[index * stride] = scratch[index];
    }
}

/// An outline without the corners that lie on a straight run.
fn simplify(points: &[(i32, i32)]) -> Vec<[f64; 2]> {
    let count = points.len();
    let mut out = Vec::new();
    for index in 0..count {
        let before = points[(index + count - 1) % count];
        let here = points[index];
        let after = points[(index + 1) % count];
        let straight = (here.0 - before.0) * (after.1 - here.1) == (here.1 - before.1) * (after.0 - here.0);
        if !straight {
            out.push([f64::from(here.0), f64::from(here.1)]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brush(size: f64, hardness: f64, opacity: f64) -> Brush {
        Brush { rgb: [255, 0, 0], size, hardness, opacity, pressure: true, erase: false }
    }

    /// Catches a stroke that builds past its opacity where dabs overlap.
    #[test]
    fn one_stroke_never_builds_past_its_opacity() {
        let mut pixels = Pixels::filled(40, 10, [255, 255, 255, 255]);
        let stroke = [[2.0, 5.0, 1.0], [38.0, 5.0, 1.0], [2.0, 5.0, 1.0]];
        let coverage = stroke_coverage(&stroke, &brush(6.0, 1.0, 0.5)).unwrap();
        apply(&mut pixels, [0, 0], &coverage, Rect::new(0, 0, 40, 10), None, &brush(6.0, 1.0, 0.5), false).unwrap();
        let at = (5 * 40 + 20) * 4;
        // Half red over white: 255, 128, 128.
        assert_eq!(&pixels.data[at..at + 4], &[255, 128, 128, 255]);

        // A soft stroke's fading edge is no stronger than one dab's there:
        // the dabs along it do not add up.
        let soft = brush(12.0, 0.0, 1.0);
        let value = |coverage: &Coverage, x: i32, y: i32| {
            coverage.values[((y - coverage.rect.y) * coverage.rect.w + (x - coverage.rect.x)) as usize]
        };
        let dab = stroke_coverage(&[[20.0, 20.0, 1.0]], &soft).unwrap();
        let line = stroke_coverage(&[[5.0, 20.0, 1.0], [35.0, 20.0, 1.0]], &soft).unwrap();
        let single = value(&dab, 20, 23);
        assert!(single > 0.05 && single < 0.9, "{single}");
        assert!(value(&line, 20, 23) <= single + 0.05, "{} against one dab's {single}", value(&line, 20, 23));
    }

    /// Catches pressure ignored: a light press makes a smaller dab.
    #[test]
    fn a_hard_edge_has_no_ridges() {
        // A long straight stroke drawn from few points: every column across
        // it covers the same, as a smooth edge does. Dabs a quarter of the
        // size apart left a ripple along the edge.
        let coverage = stroke_coverage(&[[10.0, 50.0, 1.0], [210.0, 50.3, 1.0]], &brush(41.0, 0.8, 1.0)).unwrap();
        let at = |x: i32, y: i32| coverage.values[((y - coverage.rect.y) * coverage.rect.w + (x - coverage.rect.x)) as usize];
        for y in [30, 31, 69, 70] {
            let row: Vec<f32> = (60..160).map(|x| at(x, y)).collect();
            let low = row.iter().cloned().fold(f32::MAX, f32::min);
            let high = row.iter().cloned().fold(f32::MIN, f32::max);
            assert!(high - low < 0.08, "row {y} ripples from {low} to {high}");
        }
    }

    #[test]
    fn pressure_scales_the_size() {
        let full = stroke_coverage(&[[20.0, 20.0, 1.0]], &brush(20.0, 1.0, 1.0)).unwrap();
        let light = stroke_coverage(&[[20.0, 20.0, 0.25]], &brush(20.0, 1.0, 1.0)).unwrap();
        let covered = |coverage: &Coverage| coverage.values.iter().filter(|value| **value > 0.5).count();
        assert!(covered(&light) * 8 < covered(&full), "{} vs {}", covered(&light), covered(&full));
        let mut flat = brush(20.0, 1.0, 1.0);
        flat.pressure = false;
        let ignored = stroke_coverage(&[[20.0, 20.0, 0.25]], &flat).unwrap();
        assert_eq!(covered(&ignored), covered(&full));
    }

    /// Catches hardness ignored: a soft brush's dab fades toward its edge.
    #[test]
    fn hardness_softens_the_edge() {
        let hard = stroke_coverage(&[[20.0, 20.0, 1.0]], &brush(20.0, 1.0, 1.0)).unwrap();
        let soft = stroke_coverage(&[[20.0, 20.0, 1.0]], &brush(20.0, 0.0, 1.0)).unwrap();
        let near_edge = |coverage: &Coverage| {
            let x = 27 - coverage.rect.x;
            let y = 20 - coverage.rect.y;
            coverage.values[y as usize * coverage.rect.w as usize + x as usize]
        };
        assert_eq!(near_edge(&hard), 1.0);
        assert!(near_edge(&soft) < 0.3, "{}", near_edge(&soft));
    }

    /// Catches a lasso filled by the wrong rule, and an outline that is not
    /// closed around the selection.
    #[test]
    fn lasso_and_outline() {
        let mask = Mask::lasso(20, 20, &[[2.0, 2.0], [12.0, 2.0], [12.0, 8.0], [2.0, 8.0]]);
        assert_eq!(mask.data.iter().filter(|value| **value == 255).count(), 60);
        assert_eq!(mask.at(2, 2), 255);
        assert_eq!(mask.at(12, 2), 0);
        let outlines = mask.outlines();
        assert_eq!(outlines.len(), 1);
        let mut corners = outlines[0].clone();
        corners.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
        assert_eq!(corners, vec![[2.0, 2.0], [2.0, 8.0], [12.0, 2.0], [12.0, 8.0]]);
    }

    /// Catches the four ways of joining a selection mixed up, and a feather
    /// that does nothing.
    #[test]
    fn combine_and_feather() {
        let left = Mask::rect(10, 1, [0.0, 0.0], [6.0, 1.0]);
        let right = Mask::rect(10, 1, [4.0, 0.0], [10.0, 1.0]);
        let ones = |mask: &Mask| mask.data.iter().map(|value| u8::from(*value == 255)).collect::<Vec<_>>();
        let mut add = left.clone();
        add.combine(&right, Combine::Add);
        assert_eq!(ones(&add), vec![1; 10]);
        let mut subtract = left.clone();
        subtract.combine(&right, Combine::Subtract);
        assert_eq!(ones(&subtract), vec![1, 1, 1, 1, 0, 0, 0, 0, 0, 0]);
        let mut intersect = left.clone();
        intersect.combine(&right, Combine::Intersect);
        assert_eq!(ones(&intersect), vec![0, 0, 0, 0, 1, 1, 0, 0, 0, 0]);
        let mut new = left.clone();
        new.combine(&right, Combine::New);
        assert_eq!(new, right);
        let mut soft = Mask::rect(40, 40, [10.0, 10.0], [30.0, 30.0]);
        soft.feather(6.0);
        assert_eq!(soft.at(20, 20), 255);
        let edge = soft.at(10, 20);
        assert!(edge > 60 && edge < 200, "{edge}");
        assert!(soft.at(8, 20) > 0);
    }
}
