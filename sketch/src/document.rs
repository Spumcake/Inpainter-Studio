//! One sketch's object tree, palette layers, camera, and history.
//! The session and this module do not depend on egui.

use crate::defaults::{self, SIZE_MAX, SIZE_MIN};
use crate::ink::{self, Shapes};
use crate::schema::{self, Inspect, PropId, PropValue};
use kurbo::{Affine, BezPath, Join, PathEl, Shape as KurboShape};
use std::cell::{Cell, OnceCell};
use std::collections::HashSet;
use std::path::PathBuf;

pub const MIN_ZOOM: f64 = 0.125;
pub const MAX_ZOOM: f64 = 8.0;
pub const FIT_MARGIN: f64 = 24.0;
pub const GRID_SPACING: f64 = 24.0;

const ZOOM_IN_FACTOR: f64 = 5.0 / 4.0;
const ZOOM_OUT_FACTOR: f64 = 4.0 / 5.0;

pub type Point = [f64; 2];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SketchId(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ColorId(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StrokeId(u64);

macro_rules! id_newtype {
    ($name:ident) => {
        impl $name {
            pub fn new(id: u64) -> Self {
                debug_assert!(id != 0);
                Self(id)
            }

            pub fn get(self) -> u64 {
                self.0
            }
        }
    };
}

id_newtype!(SketchId);
id_newtype!(NodeId);
id_newtype!(ColorId);
id_newtype!(StrokeId);

/// Hands out ids for one session. Values start at 1 and are never reused.
#[derive(Clone, Debug)]
pub struct Allocator {
    node: u64,
    color: u64,
    stroke: u64,
}

impl Allocator {
    pub fn new() -> Self {
        Self {
            node: 1,
            color: 1,
            stroke: 1,
        }
    }

    fn node(&mut self) -> NodeId {
        let id = NodeId::new(self.node);
        self.node += 1;
        id
    }

    pub fn color(&mut self) -> ColorId {
        let id = ColorId::new(self.color);
        self.color += 1;
        id
    }

    /// Makes sure `id` is never handed out again. Tool palette colors keep
    /// their ids in preferences, so loading them reserves those ids.
    pub fn reserve_color(&mut self, id: ColorId) {
        self.color = self.color.max(id.get() + 1);
    }

    pub fn reserve_node(&mut self, id: NodeId) {
        self.node = self.node.max(id.get() + 1);
    }

    pub fn reserve_stroke(&mut self, id: StrokeId) {
        self.stroke = self.stroke.max(id.get() + 1);
    }

    pub fn stroke(&mut self) -> StrokeId {
        let id = StrokeId::new(self.stroke);
        self.stroke += 1;
        id
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub x: f64,
    pub y: f64,
    pub pressure: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurvePoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dynamics {
    pub pressure_size: bool,
    pub curve: Vec<CurvePoint>,
    pub smoothing: u8,
    pub stabilization: u8,
    pub streamline: u8,
}

impl Dynamics {
    pub fn round() -> Self {
        Self {
            pressure_size: defaults::PRESSURE_SIZE,
            curve: linear_curve(),
            smoothing: defaults::SMOOTHING,
            stabilization: defaults::STABILIZATION,
            streamline: defaults::STREAMLINE,
        }
    }

    /// Round Brush with pressure off. The other dynamics stay at the round values.
    pub fn monoline() -> Self {
        Self {
            pressure_size: false,
            ..Self::round()
        }
    }

    /// Clamps every field. An unusable curve becomes the linear curve.
    pub fn sanitized(self) -> Self {
        let mut clean = Self::round();
        let _ = clean.write(PropId::PressureSize, &PropValue::Bool(self.pressure_size));
        let _ = clean.write(
            PropId::Smoothing,
            &PropValue::Number(f64::from(self.smoothing)),
        );
        let _ = clean.write(
            PropId::Stabilization,
            &PropValue::Number(f64::from(self.stabilization)),
        );
        let _ = clean.write(
            PropId::Streamline,
            &PropValue::Number(f64::from(self.streamline)),
        );
        let curve = self.curve.iter().map(|point| (point.x, point.y)).collect();
        if !clean.write(PropId::Curve, &PropValue::Curve(curve)) {
            clean.curve = linear_curve();
        }
        clean
    }
}

fn linear_curve() -> Vec<CurvePoint> {
    vec![CurvePoint { x: 0.0, y: 0.0 }, CurvePoint { x: 1.0, y: 1.0 }]
}

impl Inspect for Dynamics {
    fn read(&self, id: PropId) -> Option<PropValue> {
        match id {
            PropId::PressureSize => Some(PropValue::Bool(self.pressure_size)),
            PropId::Curve => Some(PropValue::Curve(
                self.curve.iter().map(|point| (point.x, point.y)).collect(),
            )),
            PropId::Smoothing => Some(PropValue::Number(f64::from(self.smoothing))),
            PropId::Stabilization => Some(PropValue::Number(f64::from(self.stabilization))),
            PropId::Streamline => Some(PropValue::Number(f64::from(self.streamline))),
            _ => None,
        }
    }

    fn write(&mut self, id: PropId, value: &PropValue) -> bool {
        match id {
            PropId::PressureSize => {
                let PropValue::Bool(on) = value else {
                    return false;
                };
                self.pressure_size = *on;
                true
            }
            PropId::Smoothing => write_u8(&mut self.smoothing, 0, 100, value),
            PropId::Stabilization => write_u8(&mut self.stabilization, 0, 100, value),
            PropId::Streamline => write_u8(&mut self.streamline, 0, 100, value),
            PropId::Curve => {
                let PropValue::Curve(points) = value else {
                    return false;
                };
                let Some(curve) = sanitize_curve(points) else {
                    return false;
                };
                self.curve = curve;
                true
            }
            _ => false,
        }
    }
}

/// Whole-number size. `write` clamps into `min..=max`.
pub struct BoundedSize {
    pub value: u8,
    min: u8,
    max: u8,
}

impl BoundedSize {
    pub fn new(value: u8, min: u8, max: u8) -> Self {
        Self {
            value: value.clamp(min, max),
            min,
            max,
        }
    }
}

impl Inspect for BoundedSize {
    fn read(&self, _id: PropId) -> Option<PropValue> {
        Some(PropValue::Number(f64::from(self.value)))
    }

    fn write(&mut self, _id: PropId, value: &PropValue) -> bool {
        let PropValue::Number(number) = value else {
            return false;
        };
        let Some(clamped) = schema::clamp_u8(*number, self.min, self.max) else {
            return false;
        };
        self.value = clamped;
        true
    }
}

fn write_u8(slot: &mut u8, min: u8, max: u8, value: &PropValue) -> bool {
    let mut bounded = BoundedSize {
        value: *slot,
        min,
        max,
    };
    if !bounded.write(PropId::Smoothing, value) {
        return false;
    }
    *slot = bounded.value;
    true
}

/// 2 to 5 points, x strictly increasing from 0 to 1, y non-decreasing in 0..=1.
/// Fewer than two finite points cannot be repaired.
fn sanitize_curve(points: &[(f64, f64)]) -> Option<Vec<CurvePoint>> {
    if points.len() < 2 {
        return None;
    }
    let mut curve: Vec<CurvePoint> = points
        .iter()
        .take(5)
        .filter(|point| point.0.is_finite() && point.1.is_finite())
        .map(|point| CurvePoint {
            x: point.0,
            y: point.1,
        })
        .collect();
    if curve.len() < 2 {
        return None;
    }
    let last = curve.len() - 1;
    curve[0].x = 0.0;
    curve[last].x = 1.0;
    let eps = 1.0e-4;
    for index in 1..last {
        let max_x = 1.0 - eps * (last - index) as f64;
        curve[index].x = curve[index].x.clamp(
            curve[index - 1].x + eps,
            max_x.max(curve[index - 1].x + eps),
        );
    }
    for index in 0..curve.len() {
        curve[index].y = curve[index].y.clamp(0.0, 1.0);
        if index > 0 {
            curve[index].y = curve[index].y.max(curve[index - 1].y);
        }
    }
    if curve.windows(2).any(|pair| pair[1].x <= pair[0].x) {
        return None;
    }
    Some(curve)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dash {
    Solid,
    Dashed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cap {
    Round,
    Flat,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ErasePath {
    samples: Vec<Sample>,
    size: u8,
    dynamics: Dynamics,
}

impl ErasePath {
    pub fn new(samples: Vec<Sample>, size: u8, dynamics: Dynamics) -> Self {
        Self {
            samples,
            size,
            dynamics,
        }
    }

    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }

    pub fn size(&self) -> u8 {
        self.size
    }

    pub fn dynamics(&self) -> &Dynamics {
        &self.dynamics
    }
}

#[derive(Clone, Debug)]
pub struct Stroke {
    id: StrokeId,
    samples: Vec<Sample>,
    dynamics: Dynamics,
    /// `None` follows the layer's size. A size set on the Size slider is kept.
    size: Option<u8>,
    erases: Vec<ErasePath>,
    visible: OnceCell<Option<Shapes>>,
}

impl Stroke {
    pub fn new(
        id: StrokeId,
        samples: Vec<Sample>,
        dynamics: Dynamics,
        size: Option<u8>,
    ) -> Self {
        Self {
            id,
            samples,
            dynamics,
            size: size.map(|size| size.clamp(SIZE_MIN, SIZE_MAX)),
            erases: Vec::new(),
            visible: OnceCell::new(),
        }
    }

    /// The stroke's own size, or `None` when it follows its layer.
    #[cfg(test)]
    pub fn own_size(&self) -> Option<u8> {
        self.size
    }

    pub fn id(&self) -> StrokeId {
        self.id
    }

    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }

    pub fn dynamics(&self) -> &Dynamics {
        &self.dynamics
    }

    pub fn erase_paths(&self) -> &[ErasePath] {
        &self.erases
    }

    fn invalidate(&mut self) {
        self.visible = OnceCell::new();
    }

    fn rekey(&mut self, id: StrokeId) {
        self.id = id;
        self.invalidate();
    }

    fn visible(&self, diameter: f64, cap: Cap, dash: Dash) -> Option<&Shapes> {
        if self.visible.get().is_none() {
            let built = ink::stroke_visible(
                &self.samples,
                &self.dynamics,
                &self.erases,
                diameter,
                cap,
                dash,
            );
            let _ = self.visible.set(built);
        }
        self.visible.get().and_then(Option::as_ref)
    }
}

/// What a palette color is for. A layer is a brush color or one of a canvas's
/// layers. The others are the fixed roles of a shape's or text's palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorRole {
    Layer,
    Outline,
    Stroke,
    Fill,
    Text,
    Background,
    /// The line around a text box, not around each letter.
    Border,
}

impl ColorRole {
    /// Size and dash apply: the color draws a line.
    pub fn has_stroke(self) -> bool {
        matches!(
            self,
            ColorRole::Layer | ColorRole::Outline | ColorRole::Stroke | ColorRole::Border
        )
    }

    /// The toolbar's Size slider sets this color's next size, so its tool
    /// color has Lock size. A text's Size slider is the font's, so its
    /// Border's size is set on its page alone.
    pub fn sized_by_toolbar(self) -> bool {
        self.has_stroke() && self != ColorRole::Border
    }

    /// Round or flat ends apply: brush ink and a line's stroke.
    pub fn has_cap(self) -> bool {
        matches!(self, ColorRole::Layer | ColorRole::Stroke)
    }

    /// A fixed role cannot be added, removed, sorted, or renamed.
    pub fn fixed(self) -> bool {
        self != ColorRole::Layer
    }

    fn label(self) -> &'static str {
        match self {
            ColorRole::Layer => "Color",
            ColorRole::Outline => "Outline",
            ColorRole::Stroke => "Stroke",
            ColorRole::Fill => "Fill",
            ColorRole::Text => "Text",
            ColorRole::Background => "Background",
            ColorRole::Border => "Border",
        }
    }
}

/// One color of any palette: a brush color, a canvas layer, or a role of a
/// shape's or text's palette. The color page edits every one of them through
/// this `Inspect`. A role without a stroke uses only value, opacity, and
/// visibility; `visible` on a fill or background is whether it is drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteColor {
    pub id: ColorId,
    pub role: ColorRole,
    pub name: String,
    pub rgb: [u8; 3],
    pub opacity: f64,
    pub size: u8,
    pub dash: Dash,
    pub cap: Cap,
    pub visible: bool,
}

impl PaletteColor {
    fn role(id: ColorId, role: ColorRole, rgb: [u8; 3], opacity: f64, visible: bool) -> Self {
        Self {
            id,
            role,
            name: role.label().to_string(),
            rgb,
            opacity,
            size: defaults::SHAPE_STROKE,
            dash: Dash::Solid,
            cap: Cap::Round,
            visible,
        }
    }

    /// The same color under a new id.
    pub fn copied(&self, ids: &mut Allocator) -> Self {
        let mut copy = self.clone();
        copy.id = ids.color();
        copy
    }

    /// Everything but the id is the same.
    fn same_look(&self, other: &PaletteColor) -> bool {
        self.role == other.role
            && self.name == other.name
            && self.rgb == other.rgb
            && self.opacity == other.opacity
            && self.size == other.size
            && self.dash == other.dash
            && self.cap == other.cap
            && self.visible == other.visible
    }
}

impl Inspect for PaletteColor {
    fn read(&self, id: PropId) -> Option<PropValue> {
        match id {
            // A fixed role's name is its role, so it has no Name row.
            PropId::Name if !self.role.fixed() => Some(PropValue::Text(self.name.clone())),
            PropId::Hex => Some(PropValue::Text(format_hex(self.rgb))),
            PropId::Rgb => Some(PropValue::Rgb(self.rgb)),
            PropId::Opacity => Some(PropValue::Number(self.opacity)),
            PropId::ColorSize if self.role.has_stroke() => {
                Some(PropValue::Number(f64::from(self.size)))
            }
            PropId::Dash if self.role.has_stroke() => Some(PropValue::Index(match self.dash {
                Dash::Solid => 0,
                Dash::Dashed => 1,
            })),
            PropId::Cap if self.role.has_cap() => Some(PropValue::Index(match self.cap {
                Cap::Round => 0,
                Cap::Flat => 1,
            })),
            PropId::Visible => Some(PropValue::Bool(self.visible)),
            _ => None,
        }
    }

    fn write(&mut self, id: PropId, value: &PropValue) -> bool {
        match id {
            PropId::Name => {
                if self.role.fixed() {
                    return false;
                }
                let PropValue::Text(name) = value else {
                    return false;
                };
                let name: String = name.trim().chars().take(80).collect();
                if name.is_empty() {
                    return false;
                }
                self.name = name;
                true
            }
            PropId::Hex => {
                let PropValue::Text(text) = value else {
                    return false;
                };
                let Some(rgb) = parse_hex(text) else {
                    return false;
                };
                self.rgb = rgb;
                true
            }
            PropId::Rgb => {
                let PropValue::Rgb(rgb) = value else {
                    return false;
                };
                self.rgb = *rgb;
                true
            }
            PropId::Opacity => write_opacity(&mut self.opacity, value),
            PropId::ColorSize if self.role.has_stroke() => {
                write_u8(&mut self.size, SIZE_MIN, SIZE_MAX, value)
            }
            PropId::Dash if self.role.has_stroke() => {
                let PropValue::Index(index) = value else {
                    return false;
                };
                self.dash = match index {
                    0 => Dash::Solid,
                    1 => Dash::Dashed,
                    _ => return false,
                };
                true
            }
            PropId::Cap if self.role.has_cap() => {
                let PropValue::Index(index) = value else {
                    return false;
                };
                self.cap = match index {
                    0 => Cap::Round,
                    1 => Cap::Flat,
                    _ => return false,
                };
                true
            }
            PropId::Visible => write_bool(&mut self.visible, value),
            _ => false,
        }
    }
}

/// A color in a tool's palette: a tool setting. `lock_size` makes the next
/// mark follow the color's size; moving the Size slider turns it off and the
/// slider's value, `next_size`, is used instead.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolColor {
    pub color: PaletteColor,
    pub lock_size: bool,
    pub next_size: u8,
}

impl ToolColor {
    pub fn new(color: PaletteColor) -> Self {
        Self {
            next_size: color.size,
            lock_size: true,
            color,
        }
    }

    pub fn id(&self) -> ColorId {
        self.color.id
    }

    /// The size the next mark in this color is drawn at.
    pub fn mark_size(&self) -> u8 {
        if self.lock_size {
            self.color.size
        } else {
            self.next_size
        }
    }

    /// Clamps every stored value, as preferences can hold anything.
    pub fn sanitized(mut self) -> Self {
        let color = self.color.clone();
        let _ = self
            .color
            .write(PropId::Opacity, &PropValue::Number(color.opacity));
        self.color.size = color.size.clamp(SIZE_MIN, SIZE_MAX);
        self.next_size = self.next_size.clamp(SIZE_MIN, SIZE_MAX);
        if self.color.name.trim().is_empty() {
            self.color.name = self.color.role.label().to_string();
        }
        self
    }
}

impl Inspect for ToolColor {
    fn read(&self, id: PropId) -> Option<PropValue> {
        match id {
            PropId::LockSize if self.color.role.sized_by_toolbar() => {
                Some(PropValue::Bool(self.lock_size))
            }
            _ => self.color.read(id),
        }
    }

    fn write(&mut self, id: PropId, value: &PropValue) -> bool {
        match id {
            PropId::LockSize if self.color.role.sized_by_toolbar() => {
                write_bool(&mut self.lock_size, value)
            }
            _ => self.color.write(id, value),
        }
    }
}

/// The brush's starting palette: black through light gray, each at size 3,
/// full opacity, solid, with round caps and Lock size on.
pub fn brush_palette(ids: &mut Allocator) -> Vec<ToolColor> {
    defaults::PALETTE
        .iter()
        .map(|spec| {
            ToolColor::new(PaletteColor {
                id: ids.color(),
                role: ColorRole::Layer,
                name: spec.name.to_string(),
                rgb: spec.rgb,
                opacity: defaults::COLOR_OPACITY,
                size: defaults::COLOR_SIZE,
                dash: Dash::Solid,
                cap: Cap::Round,
                visible: true,
            })
        })
        .collect()
}

/// A shape tool's starting palette: a black Outline at size 3, and a light
/// red Fill at 40% that starts off. A line has only its Stroke.
pub fn shape_palette(kind: ShapeKind, ids: &mut Allocator) -> Vec<ToolColor> {
    let line_role = match kind {
        ShapeKind::Line => ColorRole::Stroke,
        ShapeKind::Rectangle | ShapeKind::Ellipse => ColorRole::Outline,
    };
    let mut colors = vec![ToolColor::new(PaletteColor::role(
        ids.color(),
        line_role,
        defaults::SHAPE_STROKE_RGB,
        1.0,
        true,
    ))];
    if kind != ShapeKind::Line {
        colors.push(ToolColor::new(PaletteColor::role(
            ids.color(),
            ColorRole::Fill,
            defaults::SHAPE_FILL_RGB,
            defaults::SHAPE_FILL_OPACITY,
            false,
        )));
    }
    colors
}

/// The text tool's starting palette: black Text, and a white Background that
/// starts off.
pub fn text_palette(ids: &mut Allocator) -> Vec<ToolColor> {
    vec![
        ToolColor::new(PaletteColor::role(
            ids.color(),
            ColorRole::Text,
            defaults::TEXT_RGB,
            1.0,
            true,
        )),
        ToolColor::new(PaletteColor::role(
            ids.color(),
            ColorRole::Background,
            defaults::TEXT_BACKGROUND_RGB,
            1.0,
            false,
        )),
        text_border(ids),
    ]
}

/// The text tool's Border, off at first.
pub fn text_border(ids: &mut Allocator) -> ToolColor {
    let mut border = PaletteColor::role(
        ids.color(),
        ColorRole::Border,
        defaults::TEXT_BORDER_RGB,
        1.0,
        false,
    );
    border.size = defaults::TEXT_BORDER_SIZE;
    ToolColor::new(border)
}

/// The roles a palette must have, in order, for a shape kind or text. A
/// stored palette that does not match is replaced by the starting one.
pub fn palette_roles_match(colors: &[ToolColor], roles: &[ColorRole]) -> bool {
    colors.len() == roles.len()
        && colors
            .iter()
            .zip(roles)
            .all(|(color, role)| color.color.role == *role)
}

/// A copy of a tool's palette for a new object, under new ids. The first
/// stroked color takes the size the next mark uses.
pub fn object_palette(tool: &[ToolColor], ids: &mut Allocator) -> Vec<PaletteColor> {
    tool.iter()
        .map(|color| {
            let mut copy = color.color.copied(ids);
            if copy.role.has_stroke() {
                copy.size = color.mark_size();
            }
            copy
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct ColorLayer {
    color: PaletteColor,
    locked: bool,
    /// The brush color this layer was copied from.
    source: Option<ColorId>,
    strokes: Vec<Stroke>,
    union_cache: OnceCell<Option<ink::Mesh>>,
}

impl ColorLayer {
    fn from_tool(id: ColorId, tool: &ToolColor) -> Self {
        let mut color = tool.color.clone();
        color.id = id;
        color.role = ColorRole::Layer;
        Self {
            color,
            locked: false,
            source: Some(tool.color.id),
            strokes: Vec::new(),
            union_cache: OnceCell::new(),
        }
    }

    pub fn id(&self) -> ColorId {
        self.color.id
    }

    pub fn name(&self) -> &str {
        &self.color.name
    }

    pub fn rgb(&self) -> [u8; 3] {
        self.color.rgb
    }

    pub fn size(&self) -> u8 {
        self.color.size
    }

    pub fn visible(&self) -> bool {
        self.color.visible
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    /// A stroke's diameter in this layer: its own size, or the layer's.
    fn diameter(&self, stroke: &Stroke) -> f64 {
        f64::from(stroke.size.unwrap_or(self.color.size))
    }

    fn stroke_ink<'a>(&self, stroke: &'a Stroke) -> Option<&'a Shapes> {
        stroke.visible(self.diameter(stroke), self.color.cap, self.color.dash)
    }

    fn invalidate(&mut self) {
        self.union_cache = OnceCell::new();
        for stroke in &mut self.strokes {
            stroke.invalidate();
        }
    }

    /// A size change redraws only the strokes that follow the layer's size.
    fn invalidate_following(&mut self) {
        self.union_cache = OnceCell::new();
        for stroke in &mut self.strokes {
            if stroke.size.is_none() {
                stroke.invalidate();
            }
        }
    }

    fn mesh(&self) -> Option<&ink::Mesh> {
        if self.union_cache.get().is_none() {
            let built = self.build_union();
            let _ = self.union_cache.set(built);
        }
        self.union_cache.get().and_then(Option::as_ref)
    }

    fn build_union(&self) -> Option<ink::Mesh> {
        let mut parts = Vec::new();
        for stroke in &self.strokes {
            let Some(shapes) = self.stroke_ink(stroke) else {
                continue;
            };
            if !ink::shapes_empty(shapes) {
                parts.push(shapes.clone());
            }
        }
        if parts.is_empty() {
            return Some(ink::Mesh {
                shapes: Vec::new(),
                triangles: Vec::new(),
                bounds: None,
                hull: Vec::new(),
            });
        }
        if parts.len() == 1 {
            return ink::mesh_from_shapes(parts.pop().unwrap()).ok();
        }
        ink::union_meshes(&parts)
    }

    /// A copy below the original, with its strokes. It is no longer linked
    /// to a brush color.
    fn duplicate(&self, ids: &mut Allocator) -> Self {
        let mut copy = self.clone();
        copy.color.id = ids.color();
        copy.source = None;
        copy.union_cache = OnceCell::new();
        for stroke in &mut copy.strokes {
            stroke.rekey(ids.stroke());
        }
        copy
    }
}

impl Inspect for ColorLayer {
    fn read(&self, id: PropId) -> Option<PropValue> {
        match id {
            PropId::Locked => Some(PropValue::Bool(self.locked)),
            _ => self.color.read(id),
        }
    }

    fn write(&mut self, id: PropId, value: &PropValue) -> bool {
        match id {
            PropId::Locked => write_bool(&mut self.locked, value),
            PropId::ColorSize => {
                let previous = self.color.size;
                if !self.color.write(id, value) {
                    return false;
                }
                if self.color.size != previous {
                    self.invalidate_following();
                }
                true
            }
            PropId::Dash | PropId::Cap => {
                let (dash, cap) = (self.color.dash, self.color.cap);
                if !self.color.write(id, value) {
                    return false;
                }
                if self.color.dash != dash || self.color.cap != cap {
                    self.invalidate();
                }
                true
            }
            _ => self.color.write(id, value),
        }
    }
}

fn format_hex(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let text = text.trim();
    let text = text.strip_prefix('#').unwrap_or(text);
    if text.len() != 6 || !text.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let r = u8::from_str_radix(&text[0..2], 16).ok()?;
    let g = u8::from_str_radix(&text[2..4], 16).ok()?;
    let b = u8::from_str_radix(&text[4..6], 16).ok()?;
    Some([r, g, b])
}

struct Canvas {
    padding: f64,
    colors: Vec<ColorLayer>,
}

/// Flattened shape outlines, in local units.
const SHAPE_FLATNESS: f64 = 0.05;

/// Window settings for one shape tool: its palette and the next shape's
/// corner radius. Constrain and snap are not object data.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeTool {
    pub palette: Vec<ToolColor>,
    pub corner_radius: f64,
    pub constrain: bool,
    pub snap: bool,
}

impl ShapeTool {
    pub fn builtin(kind: ShapeKind, ids: &mut Allocator) -> Self {
        Self {
            palette: shape_palette(kind, ids),
            corner_radius: 0.0,
            constrain: false,
            snap: false,
        }
    }

    pub fn read(&self, id: PropId) -> Option<PropValue> {
        match id {
            PropId::CornerRadius => Some(PropValue::Number(self.corner_radius)),
            PropId::Constrain => Some(PropValue::Bool(self.constrain)),
            PropId::Snap => Some(PropValue::Bool(self.snap)),
            _ => None,
        }
    }

    pub fn write(&mut self, id: PropId, value: &PropValue) -> bool {
        match id {
            PropId::CornerRadius => write_radius(&mut self.corner_radius, value),
            PropId::Constrain => write_bool(&mut self.constrain, value),
            PropId::Snap => write_bool(&mut self.snap, value),
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    Rectangle,
    Ellipse,
    Line,
}

/// The drag, after snap and constrain. `None` from [`resolve_drag`] is a drag
/// that must not be inserted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlacedShape {
    Box { min: Point, max: Point },
    Line { start: Point, end: Point },
}

/// Snap the press and the pointer to the grid first, then constrain.
/// The press stays the fixed corner, or the line's start. The pointer is the
/// moving point. A constrained box takes the greater absolute side and grows
/// in the pointer's direction on each axis. A constrained line keeps its
/// length and lands on the nearest 45-degree step.
pub fn resolve_drag(
    kind: ShapeKind,
    origin: Point,
    pointer: Point,
    constrain: bool,
    snap: bool,
) -> Option<PlacedShape> {
    if !point_finite(origin) || !point_finite(pointer) {
        return None;
    }
    let origin = if snap { snap_point(origin) } else { origin };
    let pointer = if snap { snap_point(pointer) } else { pointer };
    let (start, end) = if constrain {
        match kind {
            ShapeKind::Line => constrain_line(origin, pointer),
            ShapeKind::Rectangle | ShapeKind::Ellipse => constrain_box(origin, pointer),
        }
    } else {
        (origin, pointer)
    };
    match kind {
        ShapeKind::Line => {
            if start == end {
                None
            } else {
                Some(PlacedShape::Line { start, end })
            }
        }
        ShapeKind::Rectangle | ShapeKind::Ellipse => {
            let (min, max) = box_corners_of(start, end);
            if min == max {
                None
            } else {
                Some(PlacedShape::Box { min, max })
            }
        }
    }
}

fn point_finite(point: Point) -> bool {
    point[0].is_finite() && point[1].is_finite()
}

fn snap_point(point: Point) -> Point {
    [
        (point[0] / GRID_SPACING).round() * GRID_SPACING,
        (point[1] / GRID_SPACING).round() * GRID_SPACING,
    ]
}

fn constrain_box(origin: Point, pointer: Point) -> (Point, Point) {
    let dx = pointer[0] - origin[0];
    let dy = pointer[1] - origin[1];
    let side = dx.abs().max(dy.abs());
    let end = [
        origin[0] + side * dx.signum(),
        origin[1] + side * dy.signum(),
    ];
    (origin, end)
}

fn constrain_line(origin: Point, pointer: Point) -> (Point, Point) {
    let dx = pointer[0] - origin[0];
    let dy = pointer[1] - origin[1];
    let length = dx.hypot(dy);
    if length == 0.0 {
        return (origin, origin);
    }
    let step = std::f64::consts::FRAC_PI_4;
    let angle = (dy.atan2(dx) / step).round() * step;
    (
        origin,
        [
            origin[0] + length * angle.cos(),
            origin[1] + length * angle.sin(),
        ],
    )
}

fn box_corners_of(a: Point, b: Point) -> (Point, Point) {
    (
        [a[0].min(b[0]), a[1].min(b[1])],
        [a[0].max(b[0]), a[1].max(b[1])],
    )
}

fn write_opacity(slot: &mut f64, value: &PropValue) -> bool {
    let PropValue::Number(number) = value else {
        return false;
    };
    if !number.is_finite() {
        return false;
    }
    *slot = number.clamp(0.0, 1.0);
    true
}

fn write_radius(slot: &mut f64, value: &PropValue) -> bool {
    let PropValue::Number(number) = value else {
        return false;
    };
    if !number.is_finite() {
        return false;
    }
    *slot = number.clamp(defaults::CORNER_MIN, defaults::CORNER_MAX);
    true
}

fn write_bool(slot: &mut bool, value: &PropValue) -> bool {
    let PropValue::Bool(on) = value else {
        return false;
    };
    *slot = *on;
    true
}

#[derive(Clone, Debug)]
enum ShapeGeom {
    Rect {
        min: Point,
        max: Point,
        corner_radius: f64,
    },
    Ellipse {
        min: Point,
        max: Point,
    },
    Line {
        start: Point,
        end: Point,
    },
}

#[derive(Clone, Debug)]
struct ShapeInk {
    fill: Option<ink::Mesh>,
    stroke: Option<ink::Mesh>,
}

#[derive(Clone, Debug)]
struct ShapeNode {
    geom: ShapeGeom,
    /// Outline then Fill for a rectangle or an ellipse; Stroke for a line.
    palette: Vec<PaletteColor>,
    ink: OnceCell<ShapeInk>,
}

impl ShapeNode {
    fn new(geom: ShapeGeom, palette: Vec<PaletteColor>) -> Self {
        Self {
            geom,
            palette,
            ink: OnceCell::new(),
        }
    }

    fn rect(min: Point, max: Point, radius: f64, palette: Vec<PaletteColor>) -> Self {
        Self::new(
            ShapeGeom::Rect {
                min,
                max,
                corner_radius: radius.clamp(defaults::CORNER_MIN, defaults::CORNER_MAX),
            },
            palette,
        )
    }

    fn kind(&self) -> ShapeKind {
        match self.geom {
            ShapeGeom::Rect { .. } => ShapeKind::Rectangle,
            ShapeGeom::Ellipse { .. } => ShapeKind::Ellipse,
            ShapeGeom::Line { .. } => ShapeKind::Line,
        }
    }

    /// The Outline or Stroke color.
    fn outline(&self) -> Option<&PaletteColor> {
        self.palette.iter().find(|color| color.role.has_stroke())
    }

    /// The Fill, when it is on.
    fn fill(&self) -> Option<&PaletteColor> {
        self.palette
            .iter()
            .find(|color| color.role == ColorRole::Fill && color.visible)
    }

    fn invalidate(&mut self) {
        self.ink = OnceCell::new();
    }

    fn ink(&self) -> &ShapeInk {
        if self.ink.get().is_none() {
            let _ = self.ink.set(self.build_ink());
        }
        self.ink.get().expect("the ink cell was just filled")
    }

    fn build_ink(&self) -> ShapeInk {
        let path = shape_path(&self.geom);
        let fill = self
            .fill()
            .and_then(|_| ink::mesh_from_shapes(path_rings(&flatten_path(&path))).ok());
        let stroke = self
            .outline()
            .filter(|color| color.visible)
            .and_then(|color| {
                let expanded = stroke_path(&path, color, self.kind() == ShapeKind::Line);
                ink::resolved_mesh(path_rings(&flatten_path(&expanded))).ok()
            });
        ShapeInk { fill, stroke }
    }

    fn read(&self, id: PropId) -> Option<PropValue> {
        match (id, &self.geom) {
            (PropId::CornerRadius, ShapeGeom::Rect { corner_radius, .. }) => {
                Some(PropValue::Number(*corner_radius))
            }
            (PropId::Palette, _) => Some(PropValue::Idle),
            _ => None,
        }
    }

    fn write(&mut self, id: PropId, value: &PropValue) -> bool {
        let ok = match (id, &mut self.geom) {
            (PropId::CornerRadius, ShapeGeom::Rect { corner_radius, .. }) => {
                write_radius(corner_radius, value)
            }
            _ => false,
        };
        if ok {
            self.invalidate();
        }
        ok
    }

    #[cfg(test)]
    fn corners(&self) -> Option<(Point, Point)> {
        match self.geom {
            ShapeGeom::Rect { min, max, .. } | ShapeGeom::Ellipse { min, max } => Some((min, max)),
            ShapeGeom::Line { .. } => None,
        }
    }
}

fn shape_path(geom: &ShapeGeom) -> BezPath {
    match *geom {
        ShapeGeom::Rect {
            min,
            max,
            corner_radius,
        } => {
            let rect = kurbo::Rect::new(min[0], min[1], max[0], max[1]).abs();
            if corner_radius <= 0.0 {
                rect.to_path(SHAPE_FLATNESS)
            } else {
                kurbo::RoundedRect::from_rect(rect, corner_radius).to_path(SHAPE_FLATNESS)
            }
        }
        ShapeGeom::Ellipse { min, max } => {
            let rect = kurbo::Rect::new(min[0], min[1], max[0], max[1]).abs();
            kurbo::Ellipse::from_rect(rect).to_path(SHAPE_FLATNESS)
        }
        ShapeGeom::Line { start, end } => kurbo::Line::new(
            kurbo::Point::new(start[0], start[1]),
            kurbo::Point::new(end[0], end[1]),
        )
        .to_path(SHAPE_FLATNESS),
    }
}

fn stroke_path(path: &BezPath, color: &PaletteColor, line: bool) -> BezPath {
    let width = f64::from(color.size);
    let mut style = kurbo::Stroke::new(width).with_miter_limit(4.0);
    style = if line {
        style.with_caps(match color.cap {
            Cap::Round => kurbo::Cap::Round,
            Cap::Flat => kurbo::Cap::Butt,
        })
    } else {
        style.with_join(Join::Miter).with_caps(kurbo::Cap::Butt)
    };
    if color.dash == Dash::Dashed {
        style = style.with_dashes(0.0, [width * 3.0, width * 2.0]);
    }
    kurbo::stroke(path, &style, &kurbo::StrokeOpts::default(), SHAPE_FLATNESS)
}

fn flatten_path(path: &BezPath) -> BezPath {
    let mut flat = BezPath::new();
    kurbo::flatten(path, SHAPE_FLATNESS, |element| flat.push(element));
    flat
}

fn path_rings(path: &BezPath) -> ink::Shapes {
    let mut shapes = Vec::new();
    let mut ring = Vec::new();
    for element in path.elements() {
        match element {
            PathEl::MoveTo(point) => {
                push_ring(&mut shapes, &mut ring);
                ring.push([point.x, point.y]);
            }
            PathEl::LineTo(point) => ring.push([point.x, point.y]),
            PathEl::ClosePath => push_ring(&mut shapes, &mut ring),
            PathEl::QuadTo(_, point) | PathEl::CurveTo(_, _, point) => {
                ring.push([point.x, point.y]);
            }
        }
    }
    push_ring(&mut shapes, &mut ring);
    shapes
}

fn push_ring(shapes: &mut ink::Shapes, ring: &mut Vec<Point>) {
    if ring.len() >= 3 {
        shapes.push(vec![std::mem::take(ring)]);
    } else {
        ring.clear();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextFamily {
    Inter,
    Monospace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
    /// Wrapped lines stretch to both edges of the box; the last line is left.
    Justify,
}

/// Where a text's lines sit in a box taller than they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VAlign {
    Top,
    Middle,
    Bottom,
}

impl VAlign {
    /// The share of the spare height above the lines.
    pub fn share(self) -> f64 {
        match self {
            VAlign::Top => 0.0,
            VAlign::Middle => 0.5,
            VAlign::Bottom => 1.0,
        }
    }
}

/// Every line of a text object as a list item. Markers are drawn, not stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListKind {
    Bullets,
    Numbers,
    Checks,
}

/// Words are stored as typed. All caps is a style, applied when the text is drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub family: TextFamily,
    pub size: u8,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub align: TextAlign,
    pub valign: VAlign,
    pub line_height: f64,
    pub letter_spacing: f64,
    pub all_caps: bool,
    pub wrap: bool,
    /// Space between the box's edge and its words, in document units.
    pub padding: f64,
    /// The box's corner radius, in document units: its background's corners.
    pub radius: f64,
    /// `None` is plain lines. The kind is kept while the list is off, so the
    /// list button turns the last kind back on.
    pub list: Option<ListKind>,
    pub list_kind: ListKind,
}

impl TextStyle {
    pub fn builtin() -> Self {
        Self {
            family: TextFamily::Inter,
            size: defaults::FONT_SIZE,
            bold: false,
            italic: false,
            underline: false,
            align: TextAlign::Left,
            valign: VAlign::Top,
            line_height: defaults::LINE_HEIGHT,
            letter_spacing: 0.0,
            all_caps: false,
            wrap: true,
            padding: 0.0,
            radius: 0.0,
            list: None,
            list_kind: ListKind::Bullets,
        }
    }
}

impl Inspect for TextStyle {
    fn read(&self, id: PropId) -> Option<PropValue> {
        match id {
            PropId::Family => Some(PropValue::Index(match self.family {
                TextFamily::Inter => 0,
                TextFamily::Monospace => 1,
            })),
            PropId::FontSize => Some(PropValue::Number(f64::from(self.size))),
            PropId::Bold => Some(PropValue::Bool(self.bold)),
            PropId::Italic => Some(PropValue::Bool(self.italic)),
            PropId::Underline => Some(PropValue::Bool(self.underline)),
            PropId::Align => Some(PropValue::Index(match self.align {
                TextAlign::Left => 0,
                TextAlign::Center => 1,
                TextAlign::Right => 2,
                TextAlign::Justify => 3,
            })),
            PropId::VAlign => Some(PropValue::Index(match self.valign {
                VAlign::Top => 0,
                VAlign::Middle => 1,
                VAlign::Bottom => 2,
            })),
            PropId::List => Some(PropValue::Index(match self.list {
                None => 0,
                Some(ListKind::Bullets) => 1,
                Some(ListKind::Numbers) => 2,
                Some(ListKind::Checks) => 3,
            })),
            PropId::TextPadding => Some(PropValue::Number(self.padding)),
            PropId::TextRadius => Some(PropValue::Number(self.radius)),
            PropId::LineHeight => Some(PropValue::Number(self.line_height)),
            PropId::LetterSpacing => Some(PropValue::Number(self.letter_spacing)),
            PropId::AllCaps => Some(PropValue::Bool(self.all_caps)),
            PropId::WrapText => Some(PropValue::Bool(self.wrap)),
            _ => None,
        }
    }

    fn write(&mut self, id: PropId, value: &PropValue) -> bool {
        match id {
            PropId::Family => {
                let PropValue::Index(index) = value else {
                    return false;
                };
                self.family = match index {
                    0 => TextFamily::Inter,
                    1 => TextFamily::Monospace,
                    _ => return false,
                };
                true
            }
            PropId::FontSize => write_u8(
                &mut self.size,
                defaults::FONT_MIN,
                defaults::FONT_MAX,
                value,
            ),
            PropId::Bold => write_bool(&mut self.bold, value),
            PropId::Italic => write_bool(&mut self.italic, value),
            PropId::Underline => write_bool(&mut self.underline, value),
            PropId::Align => {
                let PropValue::Index(index) = value else {
                    return false;
                };
                self.align = match index {
                    0 => TextAlign::Left,
                    1 => TextAlign::Center,
                    2 => TextAlign::Right,
                    3 => TextAlign::Justify,
                    _ => return false,
                };
                true
            }
            PropId::VAlign => {
                let PropValue::Index(index) = value else {
                    return false;
                };
                self.valign = match index {
                    0 => VAlign::Top,
                    1 => VAlign::Middle,
                    2 => VAlign::Bottom,
                    _ => return false,
                };
                true
            }
            PropId::List => {
                let PropValue::Index(index) = value else {
                    return false;
                };
                self.list = match index {
                    0 => None,
                    1 => Some(ListKind::Bullets),
                    2 => Some(ListKind::Numbers),
                    3 => Some(ListKind::Checks),
                    _ => return false,
                };
                if let Some(kind) = self.list {
                    self.list_kind = kind;
                }
                true
            }
            PropId::TextPadding => write_range(
                &mut self.padding,
                defaults::TEXT_PADDING_MIN,
                defaults::TEXT_PADDING_MAX,
                value,
            ),
            PropId::TextRadius => write_range(
                &mut self.radius,
                defaults::TEXT_RADIUS_MIN,
                defaults::TEXT_RADIUS_MAX,
                value,
            ),
            PropId::LineHeight => write_range(
                &mut self.line_height,
                defaults::LINE_HEIGHT_MIN,
                defaults::LINE_HEIGHT_MAX,
                value,
            ),
            PropId::LetterSpacing => write_range(
                &mut self.letter_spacing,
                defaults::LETTER_SPACING_MIN,
                defaults::LETTER_SPACING_MAX,
                value,
            ),
            PropId::AllCaps => write_bool(&mut self.all_caps, value),
            PropId::WrapText => write_bool(&mut self.wrap, value),
            _ => false,
        }
    }
}

/// Window settings for the text tool. Auto-size is the default shown for the
/// next text; a click or a drag decides the object's own box.
#[derive(Clone, Debug, PartialEq)]
pub struct TextTool {
    pub style: TextStyle,
    /// Text, Background, then Border.
    pub palette: Vec<ToolColor>,
    pub auto_size: bool,
}

impl TextTool {
    pub fn builtin(ids: &mut Allocator) -> Self {
        Self {
            style: TextStyle::builtin(),
            palette: text_palette(ids),
            auto_size: true,
        }
    }

    pub fn read(&self, id: PropId) -> Option<PropValue> {
        match id {
            PropId::AutoSize => Some(PropValue::Bool(self.auto_size)),
            _ => self.style.read(id),
        }
    }

    pub fn write(&mut self, id: PropId, value: &PropValue) -> bool {
        match id {
            PropId::AutoSize => write_bool(&mut self.auto_size, value),
            _ => self.style.write(id, value),
        }
    }
}

/// One text object. `box_width` absent means auto-size. Layout is measured at
/// paint time; `measured` remembers the last box so hit testing can use it.
#[derive(Clone, Debug)]
struct TextNode {
    content: String,
    origin: Point,
    box_width: Option<f64>,
    /// A box taller than its lines, set by a top or bottom handle. The lines
    /// sit in it by the style's vertical alignment. Never less than the
    /// lines: a shorter box is as tall as they are.
    box_height: Option<f64>,
    /// Width kept when auto-size is turned off, so undo restores it.
    fixed_width: f64,
    style: TextStyle,
    /// Text, Background, then Border.
    palette: Vec<PaletteColor>,
    measured: Cell<(f64, f64)>,
    /// The height of the lines and padding alone, from the same paint.
    natural: Cell<f64>,
}

impl TextNode {
    fn view(&self, id: NodeId, world: Affine) -> TextView {
        TextView {
            id,
            content: self.content.clone(),
            style: self.style.clone(),
            look: TextLook::of(&self.palette),
            origin: self.origin,
            box_width: self.box_width,
            box_height: self.box_height,
            world,
        }
    }

    fn frame(&self) -> TextBox {
        TextBox {
            origin: self.origin,
            width: self.box_width,
            height: self.box_height,
        }
    }

    fn local_size(&self) -> (f64, f64) {
        let (width, height) = self.measured.get();
        if width > 0.0 && height > 0.0 {
            return (self.box_width.unwrap_or(width), height);
        }
        estimate_text_size(self)
    }

    fn read(&self, id: PropId) -> Option<PropValue> {
        match id {
            PropId::AutoSize => Some(PropValue::Bool(self.box_width.is_none())),
            PropId::Palette => Some(PropValue::Idle),
            _ => self.style.read(id),
        }
    }

    fn write(&mut self, id: PropId, value: &PropValue) -> bool {
        let ok = match id {
            PropId::AutoSize => {
                let PropValue::Bool(on) = value else {
                    return false;
                };
                if *on {
                    self.box_width = None;
                    self.box_height = None;
                } else if self.box_width.is_none() {
                    let width = self.fixed_fallback();
                    self.fixed_width = width;
                    self.box_width = Some(width);
                }
                true
            }
            _ => self.style.write(id, value),
        };
        if ok {
            self.measured.set((0.0, 0.0));
        }
        ok
    }

    fn fixed_fallback(&self) -> f64 {
        if self.fixed_width > 1.0 {
            return self.fixed_width;
        }
        let (width, _) = self.measured.get();
        if width > 1.0 {
            width
        } else {
            f64::from(self.style.size) * 8.0
        }
    }
}

/// A stand-in box until egui has measured the text. Paint replaces it.
fn estimate_text_size(text: &TextNode) -> (f64, f64) {
    let shown = if text.style.all_caps {
        text.content.to_uppercase()
    } else {
        text.content.clone()
    };
    let em = f64::from(text.style.size);
    let advance = (em * 0.5 + text.style.letter_spacing).max(0.2);
    let line_h = (em * text.style.line_height).max(1.0);
    let lines: Vec<&str> = if shown.is_empty() {
        vec![""]
    } else {
        shown.split('\n').collect()
    };
    let mut width: f64 = 0.0;
    let mut height: f64 = 0.0;
    for line in lines {
        let natural = line.chars().count() as f64 * advance;
        let rows = match text.box_width {
            Some(limit) if text.style.wrap && limit >= advance && natural > limit => {
                (natural / limit).ceil().max(1.0)
            }
            _ => 1.0,
        };
        width = width.max(text.box_width.unwrap_or(natural));
        height += rows * line_h;
    }
    let pad = 2.0 * text.style.padding.max(0.0);
    let height = height.max(line_h) + pad;
    (
        width.max(em) + pad,
        text.box_height.map_or(height, |box_h| box_h.max(height)),
    )
}

fn write_range(slot: &mut f64, min: f64, max: f64, value: &PropValue) -> bool {
    let PropValue::Number(number) = value else {
        return false;
    };
    if !number.is_finite() {
        return false;
    }
    *slot = number.clamp(min, max);
    true
}

/// Text's colors, from its palette: the glyphs, and the box fill when the
/// Background is on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextLook {
    pub ink: [u8; 3],
    pub opacity: f64,
    pub background: Option<([u8; 3], f64)>,
    pub border: Option<TextBorder>,
}

/// The line around a text box: its color, opacity, width in the text's own
/// units, and dash.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextBorder {
    pub rgb: [u8; 3],
    pub opacity: f64,
    pub size: f64,
    pub dash: Dash,
}

impl TextLook {
    pub fn of(palette: &[PaletteColor]) -> Self {
        let text = palette.iter().find(|color| color.role == ColorRole::Text);
        let background = palette
            .iter()
            .find(|color| color.role == ColorRole::Background && color.visible)
            .map(|color| (color.rgb, color.opacity));
        let border = palette
            .iter()
            .find(|color| color.role == ColorRole::Border && color.visible)
            .map(|color| TextBorder {
                rgb: color.rgb,
                opacity: color.opacity,
                size: f64::from(color.size),
                dash: color.dash,
            });
        Self {
            ink: text.map_or(defaults::TEXT_RGB, |color| color.rgb),
            opacity: text
                .filter(|color| color.visible)
                .map_or(0.0, |color| color.opacity),
            background,
            border,
        }
    }

    pub fn of_tool(palette: &[ToolColor]) -> Self {
        let colors: Vec<PaletteColor> = palette.iter().map(|color| color.color.clone()).collect();
        Self::of(&colors)
    }
}

/// A visible text object, in paint order, for the frame to lay out with egui.
pub struct TextView {
    pub id: NodeId,
    pub content: String,
    pub style: TextStyle,
    pub look: TextLook,
    pub origin: Point,
    pub box_width: Option<f64>,
    pub box_height: Option<f64>,
    pub world: Affine,
}

/// A text's box in its own units: its origin, and a width and a height
/// that are `None` while the text sizes itself that way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextBox {
    pub origin: Point,
    pub width: Option<f64>,
    pub height: Option<f64>,
}

/// A text box as it is drawn: its frame, its drawn width and height, and
/// the height of its lines and padding alone, all in its own units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextFit {
    pub frame: TextBox,
    pub width: f64,
    pub height: f64,
    pub natural: f64,
}

pub enum SceneMark {
    Ink(InkPaint),
    Text(TextView),
}

#[cfg(test)]
pub struct TextRecord {
    pub content: String,
    pub box_width: Option<f64>,
    pub box_height: Option<f64>,
    pub size: u8,
    pub bold: bool,
    pub wrap: bool,
}

enum NodeKind {
    Canvas(Canvas),
    Group(Vec<Node>),
    Shape(ShapeNode),
    Text(TextNode),
}

pub struct Node {
    id: NodeId,
    name: String,
    visible: bool,
    locked: bool,
    transform: Affine,
    kind: NodeKind,
}

impl Node {
    fn canvas(&self) -> Option<&Canvas> {
        match &self.kind {
            NodeKind::Canvas(canvas) => Some(canvas),
            NodeKind::Group(_) | NodeKind::Shape(_) | NodeKind::Text(_) => None,
        }
    }

    fn canvas_mut(&mut self) -> Option<&mut Canvas> {
        match &mut self.kind {
            NodeKind::Canvas(canvas) => Some(canvas),
            NodeKind::Group(_) | NodeKind::Shape(_) | NodeKind::Text(_) => None,
        }
    }

    fn shape(&self) -> Option<&ShapeNode> {
        match &self.kind {
            NodeKind::Shape(shape) => Some(shape),
            NodeKind::Canvas(_) | NodeKind::Group(_) | NodeKind::Text(_) => None,
        }
    }

    fn shape_mut(&mut self) -> Option<&mut ShapeNode> {
        match &mut self.kind {
            NodeKind::Shape(shape) => Some(shape),
            NodeKind::Canvas(_) | NodeKind::Group(_) | NodeKind::Text(_) => None,
        }
    }

    fn is_group(&self) -> bool {
        matches!(self.kind, NodeKind::Group(_))
    }

    fn text(&self) -> Option<&TextNode> {
        match &self.kind {
            NodeKind::Text(text) => Some(text),
            NodeKind::Canvas(_) | NodeKind::Group(_) | NodeKind::Shape(_) => None,
        }
    }

    fn text_mut(&mut self) -> Option<&mut TextNode> {
        match &mut self.kind {
            NodeKind::Text(text) => Some(text),
            NodeKind::Canvas(_) | NodeKind::Group(_) | NodeKind::Shape(_) => None,
        }
    }

    /// A subtree for one paste: every id is new, then the root shifts by
    /// `delta` in document space. Children keep their places inside it.
    pub fn duplicate_for_paste(&self, ids: &mut Allocator, delta: Point) -> Self {
        let mut copy = self.clone();
        rekey_node(&mut copy, ids);
        copy.transform = Affine::translate(kurbo::Vec2::new(delta[0], delta[1])) * copy.transform;
        copy
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub origin: Point,
    pub zoom: f64,
}

impl Camera {
    fn centered(viewport: Point) -> Self {
        Self {
            origin: [-viewport[0] / 2.0, -viewport[1] / 2.0],
            zoom: 1.0,
        }
    }
}

/// Reversible edits. Forward applies the change; backward applies its inverse.
#[derive(Clone, Debug)]
pub enum Step {
    AddStroke {
        canvas: NodeId,
        color: ColorId,
        stroke: Stroke,
    },
    AddErases {
        added: Vec<(StrokeId, ErasePath)>,
    },
    /// A palette color of a canvas, a shape, or a text object.
    SetProps {
        node: NodeId,
        color: ColorId,
        before: Vec<(PropId, PropValue)>,
        after: Vec<(PropId, PropValue)>,
    },
    InsertColor {
        canvas: NodeId,
        index: usize,
        layer: ColorLayer,
    },
    RemoveColor {
        canvas: NodeId,
        index: usize,
        layer: ColorLayer,
    },
    MoveColor {
        canvas: NodeId,
        from: usize,
        to: usize,
    },
    /// Inserts `nodes` as siblings at `index`. `parent` none is the root.
    InsertNodes {
        parent: Option<NodeId>,
        index: usize,
        nodes: Vec<Node>,
    },
    /// Removes the siblings stored here. Indexes are their positions before the removal.
    RemoveNodes {
        parent: Option<NodeId>,
        nodes: Vec<(usize, Node)>,
    },
    /// Moves one sibling from `from` to `to` under the same parent.
    MoveNode {
        parent: Option<NodeId>,
        from: usize,
        to: usize,
    },
    /// Local transforms. Geometry and samples stay as they are.
    SetTransforms {
        changes: Vec<(NodeId, Affine, Affine)>,
    },
    SetNodeProps {
        id: NodeId,
        before: Vec<(PropId, PropValue)>,
        after: Vec<(PropId, PropValue)>,
    },
    /// One undo step made of several reversible edits, applied in order.
    Batch(Vec<Step>),
    Group {
        parent: Option<NodeId>,
        index: usize,
        group: Node,
        child_indexes: Vec<usize>,
    },
    Ungroup {
        parent: Option<NodeId>,
        index: usize,
        group: Node,
        released: Vec<Node>,
    },
    /// One text object's box: its local origin, width, and height, `None`
    /// for sizing itself. A handle's drag changes it; the words re-wrap.
    SetTextBox {
        id: NodeId,
        before: TextBox,
        after: TextBox,
    },
    /// Replaces one text object's words. Style and the box stay.
    SetText {
        id: NodeId,
        before: String,
        after: String,
    },
}

/// The parts of a color that shape its strokes' outlines.
pub struct ColorStyle {
    pub size: u8,
    pub cap: Cap,
    pub dash: Dash,
    pub rgb: [u8; 3],
    pub opacity: f64,
}

/// The brush stroke still being drawn, outlined in its canvas's local space
/// exactly as it will be once committed. It paints inside its color layer:
/// above that color's older strokes and below every color above it.
pub struct LiveStroke {
    /// `None` while the stroke will make a new canvas: it paints on top.
    pub canvas: Option<NodeId>,
    /// The layer it joins. `None` when the layer does not exist yet: the
    /// commit adds it at the bottom of the palette, so the stroke paints
    /// below every layer, in `look`.
    pub color: Option<ColorId>,
    pub look: ([u8; 3], f64),
    pub shapes: Shapes,
}

pub struct InkPaint {
    pub rgb: [u8; 3],
    pub opacity: f64,
    pub eraser_preview: bool,
    pub triangles: Vec<[Point; 3]>,
}

pub struct Sketch {
    id: SketchId,
    name: String,
    generated_name: String,
    /// A save of this id has succeeded. Later edits set `dirty`.
    stored: bool,
    /// Content, the name, or the camera differs from the last successful save.
    dirty: bool,
    /// The `.smx` later saves write. Absent until Save or Save As succeeds.
    path: Option<PathBuf>,
    /// Recovery file id, when a restore kept a file named with the old id.
    recovery_file: Option<SketchId>,
    origin_canvas: NodeId,
    /// The layers this sketch's canvas was created with. A clean sketch still matches them.
    born: Vec<PaletteColor>,
    nodes: Vec<Node>,
    selection: Vec<NodeId>,
    /// The canvas the brush and eraser work on. Selecting one canvas makes it
    /// active; selecting a shape or text, or making one, leaves it.
    active_canvas: Option<NodeId>,
    camera: Camera,
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl Sketch {
    pub fn untitled(
        id: SketchId,
        name: String,
        ids: &mut Allocator,
        palette: &[ToolColor],
    ) -> Self {
        let canvas_id = ids.node();
        let canvas = canvas_node(canvas_id, "Canvas 1", palette, ids);
        let born = layer_looks(&canvas);
        Self {
            id,
            generated_name: name.clone(),
            name,
            stored: false,
            dirty: false,
            path: None,
            recovery_file: None,
            origin_canvas: canvas_id,
            born,
            nodes: vec![canvas],
            selection: vec![canvas_id],
            active_canvas: Some(canvas_id),
            camera: Camera {
                origin: [0.0, 0.0],
                zoom: 1.0,
            },
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn id(&self) -> SketchId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: String) {
        if self.name == name {
            return;
        }
        self.name = name;
        self.dirty = true;
    }

    pub fn canvas_name(&self) -> Option<&str> {
        self.nodes
            .iter()
            .find(|node| node.id == self.origin_canvas)
            .map(|node| node.name.as_str())
    }

    /// Starting canvas only, no strokes, the default palette, the generated
    /// name, and never saved. Any color may be active. A sketch that has been
    /// saved is not this blank, even when nothing has changed since the save.
    pub fn is_clean(&self) -> bool {
        if self.stored || self.name != self.generated_name {
            return false;
        }
        if self.nodes.len() != 1 || self.nodes[0].id != self.origin_canvas {
            return false;
        }
        let node = &self.nodes[0];
        if !node.visible || node.locked || node.transform != Affine::IDENTITY {
            return false;
        }
        let Some(canvas) = node.canvas() else {
            return false;
        };
        // The active color is a view choice, like the camera, so it does not
        // make a sketch unclean. Padding and the born palette do.
        if (canvas.padding - defaults::CANVAS_PADDING).abs() > 1.0e-9 {
            return false;
        }
        if canvas.colors.len() != self.born.len() {
            return false;
        }
        canvas.colors.iter().zip(&self.born).all(|(layer, born)| {
            layer.strokes.is_empty() && !layer.locked && layer.color.same_look(born)
        })
    }

    /// Changes since the last save, or a renamed title that has not been saved.
    /// Closing asks once. A saved sketch with no later edits does not ask.
    pub fn needs_keep(&self) -> bool {
        if self.stored {
            self.dirty
        } else {
            !self.is_clean()
        }
    }

    /// A save of this id succeeded. The sketch matches that file.
    pub fn mark_saved(&mut self) {
        self.stored = true;
        self.dirty = false;
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    pub fn set_path(&mut self, path: Option<PathBuf>) {
        self.path = path;
    }

    pub fn recovery_file(&self) -> Option<SketchId> {
        self.recovery_file
    }

    pub fn set_recovery_file(&mut self, id: SketchId) {
        self.recovery_file = Some(id);
    }

    pub fn set_id(&mut self, id: SketchId) {
        self.id = id;
    }

    /// Opened from a `.smx` or restored from recovery. The stored camera stays.
    pub fn keeps_stored_view(&self) -> bool {
        self.stored || self.path.is_some() || self.recovery_file.is_some()
    }

    /// Centers a blank on the viewport and leaves the dirty flag as it was.
    pub fn center_without_dirty(&mut self, viewport: Point) {
        if viewport[0] <= 0.0
            || viewport[1] <= 0.0
            || !viewport[0].is_finite()
            || !viewport[1].is_finite()
        {
            return;
        }
        self.camera = Camera::centered(viewport);
    }

    pub fn set_dirty(&mut self, dirty: bool) {
        self.dirty = dirty;
    }

    pub fn clear_redo(&mut self) {
        self.redo.clear();
    }

    /// A `.smx` just opened. No selection and no highlighted canvas.
    pub fn mark_opened(&mut self, path: PathBuf, title: String) {
        self.name = title.clone();
        self.generated_name = title;
        self.path = Some(path);
        self.stored = true;
        self.dirty = false;
        self.recovery_file = None;
        self.selection.clear();
        self.active_canvas = None;
        self.undo.clear();
        self.redo.clear();
    }

    /// A recovery copy just opened. Still unsaved. The original path is kept
    /// when the recovery file stored one.
    pub fn mark_restored(
        &mut self,
        path: Option<PathBuf>,
        title: String,
        file_id: SketchId,
    ) {
        self.name = title.clone();
        self.generated_name = title;
        self.stored = path.is_some();
        self.path = path;
        self.dirty = true;
        self.recovery_file = Some(file_id);
        self.selection.clear();
        self.active_canvas = None;
        self.undo.clear();
        self.redo.clear();
    }

    pub fn collect_held(&self, held: &mut HeldIds) {
        held.sketches.insert(self.id.get());
        for node in &self.nodes {
            collect_held_node(node, held);
        }
    }

    /// Ids this sketch shares with `held` become new ones. A layer's `source`
    /// stays as stored: it names a brush color, not this sketch's palette.
    pub fn separate_ids(
        &mut self,
        held: &HeldIds,
        ids: &mut Allocator,
        next_sketch: &mut u64,
    ) {
        if held.sketches.contains(&self.id.get()) {
            self.id = SketchId::new(*next_sketch);
            *next_sketch += 1;
        }
        for node in &mut self.nodes {
            separate_node(node, held, ids, &mut self.origin_canvas);
        }
    }

    fn note_view(&mut self, before: Camera) {
        if self.camera.origin != before.origin || self.camera.zoom != before.zoom {
            self.dirty = true;
        }
    }

    /// A clean opening sketch takes a palette loaded after it was made, so
    /// its canvas starts from the brush's saved colors.
    pub fn adopt_palette(&mut self, palette: &[ToolColor], ids: &mut Allocator) {
        if palette.is_empty() || !self.is_clean() {
            return;
        }
        let canvas_id = self.origin_canvas;
        let canvas = canvas_node(canvas_id, "Canvas 1", palette, ids);
        self.born = layer_looks(&canvas);
        self.nodes = vec![canvas];
        self.selection = vec![canvas_id];
        self.active_canvas = Some(canvas_id);
    }

    pub fn camera(&self) -> Camera {
        self.camera
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    pub fn highlighted_id(&self) -> Option<NodeId> {
        self.highlighted().map(|node| node.id)
    }

    /// The active canvas, while it and its ancestors are visible and unlocked.
    fn highlighted(&self) -> Option<&Node> {
        let id = self.active_canvas?;
        let node = find_node(&self.nodes, id)?;
        let place = placement(&self.nodes, id)?;
        if node.visible
            && !node.locked
            && place.ancestors_visible
            && place.ancestors_unlocked
            && node.canvas().is_some()
        {
            Some(node)
        } else {
            None
        }
    }

    /// The brush can draw with `source`, a brush color, on the highlighted
    /// canvas: the layer that came from it is visible and unlocked, or the
    /// canvas does not have one yet and the stroke will add it.
    pub fn brush_ready(&self, source: ColorId) -> bool {
        let Some(canvas) = self.highlighted().and_then(Node::canvas) else {
            return false;
        };
        canvas
            .colors
            .iter()
            .find(|layer| layer.source == Some(source))
            .is_none_or(|layer| layer.visible() && !layer.locked)
    }

    /// The highlighted canvas's layer that came from `source`, a brush color.
    pub fn layer_for_source(&self, canvas: NodeId, source: ColorId) -> Option<ColorId> {
        self.color_slice(canvas)?
            .iter()
            .find(|layer| layer.source == Some(source))
            .map(ColorLayer::id)
    }

    /// The layer the brush would draw into, and whether it can be drawn on.
    pub fn source_layer_flags(
        &self,
        canvas: NodeId,
        source: ColorId,
    ) -> Option<(bool, bool)> {
        self.color_slice(canvas)?
            .iter()
            .find(|layer| layer.source == Some(source))
            .map(|layer| (layer.visible(), layer.locked))
    }

    pub fn color_size(&self, canvas: NodeId, color: ColorId) -> Option<u8> {
        self.color(canvas, color).map(ColorLayer::size)
    }

    pub fn color_style(&self, canvas: NodeId, color: ColorId) -> Option<ColorStyle> {
        let layer = self.color(canvas, color)?;
        Some(ColorStyle {
            size: layer.color.size,
            cap: layer.color.cap,
            dash: layer.color.dash,
            rgb: layer.color.rgb,
            opacity: layer.color.opacity,
        })
    }

    pub fn color_index(&self, canvas: NodeId, color: ColorId) -> Option<usize> {
        self.color_slice(canvas)?
            .iter()
            .position(|layer| layer.id() == color)
    }

    pub fn palette_len(&self, canvas: NodeId) -> Option<usize> {
        self.color_slice(canvas).map(|colors| colors.len())
    }

    /// A property of a palette color owned by `node`: a canvas layer, or a
    /// color of a shape's or text's palette.
    pub fn color_prop(&self, node: NodeId, color: ColorId, id: PropId) -> Option<PropValue> {
        match &find_node(&self.nodes, node)?.kind {
            NodeKind::Canvas(canvas) => canvas
                .colors
                .iter()
                .find(|layer| layer.id() == color)?
                .read(id),
            NodeKind::Shape(shape) => shape.palette.iter().find(|item| item.id == color)?.read(id),
            NodeKind::Text(text) => text.palette.iter().find(|item| item.id == color)?.read(id),
            NodeKind::Group(_) => None,
        }
    }

    pub fn set_color_prop(
        &mut self,
        node: NodeId,
        color: ColorId,
        id: PropId,
        value: &PropValue,
    ) -> bool {
        let Some(node) = find_node_mut(&mut self.nodes, node) else {
            return false;
        };
        match &mut node.kind {
            NodeKind::Canvas(canvas) => canvas
                .colors
                .iter_mut()
                .find(|layer| layer.id() == color)
                .is_some_and(|layer| layer.write(id, value)),
            NodeKind::Shape(shape) => {
                let ok = shape
                    .palette
                    .iter_mut()
                    .find(|item| item.id == color)
                    .is_some_and(|item| item.write(id, value));
                if ok {
                    shape.invalidate();
                }
                ok
            }
            NodeKind::Text(text) => text
                .palette
                .iter_mut()
                .find(|item| item.id == color)
                .is_some_and(|item| item.write(id, value)),
            NodeKind::Group(_) => false,
        }
    }

    /// The colors a node owns, in palette order: a canvas's layers, or a
    /// shape's or text's roles.
    pub fn node_palette(&self, node: NodeId) -> Option<Vec<PaletteColor>> {
        match &find_node(&self.nodes, node)?.kind {
            NodeKind::Canvas(canvas) => Some(
                canvas
                    .colors
                    .iter()
                    .map(|layer| layer.color.clone())
                    .collect(),
            ),
            NodeKind::Shape(shape) => Some(shape.palette.clone()),
            NodeKind::Text(text) => Some(text.palette.clone()),
            NodeKind::Group(_) => None,
        }
    }

    pub fn has_color(&self, node: NodeId, color: ColorId) -> bool {
        self.node_palette(node)
            .is_some_and(|colors| colors.iter().any(|item| item.id == color))
    }

    pub fn color_id_at(&self, canvas: NodeId, index: usize) -> Option<ColorId> {
        self.color_slice(canvas)?.get(index).map(ColorLayer::id)
    }

    pub fn duplicate_color(
        &mut self,
        canvas: NodeId,
        color: ColorId,
        ids: &mut Allocator,
    ) -> bool {
        let Some(index) = self.color_index(canvas, color) else {
            return false;
        };
        let Some(layer) = self.color(canvas, color).map(|layer| layer.duplicate(ids)) else {
            return false;
        };
        self.commit_step(Step::InsertColor {
            canvas,
            index: index + 1,
            layer,
        });
        true
    }

    /// Removes the color and its strokes. The last color on a canvas stays.
    pub fn delete_color(&mut self, canvas: NodeId, color: ColorId) -> bool {
        let Some(len) = self.palette_len(canvas) else {
            return false;
        };
        if len <= 1 {
            return false;
        }
        let Some(index) = self.color_index(canvas, color) else {
            return false;
        };
        let Some(layer) = self.color(canvas, color).cloned() else {
            return false;
        };
        self.commit_step(Step::RemoveColor {
            canvas,
            index,
            layer,
        });
        true
    }

    pub fn commit_step(&mut self, step: Step) {
        self.dirty = true;
        self.redo.clear();
        self.apply(true, &step);
        self.undo.push(step);
    }

    /// Stores a camera the host already changed. No undo step.
    pub fn store_camera(&mut self, camera: Camera) {
        let before = self.camera;
        if camera.origin[0].is_finite()
            && camera.origin[1].is_finite()
            && camera.zoom.is_finite()
            && camera.zoom > 0.0
        {
            self.camera = Camera {
                origin: camera.origin,
                zoom: camera.zoom.clamp(MIN_ZOOM, MAX_ZOOM),
            };
        }
        self.note_view(before);
    }

    /// Replaces nodes and camera from a snapshot of this sketch. The undo
    /// stack, the selection, the name, and the path stay.
    pub fn install_nodes_and_camera(&mut self, mut incoming: Sketch) -> bool {
        if incoming.id != self.id {
            return false;
        }
        self.nodes = std::mem::take(&mut incoming.nodes);
        self.camera = incoming.camera;
        true
    }

    /// Builds one undo step from the difference between this sketch and a
    /// snapshot the host already applied. A difference the existing step
    /// variants cannot record is left untouched.
    pub fn record_snapshot(&mut self, incoming: &Sketch) -> bool {
        if incoming.id != self.id {
            return false;
        }
        let Some(step) = diff_committed(&self.nodes, &incoming.nodes) else {
            return false;
        };
        self.commit_step(step);
        self.store_camera(incoming.camera);
        true
    }

    pub fn undo(&mut self) -> bool {
        let Some(step) = self.undo.pop() else {
            return false;
        };
        self.dirty = true;
        self.apply(false, &step);
        self.redo.push(step);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else {
            return false;
        };
        self.dirty = true;
        self.apply(true, &step);
        self.undo.push(step);
        true
    }

    pub fn translate_node(&mut self, id: NodeId, delta: Point) -> bool {
        let Some(node) = find_node_mut(&mut self.nodes, id) else {
            return false;
        };
        node.transform = Affine::translate(kurbo::Vec2::new(delta[0], delta[1])) * node.transform;
        true
    }

    pub fn world_transform(&self, id: NodeId) -> Option<Affine> {
        transform_of(&self.nodes, id, Affine::IDENTITY).map(|(_, world)| world)
    }

    fn parent_world(&self, id: NodeId) -> Option<Affine> {
        transform_of(&self.nodes, id, Affine::IDENTITY).map(|(parent, _)| parent)
    }

    pub fn stroke_count(&self) -> usize {
        fn count(nodes: &[Node]) -> usize {
            nodes
                .iter()
                .map(|node| match &node.kind {
                    NodeKind::Canvas(canvas) => {
                        canvas.colors.iter().map(|color| color.strokes.len()).sum()
                    }
                    NodeKind::Group(children) => count(children),
                    NodeKind::Shape(_) | NodeKind::Text(_) => 0,
                })
                .sum()
        }
        count(&self.nodes)
    }

    pub fn stroke_ids(&self) -> Vec<StrokeId> {
        fn collect(nodes: &[Node], ids: &mut Vec<StrokeId>) {
            for node in nodes {
                match &node.kind {
                    NodeKind::Canvas(canvas) => {
                        for color in &canvas.colors {
                            ids.extend(color.strokes.iter().map(Stroke::id));
                        }
                    }
                    NodeKind::Group(children) => collect(children, ids),
                    NodeKind::Shape(_) | NodeKind::Text(_) => {}
                }
            }
        }
        let mut ids = Vec::new();
        collect(&self.nodes, &mut ids);
        ids
    }

    pub fn stroke(&self, id: StrokeId) -> Option<&Stroke> {
        self.locate_stroke(id).map(|(_, _, stroke)| stroke)
    }

    pub fn ink_outline(&self, id: StrokeId) -> Option<Vec<Point>> {
        let (transform, layer, stroke) = self.locate_stroke(id)?;
        let ring = ink::raw_outline(
            stroke.samples(),
            layer.diameter(stroke),
            stroke.dynamics(),
            layer.color.cap,
        );
        Some(
            ring.into_iter()
                .map(|point| map_point(transform, point))
                .collect(),
        )
    }

    pub fn erase_outline(&self, id: StrokeId, index: usize) -> Option<Vec<Point>> {
        let (transform, _, stroke) = self.locate_stroke(id)?;
        let path = stroke.erase_paths().get(index)?;
        Some(
            ink::eraser_outline(path)
                .into_iter()
                .map(|point| map_point(transform, point))
                .collect(),
        )
    }

    pub fn visible_polygons(&self, id: StrokeId) -> Option<Shapes> {
        let (transform, layer, stroke) = self.locate_stroke(id)?;
        let shapes = layer.stroke_ink(stroke)?;
        Some(transform_shapes(transform, shapes))
    }

    pub fn visible_at(&self, id: StrokeId, point: Point) -> bool {
        let Some((transform, layer, stroke)) = self.locate_stroke(id) else {
            return false;
        };
        let Some(shapes) = layer.stroke_ink(stroke) else {
            return false;
        };
        ink::point_in_shapes(unmap_point(transform, point), shapes)
    }

    /// The topmost visible, unlocked root node whose ink is under `point`.
    /// A group is the hit, never a child. Hidden and locked roots are skipped.
    pub fn hit_test(&self, point: Point) -> Option<NodeId> {
        for node in &self.nodes {
            if !node.visible || node.locked {
                continue;
            }
            if ink_hit(node, node.transform, point) {
                return Some(node.id);
            }
        }
        None
    }

    /// Root nodes whose visible ink crosses the rectangle. Groups, not children.
    pub fn marquee_hits(&self, min: Point, max: Point) -> Vec<NodeId> {
        let rect = (min, max);
        let mut hits = Vec::new();
        for node in &self.nodes {
            if !node.visible || node.locked {
                continue;
            }
            if ink_crosses(node, node.transform, rect) {
                hits.push(node.id);
            }
        }
        hits
    }

    pub fn scene(&self, live: Option<&LiveStroke>) -> Vec<SceneMark> {
        let mut marks = Vec::new();
        push_scene(&mut marks, &self.nodes, Affine::IDENTITY, live);
        marks
    }

    pub fn strokes_hit_by_eraser(&self, canvas: NodeId, outline: &[Point]) -> Vec<StrokeId> {
        let Some(node) = find_node(&self.nodes, canvas) else {
            return Vec::new();
        };
        let Some(canvas) = node.canvas() else {
            return Vec::new();
        };
        let Some(eraser_bounds) = ink::bounds_of(outline) else {
            return Vec::new();
        };
        let mut hits = Vec::new();
        for layer in &canvas.colors {
            if !layer.visible() || layer.locked {
                continue;
            }
            for stroke in &layer.strokes {
                let Some(shapes) = layer.stroke_ink(stroke) else {
                    continue;
                };
                let Some(ink_bounds) = ink::bounds_of_shapes(shapes) else {
                    continue;
                };
                if ink::bounds_overlap(ink_bounds, eraser_bounds)
                    && ink::polygons_intersect(shapes, outline)
                {
                    hits.push(stroke.id);
                }
            }
        }
        hits
    }

    pub fn pan_screen(&mut self, delta: Point) {
        if !delta[0].is_finite() || !delta[1].is_finite() || self.camera.zoom == 0.0 {
            return;
        }
        let before = self.camera;
        self.camera.origin[0] -= delta[0] / self.camera.zoom;
        self.camera.origin[1] -= delta[1] / self.camera.zoom;
        self.note_view(before);
    }

    pub fn zoom_toward(&mut self, factor: f64, pointer: Point) {
        if !factor.is_finite()
            || factor <= 0.0
            || !pointer[0].is_finite()
            || !pointer[1].is_finite()
            || self.camera.zoom <= 0.0
        {
            return;
        }
        let before = self.camera;
        let old = self.camera.zoom;
        let new = (old * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let doc_x = self.camera.origin[0] + pointer[0] / old;
        let doc_y = self.camera.origin[1] + pointer[1] / old;
        self.camera.zoom = new;
        self.camera.origin = [doc_x - pointer[0] / new, doc_y - pointer[1] / new];
        self.note_view(before);
    }

    pub fn zoom_at_center(&mut self, zoom_in: bool, viewport: Point) {
        let factor = if zoom_in {
            ZOOM_IN_FACTOR
        } else {
            ZOOM_OUT_FACTOR
        };
        self.zoom_toward(factor, [viewport[0] / 2.0, viewport[1] / 2.0]);
    }

    pub fn fit(&mut self, viewport: Point) {
        self.frame_bounds(self.visible_bounds(), viewport);
    }

    /// Frames `bounds` the way Fit frames every visible object. No bounds
    /// returns to the default view centered on the origin.
    pub fn frame_bounds(&mut self, bounds: Option<(Point, Point)>, viewport: Point) {
        if viewport[0] <= 0.0
            || viewport[1] <= 0.0
            || !viewport[0].is_finite()
            || !viewport[1].is_finite()
        {
            return;
        }
        let before = self.camera;
        let Some((min, max)) = bounds else {
            self.camera = Camera::centered(viewport);
            self.note_view(before);
            return;
        };
        let width = (max[0] - min[0]) + 2.0 * FIT_MARGIN;
        let height = (max[1] - min[1]) + 2.0 * FIT_MARGIN;
        let mut zoom = if width > 0.0 && height > 0.0 {
            (viewport[0] / width).min(viewport[1] / height)
        } else {
            MAX_ZOOM
        };
        if !zoom.is_finite() {
            zoom = 1.0;
        }
        zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let center = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0];
        self.camera.zoom = zoom;
        self.camera.origin = [
            center[0] - viewport[0] / (2.0 * zoom),
            center[1] - viewport[1] / (2.0 * zoom),
        ];
        self.note_view(before);
    }

    pub fn zoom_to_selection(&mut self, viewport: Point) -> bool {
        let Some(bounds) = self.selection_bounds() else {
            return false;
        };
        self.frame_bounds(Some(bounds), viewport);
        true
    }

    fn visible_bounds(&self) -> Option<(Point, Point)> {
        accumulate_bounds(&self.nodes, Affine::IDENTITY, None)
    }

    pub fn selection_bounds(&self) -> Option<(Point, Point)> {
        let mut bounds = None;
        for id in &self.selection {
            let Some((parent, world)) = transform_of(&self.nodes, *id, Affine::IDENTITY) else {
                continue;
            };
            let Some(node) = find_node(&self.nodes, *id) else {
                continue;
            };
            if !node.visible {
                continue;
            }
            let _ = parent;
            if let Some(next) = subtree_bounds(node, world) {
                bounds = Some(union_bounds(bounds, next));
            }
        }
        bounds
    }

    fn color_slice(&self, canvas: NodeId) -> Option<&[ColorLayer]> {
        find_node(&self.nodes, canvas)
            .and_then(Node::canvas)
            .map(|canvas| canvas.colors.as_slice())
    }

    fn color(&self, canvas: NodeId, color: ColorId) -> Option<&ColorLayer> {
        self.color_slice(canvas)?
            .iter()
            .find(|layer| layer.id() == color)
    }

    fn canvas_mut(&mut self, id: NodeId) -> Option<&mut Canvas> {
        find_node_mut(&mut self.nodes, id)?.canvas_mut()
    }

    fn color_mut(&mut self, canvas: NodeId, color: ColorId) -> Option<&mut ColorLayer> {
        self.canvas_mut(canvas)?
            .colors
            .iter_mut()
            .find(|layer| layer.id() == color)
    }

    fn locate_stroke(&self, id: StrokeId) -> Option<(Affine, &ColorLayer, &Stroke)> {
        locate_stroke(&self.nodes, Affine::IDENTITY, id)
    }

    fn apply(&mut self, forward: bool, step: &Step) {
        match step {
            Step::AddStroke {
                canvas,
                color,
                stroke,
            } => {
                let Some(layer) = self.color_mut(*canvas, *color) else {
                    return;
                };
                layer.union_cache = OnceCell::new();
                if forward {
                    layer.strokes.push(stroke.clone());
                } else {
                    layer.strokes.retain(|item| item.id != stroke.id);
                }
            }
            Step::AddErases { added } => {
                for (id, path) in added {
                    if forward {
                        self.push_erase(*id, path);
                    } else {
                        self.pop_erase(*id, path);
                    }
                }
            }
            Step::SetProps {
                node,
                color,
                before,
                after,
            } => {
                let values = if forward { after } else { before };
                for (id, value) in values {
                    self.set_color_prop(*node, *color, *id, value);
                }
            }
            Step::InsertColor {
                canvas,
                index,
                layer,
            } => {
                if forward {
                    self.insert_layer(*canvas, *index, layer.clone());
                } else {
                    self.remove_layer(*canvas, layer.id());
                }
            }
            Step::RemoveColor {
                canvas,
                index,
                layer,
            } => {
                if forward {
                    self.remove_layer(*canvas, layer.id());
                } else {
                    self.insert_layer(*canvas, *index, layer.clone());
                }
            }
            Step::MoveColor { canvas, from, to } => {
                let (from, to) = if forward { (*from, *to) } else { (*to, *from) };
                self.relocate_color(*canvas, from, to);
            }
            Step::InsertNodes {
                parent,
                index,
                nodes,
            } => {
                if forward {
                    self.insert_siblings(*parent, *index, nodes);
                } else {
                    for node in nodes {
                        self.remove_id(node.id);
                    }
                }
            }
            Step::RemoveNodes { parent, nodes } => {
                if forward {
                    for (_, node) in nodes.iter().rev() {
                        self.remove_id(node.id);
                    }
                } else if let Some(children) = self.siblings_mut(*parent) {
                    for (index, node) in nodes {
                        let index = (*index).min(children.len());
                        children.insert(index, node.clone());
                    }
                }
            }
            Step::MoveNode { parent, from, to } => {
                let (from, to) = if forward { (*from, *to) } else { (*to, *from) };
                self.move_sibling(*parent, from, to);
            }
            Step::SetTransforms { changes } => {
                for (id, before, after) in changes {
                    let Some(node) = find_node_mut(&mut self.nodes, *id) else {
                        continue;
                    };
                    node.transform = if forward { *after } else { *before };
                }
            }
            Step::SetNodeProps { id, before, after } => {
                let values = if forward { after } else { before };
                for (prop, value) in values {
                    self.write_node_prop(*id, *prop, value);
                }
            }
            Step::Batch(steps) => {
                if forward {
                    for step in steps {
                        self.apply(true, step);
                    }
                } else {
                    for step in steps.iter().rev() {
                        self.apply(false, step);
                    }
                }
            }
            Step::Group {
                parent,
                index,
                group,
                child_indexes,
            } => {
                if forward {
                    let mut indexes = child_indexes.clone();
                    indexes.sort_unstable();
                    indexes.reverse();
                    for child_index in indexes {
                        self.remove_sibling_at(*parent, child_index);
                    }
                    self.insert_siblings(*parent, *index, std::slice::from_ref(group));
                } else {
                    self.remove_id(group.id);
                    if let NodeKind::Group(children) = &group.kind
                        && let Some(siblings) = self.siblings_mut(*parent)
                    {
                        for (child_index, child) in child_indexes.iter().zip(children.iter()) {
                            let at = (*child_index).min(siblings.len());
                            siblings.insert(at, child.clone());
                        }
                    }
                }
            }
            Step::Ungroup {
                parent,
                index,
                group,
                released,
            } => {
                if forward {
                    self.remove_id(group.id);
                    self.insert_siblings(*parent, *index, released);
                } else {
                    for node in released {
                        self.remove_id(node.id);
                    }
                    self.insert_siblings(*parent, *index, std::slice::from_ref(group));
                }
            }
            Step::SetTextBox { id, before, after } => {
                self.place_text_box(*id, if forward { *after } else { *before });
            }
            Step::SetText { id, before, after } => {
                let content = if forward { after } else { before };
                if let Some(text) = find_node_mut(&mut self.nodes, *id).and_then(Node::text_mut) {
                    text.content = content.clone();
                    text.measured.set((0.0, 0.0));
                }
            }
        }
    }

    fn push_erase(&mut self, id: StrokeId, path: &ErasePath) {
        edit_stroke(&mut self.nodes, id, &mut |stroke: &mut Stroke| {
            stroke.erases.push(path.clone());
            stroke.invalidate();
        });
    }

    fn pop_erase(&mut self, id: StrokeId, path: &ErasePath) {
        edit_stroke(&mut self.nodes, id, &mut |stroke: &mut Stroke| {
            if stroke.erases.last() == Some(path) {
                stroke.erases.pop();
                stroke.invalidate();
            }
        });
    }

    fn siblings_mut(&mut self, parent: Option<NodeId>) -> Option<&mut Vec<Node>> {
        match parent {
            None => Some(&mut self.nodes),
            Some(id) => match &mut find_node_mut(&mut self.nodes, id)?.kind {
                NodeKind::Group(children) => Some(children),
                NodeKind::Canvas(_) | NodeKind::Shape(_) | NodeKind::Text(_) => None,
            },
        }
    }

    fn insert_siblings(&mut self, parent: Option<NodeId>, index: usize, nodes: &[Node]) {
        let Some(children) = self.siblings_mut(parent) else {
            return;
        };
        let mut index = index.min(children.len());
        for node in nodes {
            if children.iter().any(|item| item.id == node.id) {
                continue;
            }
            children.insert(index, node.clone());
            index += 1;
        }
    }

    fn remove_id(&mut self, id: NodeId) {
        fn take(nodes: &mut Vec<Node>, id: NodeId) -> bool {
            if let Some(index) = nodes.iter().position(|node| node.id == id) {
                nodes.remove(index);
                return true;
            }
            for node in nodes.iter_mut() {
                if let NodeKind::Group(children) = &mut node.kind
                    && take(children, id)
                {
                    return true;
                }
            }
            false
        }
        if take(&mut self.nodes, id) {
            self.selection.retain(|item| *item != id);
        }
    }

    fn remove_sibling_at(&mut self, parent: Option<NodeId>, index: usize) {
        let removed = {
            let Some(children) = self.siblings_mut(parent) else {
                return;
            };
            if index >= children.len() {
                return;
            }
            children.remove(index).id
        };
        self.selection.retain(|item| *item != removed);
    }

    fn move_sibling(&mut self, parent: Option<NodeId>, from: usize, to: usize) {
        let Some(children) = self.siblings_mut(parent) else {
            return;
        };
        if from >= children.len() || to >= children.len() || from == to {
            return;
        }
        let node = children.remove(from);
        children.insert(to, node);
    }

    fn insert_layer(&mut self, canvas_id: NodeId, index: usize, layer: ColorLayer) {
        let Some(canvas) = self.canvas_mut(canvas_id) else {
            return;
        };
        if canvas.colors.iter().any(|item| item.id() == layer.id()) {
            return;
        }
        let index = index.min(canvas.colors.len());
        canvas.colors.insert(index, layer);
    }

    fn remove_layer(&mut self, canvas_id: NodeId, color: ColorId) {
        let Some(canvas) = self.canvas_mut(canvas_id) else {
            return;
        };
        let Some(index) = canvas.colors.iter().position(|layer| layer.id() == color) else {
            return;
        };
        canvas.colors.remove(index);
    }

    fn relocate_color(&mut self, canvas: NodeId, from: usize, to: usize) {
        let Some(canvas) = self.canvas_mut(canvas) else {
            return;
        };
        if from >= canvas.colors.len() || to >= canvas.colors.len() || from == to {
            return;
        }
        let color = canvas.colors.remove(from);
        canvas.colors.insert(to, color);
    }
    pub fn selection(&self) -> &[NodeId] {
        &self.selection
    }

    pub fn set_selection(&mut self, ids: impl IntoIterator<Item = NodeId>) {
        let mut unique = Vec::new();
        for id in ids {
            if find_node(&self.nodes, id).is_some() && !unique.contains(&id) {
                unique.push(id);
            }
        }
        self.selection = unique;
        self.note_canvas();
    }

    pub fn toggle_selection(&mut self, id: NodeId) {
        if find_node(&self.nodes, id).is_none() {
            return;
        }
        if let Some(index) = self.selection.iter().position(|item| *item == id) {
            self.selection.remove(index);
        } else {
            self.selection.push(id);
        }
        self.note_canvas();
    }

    /// Selecting exactly one canvas makes it the active canvas.
    fn note_canvas(&mut self) {
        if let [id] = self.selection[..]
            && find_node(&self.nodes, id).is_some_and(|node| node.canvas().is_some())
        {
            self.active_canvas = Some(id);
        }
    }

    /// The brush or eraser was chosen: the canvas it works on is what is
    /// selected.
    pub fn select_active_canvas(&mut self) {
        if let Some(id) = self.highlighted_id() {
            self.selection = vec![id];
        }
    }

    pub fn local_transform(&self, id: NodeId) -> Option<Affine> {
        find_node(&self.nodes, id).map(|node| node.transform)
    }

    pub fn parent_of_world(&self, id: NodeId) -> Option<Affine> {
        self.parent_world(id)
    }

    pub fn node_prop(&self, id: NodeId, prop: PropId) -> Option<PropValue> {
        let node = find_node(&self.nodes, id)?;
        match prop {
            PropId::Name => Some(PropValue::Text(node.name.clone())),
            PropId::Visible => Some(PropValue::Bool(node.visible)),
            PropId::Locked => Some(PropValue::Bool(node.locked)),
            PropId::Padding => node
                .canvas()
                .map(|canvas| PropValue::Number(canvas.padding)),
            PropId::Palette if node.canvas().is_some() => Some(PropValue::Idle),
            _ => node
                .shape()
                .and_then(|shape| shape.read(prop))
                .or_else(|| node.text().and_then(|text| text.read(prop))),
        }
    }

    /// True when a person can change this node. A locked node can still be unlocked.
    pub fn can_edit(&self, id: NodeId, prop: PropId) -> bool {
        let Some(node) = find_node(&self.nodes, id) else {
            return false;
        };
        let Some(place) = placement(&self.nodes, id) else {
            return false;
        };
        if !place.ancestors_unlocked {
            return false;
        }
        !node.locked || prop == PropId::Locked
    }

    pub fn assign_node_prop(&mut self, id: NodeId, prop: PropId, value: &PropValue) -> bool {
        self.write_node_prop(id, prop, value)
    }

    pub fn record_node_props(
        &mut self,
        edits: Vec<(NodeId, PropId, PropValue, PropValue)>,
    ) -> bool {
        let steps: Vec<Step> = edits
            .into_iter()
            .filter(|(_, _, before, after)| before != after)
            .map(|(id, prop, before, after)| Step::SetNodeProps {
                id,
                before: vec![(prop, before)],
                after: vec![(prop, after)],
            })
            .collect();
        if steps.is_empty() {
            return false;
        }
        self.record_steps(steps);
        true
    }

    pub fn set_locals(&mut self, locals: &[(NodeId, Affine)]) {
        for (id, transform) in locals {
            if !affine_finite(*transform) {
                continue;
            }
            if let Some(node) = find_node_mut(&mut self.nodes, *id) {
                node.transform = *transform;
            }
        }
    }

    /// The transforms are already applied. This only records them.
    pub fn record_transform_changes(&mut self, before: &[(NodeId, Affine)]) -> bool {
        let mut changes = Vec::new();
        for (id, previous) in before {
            let Some(node) = find_node(&self.nodes, *id) else {
                continue;
            };
            if node.transform != *previous && affine_finite(node.transform) {
                changes.push((*id, *previous, node.transform));
            }
        }
        if changes.is_empty() {
            return false;
        }
        self.record(Step::SetTransforms { changes });
        true
    }

    pub fn layers(&self) -> Vec<LayerNode> {
        let mut rows = Vec::new();
        collect_layers(&self.nodes, None, 0, &self.selection, &mut rows);
        rows
    }

    /// A canvas's layers as colors, for making them the brush's palette.
    pub fn layer_colors(&self, canvas: NodeId) -> Option<Vec<PaletteColor>> {
        let canvas = find_node(&self.nodes, canvas)?.canvas()?;
        Some(
            canvas
                .colors
                .iter()
                .map(|layer| layer.color.clone())
                .collect(),
        )
    }

    /// Links a canvas's layers, in order, to the brush colors they now match.
    /// Not history: which brush color a layer answers to is a tool matter.
    pub fn link_layers(&mut self, canvas: NodeId, sources: &[ColorId]) {
        let changed = {
            let Some(canvas) = self.canvas_mut(canvas) else {
                return;
            };
            let mut changed = false;
            for (layer, source) in canvas.colors.iter_mut().zip(sources) {
                if layer.source != Some(*source) {
                    layer.source = Some(*source);
                    changed = true;
                }
            }
            changed
        };
        if changed {
            self.dirty = true;
        }
    }

    #[cfg(test)]
    pub fn root_len(&self) -> usize {
        self.nodes.len()
    }

    #[cfg(test)]
    pub fn root_at(&self, index: usize) -> Option<NodeId> {
        self.nodes.get(index).map(|node| node.id)
    }

    #[cfg(test)]
    pub fn shape_box(&self, id: NodeId) -> Option<(Point, Point)> {
        find_node(&self.nodes, id)?.shape()?.corners()
    }

    #[cfg(test)]
    pub fn shape_outline(&self, id: NodeId) -> Option<&PaletteColor> {
        find_node(&self.nodes, id)?.shape()?.outline()
    }

    /// Inserts a shape at the root top and selects it. Geometry is local and
    /// the transform is identity. A degenerate drag is `None` and writes no
    /// history.
    pub fn add_shape(
        &mut self,
        ids: &mut Allocator,
        kind: ShapeKind,
        placed: PlacedShape,
        palette: Vec<PaletteColor>,
        corner_radius: f64,
    ) -> Option<NodeId> {
        let shape = shape_node(kind, placed, palette, corner_radius)?;
        let mut names = Vec::new();
        collect_shape_names(&self.nodes, kind, &mut names);
        let id = ids.node();
        let node = Node {
            id,
            name: numbered_name(shape_name(kind), &names),
            visible: true,
            locked: false,
            transform: Affine::IDENTITY,
            kind: NodeKind::Shape(shape),
        };
        self.commit_step(Step::InsertNodes {
            parent: None,
            index: 0,
            nodes: vec![node],
        });
        // A new object is what the person is working with.
        self.selection = vec![id];
        Some(id)
    }

    pub fn shape_kind(&self, id: NodeId) -> Option<ShapeKind> {
        find_node(&self.nodes, id)?.shape().map(ShapeNode::kind)
    }

    pub fn is_canvas(&self, id: NodeId) -> bool {
        find_node(&self.nodes, id).is_some_and(|node| node.canvas().is_some())
    }

    pub fn is_text(&self, id: NodeId) -> bool {
        find_node(&self.nodes, id).is_some_and(|node| node.text().is_some())
    }

    pub fn text_content(&self, id: NodeId) -> Option<String> {
        find_node(&self.nodes, id)
            .and_then(Node::text)
            .map(|text| text.content.clone())
    }

    /// The frame's measurement of a text box, in local units: its drawn
    /// size, and the height of its lines and padding alone. `(0, 0)` until
    /// the first paint.
    pub fn set_text_metrics(&mut self, id: NodeId, width: f64, height: f64, natural: f64) {
        if [width, height, natural]
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return;
        }
        if let Some(text) = find_node_mut(&mut self.nodes, id).and_then(Node::text_mut) {
            text.measured.set((width, height));
            text.natural.set(natural);
        }
    }

    /// Inserts text at the root top and selects it. The transform is
    /// identity. Empty content is not inserted.
    pub fn add_text(
        &mut self,
        ids: &mut Allocator,
        frame: TextBox,
        content: String,
        style: TextStyle,
        palette: Vec<PaletteColor>,
    ) -> Option<NodeId> {
        let origin = frame.origin;
        if content.is_empty() || !point_finite(origin) {
            return None;
        }
        let width = frame
            .width
            .filter(|width| width.is_finite() && *width > 0.0);
        let height = frame
            .height
            .filter(|height| height.is_finite() && *height > 0.0);
        let mut names = Vec::new();
        collect_text_names(&self.nodes, &mut names);
        let id = ids.node();
        let node = Node {
            id,
            name: numbered_name("Text", &names),
            visible: true,
            locked: false,
            transform: Affine::IDENTITY,
            kind: NodeKind::Text(TextNode {
                content,
                origin,
                box_width: width,
                box_height: height,
                fixed_width: width.unwrap_or(0.0),
                style,
                palette,
                measured: Cell::new((0.0, 0.0)),
                natural: Cell::new(0.0),
            }),
        };
        self.commit_step(Step::InsertNodes {
            parent: None,
            index: 0,
            nodes: vec![node],
        });
        // A new object is what the person is working with.
        self.selection = vec![id];
        Some(id)
    }

    /// Adds a brush stroke to the canvas's layer that came from the brush
    /// color `source`. A canvas without that layer gains it at the bottom of
    /// its palette, in the same undo step.
    pub fn commit_brush_stroke(
        &mut self,
        ids: &mut Allocator,
        canvas: NodeId,
        source: &ToolColor,
        stroke: Stroke,
    ) -> bool {
        // The canvas drawn on is what the person is working with.
        self.selection = vec![canvas];
        if let Some(color) = self.layer_for_source(canvas, source.id()) {
            self.commit_step(Step::AddStroke {
                canvas,
                color,
                stroke,
            });
            return true;
        }
        let Some(len) = self.palette_len(canvas) else {
            return false;
        };
        let layer = ColorLayer::from_tool(ids.color(), source);
        let color = layer.id();
        self.commit_step(Step::Batch(vec![
            Step::InsertColor {
                canvas,
                index: len,
                layer,
            },
            Step::AddStroke {
                canvas,
                color,
                stroke,
            },
        ]));
        true
    }

    /// A brush stroke drawn with no canvas highlighted: a new canvas, named
    /// Canvas N, at the top of the sketch with the stroke in the layer that
    /// came from `source`, as one undo step. The new canvas is highlighted.
    pub fn commit_stroke_on_new_canvas(
        &mut self,
        ids: &mut Allocator,
        palette: &[ToolColor],
        source: ColorId,
        stroke: Stroke,
    ) -> Option<NodeId> {
        let mut names = Vec::new();
        collect_kind_names(&self.nodes, false, &mut names);
        let name = numbered_name("Canvas", &names);
        let id = ids.node();
        let node = canvas_node(id, &name, palette, ids);
        let color = node
            .canvas()?
            .colors
            .iter()
            .find(|layer| layer.source == Some(source))?
            .id();
        self.commit_step(Step::Batch(vec![
            Step::InsertNodes {
                parent: None,
                index: 0,
                nodes: vec![node],
            },
            Step::AddStroke {
                canvas: id,
                color,
                stroke,
            },
        ]));
        self.selection = vec![id];
        self.active_canvas = Some(id);
        Some(id)
    }

    /// A text object's box and the size it is drawn at.
    pub fn text_box(&self, id: NodeId) -> Option<TextFit> {
        let text = find_node(&self.nodes, id)?.text()?;
        let (width, height) = text.local_size();
        let natural = match text.natural.get() {
            natural if natural > 0.0 => natural,
            _ => {
                estimate_text_size(&TextNode {
                    box_height: None,
                    ..text.clone()
                })
                .1
            }
        };
        Some(TextFit {
            frame: text.frame(),
            width,
            height,
            natural,
        })
    }

    /// Moves or resizes a text object's box without history, while a handle
    /// is dragged. A width turns auto-size off, so the words wrap in it.
    pub fn place_text_box(&mut self, id: NodeId, frame: TextBox) {
        let bad = |size: Option<f64>| size.is_some_and(|size| !size.is_finite() || size <= 0.0);
        if !point_finite(frame.origin) || bad(frame.width) || bad(frame.height) {
            return;
        }
        if let Some(text) = find_node_mut(&mut self.nodes, id).and_then(Node::text_mut) {
            text.origin = frame.origin;
            text.box_width = frame.width;
            text.box_height = frame.height;
            if let Some(width) = frame.width {
                text.fixed_width = width;
            }
            text.measured.set((0.0, 0.0));
        }
    }

    /// Records a text box change already applied, as one undo step.
    pub fn record_text_box(&mut self, id: NodeId, before: TextBox) -> bool {
        let Some(fit) = self.text_box(id) else {
            return false;
        };
        if fit.frame == before {
            return false;
        }
        self.record(Step::SetTextBox {
            id,
            before,
            after: fit.frame,
        });
        true
    }

    /// A text object's font size, in document units.
    pub fn text_size(&self, id: NodeId) -> Option<u8> {
        Some(find_node(&self.nodes, id)?.text()?.style.size)
    }

    /// A document point in a node's own local space.
    pub fn local_point(&self, id: NodeId, point: Point) -> Option<Point> {
        Some(unmap_point(self.world_transform(id)?, point))
    }

    /// Removes a text object that is already in the sketch. One undo step
    /// restores it, including the content it had before the edit.
    pub fn remove_text(&mut self, id: NodeId) -> bool {
        if !self.is_text(id) {
            return false;
        }
        let Some(place) = placement(&self.nodes, id) else {
            return false;
        };
        let Some(node) = find_node(&self.nodes, id).cloned() else {
            return false;
        };
        self.commit_step(Step::RemoveNodes {
            parent: place.parent,
            nodes: vec![(place.index, node)],
        });
        true
    }

    #[cfg(test)]
    pub fn text_record(&self, id: NodeId) -> Option<TextRecord> {
        let text = find_node(&self.nodes, id)?.text()?;
        Some(TextRecord {
            content: text.content.clone(),
            box_width: text.box_width,
            box_height: text.box_height,
            size: text.style.size,
            bold: text.style.bold,
            wrap: text.style.wrap,
        })
    }

    pub fn add_canvas(&mut self, ids: &mut Allocator, palette: &[ToolColor]) -> NodeId {
        let mut names = Vec::new();
        collect_kind_names(&self.nodes, false, &mut names);
        let name = numbered_name("Canvas", &names);
        let id = ids.node();
        let node = canvas_node(id, &name, palette, ids);
        self.commit_step(Step::InsertNodes {
            parent: None,
            index: 0,
            nodes: vec![node],
        });
        self.selection = vec![id];
        self.active_canvas = Some(id);
        id
    }

    pub fn can_group(&self) -> bool {
        self.group_places().is_some()
    }

    pub fn group_selected(&mut self, ids: &mut Allocator) -> bool {
        let Some((parent, indexes)) = self.group_places() else {
            return false;
        };
        let children: Vec<Node> = indexes
            .iter()
            .filter_map(|index| children_slice(&self.nodes, parent)?.get(*index).cloned())
            .collect();
        if children.len() != indexes.len() {
            return false;
        }
        let mut names = Vec::new();
        collect_kind_names(&self.nodes, true, &mut names);
        let group_id = ids.node();
        let group = Node {
            id: group_id,
            name: numbered_name("Group", &names),
            visible: true,
            locked: false,
            transform: Affine::IDENTITY,
            kind: NodeKind::Group(children),
        };
        self.commit_step(Step::Group {
            parent,
            index: indexes[0],
            group,
            child_indexes: indexes,
        });
        self.selection = vec![group_id];
        true
    }

    pub fn can_ungroup(&self) -> bool {
        self.ungroup_targets().next().is_some()
    }

    pub fn ungroup_selected(&mut self) -> bool {
        let targets: Vec<NodeId> = self.ungroup_targets().collect();
        if targets.is_empty() {
            return false;
        }
        let mut planned: Vec<(usize, Step, Vec<NodeId>)> = Vec::new();
        for id in targets {
            let Some(place) = placement(&self.nodes, id) else {
                continue;
            };
            let Some(node) = find_node(&self.nodes, id).cloned() else {
                continue;
            };
            let NodeKind::Group(children) = &node.kind else {
                continue;
            };
            let mut released_ids = Vec::new();
            let released = children
                .iter()
                .map(|child| {
                    let mut child = child.clone();
                    child.transform = node.transform * child.transform;
                    released_ids.push(child.id);
                    child
                })
                .collect();
            planned.push((
                place.index,
                Step::Ungroup {
                    parent: place.parent,
                    index: place.index,
                    group: node,
                    released,
                },
                released_ids,
            ));
        }
        if planned.is_empty() {
            return false;
        }
        planned.sort_by(|left, right| right.0.cmp(&left.0));
        let mut released_ids = Vec::new();
        let steps = planned
            .into_iter()
            .map(|(_, step, ids)| {
                released_ids.extend(ids);
                step
            })
            .collect();
        self.commit_steps(steps);
        self.selection = released_ids;
        true
    }

    pub fn can_delete(&self) -> bool {
        self.selection.iter().any(|id| self.changeable(*id))
    }

    pub fn delete_selected(&mut self) -> bool {
        let selected = self.selection.clone();
        let victims: Vec<NodeId> = selected
            .iter()
            .copied()
            .filter(|id| {
                self.changeable(*id) && !has_selected_ancestor(&self.nodes, *id, &selected)
            })
            .collect();
        if victims.is_empty() {
            return false;
        }
        let mut grouped: Vec<(Option<NodeId>, Vec<(usize, Node)>)> = Vec::new();
        for id in victims {
            let Some(place) = placement(&self.nodes, id) else {
                continue;
            };
            let Some(node) = find_node(&self.nodes, id).cloned() else {
                continue;
            };
            if let Some(group) = grouped
                .iter_mut()
                .find(|(parent, _)| *parent == place.parent)
            {
                group.1.push((place.index, node));
            } else {
                grouped.push((place.parent, vec![(place.index, node)]));
            }
        }
        let steps = grouped
            .into_iter()
            .map(|(parent, mut nodes)| {
                nodes.sort_by_key(|(index, _)| *index);
                Step::RemoveNodes { parent, nodes }
            })
            .collect();
        self.commit_steps(steps);
        true
    }

    /// Deep copies of the selected subtrees, top to bottom, as they would sit
    /// at the root. A selected child of a selected node is not its own copy.
    /// Cut passes `changeable_only`: a locked object is left where it is.
    pub fn copied_subtrees(&self, changeable_only: bool) -> Vec<Node> {
        let selected = self.selection.clone();
        let mut ids = Vec::new();
        collect_copy_ids(&self.nodes, &self.nodes, &selected, &mut ids);
        let mut copies = Vec::new();
        for id in ids {
            if changeable_only && !self.changeable(id) {
                continue;
            }
            let Some(node) = find_node(&self.nodes, id) else {
                continue;
            };
            let mut copy = node.clone();
            if let Some(world) = self.world_transform(id) {
                copy.transform = world;
            }
            copies.push(copy);
        }
        copies
    }

    /// Inserts already-rekeyed subtrees at the root top, as one undo step, and
    /// selects them.
    pub fn paste_copies(&mut self, nodes: Vec<Node>) -> bool {
        if nodes.is_empty() {
            return false;
        }
        let ids: Vec<NodeId> = nodes.iter().map(|node| node.id).collect();
        self.commit_step(Step::InsertNodes {
            parent: None,
            index: 0,
            nodes,
        });
        self.set_selection(ids);
        true
    }

    pub fn can_reorder(&self, how: Reorder) -> bool {
        !self.reorder_steps(how).is_empty()
    }

    pub fn reorder_selected(&mut self, how: Reorder) -> bool {
        let steps = self.reorder_steps(how);
        if steps.is_empty() {
            return false;
        }
        self.commit_steps(steps);
        true
    }

    /// Moves `id` among its own siblings. `to` is the index after the node is removed.
    pub fn reorder_sibling(&mut self, id: NodeId, to: usize) -> bool {
        if !self.changeable(id) {
            return false;
        }
        let Some(place) = placement(&self.nodes, id) else {
            return false;
        };
        let Some(len) = children_slice(&self.nodes, place.parent).map(|nodes| nodes.len()) else {
            return false;
        };
        if len < 2 || to >= len || place.index == to {
            return false;
        }
        self.commit_step(Step::MoveNode {
            parent: place.parent,
            from: place.index,
            to,
        });
        true
    }

    pub fn can_align(&self) -> bool {
        self.bounded_changeable().len() >= 2
    }

    pub fn align_selected(&mut self, edge: AlignEdge) -> bool {
        let Some((sel_min, sel_max)) = self.selection_bounds() else {
            return false;
        };
        if self.bounded_changeable().len() < 2 {
            return false;
        }
        let mut changes = Vec::new();
        for id in self.bounded_changeable() {
            let Some(node) = find_node(&self.nodes, id) else {
                continue;
            };
            let Some((parent, world)) = transform_of(&self.nodes, id, Affine::IDENTITY) else {
                continue;
            };
            let Some(bounds) = subtree_bounds(node, world) else {
                continue;
            };
            let dx = match edge {
                AlignEdge::Left => sel_min[0] - bounds.0[0],
                AlignEdge::Center => {
                    (sel_min[0] + sel_max[0]) / 2.0 - (bounds.0[0] + bounds.1[0]) / 2.0
                }
                AlignEdge::Right => sel_max[0] - bounds.1[0],
            };
            if !dx.is_finite() || dx.abs() < 1.0e-9 {
                continue;
            }
            let new_local = parent.inverse() * Affine::translate(kurbo::Vec2::new(dx, 0.0)) * world;
            if affine_finite(new_local) && new_local != node.transform {
                changes.push((id, node.transform, new_local));
            }
        }
        if changes.is_empty() {
            return false;
        }
        self.commit_step(Step::SetTransforms { changes });
        true
    }

    pub fn can_nudge(&self) -> bool {
        self.selection.iter().any(|id| self.changeable(*id))
    }

    pub fn can_rotate(&self) -> bool {
        self.can_nudge() && self.selection_bounds().is_some() && self.selection_turns()
    }

    /// Text is not turned or mirrored, as the owner set on 2026-10-12, so a
    /// selection holding text, alone or inside a group, has no rotation
    /// handle, rotate, or flip.
    pub fn selection_turns(&self) -> bool {
        !self
            .selection
            .iter()
            .any(|id| find_node(&self.nodes, *id).is_some_and(holds_text))
    }

    pub fn nudge_selected(&mut self, delta: Point) -> bool {
        if !delta[0].is_finite() || !delta[1].is_finite() || (delta[0] == 0.0 && delta[1] == 0.0) {
            return false;
        }
        let shift = Affine::translate(kurbo::Vec2::new(delta[0], delta[1]));
        self.map_selection(|world| shift * world)
    }

    pub fn rotate_selected(&mut self, radians: f64) -> bool {
        if !radians.is_finite() || radians == 0.0 || !self.selection_turns() {
            return false;
        }
        let Some(center) = self.bounds_center() else {
            return false;
        };
        let turn = Affine::rotate_about(radians, center);
        self.map_selection(|world| turn * world)
    }

    pub fn flip_selected(&mut self, horizontal: bool) -> bool {
        if !self.selection_turns() {
            return false;
        }
        let Some(center) = self.bounds_center() else {
            return false;
        };
        let (sx, sy) = if horizontal { (-1.0, 1.0) } else { (1.0, -1.0) };
        let flip = scale_about(sx, sy, center);
        self.map_selection(|world| flip * world)
    }

    fn bounds_center(&self) -> Option<kurbo::Point> {
        let (min, max) = self.selection_bounds()?;
        Some(kurbo::Point::new(
            (min[0] + max[0]) / 2.0,
            (min[1] + max[1]) / 2.0,
        ))
    }

    fn map_selection(&mut self, map: impl Fn(Affine) -> Affine) -> bool {
        let ids: Vec<NodeId> = self
            .selection
            .iter()
            .copied()
            .filter(|id| self.transform_target(*id))
            .collect();
        if ids.is_empty() {
            return false;
        }
        let mut changes = Vec::new();
        for id in ids {
            let Some(node) = find_node(&self.nodes, id) else {
                continue;
            };
            let before = node.transform;
            let Some((parent, world)) = transform_of(&self.nodes, id, Affine::IDENTITY) else {
                continue;
            };
            let new_local = parent.inverse() * map(world);
            if affine_finite(new_local) && new_local != before {
                changes.push((id, before, new_local));
            }
        }
        if changes.is_empty() {
            return false;
        }
        self.commit_step(Step::SetTransforms { changes });
        true
    }

    fn changeable(&self, id: NodeId) -> bool {
        let Some(node) = find_node(&self.nodes, id) else {
            return false;
        };
        let Some(place) = placement(&self.nodes, id) else {
            return false;
        };
        place.ancestors_unlocked && !node.locked
    }

    /// A selected node a transform can write. A child of a selected ancestor
    /// moves with that ancestor, so writing it again would apply the change twice.
    pub fn transform_target(&self, id: NodeId) -> bool {
        self.changeable(id) && !has_selected_ancestor(&self.nodes, id, &self.selection)
    }

    fn bounded_changeable(&self) -> Vec<NodeId> {
        self.selection
            .iter()
            .copied()
            .filter(|id| {
                self.transform_target(*id)
                    && find_node(&self.nodes, *id).is_some_and(|node| {
                        transform_of(&self.nodes, *id, Affine::IDENTITY)
                            .and_then(|(_, world)| subtree_bounds(node, world))
                            .is_some()
                    })
            })
            .collect()
    }

    fn group_places(&self) -> Option<(Option<NodeId>, Vec<usize>)> {
        if self.selection.len() < 2 {
            return None;
        }
        let mut parent = None;
        let mut indexes = Vec::new();
        for id in &self.selection {
            if !self.changeable(*id) {
                return None;
            }
            let place = placement(&self.nodes, *id)?;
            match parent {
                None => parent = Some(place.parent),
                Some(existing) if existing != place.parent => return None,
                _ => {}
            }
            indexes.push(place.index);
        }
        indexes.sort_unstable();
        indexes.dedup();
        if indexes.len() < 2 {
            return None;
        }
        Some((parent?, indexes))
    }

    fn ungroup_targets(&self) -> impl Iterator<Item = NodeId> + '_ {
        let selected = self.selection.clone();
        self.selection.iter().copied().filter(move |id| {
            self.changeable(*id)
                && find_node(&self.nodes, *id).is_some_and(Node::is_group)
                && !has_selected_ancestor(&self.nodes, *id, &selected)
        })
    }

    fn reorder_steps(&self, how: Reorder) -> Vec<Step> {
        let forward = match how {
            Reorder::Forward => true,
            Reorder::Backward => false,
            Reorder::ToFront | Reorder::ToBack => {
                return self.reorder_to_end_steps(how == Reorder::ToFront);
            }
        };
        let mut groups: Vec<(Option<NodeId>, Vec<NodeId>)> = Vec::new();
        for id in &self.selection {
            if !self.changeable(*id) {
                continue;
            }
            let Some(place) = placement(&self.nodes, *id) else {
                continue;
            };
            if let Some(group) = groups
                .iter_mut()
                .find(|(parent, _)| *parent == place.parent)
            {
                group.1.push(*id);
            } else {
                groups.push((place.parent, vec![*id]));
            }
        }
        let mut steps = Vec::new();
        for (parent, selected) in groups {
            let Some(children) = children_slice(&self.nodes, parent) else {
                continue;
            };
            let mut order: Vec<NodeId> = children.iter().map(|node| node.id).collect();
            let mut moved = Vec::new();
            if forward {
                let mut index = 0;
                while index < order.len() {
                    let id = order[index];
                    if selected.contains(&id)
                        && !moved.contains(&id)
                        && index > 0
                        && !selected.contains(&order[index - 1])
                    {
                        steps.push(Step::MoveNode {
                            parent,
                            from: index,
                            to: index - 1,
                        });
                        order.swap(index, index - 1);
                        moved.push(id);
                    }
                    index += 1;
                }
            } else {
                let mut index = order.len();
                while index > 0 {
                    index -= 1;
                    let id = order[index];
                    if selected.contains(&id)
                        && !moved.contains(&id)
                        && index + 1 < order.len()
                        && !selected.contains(&order[index + 1])
                    {
                        steps.push(Step::MoveNode {
                            parent,
                            from: index,
                            to: index + 1,
                        });
                        order.swap(index, index + 1);
                        moved.push(id);
                    }
                }
            }
        }
        steps
    }

    /// Moves the selected siblings under each parent to the front (index 0 is
    /// the top) or the back, keeping their order among themselves.
    fn reorder_to_end_steps(&self, front: bool) -> Vec<Step> {
        let mut parents: Vec<Option<NodeId>> = Vec::new();
        for id in &self.selection {
            if !self.changeable(*id) {
                continue;
            }
            if let Some(place) = placement(&self.nodes, *id)
                && !parents.contains(&place.parent)
            {
                parents.push(place.parent);
            }
        }
        let mut steps = Vec::new();
        for parent in parents {
            let Some(children) = children_slice(&self.nodes, parent) else {
                continue;
            };
            let mut order: Vec<NodeId> = children.iter().map(|node| node.id).collect();
            let moving = |id: &NodeId| self.selection.contains(id) && self.changeable(*id);
            let (picked, rest): (Vec<NodeId>, Vec<NodeId>) =
                order.iter().copied().partition(|id| moving(id));
            let target: Vec<NodeId> = if front {
                picked.into_iter().chain(rest).collect()
            } else {
                rest.into_iter().chain(picked).collect()
            };
            for index in 0..order.len() {
                if order[index] == target[index] {
                    continue;
                }
                let from = order
                    .iter()
                    .position(|id| *id == target[index])
                    .expect("the target is a permutation of the order");
                steps.push(Step::MoveNode {
                    parent,
                    from,
                    to: index,
                });
                let id = order.remove(from);
                order.insert(index, id);
            }
        }
        steps
    }

    fn write_node_prop(&mut self, id: NodeId, prop: PropId, value: &PropValue) -> bool {
        let Some(node) = find_node_mut(&mut self.nodes, id) else {
            return false;
        };
        match prop {
            PropId::Name => {
                let PropValue::Text(name) = value else {
                    return false;
                };
                let name = name.trim();
                if name.is_empty() {
                    return false;
                }
                let name = name.chars().take(80).collect::<String>();
                if name.is_empty() {
                    return false;
                }
                node.name = name;
                true
            }
            PropId::Visible => {
                let PropValue::Bool(visible) = value else {
                    return false;
                };
                node.visible = *visible;
                true
            }
            PropId::Locked => {
                let PropValue::Bool(locked) = value else {
                    return false;
                };
                node.locked = *locked;
                true
            }
            PropId::Padding => {
                let PropValue::Number(number) = value else {
                    return false;
                };
                if !number.is_finite() {
                    return false;
                }
                let Some(canvas) = node.canvas_mut() else {
                    return false;
                };
                canvas.padding = number.clamp(0.0, 128.0);
                true
            }
            _ => {
                if let Some(shape) = node.shape_mut() {
                    return shape.write(prop, value);
                }
                node.text_mut().is_some_and(|text| text.write(prop, value))
            }
        }
    }

    fn commit_steps(&mut self, mut steps: Vec<Step>) {
        if steps.is_empty() {
            return;
        }
        if steps.len() == 1 {
            self.commit_step(steps.pop().unwrap());
            return;
        }
        self.commit_step(Step::Batch(steps));
    }

    fn record(&mut self, step: Step) {
        self.dirty = true;
        self.redo.clear();
        self.undo.push(step);
    }

    fn record_steps(&mut self, mut steps: Vec<Step>) {
        if steps.is_empty() {
            return;
        }
        if steps.len() == 1 {
            self.record(steps.pop().unwrap());
            return;
        }
        self.record(Step::Batch(steps));
    }
}

/// Where Arrange moves the selection among its siblings: one place up or
/// down, or all the way to the top or bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reorder {
    Forward,
    Backward,
    ToFront,
    ToBack,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignEdge {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug)]
pub struct LayerColor {
    pub id: ColorId,
    pub name: String,
    pub rgb: [u8; 3],
    pub visible: bool,
    pub locked: bool,
    /// A fixed role of a shape's or text's palette: it cannot be renamed,
    /// locked, sorted, or removed.
    pub fixed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerKind {
    Canvas,
    Group,
    Rectangle,
    Ellipse,
    Line,
    Text,
}

#[derive(Clone, Debug)]
pub struct LayerNode {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub depth: u32,
    pub name: String,
    pub kind: LayerKind,
    pub visible: bool,
    pub locked: bool,
    pub selected: bool,
    pub colors: Vec<LayerColor>,
}

struct Place {
    parent: Option<NodeId>,
    index: usize,
    ancestors_visible: bool,
    ancestors_unlocked: bool,
}

/// A new canvas whose layers are copies of the brush's palette, each linked
/// to the brush color it came from.
fn canvas_node(id: NodeId, name: &str, palette: &[ToolColor], ids: &mut Allocator) -> Node {
    let owned;
    let palette = if palette.is_empty() {
        owned = brush_palette(ids);
        owned.as_slice()
    } else {
        palette
    };
    let colors = palette
        .iter()
        .map(|color| ColorLayer::from_tool(ids.color(), color))
        .collect();
    Node {
        id,
        name: name.to_string(),
        visible: true,
        locked: false,
        transform: Affine::IDENTITY,
        kind: NodeKind::Canvas(Canvas {
            padding: defaults::CANVAS_PADDING,
            colors,
        }),
    }
}

fn layer_looks(node: &Node) -> Vec<PaletteColor> {
    node.canvas()
        .map(|canvas| {
            canvas
                .colors
                .iter()
                .map(|layer| layer.color.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn shape_node(
    kind: ShapeKind,
    placed: PlacedShape,
    palette: Vec<PaletteColor>,
    corner_radius: f64,
) -> Option<ShapeNode> {
    match (kind, placed) {
        (ShapeKind::Rectangle, PlacedShape::Box { min, max }) => {
            Some(ShapeNode::rect(min, max, corner_radius, palette))
        }
        (ShapeKind::Ellipse, PlacedShape::Box { min, max }) => {
            Some(ShapeNode::new(ShapeGeom::Ellipse { min, max }, palette))
        }
        (ShapeKind::Line, PlacedShape::Line { start, end }) => {
            Some(ShapeNode::new(ShapeGeom::Line { start, end }, palette))
        }
        _ => None,
    }
}

fn holds_text(node: &Node) -> bool {
    match &node.kind {
        NodeKind::Text(_) => true,
        NodeKind::Group(children) => children.iter().any(holds_text),
        NodeKind::Canvas(_) | NodeKind::Shape(_) => false,
    }
}

fn find_node<'a>(nodes: &'a [Node], id: NodeId) -> Option<&'a Node> {
    for node in nodes {
        if node.id == id {
            return Some(node);
        }
        if let NodeKind::Group(children) = &node.kind
            && let Some(found) = find_node(children, id)
        {
            return Some(found);
        }
    }
    None
}

fn find_node_mut<'a>(nodes: &'a mut [Node], id: NodeId) -> Option<&'a mut Node> {
    for node in nodes {
        if node.id == id {
            return Some(node);
        }
        if let NodeKind::Group(children) = &mut node.kind
            && let Some(found) = find_node_mut(children, id)
        {
            return Some(found);
        }
    }
    None
}

fn placement(nodes: &[Node], id: NodeId) -> Option<Place> {
    fn walk(
        nodes: &[Node],
        id: NodeId,
        parent: Option<NodeId>,
        ancestors_visible: bool,
        ancestors_unlocked: bool,
    ) -> Option<Place> {
        for (index, node) in nodes.iter().enumerate() {
            if node.id == id {
                return Some(Place {
                    parent,
                    index,
                    ancestors_visible,
                    ancestors_unlocked,
                });
            }
            if let NodeKind::Group(children) = &node.kind
                && let Some(found) = walk(
                    children,
                    id,
                    Some(node.id),
                    ancestors_visible && node.visible,
                    ancestors_unlocked && !node.locked,
                )
            {
                return Some(found);
            }
        }
        None
    }
    walk(nodes, id, None, true, true)
}

fn transform_of(nodes: &[Node], id: NodeId, parent_world: Affine) -> Option<(Affine, Affine)> {
    for node in nodes {
        let world = parent_world * node.transform;
        if node.id == id {
            return Some((parent_world, world));
        }
        if let NodeKind::Group(children) = &node.kind
            && let Some(found) = transform_of(children, id, world)
        {
            return Some(found);
        }
    }
    None
}

fn children_slice(nodes: &[Node], parent: Option<NodeId>) -> Option<&[Node]> {
    match parent {
        None => Some(nodes),
        Some(id) => match &find_node(nodes, id)?.kind {
            NodeKind::Group(children) => Some(children),
            NodeKind::Canvas(_) | NodeKind::Shape(_) | NodeKind::Text(_) => None,
        },
    }
}

/// Selected nodes with no selected ancestor, in paint order. Descendants of a
/// selected node stay inside that copy.
fn collect_copy_ids(root: &[Node], nodes: &[Node], selected: &[NodeId], out: &mut Vec<NodeId>) {
    for node in nodes {
        if selected.contains(&node.id) && !has_selected_ancestor(root, node.id, selected) {
            out.push(node.id);
            continue;
        }
        if let NodeKind::Group(children) = &node.kind {
            collect_copy_ids(root, children, selected, out);
        }
    }
}

fn has_selected_ancestor(nodes: &[Node], id: NodeId, selected: &[NodeId]) -> bool {
    let mut current = placement(nodes, id).and_then(|place| place.parent);
    while let Some(parent) = current {
        if selected.contains(&parent) {
            return true;
        }
        current = placement(nodes, parent).and_then(|place| place.parent);
    }
    false
}

fn locate_stroke<'a>(
    nodes: &'a [Node],
    parent: Affine,
    id: StrokeId,
) -> Option<(Affine, &'a ColorLayer, &'a Stroke)> {
    for node in nodes {
        let world = parent * node.transform;
        match &node.kind {
            NodeKind::Canvas(canvas) => {
                for color in &canvas.colors {
                    if let Some(stroke) = color.strokes.iter().find(|stroke| stroke.id == id) {
                        return Some((world, color, stroke));
                    }
                }
            }
            NodeKind::Group(children) => {
                if let Some(found) = locate_stroke(children, world, id) {
                    return Some(found);
                }
            }
            NodeKind::Shape(_) | NodeKind::Text(_) => {}
        }
    }
    None
}

fn edit_stroke(nodes: &mut [Node], id: StrokeId, edit: &mut impl FnMut(&mut Stroke)) -> bool {
    for node in nodes {
        match &mut node.kind {
            NodeKind::Canvas(canvas) => {
                let Some(index) = canvas
                    .colors
                    .iter()
                    .position(|color| color.strokes.iter().any(|stroke| stroke.id == id))
                else {
                    continue;
                };
                let color = &mut canvas.colors[index];
                color.union_cache = OnceCell::new();
                if let Some(stroke) = color.strokes.iter_mut().find(|stroke| stroke.id == id) {
                    edit(stroke);
                }
                return true;
            }
            NodeKind::Group(children) => {
                if edit_stroke(children, id, edit) {
                    return true;
                }
            }
            NodeKind::Shape(_) | NodeKind::Text(_) => {}
        }
    }
    false
}

fn collect_layers(
    nodes: &[Node],
    parent: Option<NodeId>,
    depth: u32,
    selection: &[NodeId],
    out: &mut Vec<LayerNode>,
) {
    for node in nodes {
        let colors = node
            .canvas()
            .map(|canvas| {
                canvas
                    .colors
                    .iter()
                    .map(|color| LayerColor {
                        id: color.id(),
                        name: color.name().to_string(),
                        rgb: color.rgb(),
                        visible: color.visible(),
                        locked: color.locked(),
                        fixed: false,
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push(LayerNode {
            id: node.id,
            parent,
            depth,
            name: node.name.clone(),
            kind: layer_kind(node),
            visible: node.visible,
            locked: node.locked,
            selected: selection.contains(&node.id),
            colors,
        });
        if let NodeKind::Group(children) = &node.kind {
            collect_layers(children, Some(node.id), depth + 1, selection, out);
        }
    }
}

fn collect_kind_names(nodes: &[Node], groups: bool, out: &mut Vec<String>) {
    for node in nodes {
        match &node.kind {
            NodeKind::Canvas(_) => {
                if !groups {
                    out.push(node.name.clone());
                }
            }
            NodeKind::Group(children) => {
                if groups {
                    out.push(node.name.clone());
                }
                collect_kind_names(children, groups, out);
            }
            NodeKind::Shape(_) | NodeKind::Text(_) => {}
        }
    }
}

fn collect_text_names(nodes: &[Node], out: &mut Vec<String>) {
    for node in nodes {
        match &node.kind {
            NodeKind::Text(_) => out.push(node.name.clone()),
            NodeKind::Group(children) => collect_text_names(children, out),
            NodeKind::Canvas(_) | NodeKind::Shape(_) => {}
        }
    }
}

fn collect_shape_names(nodes: &[Node], kind: ShapeKind, out: &mut Vec<String>) {
    for node in nodes {
        match &node.kind {
            NodeKind::Shape(shape) if shape.kind() == kind => out.push(node.name.clone()),
            NodeKind::Group(children) => collect_shape_names(children, kind, out),
            NodeKind::Shape(_) | NodeKind::Canvas(_) | NodeKind::Text(_) => {}
        }
    }
}

fn shape_name(kind: ShapeKind) -> &'static str {
    match kind {
        ShapeKind::Rectangle => "Rectangle",
        ShapeKind::Ellipse => "Ellipse",
        ShapeKind::Line => "Line",
    }
}

fn layer_kind(node: &Node) -> LayerKind {
    match &node.kind {
        NodeKind::Canvas(_) => LayerKind::Canvas,
        NodeKind::Group(_) => LayerKind::Group,
        NodeKind::Shape(shape) => match shape.kind() {
            ShapeKind::Rectangle => LayerKind::Rectangle,
            ShapeKind::Ellipse => LayerKind::Ellipse,
            ShapeKind::Line => LayerKind::Line,
        },
        NodeKind::Text(_) => LayerKind::Text,
    }
}

fn numbered_name(prefix: &str, names: &[String]) -> String {
    let head = format!("{prefix} ");
    let mut used = Vec::new();
    for name in names {
        if let Some(rest) = name.strip_prefix(&head)
            && let Ok(number) = rest.parse::<u32>()
            && number > 0
        {
            used.push(number);
        }
    }
    let mut number = 1u32;
    while used.contains(&number) {
        number += 1;
    }
    format!("{prefix} {number}")
}

fn affine_finite(transform: Affine) -> bool {
    transform.as_coeffs().iter().all(|value| value.is_finite())
}

fn scale_about(sx: f64, sy: f64, center: kurbo::Point) -> Affine {
    let center = center.to_vec2();
    Affine::translate(-center)
        .then_scale_non_uniform(sx, sy)
        .then_translate(center)
}

fn subtree_bounds(node: &Node, world: Affine) -> Option<(Point, Point)> {
    if !node.visible {
        return None;
    }
    match &node.kind {
        NodeKind::Canvas(canvas) => canvas_world_bounds(canvas, world),
        NodeKind::Group(children) => {
            let mut bounds = None;
            for child in children {
                if let Some(next) = subtree_bounds(child, world * child.transform) {
                    bounds = Some(union_bounds(bounds, next));
                }
            }
            bounds
        }
        NodeKind::Shape(shape) => shape_world_bounds(shape, world),
        NodeKind::Text(text) => text_world_bounds(text, world),
    }
}

fn shape_world_bounds(shape: &ShapeNode, world: Affine) -> Option<(Point, Point)> {
    let mut bounds = None;
    for mesh in shape_meshes(shape) {
        for point in &mesh.hull {
            let mapped = map_point(world, *point);
            bounds = Some(union_bounds(bounds, (mapped, mapped)));
        }
    }
    bounds
}

fn shape_meshes(shape: &ShapeNode) -> Vec<&ink::Mesh> {
    let ink = shape.ink();
    let mut meshes = Vec::new();
    if shape.fill().is_some()
        && let Some(mesh) = ink.fill.as_ref()
    {
        meshes.push(mesh);
    }
    if let Some(mesh) = ink.stroke.as_ref() {
        meshes.push(mesh);
    }
    meshes
}

/// A canvas's box in world space: the extent of its visible ink as it is
/// drawn, plus its padding. The ink's hull is mapped, not the box around the
/// ink, so a rotated drawing keeps a box that hugs it instead of growing to
/// enclose its own rotated corners.
fn canvas_world_bounds(canvas: &Canvas, world: Affine) -> Option<(Point, Point)> {
    let mut bounds: Option<(Point, Point)> = None;
    for color in &canvas.colors {
        if !color.visible() {
            continue;
        }
        let Some(mesh) = color.mesh() else {
            continue;
        };
        for point in &mesh.hull {
            let mapped = world * kurbo::Point::new(point[0], point[1]);
            let mapped = [mapped.x, mapped.y];
            bounds = Some(union_bounds(bounds, (mapped, mapped)));
        }
    }
    let (min, max) = bounds?;
    // Padding is a circle of `padding` canvas units around the ink. The map
    // turns it into an ellipse whose half-width and half-height come from the
    // map's rows: even under rotation, and per axis under a stretch.
    let [a, b, c, d, _, _] = world.as_coeffs();
    let pad_x = canvas.padding * a.hypot(c);
    let pad_y = canvas.padding * b.hypot(d);
    Some((
        [min[0] - pad_x, min[1] - pad_y],
        [max[0] + pad_x, max[1] + pad_y],
    ))
}

fn accumulate_bounds(
    nodes: &[Node],
    parent: Affine,
    mut bounds: Option<(Point, Point)>,
) -> Option<(Point, Point)> {
    for node in nodes {
        if !node.visible {
            continue;
        }
        if let Some(next) = subtree_bounds(node, parent * node.transform) {
            bounds = Some(union_bounds(bounds, next));
        }
    }
    bounds
}

fn ink_hit(node: &Node, world: Affine, point: Point) -> bool {
    if !node.visible {
        return false;
    }
    match &node.kind {
        NodeKind::Canvas(canvas) => {
            let local = unmap_point(world, point);
            for color in &canvas.colors {
                if !color.visible() {
                    continue;
                }
                let Some(mesh) = color.mesh() else {
                    continue;
                };
                if ink::point_in_shapes(local, &mesh.shapes) {
                    return true;
                }
            }
            false
        }
        NodeKind::Group(children) => children
            .iter()
            .any(|child| ink_hit(child, world * child.transform, point)),
        NodeKind::Shape(shape) => shape_hit(shape, world, point),
        NodeKind::Text(text) => text_hit(text, world, point),
    }
}

fn shape_hit(shape: &ShapeNode, world: Affine, point: Point) -> bool {
    let local = unmap_point(world, point);
    shape_meshes(shape)
        .into_iter()
        .any(|mesh| ink::point_in_shapes(local, &mesh.shapes))
}

fn text_world_bounds(text: &TextNode, world: Affine) -> Option<(Point, Point)> {
    let (width, height) = text.local_size();
    let local = (
        text.origin,
        [text.origin[0] + width, text.origin[1] + height],
    );
    Some(transformed_bounds(world, local))
}

fn text_hit(text: &TextNode, world: Affine, point: Point) -> bool {
    let local = unmap_point(world, point);
    let (width, height) = text.local_size();
    local[0] >= text.origin[0]
        && local[1] >= text.origin[1]
        && local[0] <= text.origin[0] + width
        && local[1] <= text.origin[1] + height
}

fn text_crosses(text: &TextNode, world: Affine, rect: (Point, Point)) -> bool {
    let Some(bounds) = text_world_bounds(text, world) else {
        return false;
    };
    if !ink::bounds_overlap(bounds, rect) {
        return false;
    }
    if rect_contains(rect, bounds) {
        return true;
    }
    let (width, height) = text.local_size();
    let corners = [
        text.origin,
        [text.origin[0] + width, text.origin[1]],
        [text.origin[0] + width, text.origin[1] + height],
        [text.origin[0], text.origin[1] + height],
    ];
    corners
        .into_iter()
        .map(|corner| map_point(world, corner))
        .any(|corner| point_in_rect(corner, rect))
        || rect_corners(rect).into_iter().any(|corner| {
            let local = unmap_point(world, corner);
            text_hit(text, Affine::IDENTITY, local)
        })
}

fn point_in_rect(point: Point, rect: (Point, Point)) -> bool {
    point[0] >= rect.0[0] && point[1] >= rect.0[1] && point[0] <= rect.1[0] && point[1] <= rect.1[1]
}

fn rect_corners(rect: (Point, Point)) -> [Point; 4] {
    [
        rect.0,
        [rect.1[0], rect.0[1]],
        rect.1,
        [rect.0[0], rect.1[1]],
    ]
}

fn ink_crosses(node: &Node, world: Affine, rect: (Point, Point)) -> bool {
    if !node.visible {
        return false;
    }
    match &node.kind {
        NodeKind::Canvas(canvas) => canvas_crosses(canvas, world, rect),
        NodeKind::Group(children) => children
            .iter()
            .any(|child| ink_crosses(child, world * child.transform, rect)),
        NodeKind::Shape(shape) => shape_crosses(shape, world, rect),
        NodeKind::Text(text) => text_crosses(text, world, rect),
    }
}

fn shape_crosses(shape: &ShapeNode, world: Affine, rect: (Point, Point)) -> bool {
    for mesh in shape_meshes(shape) {
        let Some(local) = mesh.bounds else {
            continue;
        };
        let bounds = transformed_bounds(world, local);
        if !ink::bounds_overlap(bounds, rect) {
            continue;
        }
        if rect_contains(rect, bounds) {
            return true;
        }
        let shapes = transform_shapes(world, &mesh.shapes);
        let ring = [
            rect.0,
            [rect.1[0], rect.0[1]],
            rect.1,
            [rect.0[0], rect.1[1]],
        ];
        if ink::polygons_intersect(&shapes, &ring) {
            return true;
        }
    }
    false
}

fn canvas_crosses(canvas: &Canvas, world: Affine, rect: (Point, Point)) -> bool {
    for color in &canvas.colors {
        if !color.visible() {
            continue;
        }
        let Some(mesh) = color.mesh() else {
            continue;
        };
        let Some(local) = mesh.bounds else {
            continue;
        };
        let bounds = transformed_bounds(world, local);
        if !ink::bounds_overlap(bounds, rect) {
            continue;
        }
        if rect_contains(rect, bounds) {
            return true;
        }
        let shapes = transform_shapes(world, &mesh.shapes);
        let ring = [
            rect.0,
            [rect.1[0], rect.0[1]],
            rect.1,
            [rect.0[0], rect.1[1]],
        ];
        if ink::polygons_intersect(&shapes, &ring) {
            return true;
        }
    }
    false
}

fn transformed_bounds(transform: Affine, bounds: (Point, Point)) -> (Point, Point) {
    let rect = kurbo::Rect::new(bounds.0[0], bounds.0[1], bounds.1[0], bounds.1[1]);
    let world = transform.transform_rect_bbox(rect);
    ([world.x0, world.y0], [world.x1, world.y1])
}

fn rect_contains(rect: (Point, Point), bounds: (Point, Point)) -> bool {
    rect.0[0] <= bounds.0[0]
        && rect.0[1] <= bounds.0[1]
        && rect.1[0] >= bounds.1[0]
        && rect.1[1] >= bounds.1[1]
}

fn push_scene(
    marks: &mut Vec<SceneMark>,
    nodes: &[Node],
    parent: Affine,
    live: Option<&LiveStroke>,
) {
    for node in nodes.iter().rev() {
        if !node.visible {
            continue;
        }
        let world = parent * node.transform;
        match &node.kind {
            NodeKind::Canvas(canvas) => {
                let mut paints = Vec::new();
                paint_canvas(&mut paints, node.id, canvas, world, live);
                marks.extend(paints.into_iter().map(SceneMark::Ink));
            }
            NodeKind::Group(children) => push_scene(marks, children, world, live),
            NodeKind::Shape(shape) => {
                let mut paints = Vec::new();
                paint_shape(&mut paints, shape, world);
                marks.extend(paints.into_iter().map(SceneMark::Ink));
            }
            NodeKind::Text(text) => marks.push(SceneMark::Text(text.view(node.id, world))),
        }
    }
}

fn paint_canvas(
    paints: &mut Vec<InkPaint>,
    id: NodeId,
    canvas: &Canvas,
    world: Affine,
    live: Option<&LiveStroke>,
) {
    // A stroke into a layer the commit will add paints below every layer.
    if let Some(live) = live.filter(|live| live.canvas == Some(id) && live.color.is_none())
        && let Ok(mesh) = ink::mesh_from_shapes(live.shapes.clone())
    {
        push_paint(paints, world, live.look.0, live.look.1, &mesh.triangles);
    }
    for color in canvas.colors.iter().rev() {
        if !color.visible() {
            continue;
        }
        let mesh = color.mesh().filter(|mesh| !mesh.triangles.is_empty());
        let live = live.filter(|live| live.canvas == Some(id) && live.color == Some(color.id()));
        let Some(live) = live else {
            if let Some(mesh) = mesh {
                push_triangles(paints, world, color, &mesh.triangles);
            }
            continue;
        };
        // A translucent layer is one surface, so the live stroke joins its
        // union and overlaps do not darken. An opaque layer can simply paint
        // the live stroke over its older strokes.
        if let Some(mesh) = mesh
            && color.color.opacity < 1.0
            && let Some(joined) = ink::union_meshes(&[mesh.shapes.clone(), live.shapes.clone()])
        {
            push_triangles(paints, world, color, &joined.triangles);
            continue;
        }
        if let Some(mesh) = mesh {
            push_triangles(paints, world, color, &mesh.triangles);
        }
        if let Ok(live_mesh) = ink::mesh_from_shapes(live.shapes.clone()) {
            push_triangles(paints, world, color, &live_mesh.triangles);
        }
    }
}

fn paint_shape(paints: &mut Vec<InkPaint>, shape: &ShapeNode, world: Affine) {
    let ink = shape.ink();
    if let Some(fill) = shape.fill()
        && let Some(mesh) = ink.fill.as_ref()
    {
        push_paint(paints, world, fill.rgb, fill.opacity, &mesh.triangles);
    }
    if let Some(outline) = shape.outline()
        && let Some(mesh) = ink.stroke.as_ref()
    {
        push_paint(paints, world, outline.rgb, outline.opacity, &mesh.triangles);
    }
}

pub fn preview_shape_paints(
    kind: ShapeKind,
    placed: PlacedShape,
    palette: Vec<PaletteColor>,
    corner_radius: f64,
) -> Vec<InkPaint> {
    let Some(shape) = shape_node(kind, placed, palette, corner_radius) else {
        return Vec::new();
    };
    let mut paints = Vec::new();
    paint_shape(&mut paints, &shape, Affine::IDENTITY);
    paints
}

fn push_paint(
    paints: &mut Vec<InkPaint>,
    transform: Affine,
    rgb: [u8; 3],
    opacity: f64,
    triangles: &[[Point; 3]],
) {
    if triangles.is_empty() {
        return;
    }
    paints.push(InkPaint {
        rgb,
        opacity,
        eraser_preview: false,
        triangles: triangles
            .iter()
            .map(|triangle| triangle.map(|point| map_point(transform, point)))
            .collect(),
    });
}

fn push_triangles(
    paints: &mut Vec<InkPaint>,
    transform: Affine,
    color: &ColorLayer,
    triangles: &[[Point; 3]],
) {
    if triangles.is_empty() {
        return;
    }
    push_paint(
        paints,
        transform,
        color.color.rgb,
        color.color.opacity,
        triangles,
    );
}

fn union_bounds(bounds: Option<(Point, Point)>, next: (Point, Point)) -> (Point, Point) {
    match bounds {
        None => next,
        Some((min, max)) => (
            [min[0].min(next.0[0]), min[1].min(next.0[1])],
            [max[0].max(next.1[0]), max[1].max(next.1[1])],
        ),
    }
}

pub fn map_samples(transform: Affine, samples: &[Sample]) -> Vec<Sample> {
    let inverse = transform.inverse();
    samples
        .iter()
        .map(|sample| {
            let point = inverse * kurbo::Point::new(sample.x, sample.y);
            Sample {
                x: point.x,
                y: point.y,
                pressure: sample.pressure,
            }
        })
        .collect()
}

fn map_point(transform: Affine, point: Point) -> Point {
    let mapped = transform * kurbo::Point::new(point[0], point[1]);
    [mapped.x, mapped.y]
}

fn unmap_point(transform: Affine, point: Point) -> Point {
    let mapped = transform.inverse() * kurbo::Point::new(point[0], point[1]);
    [mapped.x, mapped.y]
}

fn transform_shapes(transform: Affine, shapes: &Shapes) -> Shapes {
    shapes
        .iter()
        .map(|shape| {
            shape
                .iter()
                .map(|ring| {
                    ring.iter()
                        .copied()
                        .map(|point| map_point(transform, point))
                        .collect()
                })
                .collect()
        })
        .collect()
}

fn rekey_node(node: &mut Node, ids: &mut Allocator) {
    node.id = ids.node();
    match &mut node.kind {
        NodeKind::Canvas(canvas) => {
            for layer in &mut canvas.colors {
                layer.color.id = ids.color();
                layer.union_cache = OnceCell::new();
                for stroke in &mut layer.strokes {
                    stroke.rekey(ids.stroke());
                }
            }
        }
        NodeKind::Group(children) => {
            for child in children {
                rekey_node(child, ids);
            }
        }
        NodeKind::Shape(shape) => {
            for color in &mut shape.palette {
                color.id = ids.color();
            }
            shape.ink = OnceCell::new();
        }
        NodeKind::Text(text) => {
            for color in &mut text.palette {
                color.id = ids.color();
            }
        }
    }
}

// Node must be Clone because history stores inserted subtrees.
impl Clone for Node {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            name: self.name.clone(),
            visible: self.visible,
            locked: self.locked,
            transform: self.transform,
            kind: self.kind.clone(),
        }
    }
}

impl std::fmt::Debug for Node {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Node")
            .field("id", &self.id)
            .field("name", &self.name)
            .finish()
    }
}

impl Clone for NodeKind {
    fn clone(&self) -> Self {
        match self {
            NodeKind::Canvas(canvas) => NodeKind::Canvas(canvas.clone()),
            NodeKind::Group(children) => NodeKind::Group(children.clone()),
            NodeKind::Shape(shape) => NodeKind::Shape(shape.clone()),
            NodeKind::Text(text) => NodeKind::Text(text.clone()),
        }
    }
}

impl Clone for Canvas {
    fn clone(&self) -> Self {
        Self {
            padding: self.padding,
            colors: self.colors.clone(),
        }
    }
}

fn separate_node(node: &mut Node, held: &HeldIds, ids: &mut Allocator, origin: &mut NodeId) {
    let previous = node.id;
    if held.nodes.contains(&previous.get()) {
        node.id = ids.node();
        if *origin == previous {
            *origin = node.id;
        }
    }
    match &mut node.kind {
        NodeKind::Canvas(canvas) => {
            for layer in &mut canvas.colors {
                if held.colors.contains(&layer.color.id.get()) {
                    layer.color.id = ids.color();
                }
                for stroke in &mut layer.strokes {
                    if held.strokes.contains(&stroke.id().get()) {
                        stroke.rekey(ids.stroke());
                    }
                }
            }
        }
        NodeKind::Group(children) => {
            for child in children {
                separate_node(child, held, ids, origin);
            }
        }
        NodeKind::Shape(shape) => {
            for color in &mut shape.palette {
                if held.colors.contains(&color.id.get()) {
                    color.id = ids.color();
                }
            }
        }
        NodeKind::Text(text) => {
            for color in &mut text.palette {
                if held.colors.contains(&color.id.get()) {
                    color.id = ids.color();
                }
            }
        }
    }
}

fn collect_held_node(node: &Node, held: &mut HeldIds) {
    held.nodes.insert(node.id.get());
    match &node.kind {
        NodeKind::Canvas(canvas) => {
            for layer in &canvas.colors {
                held.colors.insert(layer.color.id.get());
                for stroke in &layer.strokes {
                    held.strokes.insert(stroke.id().get());
                }
            }
        }
        NodeKind::Group(children) => {
            for child in children {
                collect_held_node(child, held);
            }
        }
        NodeKind::Shape(shape) => {
            for color in &shape.palette {
                held.colors.insert(color.id.get());
            }
        }
        NodeKind::Text(text) => {
            for color in &text.palette {
                held.colors.insert(color.id.get());
            }
        }
    }
}

/// Ids already live in the session. An opened sketch does not reuse them.
#[derive(Default)]
pub struct HeldIds {
    pub sketches: HashSet<u64>,
    pub nodes: HashSet<u64>,
    pub colors: HashSet<u64>,
    pub strokes: HashSet<u64>,
}

/// A stroke or an erasure the host already applied, as one of the existing
/// undo steps. Anything else — a shape, text, or a reorder — is not invented
/// here.
fn diff_committed(before: &[Node], after: &[Node]) -> Option<Step> {
    let mut steps = Vec::new();
    if !diff_nodes(before, after, &mut steps) || steps.is_empty() {
        return None;
    }
    if steps.len() == 1 {
        steps.pop()
    } else {
        Some(Step::Batch(steps))
    }
}

fn diff_nodes(before: &[Node], after: &[Node], steps: &mut Vec<Step>) -> bool {
    if before.len() != after.len() {
        return false;
    }
    for (left, right) in before.iter().zip(after) {
        if left.id != right.id
            || left.name != right.name
            || left.visible != right.visible
            || left.locked != right.locked
            || left.transform.as_coeffs() != right.transform.as_coeffs()
        {
            return false;
        }
        match (&left.kind, &right.kind) {
            (NodeKind::Canvas(left_canvas), NodeKind::Canvas(right_canvas)) => {
                if !diff_canvas(left.id, left_canvas, right_canvas, steps) {
                    return false;
                }
            }
            (NodeKind::Group(left_children), NodeKind::Group(right_children)) => {
                if !diff_nodes(left_children, right_children, steps) {
                    return false;
                }
            }
            (NodeKind::Shape(_), NodeKind::Shape(_)) | (NodeKind::Text(_), NodeKind::Text(_)) => {
                if snapshot::node_json(left) != snapshot::node_json(right) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

fn diff_canvas(canvas: NodeId, before: &Canvas, after: &Canvas, steps: &mut Vec<Step>) -> bool {
    if before.padding != after.padding || after.colors.len() < before.colors.len() {
        return false;
    }
    for (left, right) in before.colors.iter().zip(&after.colors) {
        if left.id() != right.id()
            || left.color != right.color
            || left.locked != right.locked
            || left.source != right.source
        {
            return false;
        }
        if !diff_strokes(canvas, left.id(), &left.strokes, &right.strokes, steps) {
            return false;
        }
    }
    for (index, layer) in after.colors.iter().enumerate().skip(before.colors.len()) {
        let mut bare = layer.clone();
        let strokes = std::mem::take(&mut bare.strokes);
        let color = bare.id();
        steps.push(Step::InsertColor {
            canvas,
            index,
            layer: bare,
        });
        for stroke in strokes {
            steps.push(Step::AddStroke {
                canvas,
                color,
                stroke,
            });
        }
    }
    true
}

fn diff_strokes(
    canvas: NodeId,
    color: ColorId,
    before: &[Stroke],
    after: &[Stroke],
    steps: &mut Vec<Step>,
) -> bool {
    if after.len() < before.len() {
        return false;
    }
    let mut added = Vec::new();
    for (left, right) in before.iter().zip(after) {
        if left.id != right.id
            || left.samples != right.samples
            || left.dynamics != right.dynamics
            || left.size != right.size
            || right.erases.len() < left.erases.len()
            || right.erases[..left.erases.len()] != left.erases[..]
        {
            return false;
        }
        added.extend(
            right.erases[left.erases.len()..]
                .iter()
                .cloned()
                .map(|path| (left.id, path)),
        );
    }
    if !added.is_empty() {
        steps.push(Step::AddErases { added });
    }
    for stroke in after.iter().skip(before.len()) {
        steps.push(Step::AddStroke {
            canvas,
            color,
            stroke: stroke.clone(),
        });
    }
    true
}

pub mod snapshot;
