//! What a sub-application declares for the host to draw after its greeting:
//! commands and their keys, the toolbar, the menu, the properties panel, the
//! tab list, the drawing surface's current tool, and questions.
//!
//! Every control carries the [`Action`] the host sends back when it is used.
//! The host never reads an action's id or argument; it sends them as they
//! came. A control that also has a value of its own, such as a slider, a
//! typed name, or a sort position, sends its action with that value added
//! under `value` (see [`Action::with_value`]).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What the host sends when a declared control is used: `command {id, tab, arg}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg: Option<Value>,
}

impl Action {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            arg: None,
        }
    }

    pub fn with_arg(id: impl Into<String>, arg: Value) -> Self {
        Self {
            id: id.into(),
            arg: Some(arg),
        }
    }

    /// This action with `value` added: into the argument when it is an
    /// object, and otherwise as `{"arg": <argument>, "value": value}`.
    pub fn with_value(&self, value: Value) -> Self {
        let arg = match self.arg.clone() {
            Some(Value::Object(mut object)) => {
                object.insert("value".to_string(), value);
                Value::Object(object)
            }
            Some(other) => serde_json::json!({ "arg": other, "value": value }),
            None => serde_json::json!({ "value": value }),
        };
        Self {
            id: self.id.clone(),
            arg: Some(arg),
        }
    }
}

/// A key and its modifiers. `shift: None` ignores Shift. `key` is a key's
/// name: a letter, a digit, `F4`, `Delete`, `Backspace`, `Left`, `Right`,
/// `Up`, `Down`, `Plus`, `Equals`, `Minus`, `BracketLeft`, `BracketRight`,
/// `Escape`, or `Enter`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chord {
    pub key: String,
    #[serde(default)]
    pub command: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shift: Option<bool>,
    #[serde(default)]
    pub alt: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    pub chord: Chord,
    /// Chords pressed in turn after `chord`, each within a second, such as
    /// the second D of D D. Empty for a single chord.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub then: Vec<Chord>,
    pub action: Action,
}

/// The keyboard: every shortcut and what it sends, and the actions for the
/// window's copy, cut, and paste, which arrive as their own events.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Keys {
    pub bindings: Vec<Binding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy: Option<Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut: Option<Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paste: Option<Action>,
}

/// One row of a menu. A row with `submenu` rows opens them; a row with
/// `rename` starts renaming the thing its menu belongs to in place.
/// `divider` draws a line above the row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MenuEntry {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub divider: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<Action>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub submenu: Vec<MenuEntry>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub rename: bool,
}

fn yes() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// One toolbar item, in the order declared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolItem {
    /// A divider between groups.
    Separator,
    /// Space before a run of swatches, so the active ring is not clipped.
    Inset,
    /// Color swatches. With `sort`, a swatch can be dragged along the row,
    /// and the release sends `sort` with `{"color": id, "value": position}`.
    Swatches {
        swatches: Vec<Swatch>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sort: Option<Action>,
    },
    /// An icon button. A right-click opens `menu` when it has rows.
    Button {
        icon: String,
        tip: String,
        #[serde(default = "yes")]
        enabled: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        selected: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<Action>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        menu: Vec<MenuEntry>,
    },
    /// An icon and a chevron that opens its rows.
    Menu {
        icon: String,
        label: String,
        #[serde(default = "yes")]
        enabled: bool,
        rows: Vec<MenuEntry>,
    },
    /// A caption, a slider, and its value as a number that can be typed.
    /// A change sends `set` with `{"value": n, "live": held}`; the release
    /// sends `finish`.
    Slider {
        caption: String,
        width: f32,
        value: i32,
        min: i32,
        max: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tint: Option<[u8; 3]>,
        set: Action,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        finish: Option<Action>,
    },
    /// The host window's maximize toggle.
    Maximize {
        icon: String,
        restore_icon: String,
        tip: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Swatch {
    pub id: u64,
    pub rgb: [u8; 3],
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub muted: bool,
    pub name: String,
    pub click: Action,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub menu: Vec<MenuEntry>,
}

/// The toolbar of the sub-application's selected tab. `trailing` is pinned
/// to the right and laid out from the right edge.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Toolbar {
    pub items: Vec<ToolItem>,
    #[serde(default)]
    pub trailing: Vec<ToolItem>,
}

/// A property's value, as the panel shows it and sends it back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropValue {
    Bool(bool),
    Number(f64),
    Text(String),
    Index(u8),
    Rgb([u8; 3]),
    Curve(Vec<(f64, f64)>),
    /// Several objects with different values.
    Mixed,
    /// A row with no value of its own, such as a palette.
    Idle,
}

/// How a choice row draws each option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChoicePreview {
    Label,
    Cap,
    Dash,
}

/// The stroke sample beside a preset list: drawn as an eraser or a brush,
/// with these dynamics in `draw`'s shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetSample {
    pub eraser: bool,
    pub dynamics: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RowKind {
    Toggle,
    Number {
        min: f64,
        max: f64,
        percent: bool,
        places: u8,
    },
    Text,
    Choice {
        options: Vec<String>,
        segmented: bool,
        preview: ChoicePreview,
    },
    Preset {
        options: Vec<String>,
        custom: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sample: Option<PresetSample>,
    },
    Rgb {
        #[serde(default)]
        quick: Vec<[u8; 3]>,
    },
    Curve,
    /// An object's colors, as the same rows the outliner draws.
    Palette {
        owner: u64,
        colors: Vec<ColorRow>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sort: Option<Action>,
        flags: FlagIcons,
    },
}

/// One property row. An edit sends `property {prop, value, preview}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub prop: Value,
    pub label: String,
    #[serde(flatten)]
    pub kind: RowKind,
    pub value: PropValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub title: String,
    pub rows: Vec<Row>,
}

/// A palette color's row: in the outliner under its canvas, or in an
/// object's Palette section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorRow {
    pub id: u64,
    pub name: String,
    pub rgb: [u8; 3],
    pub visible: bool,
    pub locked: bool,
    /// A shape's or text's role: no renaming, sorting, or lock.
    pub fixed: bool,
    pub open: Action,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename: Option<Action>,
    pub toggle_visible: Action,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toggle_locked: Option<Action>,
    #[serde(default)]
    pub menu: Vec<MenuEntry>,
}

/// One object row of the outliner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerRow {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u64>,
    pub depth: u32,
    pub icon: String,
    pub name: String,
    pub group: bool,
    pub selected: bool,
    pub visible: bool,
    pub locked: bool,
    /// Double-clicking the name renames it.
    pub renamable: bool,
    /// Sent with `{"value": shift}`.
    pub click: Action,
    /// Sent with `{"value": new name}`.
    pub rename: Action,
    pub toggle_visible: Action,
    pub toggle_locked: Action,
    #[serde(default)]
    pub menu: Vec<MenuEntry>,
    #[serde(default)]
    pub colors: Vec<ColorRow>,
}

/// The outliner. Dragging a row among its siblings sends `reorder` with
/// `{"node": id, "value": position}`; a color among its canvas's colors
/// sends `sort_color` with `{"color": id, "value": position}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layers {
    pub rows: Vec<LayerRow>,
    pub reorder: Action,
    pub sort_color: Action,
    pub flags: FlagIcons,
}

/// The icons a row's eye and lock are drawn with, shown and hidden, locked
/// and unlocked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlagIcons {
    pub eye_open: String,
    pub eye_closed: String,
    pub lock_closed: String,
    pub lock_open: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Back {
    pub label: String,
    pub action: Action,
}

/// The properties panel of the sub-application's selected tab. `open` is
/// whether it is shown; the edge tab and its key send `toggle`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Properties {
    pub open: bool,
    pub toggle: Action,
    pub tip: String,
    /// Sent when a slider, curve, or color drag ends.
    pub finish: Action,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back: Option<Back>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layers: Option<Layers>,
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabEntry {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub dirty: bool,
    /// The close mark waits: the only tab, clean.
    #[serde(default)]
    pub close_waits: bool,
    /// The file the tab shows, once it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The tab is a document that can be saved to a file.
    #[serde(default)]
    pub saves: bool,
}

/// The sub-application's tabs, which replace those it reported before.
/// The plus runs `new_tab` for this sub-application's tabs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tabs {
    pub tabs: Vec<TabEntry>,
    pub active: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_tab: Option<Action>,
    #[serde(default)]
    pub new_tab_tip: String,
}

/// The current tool, as the drawing surface previews and sends it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CanvasTool {
    /// Pick, marquee, move, scale, and rotate.
    Select,
    /// A pressure stroke. `canvas` is the highlighted canvas, or none when
    /// the stroke makes one. `look` is the stroke's size, cap, dash, color,
    /// and opacity when its canvas has no layer for this color yet. `ready`
    /// is false when the layer is hidden or locked.
    Brush {
        #[serde(default)]
        canvas: Option<u64>,
        source: u64,
        #[serde(default)]
        layer: Option<u64>,
        #[serde(default)]
        size: Option<u8>,
        dynamics: Value,
        look: Value,
        ready: bool,
    },
    /// An eraser path, on the highlighted canvas when there is one.
    Erase {
        #[serde(default)]
        canvas: Option<u64>,
        size: u8,
        dynamics: Value,
    },
    /// A rectangle, ellipse, or line drag, with the palette it is previewed in.
    Shape {
        shape: String,
        constrain: bool,
        snap: bool,
        palette: Value,
        corner_radius: f64,
    },
    /// Placing text. `line` is a line's height for the drag's preview;
    /// `em` is the next text's size.
    Text { line: f64, em: f64 },
}

/// Text being typed. New text carries its style, palette, and box; text
/// already in the document is drawn from the document. Each change sends
/// `edit` with the words as `value`; a press outside it, or Escape, sends
/// `commit`. A side handle's drag on new text sends `draft_box` with the box
/// as `value`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Caret {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub content: String,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<Value>,
    pub edit: Action,
    pub commit: Action,
    pub draft_box: Action,
}

/// The drawing surface's state for the selected tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Canvas {
    pub tab: String,
    pub tool: CanvasTool,
    /// The right-click menu on an object. Its rows are sent with the object as `node`.
    #[serde(default)]
    pub object_menu: Vec<MenuEntry>,
    /// Objects whose menu's Properties row can open a page.
    #[serde(default)]
    pub object_pages: Vec<u64>,
    /// The Properties row of `object_menu`, which waits for objects not in `object_pages`.
    #[serde(default)]
    pub properties_row: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caret: Option<Caret>,
    /// Double-clicking text with the select tool edits it: this action with the text as `node`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_text: Option<Action>,
    /// What a stylus's eraser end switches to while it is down, and back from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eraser_end: Option<EraserEnd>,
    /// Escape with no drag open lets go of the selection.
    #[serde(default)]
    pub escape_clears: bool,
    /// The text tool's side handles act on this text, or on new text.
    #[serde(default)]
    pub text_handles: bool,
    /// A text object's size as the host laid it out, sent with
    /// `{"node": id, "value": [width, height, natural]}` when it changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Action>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EraserEnd {
    pub enter: Action,
    pub leave: Action,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskButton {
    pub id: String,
    pub label: String,
}

/// A question in the reference's dialog look: a bold title, lines of text,
/// then the buttons in a row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ask {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub lines: Vec<String>,
    pub buttons: Vec<AskButton>,
    /// Pressing Escape or clicking outside answers with this button.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismiss: Option<String>,
}

/// A file type a file dialog offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFilter {
    pub name: String,
    pub extensions: Vec<String>,
}
