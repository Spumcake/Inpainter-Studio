//! Tool and command tables. Widgets draw labels, icons, and shortcuts from here.

use crate::defaults::{self, FONT_MAX, FONT_MIN, SIZE_MAX, SIZE_MIN};
use crate::schema::{self, PropId, SectionId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    Brush,
    Eraser,
    Select,
    Rectangle,
    Ellipse,
    Line,
    Text,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandId {
    NewSketch,
    Open,
    Save,
    SaveAs,
    SaveCopy,
    ClearRecent,
    Close,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Delete,
    NudgeLeft,
    NudgeRight,
    NudgeUp,
    NudgeDown,
    ZoomIn,
    ZoomOut,
    Fit,
    ZoomToSelection,
    Maximize,
    NewCanvas,
    Group,
    Ungroup,
    BringForward,
    SendBackward,
    BringToFront,
    SendToBack,
    AlignLeft,
    AlignCenter,
    AlignRight,
    RotateClockwise,
    RotateCounterclockwise,
    RotateHalf,
    FlipHorizontal,
    FlipVertical,
    TogglePanel,
    SizeDown,
    SizeUp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enable {
    Always,
    WhenUndo,
    WhenRedo,
    WhenClosable,
    /// The recent-file list has a path to show.
    WhenRecent,
    Waiting,
}

/// What the Size row edits while this tool is current.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeTarget {
    /// The highlighted canvas's active color. One undo step.
    Color,
    /// The eraser's own size. Not history.
    Eraser,
    /// The shape tool's stroke width. A tool setting, not history.
    ShapeStroke,
    /// The text tool's font size. A tool setting, not history.
    Font,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SizeSpec {
    pub target: SizeTarget,
    pub min: u8,
    pub max: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    B,
    C,
    E,
    G,
    L,
    N,
    O,
    R,
    S,
    T,
    V,
    W,
    X,
    Y,
    Z,
    Digit0,
    Digit2,
    Plus,
    Equals,
    Minus,
    BracketLeft,
    BracketRight,
    F4,
    Delete,
    Backspace,
    Left,
    Right,
    Up,
    Down,
}

impl Key {
    pub fn label(self) -> &'static str {
        match self {
            Key::B => "B",
            Key::C => "C",
            Key::E => "E",
            Key::G => "G",
            Key::L => "L",
            Key::N => "N",
            Key::O => "O",
            Key::R => "R",
            Key::S => "S",
            Key::T => "T",
            Key::V => "V",
            Key::W => "W",
            Key::X => "X",
            Key::Y => "Y",
            Key::Z => "Z",
            Key::Digit0 => "0",
            Key::Digit2 => "2",
            Key::Plus | Key::Equals => "+",
            Key::Minus => "-",
            Key::BracketLeft => "[",
            Key::BracketRight => "]",
            Key::F4 => "F4",
            Key::Delete => "Delete",
            Key::Backspace => "Backspace",
            Key::Left => "Left",
            Key::Right => "Right",
            Key::Up => "Up",
            Key::Down => "Down",
        }
    }
}

/// `shift: None` means the shortcut ignores Shift. Ctrl++ is Shift+= on a US keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub command: bool,
    pub shift: Option<bool>,
    pub alt: bool,
    pub key: Key,
}

impl Chord {
    const fn plain(key: Key) -> Self {
        Self {
            command: false,
            shift: Some(false),
            alt: false,
            key,
        }
    }

    const fn ctrl(key: Key) -> Self {
        Self {
            command: true,
            shift: Some(false),
            alt: false,
            key,
        }
    }

    const fn ctrl_shift(key: Key) -> Self {
        Self {
            command: true,
            shift: Some(true),
            alt: false,
            key,
        }
    }

    /// Command plus this key, whether or not Shift is held.
    const fn ctrl_any_shift(key: Key) -> Self {
        Self {
            command: true,
            shift: None,
            alt: false,
            key,
        }
    }

    const fn shift(key: Key) -> Self {
        Self {
            command: false,
            shift: Some(true),
            alt: false,
            key,
        }
    }

    pub fn label(self) -> String {
        let mut parts = Vec::new();
        if self.command {
            parts.push("Ctrl");
        }
        if self.shift == Some(true) {
            parts.push("Shift");
        }
        parts.push(self.key.label());
        parts.join("+")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Tool(Tool),
    Command(CommandId),
}

pub struct ToolDef {
    pub id: Tool,
    pub label: &'static str,
    pub icon: &'static str,
    pub shortcuts: &'static [Chord],
    pub size: Option<SizeSpec>,
    /// The controls after the tools while this tool is current, which set up
    /// its next mark. Select has its context actions instead.
    pub bar: &'static [BarControl],
    /// Panel sections, in order. Empty until that tool's page is in this version.
    pub sections: &'static [SectionId],
}

/// One control in the toolbar's tool settings, after the tools. Each reads
/// and writes a setting of the current tool, never an object.
pub enum BarControl {
    /// The Size slider, bound by the tool's `SizeSpec`.
    Size,
    /// A thin divider between groups of controls.
    Divider,
    /// A labelled slider for a whole-number setting.
    Slider {
        prop: PropId,
        label: &'static str,
        min: i32,
        max: i32,
    },
    /// One button per option of an index setting. The chosen one is blue.
    Choice {
        prop: PropId,
        options: &'static [(&'static str, &'static str)],
    },
    /// The list button: a click turns the list on or off, and a right-click
    /// chooses its kind. Its icon is the kind's.
    List,
}

const SIZE_BAR: &[BarControl] = &[BarControl::Size];

/// The text tool: size, alignment across and down, the box's padding and
/// corner radius, and the list.
const TEXT_BAR: &[BarControl] = &[
    BarControl::Size,
    BarControl::Divider,
    BarControl::Choice {
        prop: PropId::Align,
        options: &[
            ("align-left", "Align left"),
            ("align-center", "Align center"),
            ("align-right", "Align right"),
            ("align-justify", "Justify"),
        ],
    },
    BarControl::Choice {
        prop: PropId::VAlign,
        options: &[
            ("align-top-box", "Align top"),
            ("align-horizontal-centers", "Align middle"),
            ("align-bottom-box", "Align bottom"),
        ],
    },
    BarControl::Divider,
    BarControl::Slider {
        prop: PropId::TextPadding,
        label: "Padding",
        min: 0,
        max: 48,
    },
    BarControl::Slider {
        prop: PropId::TextRadius,
        label: "Radius",
        min: 0,
        max: 48,
    },
    BarControl::Divider,
    BarControl::List,
];

/// The list kinds, in the list button's menu: their icon and label. The first
/// is the default.
pub const LIST_KINDS: &[(&str, &str)] = &[
    ("list", "Bulleted list"),
    ("numbered-list-left", "Numbered list"),
    ("list-select", "Checklist"),
];

pub struct CommandDef {
    pub id: CommandId,
    pub label: &'static str,
    pub icon: &'static str,
    pub shortcuts: &'static [Chord],
    pub enable: Enable,
    /// Icon shown when a toggle command is in its other state.
    pub toggle_icon: Option<&'static str>,
}

const BRUSH_KEYS: &[Chord] = &[Chord::plain(Key::B)];
const ERASER_KEYS: &[Chord] = &[Chord::plain(Key::E)];
const SELECT_KEYS: &[Chord] = &[Chord::plain(Key::V)];
const RECTANGLE_KEYS: &[Chord] = &[Chord::plain(Key::R)];
const ELLIPSE_KEYS: &[Chord] = &[Chord::plain(Key::O)];
const LINE_KEYS: &[Chord] = &[Chord::plain(Key::L)];
const TEXT_KEYS: &[Chord] = &[Chord::plain(Key::T)];

const COLOR_SIZE: SizeSpec = SizeSpec {
    target: SizeTarget::Color,
    min: SIZE_MIN,
    max: SIZE_MAX,
};
const ERASER_SIZE: SizeSpec = SizeSpec {
    target: SizeTarget::Eraser,
    min: SIZE_MIN,
    max: SIZE_MAX,
};
const SHAPE_SIZE: SizeSpec = SizeSpec {
    target: SizeTarget::ShapeStroke,
    min: SIZE_MIN,
    max: SIZE_MAX,
};
const FONT_SIZE: SizeSpec = SizeSpec {
    target: SizeTarget::Font,
    min: FONT_MIN,
    max: FONT_MAX,
};

pub fn tools() -> &'static [ToolDef] {
    &[
        ToolDef {
            id: Tool::Brush,
            label: "Brush",
            icon: "brush",
            shortcuts: BRUSH_KEYS,
            size: Some(COLOR_SIZE),
            bar: SIZE_BAR,
            sections: schema::BRUSH_SECTIONS,
        },
        ToolDef {
            id: Tool::Eraser,
            label: "Eraser",
            icon: "erase",
            shortcuts: ERASER_KEYS,
            size: Some(ERASER_SIZE),
            bar: SIZE_BAR,
            sections: schema::ERASER_SECTIONS,
        },
        ToolDef {
            id: Tool::Select,
            label: "Select",
            icon: "cursor-pointer",
            shortcuts: SELECT_KEYS,
            size: None,
            bar: &[],
            sections: schema::NO_SECTIONS,
        },
        ToolDef {
            id: Tool::Rectangle,
            label: "Rectangle",
            icon: "square",
            shortcuts: RECTANGLE_KEYS,
            size: Some(SHAPE_SIZE),
            bar: SIZE_BAR,
            sections: schema::RECTANGLE_SECTIONS,
        },
        ToolDef {
            id: Tool::Ellipse,
            label: "Ellipse",
            icon: "circle",
            shortcuts: ELLIPSE_KEYS,
            size: Some(SHAPE_SIZE),
            bar: SIZE_BAR,
            sections: schema::ELLIPSE_SECTIONS,
        },
        ToolDef {
            id: Tool::Line,
            label: "Line",
            icon: "linear",
            shortcuts: LINE_KEYS,
            size: Some(SHAPE_SIZE),
            bar: SIZE_BAR,
            sections: schema::LINE_SECTIONS,
        },
        ToolDef {
            id: Tool::Text,
            label: "Text",
            icon: "text",
            shortcuts: TEXT_KEYS,
            size: Some(FONT_SIZE),
            bar: TEXT_BAR,
            sections: schema::TEXT_SECTIONS,
        },
    ]
}

pub fn tool(id: Tool) -> &'static ToolDef {
    tools()
        .iter()
        .find(|tool| tool.id == id)
        .expect("every tool is registered")
}

const NEW_SKETCH: &[Chord] = &[Chord::ctrl(Key::N)];
const OPEN: &[Chord] = &[Chord::ctrl(Key::O)];
const SAVE: &[Chord] = &[Chord::ctrl(Key::S)];
const SAVE_AS: &[Chord] = &[Chord::ctrl_shift(Key::S)];
const CLOSE: &[Chord] = &[Chord::ctrl(Key::W)];
const UNDO: &[Chord] = &[Chord::ctrl(Key::Z)];
const REDO: &[Chord] = &[Chord::ctrl_shift(Key::Z), Chord::ctrl(Key::Y)];
const CUT: &[Chord] = &[Chord::ctrl(Key::X)];
const COPY: &[Chord] = &[Chord::ctrl(Key::C)];
const PASTE: &[Chord] = &[Chord::ctrl(Key::V)];
const DELETE: &[Chord] = &[Chord::plain(Key::Delete), Chord::plain(Key::Backspace)];
const NUDGE_LEFT: &[Chord] = &[Chord::plain(Key::Left), Chord::shift(Key::Left)];
const NUDGE_RIGHT: &[Chord] = &[Chord::plain(Key::Right), Chord::shift(Key::Right)];
const NUDGE_UP: &[Chord] = &[Chord::plain(Key::Up), Chord::shift(Key::Up)];
const NUDGE_DOWN: &[Chord] = &[Chord::plain(Key::Down), Chord::shift(Key::Down)];
const ZOOM_IN: &[Chord] = &[
    Chord::ctrl_any_shift(Key::Plus),
    Chord::ctrl_any_shift(Key::Equals),
];
const ZOOM_OUT: &[Chord] = &[Chord::ctrl_any_shift(Key::Minus)];
const FIT: &[Chord] = &[Chord::ctrl(Key::Digit0)];
const ZOOM_SELECTION: &[Chord] = &[Chord::shift(Key::Digit2)];
const MAXIMIZE: &[Chord] = &[];
const NEW_CANVAS: &[Chord] = &[Chord::ctrl_shift(Key::N)];
const GROUP: &[Chord] = &[Chord::ctrl(Key::G)];
const UNGROUP: &[Chord] = &[Chord::ctrl_shift(Key::G)];
const FORWARD: &[Chord] = &[Chord::ctrl(Key::BracketRight)];
const BACKWARD: &[Chord] = &[Chord::ctrl(Key::BracketLeft)];
const TO_FRONT: &[Chord] = &[Chord::ctrl_shift(Key::BracketRight)];
const TO_BACK: &[Chord] = &[Chord::ctrl_shift(Key::BracketLeft)];
const PANEL: &[Chord] = &[Chord::plain(Key::F4)];
const SIZE_DOWN: &[Chord] = &[Chord::plain(Key::BracketLeft)];
const SIZE_UP: &[Chord] = &[Chord::plain(Key::BracketRight)];
const NONE: &[Chord] = &[];

pub fn commands() -> &'static [CommandDef] {
    &[
        CommandDef {
            id: CommandId::NewSketch,
            label: "New sketch",
            icon: "page",
            shortcuts: NEW_SKETCH,
            enable: Enable::Always,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::Open,
            label: "Open",
            icon: "folder",
            shortcuts: OPEN,
            enable: Enable::Always,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::Save,
            label: "Save",
            icon: "floppy-disk",
            shortcuts: SAVE,
            enable: Enable::Always,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::SaveAs,
            label: "Save As",
            icon: "floppy-disk-arrow-in",
            shortcuts: SAVE_AS,
            enable: Enable::Always,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::SaveCopy,
            label: "Save a Copy",
            icon: "copy",
            shortcuts: NONE,
            enable: Enable::Always,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::ClearRecent,
            label: "Clear Recent",
            icon: "trash",
            shortcuts: NONE,
            enable: Enable::WhenRecent,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::Close,
            label: "Close",
            icon: "xmark",
            shortcuts: CLOSE,
            enable: Enable::WhenClosable,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::Undo,
            label: "Undo",
            icon: "undo",
            shortcuts: UNDO,
            enable: Enable::WhenUndo,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::Redo,
            label: "Redo",
            icon: "redo",
            shortcuts: REDO,
            enable: Enable::WhenRedo,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::Cut,
            label: "Cut",
            icon: "cut",
            shortcuts: CUT,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::Copy,
            label: "Copy",
            icon: "copy",
            shortcuts: COPY,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::Paste,
            label: "Paste",
            icon: "paste-clipboard",
            shortcuts: PASTE,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::Delete,
            label: "Delete",
            icon: "trash",
            shortcuts: DELETE,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::NudgeLeft,
            label: "Nudge left",
            icon: "arrow-left",
            shortcuts: NUDGE_LEFT,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::NudgeRight,
            label: "Nudge right",
            icon: "arrow-right",
            shortcuts: NUDGE_RIGHT,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::NudgeUp,
            label: "Nudge up",
            icon: "arrow-up",
            shortcuts: NUDGE_UP,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::NudgeDown,
            label: "Nudge down",
            icon: "arrow-down",
            shortcuts: NUDGE_DOWN,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::ZoomIn,
            label: "Zoom in",
            icon: "zoom-in",
            shortcuts: ZOOM_IN,
            enable: Enable::Always,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::ZoomOut,
            label: "Zoom out",
            icon: "zoom-out",
            shortcuts: ZOOM_OUT,
            enable: Enable::Always,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::Fit,
            label: "Fit",
            icon: "frame",
            shortcuts: FIT,
            enable: Enable::Always,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::ZoomToSelection,
            label: "Zoom to selection",
            icon: "frame-select",
            shortcuts: ZOOM_SELECTION,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::Maximize,
            label: "Maximize",
            icon: "maximize",
            shortcuts: MAXIMIZE,
            enable: Enable::Always,
            toggle_icon: Some("reduce"),
        },
        CommandDef {
            id: CommandId::NewCanvas,
            label: "New canvas",
            icon: "plus-circle",
            shortcuts: NEW_CANVAS,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::Group,
            label: "Group",
            icon: "exclude",
            shortcuts: GROUP,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::Ungroup,
            label: "Ungroup",
            icon: "combine",
            shortcuts: UNGROUP,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::BringForward,
            label: "Bring forward",
            icon: "transition-up",
            shortcuts: FORWARD,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::SendBackward,
            label: "Send backward",
            icon: "transition-down",
            shortcuts: BACKWARD,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::BringToFront,
            label: "Bring to front",
            icon: "sort-up",
            shortcuts: TO_FRONT,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::SendToBack,
            label: "Send to back",
            icon: "sort-down",
            shortcuts: TO_BACK,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::AlignLeft,
            label: "Align left",
            icon: "comp-align-left",
            shortcuts: NONE,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::AlignCenter,
            label: "Align center",
            icon: "align-horizontal-centers",
            shortcuts: NONE,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::AlignRight,
            label: "Align right",
            icon: "comp-align-right",
            shortcuts: NONE,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::RotateClockwise,
            label: "Rotate clockwise",
            icon: "redo-circle",
            shortcuts: NONE,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::RotateCounterclockwise,
            label: "Rotate counterclockwise",
            icon: "undo-circle",
            shortcuts: NONE,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::RotateHalf,
            label: "Rotate 180",
            icon: "refresh-circle",
            shortcuts: NONE,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::FlipHorizontal,
            label: "Flip horizontal",
            icon: "flip",
            shortcuts: NONE,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::FlipVertical,
            label: "Flip vertical",
            icon: "flip-vertical",
            shortcuts: NONE,
            enable: Enable::Waiting,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::TogglePanel,
            label: "Properties",
            icon: "sidebar-collapse",
            shortcuts: PANEL,
            enable: Enable::Always,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::SizeDown,
            label: "Smaller",
            icon: "minus",
            shortcuts: SIZE_DOWN,
            enable: Enable::Always,
            toggle_icon: None,
        },
        CommandDef {
            id: CommandId::SizeUp,
            label: "Larger",
            icon: "plus",
            shortcuts: SIZE_UP,
            enable: Enable::Always,
            toggle_icon: None,
        },
    ]
}

pub fn command(id: CommandId) -> &'static CommandDef {
    commands()
        .iter()
        .find(|command| command.id == id)
        .expect("every command is registered")
}

pub fn hover(label: &str, shortcuts: &[Chord]) -> String {
    match shortcuts.first() {
        Some(chord) => format!("{label} ({})", chord.label()),
        None => label.to_string(),
    }
}

/// File commands drawn after the swatches.
pub fn file_commands() -> &'static [CommandId] {
    &[CommandId::NewSketch, CommandId::Open, CommandId::Save]
}

/// Before the tools, to the brush's left: the brush draws on a canvas, and
/// this makes one.
/// Icons the Iconoir pack does not have, drawn by the toolbar in Iconoir's
/// style instead of taken from the font.
pub const DRAWN_ICONS: &[&str] = &["brush"];

pub fn canvas_commands() -> &'static [CommandId] {
    &[CommandId::NewCanvas]
}

/// What a right-click menu can do: on a tool, a swatch, or an outliner row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Rename,
    MakePaletteDefault,
    Duplicate,
    Delete,
    /// Opens the panel on that tool's or color's page.
    Properties,
}

pub struct MenuActionDef {
    pub action: MenuAction,
    pub label: &'static str,
}

const RENAME_ACTION: MenuActionDef = MenuActionDef {
    action: MenuAction::Rename,
    label: "Rename",
};
const DUPLICATE_ACTION: MenuActionDef = MenuActionDef {
    action: MenuAction::Duplicate,
    label: "Duplicate",
};
const DELETE_ACTION: MenuActionDef = MenuActionDef {
    action: MenuAction::Delete,
    label: "Delete",
};
const PROPERTIES_ACTION: MenuActionDef = MenuActionDef {
    action: MenuAction::Properties,
    label: "Properties",
};

/// The menu on a tool button.
pub fn tool_menu() -> &'static [MenuActionDef] {
    &[PROPERTIES_ACTION]
}

/// The menu on a brush swatch, or on any color in the brush's or a canvas's
/// palette that can be added to and removed.
pub fn color_menu() -> &'static [MenuActionDef] {
    &[DUPLICATE_ACTION, DELETE_ACTION, PROPERTIES_ACTION]
}

/// The menu on a shape's or text's swatch: those colors are fixed roles.
pub fn role_menu() -> &'static [MenuActionDef] {
    &[PROPERTIES_ACTION]
}

/// The menu on a canvas's row in the outliner.
pub fn canvas_row_menu() -> &'static [MenuActionDef] {
    &[
        MenuActionDef {
            action: MenuAction::Rename,
            label: "Rename canvas",
        },
        MenuActionDef {
            action: MenuAction::MakePaletteDefault,
            label: "Make palette default",
        },
        PROPERTIES_ACTION,
    ]
}

/// The menu on a group's row in the outliner.
pub fn group_row_menu() -> &'static [MenuActionDef] {
    &[
        MenuActionDef {
            action: MenuAction::Rename,
            label: "Rename group",
        },
        PROPERTIES_ACTION,
    ]
}

/// The menu on an object with the select tool, and on a shape's or text's
/// row in the outliner.
pub fn object_menu() -> &'static [MenuActionDef] {
    &[PROPERTIES_ACTION]
}

/// The menu on a canvas layer's row in the outliner.
pub fn color_row_menu() -> &'static [MenuActionDef] {
    &[
        RENAME_ACTION,
        DUPLICATE_ACTION,
        DELETE_ACTION,
        PROPERTIES_ACTION,
    ]
}

pub fn history_commands() -> &'static [CommandId] {
    &[CommandId::Undo, CommandId::Redo]
}

/// Zoom out, zoom in, fit, then the maximize toggle, left to right.
pub fn view_commands() -> &'static [CommandId] {
    &[
        CommandId::ZoomOut,
        CommandId::ZoomIn,
        CommandId::Fit,
        CommandId::Maximize,
    ]
}

/// A row of the sketch menu: a command, or the submenu of recent files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SketchMenuRow {
    Command(CommandId),
    Recent,
}

/// The label of the sketch menu's submenu of recent files.
pub const RECENT_MENU_LABEL: &str = "Open Recent";

/// The sketch menu, top to bottom.
pub fn sketch_menu() -> &'static [SketchMenuRow] {
    &[
        SketchMenuRow::Command(CommandId::NewSketch),
        SketchMenuRow::Command(CommandId::Open),
        SketchMenuRow::Recent,
        SketchMenuRow::Command(CommandId::Save),
        SketchMenuRow::Command(CommandId::SaveAs),
        SketchMenuRow::Command(CommandId::SaveCopy),
        SketchMenuRow::Command(CommandId::Close),
    ]
}

pub struct Binding {
    pub chord: Chord,
    pub action: Action,
}

pub fn bindings() -> Vec<Binding> {
    let mut bindings = Vec::new();
    for tool in tools() {
        for chord in tool.shortcuts {
            bindings.push(Binding {
                chord: *chord,
                action: Action::Tool(tool.id),
            });
        }
    }
    for command in commands() {
        for chord in command.shortcuts {
            bindings.push(Binding {
                chord: *chord,
                action: Action::Command(command.id),
            });
        }
    }
    bindings
}

/// The word beside the size slider.
pub const SIZE_LABEL: &str = "Size";

/// One control among the select tool's context actions.
pub enum SelectItem {
    /// A command run by a plain button.
    Button(CommandId),
    /// A button with a chevron that opens a menu of commands.
    Menu(ActionMenu),
}

pub struct ActionMenu {
    pub label: &'static str,
    pub icon: &'static str,
    pub commands: &'static [CommandId],
}

/// Context actions that replace the Size control while Select is current, as
/// one run of buttons with no dividers: bring to front and send to back,
/// align, group, then rotate and flip, which open menus. Bring forward, send
/// backward, and zoom to selection are shortcuts only.
pub fn select_actions() -> &'static [SelectItem] {
    &[
        SelectItem::Button(CommandId::BringToFront),
        SelectItem::Button(CommandId::SendToBack),
        SelectItem::Button(CommandId::AlignLeft),
        SelectItem::Button(CommandId::AlignCenter),
        SelectItem::Button(CommandId::AlignRight),
        SelectItem::Button(CommandId::Group),
        SelectItem::Button(CommandId::Ungroup),
        SelectItem::Menu(ActionMenu {
            label: "Rotate",
            icon: "refresh",
            commands: &[
                CommandId::RotateClockwise,
                CommandId::RotateCounterclockwise,
                CommandId::RotateHalf,
            ],
        }),
        SelectItem::Menu(ActionMenu {
            label: "Flip",
            icon: "flip",
            commands: &[CommandId::FlipHorizontal, CommandId::FlipVertical],
        }),
    ]
}

/// How an icon name is drawn from the Iconoir font. A name ending in
/// `-solid` is the icon's filled style, as Iconoir names it on its site. A
/// name in [`TURNED_ICONS`] is another icon turned by quarter turns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IconSpec<'a> {
    pub glyph: &'a str,
    pub filled: bool,
    pub quarter_turns: u8,
}

/// Icons Iconoir has only in one direction: the name, the icon it turns, and
/// the clockwise quarter turns.
const TURNED_ICONS: &[(&str, &str, u8)] = &[("flip-vertical", "flip", 1)];

pub fn icon_spec(name: &str) -> IconSpec<'_> {
    if let Some((_, glyph, turns)) = TURNED_ICONS.iter().find(|(alias, _, _)| *alias == name) {
        return IconSpec {
            glyph,
            filled: false,
            quarter_turns: *turns,
        };
    }
    match name.strip_suffix("-solid") {
        Some(glyph) => IconSpec {
            glyph,
            filled: true,
            quarter_turns: 0,
        },
        None => IconSpec {
            glyph: name,
            filled: false,
            quarter_turns: 0,
        },
    }
}

pub fn node_icon(group: bool) -> &'static str {
    if group { "folder" } else { "page" }
}

pub const EYE_OPEN: &str = "eye";
pub const EYE_CLOSED: &str = "eye-closed";
pub const LOCK_CLOSED: &str = "lock";
pub const LOCK_OPEN: &str = "lock-slash";

#[cfg_attr(not(test), allow(dead_code))]
pub fn row_icons() -> &'static [&'static str] {
    &[
        "folder",
        "page",
        EYE_OPEN,
        EYE_CLOSED,
        LOCK_CLOSED,
        LOCK_OPEN,
    ]
}

/// Default sizes, re-exported for the session without a second table.
pub fn default_eraser_size() -> u8 {
    defaults::ERASER_SIZE
}

pub fn default_font_size() -> u8 {
    defaults::FONT_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn shortcuts_are_unique_and_icons_resolve() {
        let mut seen = HashSet::new();
        for binding in bindings() {
            assert!(
                seen.insert(binding.chord),
                "shortcut {} is used twice",
                binding.chord.label()
            );
        }
        assert!(seen.len() > 1);

        for tool in tools() {
            assert!(icon_resolves(tool.icon), "tool icon {}", tool.icon);
            for control in tool.bar {
                match control {
                    BarControl::Choice { options, .. } => {
                        for (icon, _) in *options {
                            assert!(icon_resolves(icon), "choice icon {icon}");
                        }
                    }
                    BarControl::Size
                    | BarControl::Divider
                    | BarControl::Slider { .. }
                    | BarControl::List => {}
                }
            }
        }
        for (icon, _) in LIST_KINDS {
            assert!(icon_resolves(icon), "list icon {icon}");
        }
        for command in commands() {
            assert!(icon_resolves(command.icon), "command icon {}", command.icon);
            if let Some(icon) = command.toggle_icon {
                assert!(icon_resolves(icon), "toggle icon {icon}");
            }
        }
        for icon in row_icons() {
            assert!(icon_resolves(icon), "row icon {icon}");
        }
        for item in select_actions() {
            if let SelectItem::Menu(menu) = item {
                assert!(icon_resolves(menu.icon), "menu icon {}", menu.icon);
            }
        }
    }

    fn icon_resolves(name: &str) -> bool {
        let spec = icon_spec(name);
        DRAWN_ICONS.contains(&name)
            || iconflow::try_icon(
                iconflow::Pack::Iconoir,
                spec.glyph,
                if spec.filled {
                    iconflow::Style::Filled
                } else {
                    iconflow::Style::Regular
                },
                iconflow::Size::Regular,
            )
            .is_ok()
    }

    #[test]
    fn every_select_command_is_reachable_from_the_toolbar_or_a_shortcut() {
        let shown: Vec<CommandId> = select_actions()
            .iter()
            .flat_map(|item| match item {
                SelectItem::Button(id) => std::slice::from_ref(id),
                SelectItem::Menu(menu) => menu.commands,
            })
            .copied()
            .collect();
        assert!(!shown.contains(&CommandId::ZoomToSelection));
        assert!(!command(CommandId::ZoomToSelection).shortcuts.is_empty());
        assert!(!command(CommandId::BringForward).shortcuts.is_empty());
        assert!(!command(CommandId::SendBackward).shortcuts.is_empty());
        for id in [
            CommandId::BringToFront,
            CommandId::SendToBack,
            CommandId::Group,
            CommandId::Ungroup,
            CommandId::FlipVertical,
        ] {
            assert!(shown.contains(&id), "{id:?} is not in the select actions");
        }
    }
}
