//! Text layout through egui. The document stores the words and the style;
//! this module measures them when they are drawn and caches the galley.

use crate::document::{
    Dash, ListKind, Point, TextAlign, TextFamily, TextLook, TextStyle, TextView,
};
use eframe::egui::{self, Align, Color32, FontFamily, FontId, Pos2, Rect, Stroke, TextFormat};
use egui::text::{LayoutJob, TextWrapping};
use kurbo::Affine;
use std::collections::HashMap;
use std::sync::Arc;

pub const INTER: &str = "Inter";
pub const INTER_BOLD: &str = "Inter Bold";
pub const INTER_ITALIC: &str = "Inter Italic";
pub const INTER_BOLD_ITALIC: &str = "Inter Bold Italic";

const CACHE_LIMIT: usize = 128;

pub struct Cache {
    galleys: HashMap<Key, Arc<egui::Galley>>,
}

impl Cache {
    pub fn new() -> Self {
        Self {
            galleys: HashMap::new(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    content: String,
    family: u8,
    size_bits: u32,
    bold: bool,
    italic: bool,
    underline: bool,
    rgb: [u8; 3],
    opacity_bits: u32,
    align: u8,
    line_bits: u32,
    letter_bits: u32,
    all_caps: bool,
    wrap_bits: u32,
    ppp_bits: u32,
}

pub struct LaidText {
    pub galley: Arc<egui::Galley>,
    pub local_width: f64,
    pub local_height: f64,
    /// The height of the lines and padding alone, in local units. A box set
    /// taller than this holds them by the vertical alignment.
    pub natural_height: f64,
    rotation: f32,
    anchor: Point,
}

/// Lays out `content` and remembers the box. `caps` uppercases committed text.
/// The editor passes the typed bytes through so the caret stays on them.
#[allow(clippy::too_many_arguments)]
pub fn lay(
    ctx: &egui::Context,
    content: &str,
    style: &TextStyle,
    look: &TextLook,
    origin: Point,
    box_width: Option<f64>,
    box_height: Option<f64>,
    world: Affine,
    zoom: f64,
    caps: bool,
    cache: &mut Cache,
) -> LaidText {
    let zoom = zoom.max(1e-6);
    let pose = pose(world);
    let font_px = (f64::from(style.size.max(1)) * pose.glyph_scale * zoom) as f32;
    // The padding is at the letters' scale on every side, so a box
    // stretched more one way than the other keeps an even margin. In
    // document units:
    let pad = style.padding.max(0.0) * pose.glyph_scale;
    // Wrap at the box's drawn width inside its padding. The box's width
    // follows its horizontal scale; its height is the lines and padding.
    let wrap_px = box_width
        .filter(|_| style.wrap)
        .map(|width| ((width * pose.axis - 2.0 * pad).max(1.0) * zoom) as f32);
    let galley = galley(ctx, content, style, look, font_px, wrap_px, caps, cache);
    let text_width = f64::from(galley.size().x) / zoom;
    let local_width = box_width
        .unwrap_or((text_width + 2.0 * pad) / pose.axis)
        .max(0.0);
    // The lines and padding, and the box: never shorter than they are.
    let natural = (f64::from(galley.size().y) / zoom + 2.0 * pad).max(0.0);
    let drawn = box_height.map_or(natural, |height| natural.max(height * pose.vertical));
    let local_height = drawn / pose.vertical;
    let anchor = reading_corner(
        &box_corners(origin, local_width, local_height),
        world,
        pose.reading,
    );
    let inner = (local_width * pose.axis - 2.0 * pad).max(0.0);
    let across = pad + align_offset(style.align, inner, text_width);
    let down = pad + style.valign.share() * (drawn - natural);
    let below = [-pose.reading[1], pose.reading[0]];
    LaidText {
        galley,
        local_width,
        local_height,
        natural_height: natural / pose.vertical,
        rotation: pose.rotation,
        anchor: [
            anchor[0] + pose.reading[0] * across + below[0] * down,
            anchor[1] + pose.reading[1] * across + below[1] * down,
        ],
    }
}

pub fn paint(
    painter: &egui::Painter,
    rect: Rect,
    camera: [f64; 2],
    zoom: f64,
    view: &TextView,
    laid: &LaidText,
) {
    paint_box(
        painter,
        rect,
        camera,
        zoom,
        &Boxed::of(view),
        laid.local_width,
        laid.local_height,
    );
    let pos = to_screen(rect, camera, zoom, laid.anchor);
    painter.add(
        egui::epaint::TextShape::new(pos, Arc::clone(&laid.galley), Color32::PLACEHOLDER)
            .with_angle(laid.rotation),
    );
}

/// What a text's box, its background and border, is drawn from: its look,
/// corner radius, and place. A text object and the new text being typed
/// both have one.
pub struct Boxed<'a> {
    pub look: &'a TextLook,
    pub radius: f64,
    pub origin: Point,
    pub world: Affine,
}

impl<'a> Boxed<'a> {
    pub fn of(view: &'a TextView) -> Self {
        Self {
            look: &view.look,
            radius: view.style.radius,
            origin: view.origin,
            world: view.world,
        }
    }
}

pub fn paint_box(
    painter: &egui::Painter,
    rect: Rect,
    camera: [f64; 2],
    zoom: f64,
    view: &Boxed<'_>,
    width: f64,
    height: f64,
) {
    if view.look.background.is_none() && view.look.border.is_none() {
        return;
    }
    let Some(outline) = background_outline(view, width, height) else {
        return;
    };
    let corners: Vec<Pos2> = outline
        .into_iter()
        .map(|point| to_screen(rect, camera, zoom, point))
        .collect();
    if let Some((rgb, opacity)) = view.look.background {
        painter.add(egui::Shape::convex_polygon(
            corners.clone(),
            color32(rgb, opacity),
            Stroke::NONE,
        ));
    }
    // The border runs along the box's edge, centered on it as a shape's
    // outline is, at the letters' scale like the padding.
    if let Some(border) = view.look.border {
        let width = (border.size * glyph_scale(view.world) * zoom) as f32;
        let stroke = Stroke::new(width, color32(border.rgb, border.opacity));
        match border.dash {
            Dash::Solid => {
                painter.add(egui::Shape::closed_line(corners, stroke));
            }
            Dash::Dashed => {
                let mut path = corners;
                if let Some(first) = path.first().copied() {
                    path.push(first);
                }
                painter.extend(egui::Shape::dashed_line(
                    &path,
                    stroke,
                    width * 3.0,
                    width * 2.0,
                ));
            }
        }
    }
}

/// The background's outline in document units, for a box `width` by
/// `height` in the object's own units. The box is stretched with the
/// object, but its corners stay round, at the letters' scale: the rounded
/// box is built at its drawn size and only turned and placed by the
/// object's transform.
fn background_outline(view: &Boxed<'_>, width: f64, height: f64) -> Option<Vec<Point>> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let [a, b, c, d, _, _] = view.world.as_coeffs();
    let (sx, sy) = ((a * a + b * b).sqrt(), (c * c + d * d).sqrt());
    if sx < 1e-9 || sy < 1e-9 {
        return None;
    }
    let (drawn_w, drawn_h) = (width * sx, height * sy);
    let radius = (view.radius * glyph_scale(view.world)).clamp(0.0, drawn_w.min(drawn_h) / 2.0);
    let corner = map_point(view.world, view.origin);
    let place = Affine::new([a / sx, b / sx, c / sy, d / sy, corner[0], corner[1]]);
    let drawn = kurbo::RoundedRect::new(0.0, 0.0, drawn_w, drawn_h, radius);
    let mut outline = Vec::new();
    kurbo::flatten(
        kurbo::Shape::path_elements(&drawn, 0.1),
        0.1,
        |element| match element {
            kurbo::PathEl::MoveTo(point) | kurbo::PathEl::LineTo(point) => {
                let point = place * point;
                outline.push([point.x, point.y]);
            }
            _ => {}
        },
    );
    Some(outline)
}

#[allow(clippy::too_many_arguments)]
pub fn galley(
    ctx: &egui::Context,
    content: &str,
    style: &TextStyle,
    look: &TextLook,
    font_px: f32,
    wrap_px: Option<f32>,
    caps: bool,
    cache: &mut Cache,
) -> Arc<egui::Galley> {
    let cased = if caps && style.all_caps {
        content.to_uppercase()
    } else {
        content.to_string()
    };
    // Committed text draws its list markers; the editor shows the typed
    // words alone, so the caret stays on them.
    let shown = match style.list.filter(|_| caps) {
        Some(kind) => cased
            .split('\n')
            .enumerate()
            .map(|(index, line)| format!("{}{line}", list_marker(kind, index)))
            .collect::<Vec<_>>()
            .join("\n"),
        None => cased.clone(),
    };
    let wrap = if style.wrap {
        wrap_px.unwrap_or(f32::INFINITY)
    } else {
        f32::INFINITY
    };
    let key = Key {
        content: shown.clone(),
        family: match style.family {
            TextFamily::Inter => 0,
            TextFamily::Monospace => 1,
        },
        size_bits: font_px.to_bits(),
        bold: style.bold,
        italic: style.italic,
        underline: style.underline,
        rgb: look.ink,
        opacity_bits: (look.opacity as f32).to_bits(),
        align: match style.align {
            TextAlign::Left => 0,
            TextAlign::Center => 1,
            TextAlign::Right => 2,
            TextAlign::Justify => 3,
        },
        line_bits: (style.line_height as f32).to_bits(),
        letter_bits: (style.letter_spacing as f32).to_bits(),
        all_caps: caps && style.all_caps,
        wrap_bits: wrap.to_bits(),
        ppp_bits: ctx.pixels_per_point().to_bits(),
    };
    if let Some(galley) = cache.galleys.get(&key) {
        return Arc::clone(galley);
    }
    if cache.galleys.len() >= CACHE_LIMIT {
        cache.galleys.clear();
    }
    let markers: Option<Vec<String>> = style.list.filter(|_| caps).map(|kind| {
        (0..cased.split('\n').count())
            .map(|index| list_marker(kind, index))
            .collect()
    });
    let job = layout_job(
        ctx,
        &cased,
        markers.as_deref(),
        style,
        color32(look.ink, look.opacity),
        font_px,
        wrap,
    );
    let mut galley = ctx.fonts_mut(|fonts| fonts.layout_job(job));
    align_rows(Arc::make_mut(&mut galley), style.align, wrap);
    cache.galleys.insert(key, Arc::clone(&galley));
    galley
}

/// Sets each line across the width. A line that wrapped is placed by its
/// words, so the space it wrapped at does not push a centered or
/// right-aligned line aside; a paragraph's last line counts all its
/// characters, so a space typed at its end moves the line and the caret as
/// it is typed. A wrapping box's lines span the box's `wrap` width; other
/// text spans its widest line. The galley's rect then runs over that width
/// from x = 0, so the canvas and the caret's field set it the same way.
fn align_rows(galley: &mut egui::Galley, align: TextAlign, wrap: f32) {
    let count = galley.rows.len();
    let extent = |index: usize, row: &egui::epaint::text::PlacedRow| {
        let wrapped = !row.ends_with_newline && index + 1 < count;
        row.row
            .glyphs
            .iter()
            .rev()
            .find(|glyph| !wrapped || !glyph.chr.is_whitespace())
            .map_or(0.0, |glyph| glyph.max_x())
    };
    let widest = galley
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| extent(index, row))
        .fold(0.0, f32::max);
    let width = if wrap.is_finite() { wrap } else { widest };
    let share = match align {
        TextAlign::Left | TextAlign::Justify => 0.0,
        TextAlign::Center => 0.5,
        TextAlign::Right => 1.0,
    };
    let mut bounds = Rect::NOTHING;
    for index in 0..count {
        let shift = (width - extent(index, &galley.rows[index])).max(0.0) * share;
        let row = &mut galley.rows[index];
        row.pos.x += shift;
        bounds |= row.row.visuals.mesh_bounds.translate(row.pos.to_vec2());
    }
    galley.rect = Rect::from_min_max(
        egui::pos2(0.0, galley.rect.min.y),
        egui::pos2(width, galley.rect.max.y),
    );
    if bounds.is_positive() {
        galley.mesh_bounds = bounds;
    }
}

/// The words as one layout job. With list `markers`, one per line, each
/// marker is right-aligned to the widest, so every item's words start at the
/// same place even though "1." is narrower than "2.".
fn layout_job(
    ctx: &egui::Context,
    text: &str,
    markers: Option<&[String]>,
    style: &TextStyle,
    color: Color32,
    font_px: f32,
    wrap: f32,
) -> LayoutJob {
    let font_px = font_px.max(1.0);
    let (font_id, italics) = face(style, font_px);
    let spacing = style.letter_spacing * f64::from(font_px) / f64::from(style.size.max(1));
    let format = TextFormat {
        font_id,
        extra_letter_spacing: spacing as f32,
        line_height: Some(style.line_height as f32 * font_px),
        color,
        italics,
        underline: if style.underline {
            Stroke::new((font_px / 16.0).max(1.0), color)
        } else {
            Stroke::NONE
        },
        ..Default::default()
    };
    let mut job = LayoutJob {
        wrap: TextWrapping {
            max_width: wrap,
            ..Default::default()
        },
        // Lines are laid from the left and then set across the width by
        // `align_rows`, so a wrapped line's last space is not counted.
        halign: Align::LEFT,
        justify: style.align == TextAlign::Justify,
        keep_trailing_whitespace: false,
        ..Default::default()
    };
    let Some(markers) = markers else {
        job.append(text, 0.0, format);
        return job;
    };
    let widths: Vec<f32> = markers
        .iter()
        .map(|marker| {
            ctx.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(marker.clone(), format.font_id.clone(), color)
                    .size()
                    .x
            })
        })
        .collect();
    let widest = widths.iter().copied().fold(0.0, f32::max);
    let count = markers.len();
    for (index, (line, marker)) in text.split('\n').zip(markers).enumerate() {
        job.append(marker, widest - widths[index], format.clone());
        let end = if index + 1 < count { "\n" } else { "" };
        job.append(&format!("{line}{end}"), 0.0, format.clone());
    }
    job
}

/// The marker before a list item: a bullet, its number, or an empty box.
fn list_marker(kind: ListKind, index: usize) -> String {
    match kind {
        ListKind::Bullets => "\u{2022}  ".to_string(),
        ListKind::Numbers => format!("{}.  ", index + 1),
        // A rounded white square: Inter has it, and the ballot box it lacks.
        ListKind::Checks => "\u{25a2}  ".to_string(),
    }
}

fn face(style: &TextStyle, px: f32) -> (FontId, bool) {
    match style.family {
        TextFamily::Monospace => (FontId::new(px, FontFamily::Monospace), style.italic),
        TextFamily::Inter => {
            let name = match (style.bold, style.italic) {
                (false, false) => INTER,
                (true, false) => INTER_BOLD,
                (false, true) => INTER_ITALIC,
                (true, true) => INTER_BOLD_ITALIC,
            };
            (FontId::new(px, FontFamily::Name(name.into())), false)
        }
    }
}

struct Pose {
    rotation: f32,
    glyph_scale: f64,
    /// Length of the local x axis, so a scaled box keeps its alignment gap.
    axis: f64,
    /// Length of the local y axis, so the box's height covers the lines.
    vertical: f64,
    reading: [f64; 2],
}

/// How much a text object's glyphs are scaled by its transform: the mean of
/// its horizontal and vertical scale. An even scale is that scale; an uneven
/// one, such as a corner drag without Shift, scales the glyphs between the
/// two instead of jumping back to full size. The canvas and the editor both
/// draw text at this scale.
pub fn glyph_scale(world: Affine) -> f64 {
    let [a, b, c, d, _, _] = world.as_coeffs();
    let mean = (a * d - c * b).abs().sqrt();
    if mean.is_finite() && mean > 1e-6 {
        mean
    } else {
        1.0
    }
}

/// The length of the transform's local x axis: how wide one unit of the
/// text box is drawn.
pub fn horizontal_scale(world: Affine) -> f64 {
    let [a, b, _, _, _, _] = world.as_coeffs();
    (a * a + b * b).sqrt().max(1e-6)
}

fn pose(world: Affine) -> Pose {
    let [a, b, c, d, _, _] = world.as_coeffs();
    let sx = (a * a + b * b).sqrt();
    let sy = (c * c + d * d).sqrt();
    let det = a * d - c * b;
    let x_rot = b.atan2(a);
    let glyph_scale = glyph_scale(world);
    if det > 0.0 {
        let rotation = x_rot;
        let (sin, cos) = rotation.sin_cos();
        return Pose {
            rotation: rotation as f32,
            glyph_scale,
            axis: sx.max(1e-6),
            vertical: sy.max(1e-6),
            reading: [cos, sin],
        };
    }
    let keep = x_rot;
    let negate = (-b).atan2(-a);
    let rotation = if axis_aligned(keep) && axis_aligned(negate) {
        if keep.cos() >= negate.cos() {
            keep
        } else {
            negate
        }
    } else if sx >= sy {
        keep
    } else {
        negate
    };
    let (sin, cos) = rotation.sin_cos();
    Pose {
        rotation: rotation as f32,
        glyph_scale,
        axis: sx.max(1e-6),
        vertical: sy.max(1e-6),
        reading: [cos, sin],
    }
}

fn axis_aligned(angle: f64) -> bool {
    let folded = angle.abs().rem_euclid(std::f64::consts::PI);
    folded.min(std::f64::consts::PI - folded) < 1e-2
}

fn align_offset(align: TextAlign, box_width: f64, text_width: f64) -> f64 {
    let gap = (box_width - text_width).max(0.0);
    match align {
        TextAlign::Left | TextAlign::Justify => 0.0,
        TextAlign::Center => gap / 2.0,
        TextAlign::Right => gap,
    }
}

fn box_corners(origin: Point, width: f64, height: f64) -> [Point; 4] {
    [
        origin,
        [origin[0] + width, origin[1]],
        [origin[0] + width, origin[1] + height],
        [origin[0], origin[1] + height],
    ]
}

fn reading_corner(corners: &[Point; 4], world: Affine, reading: [f64; 2]) -> Point {
    let down = [-reading[1], reading[0]];
    let mut best = map_point(world, corners[0]);
    let mut best_read = dot(best, reading);
    let mut best_down = dot(best, down);
    for corner in &corners[1..] {
        let point = map_point(world, *corner);
        let read = dot(point, reading);
        let down_p = dot(point, down);
        if read < best_read - 1e-6 || ((read - best_read).abs() <= 1e-6 && down_p < best_down) {
            best = point;
            best_read = read;
            best_down = down_p;
        }
    }
    best
}

fn dot(point: Point, direction: [f64; 2]) -> f64 {
    point[0] * direction[0] + point[1] * direction[1]
}

fn map_point(world: Affine, point: Point) -> Point {
    let [a, b, c, d, e, f] = world.as_coeffs();
    [
        a * point[0] + c * point[1] + e,
        b * point[0] + d * point[1] + f,
    ]
}

fn to_screen(rect: Rect, camera: [f64; 2], zoom: f64, point: Point) -> Pos2 {
    Pos2::new(
        rect.min.x + ((point[0] - camera[0]) * zoom) as f32,
        rect.min.y + ((point[1] - camera[1]) * zoom) as f32,
    )
}

fn color32(rgb: [u8; 3], opacity: f64) -> Color32 {
    let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], alpha)
}

/// Where an upright editor sits, and the width it offers the caret.
pub struct EditorBox {
    pub pos: Pos2,
    pub width: f32,
}

pub fn editor_box(
    rect: Rect,
    camera: [f64; 2],
    zoom: f64,
    origin: Point,
    box_width: Option<f64>,
    world: Affine,
    galley_size: egui::Vec2,
) -> EditorBox {
    let [a, b, c, d, _, _] = world.as_coeffs();
    let upright = a > 1e-3 && d > 1e-3 && b.abs() < 1e-3 && c.abs() < 1e-3;
    let zoom = zoom.max(1e-6);
    if upright {
        let width = match box_width {
            Some(width) => (width * a * zoom) as f32,
            None => galley_size.x,
        }
        .max(1.0);
        return EditorBox {
            pos: to_screen(rect, camera, zoom, map_point(world, origin)),
            width,
        };
    }
    let height = f64::from(galley_size.y) / zoom;
    let width = box_width.unwrap_or(f64::from(galley_size.x) / zoom);
    let corners = box_corners(origin, width.max(0.0), height.max(0.0));
    let mut min = to_screen(rect, camera, zoom, map_point(world, corners[0]));
    let mut max = min;
    for corner in &corners[1..] {
        let pos = to_screen(rect, camera, zoom, map_point(world, *corner));
        min = min.min(pos);
        max = max.max(pos);
    }
    EditorBox {
        pos: min,
        width: (max.x - min.x).max(galley_size.x).max(1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::{Boxed, Cache, background_outline, glyph_scale, lay, pose};
    use crate::document::{TextAlign, TextLook, TextStyle, VAlign};
    use eframe::egui;
    use kurbo::Affine;
    use std::sync::Arc;

    /// Inter, as the host's toolbar installs it. The sketch crate has no
    /// toolbar; the faces are the same files.
    fn install_test_fonts(ctx: &egui::Context) {
        const INTER: &[u8] = include_bytes!("../../host/assets/fonts/Inter-Variable.ttf");
        const INTER_ITALIC: &[u8] =
            include_bytes!("../../host/assets/fonts/Inter-Italic-Variable.ttf");
        let face = |bytes: &'static [u8], weight: f32| {
            let mut data = egui::FontData::from_static(bytes);
            data.tweak.coords = egui::epaint::text::VariationCoords::new([(b"wght", weight)]);
            data
        };
        let mut fonts = egui::FontDefinitions::default();
        for (name, data) in [
            (super::INTER, face(INTER, 400.0)),
            (super::INTER_BOLD, face(INTER, 700.0)),
            (super::INTER_ITALIC, face(INTER_ITALIC, 400.0)),
            (super::INTER_BOLD_ITALIC, face(INTER_ITALIC, 700.0)),
        ] {
            fonts.font_data.insert(name.into(), Arc::new(data));
            fonts.families.insert(
                egui::FontFamily::Name(name.into()),
                vec![name.into()],
            );
        }
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, super::INTER.into());
        ctx.set_fonts(fonts);
    }

    /// A text's Border is drawn around its box, centered on the edge, at
    /// the letters' scale, solid or dashed.
    #[test]
    fn a_border_runs_around_the_box() {
        use crate::document::{Dash, TextBorder};
        let ctx = egui::Context::default();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let world = Affine::scale(2.0);
        for (dash, solid) in [(Dash::Solid, true), (Dash::Dashed, false)] {
            let mut look = TextLook::of(&[]);
            look.border = Some(TextBorder {
                rgb: [20, 20, 20],
                opacity: 1.0,
                size: 3.0,
                dash,
            });
            let boxed = Boxed {
                look: &look,
                radius: 0.0,
                origin: [10.0, 10.0],
                world,
            };
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                super::paint_box(ui.painter(), rect, [0.0, 0.0], 1.5, &boxed, 100.0, 40.0);
            });
            output.textures_delta.clear();
            let widths: Vec<f32> = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Path(path) if path.closed => Some(path.stroke.width),
                    egui::Shape::LineSegment { stroke, .. } => Some(stroke.width),
                    _ => None,
                })
                .collect();
            // 3 units, at the letters' scale of 2, at zoom 1.5.
            assert!(!widths.is_empty(), "{dash:?} border drawn");
            assert!(widths.iter().all(|width| (width - 9.0).abs() < 1e-4));
            assert_eq!(
                widths.len() == 1,
                solid,
                "{dash:?}: {} pieces",
                widths.len()
            );
        }
    }

    /// In a box taller than its lines, they sit at its top, middle, or
    /// bottom, inside its padding; a shorter box is as tall as its lines.
    #[test]
    fn lines_sit_top_middle_or_bottom_in_a_taller_box() {
        let ctx = egui::Context::default();
        install_test_fonts(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        let mut cache = Cache::new();
        let look = TextLook::of(&[]);
        let mut style = TextStyle::builtin();
        style.padding = 10.0;
        let lay_at = |style: &TextStyle, height, cache: &mut Cache| {
            lay(
                &ctx,
                "Hi",
                style,
                &look,
                [0.0, 50.0],
                Some(200.0),
                height,
                Affine::IDENTITY,
                1.0,
                true,
                cache,
            )
        };
        let natural = lay_at(&style, None, &mut cache).local_height;
        let short = lay_at(&style, Some(natural / 2.0), &mut cache);
        assert!((short.local_height - natural).abs() < 1e-9);
        for (valign, share) in [
            (VAlign::Top, 0.0),
            (VAlign::Middle, 0.5),
            (VAlign::Bottom, 1.0),
        ] {
            style.valign = valign;
            let laid = lay_at(&style, Some(natural + 100.0), &mut cache);
            assert!((laid.local_height - (natural + 100.0)).abs() < 1e-9);
            assert!((laid.natural_height - natural).abs() < 1e-9);
            let top = laid.anchor[1] - 50.0;
            assert!(
                (top - (10.0 + share * 100.0)).abs() < 1e-6,
                "{valign:?} lines start {top} below the box's top"
            );
        }
    }

    /// A text stretched more one way than the other keeps an even padding
    /// and round corners, both at the letters' scale.
    #[test]
    fn a_stretched_texts_padding_and_corners_stay_even() {
        let ctx = egui::Context::default();
        install_test_fonts(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        let mut cache = Cache::new();
        let look = TextLook::of(&[]);
        let world = Affine::scale_non_uniform(1.5, 4.0);
        let glyph = glyph_scale(world);
        let mut style = TextStyle::builtin();
        style.padding = 8.0;
        style.radius = 10.0;
        let laid = lay(
            &ctx,
            "hello hello hello",
            &style,
            &look,
            [0.0, 0.0],
            Some(300.0),
            None,
            world,
            1.0,
            true,
            &mut cache,
        );
        let pad = 8.0 * glyph;
        let first = &laid.galley.rows[0];
        let left = laid.anchor[0] + f64::from(first.pos.x + first.row.glyphs[0].pos.x);
        assert!((left - pad).abs() < 0.5, "left padding {left}, not {pad}");
        assert!(
            (laid.anchor[1] - pad).abs() < 0.5,
            "top padding {}",
            laid.anchor[1]
        );
        let drawn_height = laid.local_height * 4.0;
        let lines = f64::from(laid.galley.size().y);
        assert!((drawn_height - (lines + 2.0 * pad)).abs() < 0.5);
        assert!(
            f64::from(laid.galley.size().x) <= 300.0 * 1.5 - 2.0 * pad + 0.5,
            "the words wrap inside the padding"
        );

        let boxed = Boxed {
            look: &look,
            radius: style.radius,
            origin: [0.0, 0.0],
            world,
        };
        let outline = background_outline(&boxed, laid.local_width, laid.local_height).unwrap();
        let along_top = outline
            .iter()
            .filter(|point| point[1].abs() < 1e-6)
            .map(|point| point[0])
            .fold(f64::INFINITY, f64::min);
        let down_left = outline
            .iter()
            .filter(|point| point[0].abs() < 1e-6)
            .map(|point| point[1])
            .fold(f64::INFINITY, f64::min);
        let radius = 10.0 * glyph;
        assert!(
            (along_top - radius).abs() < 0.01,
            "top arc ends at {along_top}"
        );
        assert!(
            (down_left - radius).abs() < 0.01,
            "left arc ends at {down_left}"
        );
    }

    /// Centered and right-aligned lines sit across the box's whole width,
    /// inside its padding, not shifted left of it. A wrapped line is placed
    /// by its words: the space it wrapped at does not count. A space typed at
    /// the end does count, so it shows as it is typed.
    #[test]
    fn a_boxes_lines_sit_left_centered_or_right_inside_it() {
        let ctx = egui::Context::default();
        install_test_fonts(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        let mut cache = Cache::new();
        let look = TextLook::of(&[]);
        let (origin, pad) = (100.0, 10.0);
        let cases = [
            ("Hello this is a test.\nAnother test.", 400.0, 2),
            ("something something something", 160.0, 3),
            ("Hello ", 400.0, 1),
            ("Hello \nthere", 400.0, 2),
        ];
        for (content, width, rows) in cases {
            for align in [TextAlign::Left, TextAlign::Center, TextAlign::Right] {
                let mut style = TextStyle::builtin();
                style.align = align;
                style.padding = pad;
                let laid = lay(
                    &ctx,
                    content,
                    &style,
                    &look,
                    [origin, 0.0],
                    Some(width),
                    None,
                    Affine::IDENTITY,
                    1.0,
                    true,
                    &mut cache,
                );
                assert_eq!(laid.galley.rows.len(), rows, "{content:?}");
                let count = laid.galley.rows.len();
                for (index, row) in laid.galley.rows.iter().enumerate() {
                    let wrapped = !row.ends_with_newline && index + 1 < count;
                    let glyphs = &row.row.glyphs;
                    let first = glyphs.first().unwrap().pos.x;
                    let last = glyphs
                        .iter()
                        .rev()
                        .find(|glyph| !wrapped || !glyph.chr.is_whitespace())
                        .unwrap()
                        .max_x();
                    let left = laid.anchor[0] + f64::from(row.pos.x + first);
                    let right = laid.anchor[0] + f64::from(row.pos.x + last);
                    let off = match align {
                        TextAlign::Center => (left + right) / 2.0 - (origin + width / 2.0),
                        TextAlign::Right => right - (origin + width - pad),
                        _ => left - (origin + pad),
                    };
                    assert!(off.abs() < 1.5, "{align:?} {content:?} is off by {off}");
                }
            }
        }
    }

    /// A corner drag without Shift scales a little unevenly. The glyphs must
    /// follow it, not jump back to full size, and the canvas and the editor
    /// read the same scale.
    #[test]
    fn an_uneven_scale_scales_the_glyphs_between_its_two_scales() {
        let uneven = kurbo::Affine::scale_non_uniform(0.25, 0.26);
        let scale = glyph_scale(uneven);
        assert!(scale > 0.25 && scale < 0.26, "glyph scale {scale}");
        assert!((pose(uneven).glyph_scale - scale).abs() < 1e-12);
        let stretched = kurbo::Affine::scale_non_uniform(2.0, 0.5);
        assert!((glyph_scale(stretched) - 1.0).abs() < 1e-9);
        let turned = kurbo::Affine::rotate(0.7) * kurbo::Affine::scale(3.0);
        assert!((glyph_scale(turned) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn a_uniform_flip_keeps_the_glyph_scale() {
        let flipped = kurbo::Affine::scale_non_uniform(-2.0, 2.0);
        assert!((pose(flipped).glyph_scale - 2.0).abs() < 1e-9);
        let pure = kurbo::Affine::scale_non_uniform(-1.0, 1.0);
        assert!((pose(pure).glyph_scale - 1.0).abs() < 1e-9);
    }
}
