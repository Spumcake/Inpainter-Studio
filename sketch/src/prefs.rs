//! One versioned JSON file of tool settings, including every tool's palette,
//! in the platform user-data directory. A missing or unreadable file is the
//! built-in defaults. It never stops the app.

use crate::document::{
    Allocator, BoundedSize, Cap, ColorId, ColorRole, Dash, Dynamics, ListKind, ShapeTool,
    TextAlign, TextFamily, TextTool, ToolColor, VAlign,
};
use crate::schema::{Inspect, PropId, PropValue};
use crate::session::ToolSnapshot;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "preferences.json";

/// The platform user-data directory for Inpainter. Preferences, recovery,
/// and leftover sketch files live in this directory.
pub fn data_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("com.spumcake", "Spumcake", "Inpainter")
        .map(|dirs| dirs.data_dir().to_path_buf())
}

pub fn home_dir() -> Option<PathBuf> {
    directories::UserDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
}

fn preferences_path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Tool settings and the recent-path list. A missing or unreadable file is
/// the built-in defaults and an empty recent list.
pub fn load_dir(dir: &Path) -> (ToolSnapshot, Vec<PathBuf>) {
    let Ok(text) = std::fs::read_to_string(preferences_path(dir)) else {
        return (ToolSnapshot::builtin(), Vec::new());
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return (ToolSnapshot::builtin(), Vec::new());
    };
    let Some(object) = value.as_object() else {
        return (ToolSnapshot::builtin(), Vec::new());
    };
    if object.get("format_version").and_then(Value::as_u64) != Some(1) {
        return (ToolSnapshot::builtin(), Vec::new());
    }
    (snapshot_from(object), recent_from(object))
}

fn recent_from(object: &Map<String, Value>) -> Vec<PathBuf> {
    object
        .get("recent")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(PathBuf::from)
                .take(10)
                .collect()
        })
        .unwrap_or_default()
}

/// Writes tool settings and keeps `recent`. Returns false when the file could
/// not be written.
pub fn save_tools(dir: &Path, tools: &ToolSnapshot) -> bool {
    let mut root = read_root(dir);
    let Some(object) = root.as_object_mut() else {
        return false;
    };
    write_tools(object, tools);
    write_atomic(&preferences_path(dir), &root)
}

/// Writes the recent paths and keeps the tool settings already in the file.
pub fn save_recent(dir: &Path, recent: &[PathBuf]) -> bool {
    let mut root = read_root(dir);
    let Some(object) = root.as_object_mut() else {
        return false;
    };
    let paths: Vec<Value> = recent
        .iter()
        .take(10)
        .map(|path| Value::String(path.to_string_lossy().into_owned()))
        .collect();
    object.insert("recent".to_string(), Value::Array(paths));
    write_atomic(&preferences_path(dir), &root)
}

fn read_root(dir: &Path) -> Value {
    match std::fs::read_to_string(preferences_path(dir))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    {
        Some(Value::Object(map))
            if map.get("format_version").and_then(Value::as_u64) == Some(1) =>
        {
            Value::Object(map)
        }
        _ => json!({ "format_version": 1 }),
    }
}

/// Puts the tool settings into a preferences object, keeping keys this
/// version does not know.
fn write_tools(object: &mut Map<String, Value>, tools: &ToolSnapshot) {
    object.insert("format_version".to_string(), json!(1));
    object.insert("brush".to_string(), dynamics_json(&tools.brush));
    object.insert(
        "eraser".to_string(),
        eraser_json(&tools.eraser, tools.eraser_size),
    );
    object.remove("rectangle_stroke");
    object.remove("ellipse_stroke");
    object.remove("line_stroke");
    object.remove("palette");
    object.insert(
        "brush_palette".to_string(),
        palette_json(&tools.brush_palette),
    );
    if let Some(active) = tools.brush_active {
        object.insert("brush_active".to_string(), json!(active.get()));
    }
    object.insert("rectangle".to_string(), shape_json(&tools.rectangle));
    object.insert("ellipse".to_string(), shape_json(&tools.ellipse));
    object.insert("line".to_string(), shape_json(&tools.line));
    object.insert("font_size".to_string(), json!(tools.font_size));
    object.insert("text".to_string(), text_json(&tools.text));
}

pub fn parse(text: &str) -> Option<ToolSnapshot> {
    let value: Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;
    if object.get("format_version")?.as_u64()? != 1 {
        return None;
    }
    Some(snapshot_from(object))
}

fn snapshot_from(object: &Map<String, Value>) -> ToolSnapshot {
    let mut tools = ToolSnapshot::builtin();
    // Colors without a stored id get one here; the session makes every id
    // unique and reserves them when it loads this snapshot.
    let mut ids = Allocator::new();
    if let Some(brush) = object.get("brush") {
        tools.brush = dynamics_from(brush);
    }
    if let Some(eraser) = object.get("eraser") {
        let (dynamics, size) = eraser_from(eraser);
        tools.eraser = dynamics;
        tools.eraser_size = size;
    }
    if let Some(shape) = object.get("rectangle") {
        tools.rectangle = shape_from(shape, tools.rectangle, &mut ids);
    }
    if let Some(shape) = object.get("ellipse") {
        tools.ellipse = shape_from(shape, tools.ellipse, &mut ids);
    }
    if let Some(shape) = object.get("line") {
        tools.line = shape_from(shape, tools.line, &mut ids);
    }
    // Before palettes, a shape tool kept its stroke width on its own.
    for (key, tool) in [
        ("rectangle_stroke", &mut tools.rectangle),
        ("ellipse_stroke", &mut tools.ellipse),
        ("line_stroke", &mut tools.line),
    ] {
        if object.get(key.trim_end_matches("_stroke")).is_none()
            && let Some(size) = object.get(key).and_then(Value::as_f64)
            && let Some(outline) = tool.palette.first_mut()
        {
            outline.color.size = stored_size(size, outline.color.size, 1, 32);
            outline.next_size = outline.color.size;
        }
    }
    if let Some(size) = object.get("font_size").and_then(Value::as_f64) {
        tools.font_size = stored_size(size, tools.font_size, 8, 144);
        tools.text.style.size = tools.font_size;
    }
    if let Some(text) = object
        .get("text")
        .and_then(|value| text_from(value, &mut ids))
    {
        tools.text = text;
        tools.font_size = tools.text.style.size;
    }
    // Before tool palettes, the brush's colors were the default palette.
    let brush = object
        .get("brush_palette")
        .or_else(|| object.get("palette"))
        .and_then(|value| palette_from(value, ColorRole::Layer, &mut ids));
    if let Some(palette) = brush {
        tools.brush_palette = palette;
    }
    tools.brush_active = object
        .get("brush_active")
        .and_then(Value::as_u64)
        .filter(|id| *id > 0)
        .map(ColorId::new);
    tools
}

fn palette_from(value: &Value, role: ColorRole, ids: &mut Allocator) -> Option<Vec<ToolColor>> {
    let colors: Vec<ToolColor> = value
        .as_array()?
        .iter()
        .filter_map(|item| color_from(item, role, ids))
        .collect();
    if colors.is_empty() {
        None
    } else {
        Some(colors)
    }
}

/// A fixed-role palette, read in role order. A stored palette with the wrong
/// roles keeps the tool's own.
fn roles_from(value: &Value, current: &[ToolColor], ids: &mut Allocator) -> Vec<ToolColor> {
    let Some(items) = value.as_array() else {
        return current.to_vec();
    };
    if items.len() != current.len() {
        return current.to_vec();
    }
    items
        .iter()
        .zip(current)
        .map(|(item, fallback)| {
            color_from(item, fallback.color.role, ids).unwrap_or_else(|| fallback.clone())
        })
        .collect()
}

fn color_from(value: &Value, role: ColorRole, ids: &mut Allocator) -> Option<ToolColor> {
    let object = value.as_object()?;
    let name = object.get("name")?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    let rgb = object.get("rgb").and_then(rgb_from)?;
    let id = object
        .get("id")
        .and_then(Value::as_u64)
        .filter(|id| *id > 0)
        .map(ColorId::new)
        .unwrap_or_else(|| ids.color());
    let size = object
        .get("size")
        .and_then(Value::as_f64)
        .map(|number| stored_size(number, crate::defaults::COLOR_SIZE, 1, 32))
        .unwrap_or(crate::defaults::COLOR_SIZE);
    let color = crate::document::PaletteColor {
        id,
        role,
        name: name.chars().take(80).collect(),
        rgb,
        opacity: defaults_opacity(object.get("opacity").and_then(Value::as_f64)),
        size,
        dash: match object.get("dash").and_then(Value::as_str) {
            Some("dashed") => Dash::Dashed,
            _ => Dash::Solid,
        },
        cap: match object.get("cap").and_then(Value::as_str) {
            Some("flat") => Cap::Flat,
            _ => Cap::Round,
        },
        visible: object
            .get("visible")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    };
    let mut tool = ToolColor::new(color);
    if let Some(lock) = object.get("lock_size").and_then(Value::as_bool) {
        tool.lock_size = lock;
    }
    if let Some(next) = object.get("next_size").and_then(Value::as_f64) {
        tool.next_size = stored_size(next, size, 1, 32);
    }
    Some(tool)
}

fn defaults_opacity(value: Option<f64>) -> f64 {
    value
        .filter(|number| number.is_finite())
        .map(|number| number.clamp(0.0, 1.0))
        .unwrap_or(crate::defaults::COLOR_OPACITY)
}

fn palette_json(palette: &[ToolColor]) -> Value {
    Value::Array(
        palette
            .iter()
            .map(|item| {
                let color = &item.color;
                json!({
                    "id": color.id.get(),
                    "name": color.name,
                    "rgb": color.rgb,
                    "opacity": color.opacity,
                    "size": color.size,
                    "dash": match color.dash {
                        Dash::Solid => "solid",
                        Dash::Dashed => "dashed",
                    },
                    "cap": match color.cap {
                        Cap::Round => "round",
                        Cap::Flat => "flat",
                    },
                    "visible": color.visible,
                    "lock_size": item.lock_size,
                    "next_size": item.next_size,
                })
            })
            .collect(),
    )
}

fn dynamics_from(value: &Value) -> Dynamics {
    let mut dynamics = Dynamics::round();
    let Some(object) = value.as_object() else {
        return dynamics;
    };
    if let Some(on) = object.get("pressure_size").and_then(Value::as_bool) {
        let _ = dynamics.write(PropId::PressureSize, &PropValue::Bool(on));
    }
    if let Some(number) = object.get("smoothing").and_then(Value::as_f64) {
        let _ = dynamics.write(PropId::Smoothing, &PropValue::Number(number));
    }
    if let Some(number) = object.get("stabilization").and_then(Value::as_f64) {
        let _ = dynamics.write(PropId::Stabilization, &PropValue::Number(number));
    }
    if let Some(number) = object.get("streamline").and_then(Value::as_f64) {
        let _ = dynamics.write(PropId::Streamline, &PropValue::Number(number));
    }
    if let Some(curve) = object.get("curve").and_then(curve_from) {
        let _ = dynamics.write(PropId::Curve, &PropValue::Curve(curve));
    }
    dynamics
}

fn eraser_from(value: &Value) -> (Dynamics, u8) {
    let dynamics = dynamics_from(value);
    let size = value
        .get("size")
        .and_then(Value::as_f64)
        .map(|number| stored_size(number, crate::defaults::ERASER_SIZE, 1, 32))
        .unwrap_or(crate::defaults::ERASER_SIZE);
    (dynamics, size)
}

fn curve_from(value: &Value) -> Option<Vec<(f64, f64)>> {
    let points = value.as_array()?;
    let curve = points
        .iter()
        .filter_map(|point| {
            let point = point.as_object()?;
            Some((point.get("x")?.as_f64()?, point.get("y")?.as_f64()?))
        })
        .collect();
    Some(curve)
}

fn stored_size(number: f64, fallback: u8, min: u8, max: u8) -> u8 {
    let mut size = BoundedSize::new(fallback, min, max);
    if size.write(PropId::EraserSize, &PropValue::Number(number)) {
        size.value
    } else {
        fallback
    }
}

fn text_json(tool: &TextTool) -> Value {
    json!({
        "family": match tool.style.family {
            TextFamily::Inter => "inter",
            TextFamily::Monospace => "monospace",
        },
        "size": tool.style.size,
        "bold": tool.style.bold,
        "italic": tool.style.italic,
        "underline": tool.style.underline,
        "palette": palette_json(&tool.palette),
        "align": match tool.style.align {
            TextAlign::Left => "left",
            TextAlign::Center => "center",
            TextAlign::Right => "right",
            TextAlign::Justify => "justify",
        },
        "valign": match tool.style.valign {
            VAlign::Top => "top",
            VAlign::Middle => "middle",
            VAlign::Bottom => "bottom",
        },
        "padding": tool.style.padding,
        "radius": tool.style.radius,
        "list": match tool.style.list {
            None => "none",
            Some(ListKind::Bullets) => "bullets",
            Some(ListKind::Numbers) => "numbers",
            Some(ListKind::Checks) => "checks",
        },
        "list_kind": match tool.style.list_kind {
            ListKind::Bullets => "bullets",
            ListKind::Numbers => "numbers",
            ListKind::Checks => "checks",
        },
        "line_height": tool.style.line_height,
        "letter_spacing": tool.style.letter_spacing,
        "all_caps": tool.style.all_caps,
        "wrap": tool.style.wrap,
        "auto_size": tool.auto_size,
    })
}

fn text_from(value: &Value, ids: &mut Allocator) -> Option<TextTool> {
    let object = value.as_object()?;
    let mut tool = TextTool::builtin(ids);
    if let Some(family) = object.get("family").and_then(Value::as_str) {
        tool.style.family = match family {
            "monospace" => TextFamily::Monospace,
            _ => TextFamily::Inter,
        };
    }
    if let Some(size) = object.get("size").and_then(Value::as_f64) {
        tool.style.size = stored_size(size, tool.style.size, 8, 144);
    }
    if let Some(on) = object.get("bold").and_then(Value::as_bool) {
        tool.style.bold = on;
    }
    if let Some(on) = object.get("italic").and_then(Value::as_bool) {
        tool.style.italic = on;
    }
    if let Some(on) = object.get("underline").and_then(Value::as_bool) {
        tool.style.underline = on;
    }
    if let Some(palette) = object.get("palette") {
        tool.palette = roles_from(palette, &tool.palette, ids);
    } else {
        // Before palettes, text kept its color and background on its style.
        let [text, background] = &mut tool.palette[..] else {
            return None;
        };
        if let Some(rgb) = object.get("rgb").and_then(rgb_from) {
            text.color.rgb = rgb;
        }
        if let Some(opacity) = object.get("opacity").and_then(Value::as_f64) {
            text.color.opacity = defaults_opacity(Some(opacity));
        }
        if let Some(on) = object.get("background_on").and_then(Value::as_bool) {
            background.color.visible = on;
        }
        if let Some(rgb) = object.get("background_rgb").and_then(rgb_from) {
            background.color.rgb = rgb;
        }
        if let Some(opacity) = object.get("background_opacity").and_then(Value::as_f64) {
            background.color.opacity = defaults_opacity(Some(opacity));
        }
    }
    if let Some(align) = object.get("align").and_then(Value::as_str) {
        tool.style.align = match align {
            "center" => TextAlign::Center,
            "right" => TextAlign::Right,
            "justify" => TextAlign::Justify,
            _ => TextAlign::Left,
        };
    }
    if let Some(valign) = object.get("valign").and_then(Value::as_str) {
        tool.style.valign = match valign {
            "middle" => VAlign::Middle,
            "bottom" => VAlign::Bottom,
            _ => VAlign::Top,
        };
    }
    let list_kind = |name: &str| match name {
        "numbers" => Some(ListKind::Numbers),
        "checks" => Some(ListKind::Checks),
        "bullets" => Some(ListKind::Bullets),
        _ => None,
    };
    if let Some(kind) = object
        .get("list_kind")
        .and_then(Value::as_str)
        .and_then(list_kind)
    {
        tool.style.list_kind = kind;
    }
    if let Some(list) = object.get("list").and_then(Value::as_str) {
        tool.style.list = list_kind(list);
    }
    for (key, prop) in [
        ("padding", PropId::TextPadding),
        ("radius", PropId::TextRadius),
    ] {
        if let Some(number) = object.get(key).and_then(Value::as_f64) {
            let _ = tool.style.write(prop, &PropValue::Number(number));
        }
    }
    if let Some(height) = object.get("line_height").and_then(Value::as_f64) {
        let _ = tool
            .style
            .write(PropId::LineHeight, &PropValue::Number(height));
    }
    if let Some(spacing) = object.get("letter_spacing").and_then(Value::as_f64) {
        let _ = tool
            .style
            .write(PropId::LetterSpacing, &PropValue::Number(spacing));
    }
    if let Some(on) = object.get("all_caps").and_then(Value::as_bool) {
        tool.style.all_caps = on;
    }
    if let Some(on) = object.get("wrap").and_then(Value::as_bool) {
        tool.style.wrap = on;
    }
    if let Some(on) = object.get("auto_size").and_then(Value::as_bool) {
        tool.auto_size = on;
    }
    Some(tool)
}

fn shape_json(tool: &ShapeTool) -> Value {
    json!({
        "palette": palette_json(&tool.palette),
        "corner_radius": tool.corner_radius,
        "constrain": tool.constrain,
        "snap": tool.snap,
    })
}

fn shape_from(value: &Value, mut tool: ShapeTool, ids: &mut Allocator) -> ShapeTool {
    let Some(object) = value.as_object() else {
        return tool;
    };
    if let Some(palette) = object.get("palette") {
        tool.palette = roles_from(palette, &tool.palette, ids);
    } else {
        // Before palettes, a shape tool kept its fill and stroke as settings.
        if let Some(outline) = tool.palette.first_mut() {
            if let Some(rgb) = object.get("stroke_rgb").and_then(rgb_from) {
                outline.color.rgb = rgb;
            }
            if let Some(width) = object.get("stroke_width").and_then(Value::as_f64) {
                outline.color.size = stored_size(width, outline.color.size, 1, 32);
                outline.next_size = outline.color.size;
            }
            if let Some(opacity) = object.get("stroke_opacity").and_then(Value::as_f64) {
                outline.color.opacity = defaults_opacity(Some(opacity));
            }
            if object.get("dash").and_then(Value::as_str) == Some("dashed") {
                outline.color.dash = Dash::Dashed;
            }
        }
        if let Some(fill) = tool.palette.get_mut(1) {
            if let Some(on) = object.get("fill_on").and_then(Value::as_bool) {
                fill.color.visible = on;
            }
            if let Some(rgb) = object.get("fill_rgb").and_then(rgb_from) {
                fill.color.rgb = rgb;
            }
            if let Some(opacity) = object.get("fill_opacity").and_then(Value::as_f64) {
                fill.color.opacity = defaults_opacity(Some(opacity));
            }
        }
    }
    if let Some(radius) = object.get("corner_radius").and_then(Value::as_f64) {
        if radius.is_finite() {
            tool.corner_radius = radius.clamp(0.0, 64.0);
        }
    }
    if let Some(on) = object.get("constrain").and_then(Value::as_bool) {
        tool.constrain = on;
    }
    if let Some(on) = object.get("snap").and_then(Value::as_bool) {
        tool.snap = on;
    }
    tool
}

fn rgb_from(value: &Value) -> Option<[u8; 3]> {
    let rgb = value.as_array()?;
    if rgb.len() != 3 {
        return None;
    }
    let channel = |index: usize| rgb.get(index)?.as_u64().and_then(|n| u8::try_from(n).ok());
    Some([channel(0)?, channel(1)?, channel(2)?])
}

fn dynamics_json(dynamics: &Dynamics) -> Value {
    json!({
        "pressure_size": dynamics.pressure_size,
        "curve": curve_json(&dynamics.curve),
        "smoothing": dynamics.smoothing,
        "stabilization": dynamics.stabilization,
        "streamline": dynamics.streamline,
    })
}

fn eraser_json(dynamics: &Dynamics, size: u8) -> Value {
    let mut value = dynamics_json(dynamics);
    if let Some(object) = value.as_object_mut() {
        object.insert("size".to_string(), json!(size));
    }
    value
}

fn curve_json(curve: &[crate::document::CurvePoint]) -> Value {
    Value::Array(
        curve
            .iter()
            .map(|point| json!({ "x": point.x, "y": point.y }))
            .collect(),
    )
}

fn write_atomic(path: &Path, value: &Value) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return false;
    }
    let Ok(text) = serde_json::to_string_pretty(value) else {
        return false;
    };
    let temporary = path.with_extension("json.tmp");
    if std::fs::write(&temporary, text).is_err() {
        return false;
    }
    if std::fs::rename(&temporary, path).is_err() {
        let _ = std::fs::remove_file(&temporary);
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_preferences_use_defaults() {
        assert!(parse("").is_none());
        assert!(parse("{").is_none());
        assert!(parse(r#"{"format_version": 2}"#).is_none());
        assert!(parse(r#"{"brush": {"smoothing": 10}}"#).is_none());

        let parsed = parse(
            r#"{"format_version": 1, "brush": {"smoothing": 240, "pressure_size": false}, "palette": [{"name": "kept"}]}"#,
        )
        .expect("version 1 is readable");
        assert_eq!(parsed.brush.smoothing, 100);
        assert!(!parsed.brush.pressure_size);
        assert_eq!(
            parsed.brush.streamline,
            ToolSnapshot::builtin().brush.streamline
        );
        assert_eq!(parsed.eraser_size, ToolSnapshot::builtin().eraser_size);

        let text = serde_json::to_string(&json!({
            "format_version": 1,
            "brush": dynamics_json(&parsed.brush),
            "palette": [{"name": "kept"}]
        }))
        .unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert!(
            value.get("palette").is_some(),
            "a palette key is not dropped by reading"
        );
    }

    #[test]
    fn tool_palettes_survive_a_save_and_an_old_palette_becomes_the_brushes() {
        let mut tools = ToolSnapshot::builtin();
        tools.brush_palette[1].color.rgb = [1, 2, 3];
        tools.brush_palette[1].lock_size = false;
        tools.brush_palette[1].next_size = 17;
        tools.brush_active = Some(tools.brush_palette[1].color.id);
        tools.rectangle.palette[1].color.visible = true;
        tools.text.palette[1].color.rgb = [4, 5, 6];
        let mut object = Map::new();
        write_tools(&mut object, &tools);
        let text = serde_json::to_string(&Value::Object(object)).unwrap();
        let read = parse(&text).expect("a saved file is readable");
        assert_eq!(read.brush_palette, tools.brush_palette);
        assert_eq!(read.brush_active, tools.brush_active);
        assert_eq!(read.rectangle.palette, tools.rectangle.palette);
        assert_eq!(read.text.palette, tools.text.palette);

        let old = r#"{"format_version": 1, "palette": [{"name": "Ink", "rgb": [9, 8, 7], "size": 5}],
            "rectangle": {"fill_on": true, "fill_rgb": [1, 1, 1], "stroke_width": 6}}"#;
        let read = parse(old).unwrap();
        assert_eq!(read.brush_palette.len(), 1);
        assert_eq!(read.brush_palette[0].color.name, "Ink");
        assert_eq!(read.brush_palette[0].color.size, 5);
        assert!(read.brush_palette[0].lock_size);
        assert!(read.rectangle.palette[1].color.visible);
        assert_eq!(read.rectangle.palette[1].color.rgb, [1, 1, 1]);
        assert_eq!(read.rectangle.palette[0].color.size, 6);
    }
}
