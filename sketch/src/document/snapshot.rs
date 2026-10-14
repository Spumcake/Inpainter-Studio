//! The versioned JSON of one sketch: its id, saved name, camera, and object
//! tree. History, selection, the clipboard, tool settings, and the panel are
//! not in it.

use super::{
    Allocator, Camera, Cap, ColorId, ColorLayer, ColorRole, CurvePoint, Dash, Dynamics, ErasePath,
    ListKind, MAX_ZOOM, MIN_ZOOM, Node, NodeId, NodeKind, PaletteColor, Point, Sample, ShapeGeom,
    ShapeNode, Sketch, SketchId, Stroke, StrokeId, TextAlign, TextFamily, TextNode, TextStyle,
    VAlign,
};
use kurbo::Affine;
use serde_json::{Map, Value, json};
use std::cell::{Cell, OnceCell};
use std::collections::HashSet;

impl Sketch {
    pub fn snapshot_value(&self) -> Value {
        json!({
            "format_version": 1,
            "id": self.id.get(),
            "name": self.name,
            "camera": {
                "origin": self.camera.origin,
                "zoom": self.camera.zoom,
            },
            "nodes": self.nodes.iter().map(node_value).collect::<Vec<_>>(),
        })
    }

    /// Reserves every id this sketch holds, including a layer's source color,
    /// so later allocations sit above them.
    pub fn reserve_ids(&self, ids: &mut Allocator) {
        for node in &self.nodes {
            reserve_node(node, ids);
        }
    }
}

pub fn sketch_from_text(text: &str) -> Option<Sketch> {
    let value: Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;
    if object.get("format_version")?.as_u64()? != 1 {
        return None;
    }
    let id = req_id(object.get("id")?)?;
    let name = object.get("name")?.as_str()?;
    if name.trim().is_empty() {
        return None;
    }
    let camera = camera_from(object.get("camera")?)?;
    let mut seen = Seen::default();
    let nodes = nodes_from(object.get("nodes")?, &mut seen)?;
    let origin_canvas = first_canvas(&nodes).unwrap_or(NodeId::new(1));
    let born = nodes
        .iter()
        .find(|node| node.id == origin_canvas)
        .map(super::layer_looks)
        .unwrap_or_default();
    Some(Sketch {
        id: SketchId::new(id),
        name: name.to_string(),
        generated_name: name.to_string(),
        stored: true,
        dirty: false,
        path: None,
        recovery_file: None,
        origin_canvas,
        born,
        nodes,
        selection: Vec::new(),
        active_canvas: None,
        camera,
        undo: Vec::new(),
        redo: Vec::new(),
    })
}

#[derive(Default)]
struct Seen {
    nodes: HashSet<u64>,
    colors: HashSet<u64>,
    strokes: HashSet<u64>,
}

impl Seen {
    fn node(&mut self, id: u64) -> Option<NodeId> {
        if id == 0 || !self.nodes.insert(id) {
            return None;
        }
        Some(NodeId::new(id))
    }

    fn color(&mut self, id: u64) -> Option<ColorId> {
        if id == 0 || !self.colors.insert(id) {
            return None;
        }
        Some(ColorId::new(id))
    }

    fn stroke(&mut self, id: u64) -> Option<StrokeId> {
        if id == 0 || !self.strokes.insert(id) {
            return None;
        }
        Some(StrokeId::new(id))
    }
}

fn reserve_node(node: &Node, ids: &mut Allocator) {
    ids.reserve_node(node.id);
    match &node.kind {
        NodeKind::Canvas(canvas) => {
            for layer in &canvas.colors {
                ids.reserve_color(layer.color.id);
                if let Some(source) = layer.source {
                    ids.reserve_color(source);
                }
                for stroke in &layer.strokes {
                    ids.reserve_stroke(stroke.id);
                }
            }
        }
        NodeKind::Group(children) => {
            for child in children {
                reserve_node(child, ids);
            }
        }
        NodeKind::Shape(shape) => {
            for color in &shape.palette {
                ids.reserve_color(color.id);
            }
        }
        NodeKind::Text(text) => {
            for color in &text.palette {
                ids.reserve_color(color.id);
            }
        }
    }
}

pub fn node_json(node: &Node) -> Value {
    node_value(node)
}

fn node_value(node: &Node) -> Value {
    let mut value = json!({
        "id": node.id.get(),
        "name": node.name,
        "visible": node.visible,
        "locked": node.locked,
        "transform": node.transform.as_coeffs(),
    });
    let object = value.as_object_mut().expect("a node is an object");
    match &node.kind {
        NodeKind::Canvas(canvas) => {
            object.insert("kind".to_string(), json!("canvas"));
            object.insert("padding".to_string(), json!(canvas.padding));
            object.insert(
                "colors".to_string(),
                Value::Array(canvas.colors.iter().map(layer_value).collect()),
            );
        }
        NodeKind::Group(children) => {
            object.insert("kind".to_string(), json!("group"));
            object.insert(
                "children".to_string(),
                Value::Array(children.iter().map(node_value).collect()),
            );
        }
        NodeKind::Shape(shape) => {
            object.insert("kind".to_string(), json!(geom_kind(&shape.geom)));
            match shape.geom {
                ShapeGeom::Rect {
                    min,
                    max,
                    corner_radius,
                } => {
                    object.insert("min".to_string(), point_value(min));
                    object.insert("max".to_string(), point_value(max));
                    object.insert("corner_radius".to_string(), json!(corner_radius));
                }
                ShapeGeom::Ellipse { min, max } => {
                    object.insert("min".to_string(), point_value(min));
                    object.insert("max".to_string(), point_value(max));
                }
                ShapeGeom::Line { start, end } => {
                    object.insert("start".to_string(), point_value(start));
                    object.insert("end".to_string(), point_value(end));
                }
            }
            object.insert(
                "palette".to_string(),
                Value::Array(shape.palette.iter().map(color_value).collect()),
            );
        }
        NodeKind::Text(text) => {
            object.insert("kind".to_string(), json!("text"));
            object.insert("content".to_string(), json!(text.content));
            object.insert("origin".to_string(), point_value(text.origin));
            object.insert("box_width".to_string(), opt_number(text.box_width));
            object.insert("box_height".to_string(), opt_number(text.box_height));
            object.insert("fixed_width".to_string(), json!(text.fixed_width));
            object.insert("style".to_string(), style_value(&text.style));
            object.insert(
                "palette".to_string(),
                Value::Array(text.palette.iter().map(color_value).collect()),
            );
        }
    }
    value
}

fn layer_value(layer: &ColorLayer) -> Value {
    json!({
        "color": color_value(&layer.color),
        "locked": layer.locked,
        "source": layer.source.map(|id| json!(id.get())).unwrap_or(Value::Null),
        "strokes": layer.strokes.iter().map(stroke_value).collect::<Vec<_>>(),
    })
}

fn stroke_value(stroke: &Stroke) -> Value {
    json!({
        "id": stroke.id.get(),
        "samples": stroke.samples.iter().map(sample_value).collect::<Vec<_>>(),
        "dynamics": dynamics_value(&stroke.dynamics),
        "size": stroke.size.map(|size| json!(size)).unwrap_or(Value::Null),
        "erases": stroke.erases.iter().map(erase_value).collect::<Vec<_>>(),
    })
}

fn erase_value(path: &ErasePath) -> Value {
    json!({
        "samples": path.samples.iter().map(sample_value).collect::<Vec<_>>(),
        "size": path.size,
        "dynamics": dynamics_value(&path.dynamics),
    })
}

fn sample_value(sample: &Sample) -> Value {
    json!({
        "x": sample.x,
        "y": sample.y,
        "pressure": sample.pressure.map(|pressure| json!(pressure)).unwrap_or(Value::Null),
    })
}

fn color_value(color: &PaletteColor) -> Value {
    json!({
        "id": color.id.get(),
        "role": role_name(color.role),
        "name": color.name,
        "rgb": color.rgb,
        "opacity": color.opacity,
        "size": color.size,
        "dash": dash_name(color.dash),
        "cap": cap_name(color.cap),
        "visible": color.visible,
    })
}

fn dynamics_value(dynamics: &Dynamics) -> Value {
    json!({
        "pressure_size": dynamics.pressure_size,
        "curve": dynamics.curve.iter().map(|point| json!({ "x": point.x, "y": point.y })).collect::<Vec<_>>(),
        "smoothing": dynamics.smoothing,
        "stabilization": dynamics.stabilization,
        "streamline": dynamics.streamline,
    })
}

fn style_value(style: &TextStyle) -> Value {
    json!({
        "family": match style.family {
            TextFamily::Inter => "inter",
            TextFamily::Monospace => "monospace",
        },
        "size": style.size,
        "bold": style.bold,
        "italic": style.italic,
        "underline": style.underline,
        "align": match style.align {
            TextAlign::Left => "left",
            TextAlign::Center => "center",
            TextAlign::Right => "right",
            TextAlign::Justify => "justify",
        },
        "valign": match style.valign {
            VAlign::Top => "top",
            VAlign::Middle => "middle",
            VAlign::Bottom => "bottom",
        },
        "line_height": style.line_height,
        "letter_spacing": style.letter_spacing,
        "all_caps": style.all_caps,
        "wrap": style.wrap,
        "padding": style.padding,
        "radius": style.radius,
        "list": style.list.map(list_name).map(|name| json!(name)).unwrap_or(Value::Null),
        "list_kind": list_name(style.list_kind),
    })
}

fn point_value(point: Point) -> Value {
    json!([point[0], point[1]])
}

fn opt_number(value: Option<f64>) -> Value {
    value.map(|number| json!(number)).unwrap_or(Value::Null)
}

fn geom_kind(geom: &ShapeGeom) -> &'static str {
    match geom {
        ShapeGeom::Rect { .. } => "rectangle",
        ShapeGeom::Ellipse { .. } => "ellipse",
        ShapeGeom::Line { .. } => "line",
    }
}

fn nodes_from(value: &Value, seen: &mut Seen) -> Option<Vec<Node>> {
    let mut nodes = Vec::new();
    for item in value.as_array()? {
        nodes.push(node_from(item, seen)?);
    }
    Some(nodes)
}

fn node_from(value: &Value, seen: &mut Seen) -> Option<Node> {
    let object = value.as_object()?;
    let id = seen.node(req_id(object.get("id")?)?)?;
    let name = object.get("name")?.as_str()?;
    if name.is_empty() {
        return None;
    }
    let visible = object.get("visible")?.as_bool()?;
    let locked = object.get("locked")?.as_bool()?;
    let transform = affine_from(object.get("transform")?)?;
    let kind = match object.get("kind")?.as_str()? {
        "canvas" => NodeKind::Canvas(canvas_from(object, seen)?),
        "group" => NodeKind::Group(nodes_from(object.get("children")?, seen)?),
        "rectangle" => NodeKind::Shape(shape_from(
            object,
            seen,
            ShapeGeom::Rect {
                min: point_from(object.get("min")?)?,
                max: point_from(object.get("max")?)?,
                corner_radius: finite(object.get("corner_radius")?)?,
            },
        )?),
        "ellipse" => NodeKind::Shape(shape_from(
            object,
            seen,
            ShapeGeom::Ellipse {
                min: point_from(object.get("min")?)?,
                max: point_from(object.get("max")?)?,
            },
        )?),
        "line" => NodeKind::Shape(shape_from(
            object,
            seen,
            ShapeGeom::Line {
                start: point_from(object.get("start")?)?,
                end: point_from(object.get("end")?)?,
            },
        )?),
        "text" => NodeKind::Text(text_from(object, seen)?),
        _ => return None,
    };
    Some(Node {
        id,
        name: name.to_string(),
        visible,
        locked,
        transform,
        kind,
    })
}

fn canvas_from(object: &Map<String, Value>, seen: &mut Seen) -> Option<super::Canvas> {
    let padding = finite(object.get("padding")?)?;
    let mut colors = Vec::new();
    for item in object.get("colors")?.as_array()? {
        colors.push(layer_from(item, seen)?);
    }
    Some(super::Canvas { padding, colors })
}

fn shape_from(object: &Map<String, Value>, seen: &mut Seen, geom: ShapeGeom) -> Option<ShapeNode> {
    let palette = palette_from(object.get("palette")?, seen)?;
    if palette.is_empty() {
        return None;
    }
    Some(ShapeNode {
        geom,
        palette,
        ink: OnceCell::new(),
    })
}

fn text_from(object: &Map<String, Value>, seen: &mut Seen) -> Option<TextNode> {
    let content = object.get("content")?.as_str()?.to_string();
    let palette = palette_from(object.get("palette")?, seen)?;
    if palette.is_empty() {
        return None;
    }
    Some(TextNode {
        content,
        origin: point_from(object.get("origin")?)?,
        box_width: opt_finite(object.get("box_width")?)?,
        box_height: opt_finite(object.get("box_height")?)?,
        fixed_width: finite(object.get("fixed_width")?)?,
        style: style_from(object.get("style")?)?,
        palette,
        measured: Cell::new((0.0, 0.0)),
        natural: Cell::new(0.0),
    })
}

fn layer_from(value: &Value, seen: &mut Seen) -> Option<ColorLayer> {
    let object = value.as_object()?;
    let mut strokes = Vec::new();
    for item in object.get("strokes")?.as_array()? {
        strokes.push(stroke_from(item, seen)?);
    }
    let source = match object.get("source")? {
        Value::Null => None,
        other => Some(ColorId::new(req_id(other)?)),
    };
    Some(ColorLayer {
        color: color_from(object.get("color")?, seen)?,
        locked: object.get("locked")?.as_bool()?,
        source,
        strokes,
        union_cache: OnceCell::new(),
    })
}

fn stroke_from(value: &Value, seen: &mut Seen) -> Option<Stroke> {
    let object = value.as_object()?;
    let mut erases = Vec::new();
    for item in object.get("erases")?.as_array()? {
        erases.push(erase_from(item)?);
    }
    Some(Stroke {
        id: seen.stroke(req_id(object.get("id")?)?)?,
        samples: samples_from(object.get("samples")?)?,
        dynamics: dynamics_from(object.get("dynamics")?)?,
        size: opt_u8(object.get("size")?)?,
        erases,
        visible: OnceCell::new(),
    })
}

fn erase_from(value: &Value) -> Option<ErasePath> {
    let object = value.as_object()?;
    Some(ErasePath {
        samples: samples_from(object.get("samples")?)?,
        size: req_u8(object.get("size")?)?,
        dynamics: dynamics_from(object.get("dynamics")?)?,
    })
}

fn samples_from(value: &Value) -> Option<Vec<Sample>> {
    let mut samples = Vec::new();
    for item in value.as_array()? {
        let object = item.as_object()?;
        let x = finite(object.get("x")?)?;
        let y = finite(object.get("y")?)?;
        let pressure = match object.get("pressure")? {
            Value::Null => None,
            other => {
                let pressure = finite(other)?;
                if !(0.0..=1.0).contains(&pressure) {
                    return None;
                }
                Some(pressure)
            }
        };
        samples.push(Sample { x, y, pressure });
    }
    Some(samples)
}

fn palette_from(value: &Value, seen: &mut Seen) -> Option<Vec<PaletteColor>> {
    let mut colors = Vec::new();
    for item in value.as_array()? {
        colors.push(color_from(item, seen)?);
    }
    Some(colors)
}

fn color_from(value: &Value, seen: &mut Seen) -> Option<PaletteColor> {
    let object = value.as_object()?;
    let name = object.get("name")?.as_str()?;
    if name.is_empty() {
        return None;
    }
    let rgb = object.get("rgb")?.as_array()?;
    if rgb.len() != 3 {
        return None;
    }
    Some(PaletteColor {
        id: seen.color(req_id(object.get("id")?)?)?,
        role: role_from(object.get("role")?.as_str()?)?,
        name: name.to_string(),
        rgb: [req_u8(&rgb[0])?, req_u8(&rgb[1])?, req_u8(&rgb[2])?],
        opacity: finite(object.get("opacity")?)?,
        size: req_u8(object.get("size")?)?,
        dash: dash_from(object.get("dash")?.as_str()?)?,
        cap: cap_from(object.get("cap")?.as_str()?)?,
        visible: object.get("visible")?.as_bool()?,
    })
}

fn dynamics_from(value: &Value) -> Option<Dynamics> {
    let object = value.as_object()?;
    let mut curve = Vec::new();
    for item in object.get("curve")?.as_array()? {
        let point = item.as_object()?;
        curve.push(CurvePoint {
            x: finite(point.get("x")?)?,
            y: finite(point.get("y")?)?,
        });
    }
    if curve.len() < 2 {
        return None;
    }
    Some(Dynamics {
        pressure_size: object.get("pressure_size")?.as_bool()?,
        curve,
        smoothing: req_u8(object.get("smoothing")?)?,
        stabilization: req_u8(object.get("stabilization")?)?,
        streamline: req_u8(object.get("streamline")?)?,
    })
}

fn style_from(value: &Value) -> Option<TextStyle> {
    let object = value.as_object()?;
    let list = match object.get("list")? {
        Value::Null => None,
        other => Some(list_from(other.as_str()?)?),
    };
    Some(TextStyle {
        family: match object.get("family")?.as_str()? {
            "inter" => TextFamily::Inter,
            "monospace" => TextFamily::Monospace,
            _ => return None,
        },
        size: req_u8(object.get("size")?)?,
        bold: object.get("bold")?.as_bool()?,
        italic: object.get("italic")?.as_bool()?,
        underline: object.get("underline")?.as_bool()?,
        align: match object.get("align")?.as_str()? {
            "left" => TextAlign::Left,
            "center" => TextAlign::Center,
            "right" => TextAlign::Right,
            "justify" => TextAlign::Justify,
            _ => return None,
        },
        valign: match object.get("valign")?.as_str()? {
            "top" => VAlign::Top,
            "middle" => VAlign::Middle,
            "bottom" => VAlign::Bottom,
            _ => return None,
        },
        line_height: finite(object.get("line_height")?)?,
        letter_spacing: finite(object.get("letter_spacing")?)?,
        all_caps: object.get("all_caps")?.as_bool()?,
        wrap: object.get("wrap")?.as_bool()?,
        padding: finite(object.get("padding")?)?,
        radius: finite(object.get("radius")?)?,
        list,
        list_kind: list_from(object.get("list_kind")?.as_str()?)?,
    })
}

fn camera_from(value: &Value) -> Option<Camera> {
    let object = value.as_object()?;
    let mut zoom = finite(object.get("zoom")?)?;
    if zoom <= 0.0 {
        return None;
    }
    zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    Some(Camera {
        origin: point_from(object.get("origin")?)?,
        zoom,
    })
}

fn first_canvas(nodes: &[Node]) -> Option<NodeId> {
    for node in nodes {
        if node.canvas().is_some() {
            return Some(node.id);
        }
        if let NodeKind::Group(children) = &node.kind
            && let Some(id) = first_canvas(children)
        {
            return Some(id);
        }
    }
    None
}

fn affine_from(value: &Value) -> Option<Affine> {
    let numbers = value.as_array()?;
    if numbers.len() != 6 {
        return None;
    }
    let mut coeffs = [0.0; 6];
    for (slot, number) in coeffs.iter_mut().zip(numbers) {
        *slot = finite(number)?;
    }
    Some(Affine::new(coeffs))
}

fn point_from(value: &Value) -> Option<Point> {
    let numbers = value.as_array()?;
    if numbers.len() != 2 {
        return None;
    }
    Some([finite(&numbers[0])?, finite(&numbers[1])?])
}

fn finite(value: &Value) -> Option<f64> {
    let number = value.as_f64()?;
    number.is_finite().then_some(number)
}

fn opt_finite(value: &Value) -> Option<Option<f64>> {
    if value.is_null() {
        return Some(None);
    }
    Some(Some(finite(value)?))
}

fn req_id(value: &Value) -> Option<u64> {
    let id = value.as_u64()?;
    (id > 0).then_some(id)
}

fn req_u8(value: &Value) -> Option<u8> {
    let number = value.as_u64()?;
    u8::try_from(number).ok()
}

fn opt_u8(value: &Value) -> Option<Option<u8>> {
    if value.is_null() {
        return Some(None);
    }
    Some(Some(req_u8(value)?))
}

fn role_name(role: ColorRole) -> &'static str {
    match role {
        ColorRole::Layer => "layer",
        ColorRole::Outline => "outline",
        ColorRole::Stroke => "stroke",
        ColorRole::Fill => "fill",
        ColorRole::Text => "text",
        ColorRole::Background => "background",
        ColorRole::Border => "border",
    }
}

fn role_from(name: &str) -> Option<ColorRole> {
    Some(match name {
        "layer" => ColorRole::Layer,
        "outline" => ColorRole::Outline,
        "stroke" => ColorRole::Stroke,
        "fill" => ColorRole::Fill,
        "text" => ColorRole::Text,
        "background" => ColorRole::Background,
        "border" => ColorRole::Border,
        _ => return None,
    })
}

fn dash_name(dash: Dash) -> &'static str {
    match dash {
        Dash::Solid => "solid",
        Dash::Dashed => "dashed",
    }
}

fn dash_from(name: &str) -> Option<Dash> {
    Some(match name {
        "solid" => Dash::Solid,
        "dashed" => Dash::Dashed,
        _ => return None,
    })
}

fn cap_name(cap: Cap) -> &'static str {
    match cap {
        Cap::Round => "round",
        Cap::Flat => "flat",
    }
}

fn cap_from(name: &str) -> Option<Cap> {
    Some(match name {
        "round" => Cap::Round,
        "flat" => Cap::Flat,
        _ => return None,
    })
}

fn list_name(kind: ListKind) -> &'static str {
    match kind {
        ListKind::Bullets => "bullets",
        ListKind::Numbers => "numbers",
        ListKind::Checks => "checks",
    }
}

fn list_from(name: &str) -> Option<ListKind> {
    Some(match name {
        "bullets" => ListKind::Bullets,
        "numbers" => ListKind::Numbers,
        "checks" => ListKind::Checks,
        _ => return None,
    })
}
