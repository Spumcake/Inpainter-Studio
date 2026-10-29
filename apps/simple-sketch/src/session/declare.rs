//! What the window of Simple Sketch 1.0.1 drew from the session, written out
//! as the protocol's declarations: the toolbar of `toolbar.rs`, the sketch
//! menu and tabs of `tabs.rs`, the panel of `panel.rs`, the outliner of
//! `outliner.rs`, and the canvas's tool of `canvas.rs`. Each reads the
//! session as those files did. Each control carries the action the host
//! sends back, which `serve.rs` runs on the session.

use super::{Session, TextDraft, shape_kind};
use crate::registry::{self, CommandId, MenuAction, MenuActionDef, SelectItem, Tool};
use crate::schema::{self, ChoicePreview, PropKind};
use draw::document::snapshot::{
    box_value, dynamics_to_value, palette_value, stroke_look_value, style_to_value,
};
use draw::document::{ColorId, LayerColor, LayerKind, NodeId, PaletteColor};
use draw::{PropId, PropValue};
use protocol::{
    Action, Back, Binding, Canvas, CanvasTool, Caret, Chord, ColorRow, EraserEnd, Keys, LayerRow,
    Layers, MenuEntry, Properties, Row, RowKind, Section, Swatch, TabEntry, Tabs, ToolItem,
    Toolbar,
};
use serde_json::{Value, json};
use std::path::PathBuf;

/// A toolbar slider's width beside the Size control's, from `toolbar.rs`.
const SIZE_W: f32 = 186.0;
const BAR_SLIDER_W: f32 = 150.0;

/// A command's id in a message: its name in snake case.
pub(crate) fn command_key(id: CommandId) -> String {
    snake(&format!("{id:?}"))
}

/// The commands that save by hand. In a project every change is written by
/// itself, so they are not offered.
fn saves(id: CommandId) -> bool {
    matches!(id, CommandId::Save | CommandId::SaveAs | CommandId::SaveCopy)
}

pub(crate) fn tool_key(tool: Tool) -> String {
    snake(&format!("{tool:?}"))
}

pub(crate) fn snake(name: &str) -> String {
    let mut out = String::new();
    for (index, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

pub(crate) fn command_named(id: &str) -> Option<CommandId> {
    registry::commands()
        .iter()
        .map(|definition| definition.id)
        .find(|command| command_key(*command) == id)
}

pub(crate) fn tool_named(name: &str) -> Option<Tool> {
    registry::tools()
        .iter()
        .map(|definition| definition.id)
        .find(|tool| tool_key(*tool) == name)
}

pub(crate) fn action_key(action: MenuAction) -> &'static str {
    match action {
        MenuAction::Rename => "rename",
        MenuAction::MakePaletteDefault => "make_palette_default",
        MenuAction::Duplicate => "duplicate",
        MenuAction::Delete => "delete",
        MenuAction::Properties => "properties",
    }
}

pub(crate) fn action_named(name: &str) -> Option<MenuAction> {
    Some(match name {
        "rename" => MenuAction::Rename,
        "make_palette_default" => MenuAction::MakePaletteDefault,
        "duplicate" => MenuAction::Duplicate,
        "delete" => MenuAction::Delete,
        "properties" => MenuAction::Properties,
        _ => return None,
    })
}

/// Zoom, fit, and zoom to selection are the drawing surface's own commands,
/// which the host carries out on its camera.
fn surface_command(id: CommandId) -> Option<&'static str> {
    match id {
        CommandId::ZoomIn => Some("canvas.zoom_in"),
        CommandId::ZoomOut => Some("canvas.zoom_out"),
        CommandId::Fit => Some("canvas.fit"),
        CommandId::ZoomToSelection => Some("canvas.zoom_to_selection"),
        _ => None,
    }
}

fn command_action(id: CommandId) -> Action {
    match surface_command(id) {
        Some(surface) => Action::new(surface),
        None => Action::new(command_key(id)),
    }
}

fn chord(chord: &registry::Chord) -> Chord {
    Chord {
        key: format!("{:?}", chord.key),
        command: chord.command,
        shift: chord.shift,
        alt: chord.alt,
    }
}

fn menu_rows(
    defs: &[MenuActionDef],
    enabled: impl Fn(MenuAction) -> bool,
    action: impl Fn(MenuAction) -> Action,
) -> Vec<MenuEntry> {
    defs.iter()
        .map(|def| MenuEntry {
            label: def.label.to_string(),
            icon: None,
            enabled: enabled(def.action),
            divider: false,
            action: (def.action != MenuAction::Rename).then(|| action(def.action)),
            submenu: Vec::new(),
            rename: def.action == MenuAction::Rename,
        })
        .collect()
}

fn command_row(session: &Session, id: CommandId, icon: bool) -> MenuEntry {
    let definition = registry::command(id);
    MenuEntry {
        label: definition.label.to_string(),
        icon: icon.then(|| definition.icon.to_string()),
        enabled: session.command_enabled(id),
        divider: false,
        action: Some(command_action(id)),
        submenu: Vec::new(),
        rename: false,
    }
}

impl Session {
    /// Every shortcut of `registry::bindings`, and the window's clipboard events.
    pub(crate) fn declare_keys(&self) -> Keys {
        let bindings = registry::bindings()
            .into_iter()
            .filter_map(|binding| {
                let action = match binding.action {
                    registry::Action::Tool(tool) => {
                        Action::with_arg("select_tool", json!(tool_key(tool)))
                    }
                    registry::Action::Command(CommandId::Maximize) => {
                        Action::new("window.maximize")
                    }
                    registry::Action::Command(id) => command_action(id),
                };
                Some(Binding {
                    chord: chord(&binding.chord),
                    then: Vec::new(),
                    action,
                })
            })
            .collect();
        Keys {
            bindings,
            copy: Some(command_action(CommandId::Copy)),
            cut: Some(command_action(CommandId::Cut)),
            paste: Some(command_action(CommandId::Paste)),
        }
    }

    /// The sketch menu of `tabs::menu`, with Open Recent's rows.
    pub(crate) fn declare_menu(&mut self) -> Vec<MenuEntry> {
        let recent = self.recent_menu();
        let clear_enabled = self.command_enabled(CommandId::ClearRecent);
        let in_project = self.project().is_some();
        registry::sketch_menu()
            .iter()
            .filter(|row| !(in_project && matches!(row, registry::SketchMenuRow::Command(id) if saves(*id))))
            .map(|row| match row {
                registry::SketchMenuRow::Command(id) => command_row(self, *id, false),
                registry::SketchMenuRow::Recent => MenuEntry {
                    label: registry::RECENT_MENU_LABEL.to_string(),
                    icon: None,
                    enabled: !recent.is_empty(),
                    divider: false,
                    action: None,
                    submenu: recent_rows(&recent, clear_enabled),
                    rename: false,
                },
            })
            .collect()
    }

    pub(crate) fn declare_tabs(&self) -> Tabs {
        let new_sketch = registry::command(CommandId::NewSketch);
        Tabs {
            tabs: self
                .tabs()
                .into_iter()
                .map(|tab| TabEntry {
                    id: tab.id.get().to_string(),
                    label: tab.name.to_string(),
                    dirty: tab.needs_keep,
                    close_waits: tab.close_waits,
                    path: self
                        .file_path(tab.id)
                        .map(|path| path.display().to_string()),
                    saves: true,
                })
                .collect(),
            active: self.active_id().get().to_string(),
            new_tab: Some(command_action(CommandId::NewSketch)),
            new_tab_tip: registry::hover(new_sketch.label, new_sketch.shortcuts),
        }
    }

    /// The toolbar of `toolbar::show`, in its order.
    pub(crate) fn declare_toolbar(&self) -> Toolbar {
        let mut items = Vec::new();
        let swatches = self.swatches();
        if !swatches.is_empty() {
            items.push(ToolItem::Inset);
            let sortable = self.tool() == Tool::Brush;
            items.push(ToolItem::Swatches {
                swatches: swatches
                    .iter()
                    .map(|swatch| {
                        let menu = self.swatch_menu(swatch.id);
                        Swatch {
                            id: swatch.id.get(),
                            rgb: swatch.rgb,
                            active: swatch.active,
                            muted: swatch.muted,
                            name: swatch.name.clone(),
                            click: Action::with_arg(
                                "swatch_click",
                                json!({ "color": swatch.id.get() }),
                            ),
                            menu: menu_rows(
                                menu,
                                |action| self.swatch_action_enabled(action),
                                |action| {
                                    Action::with_arg(
                                        "swatch_action",
                                        json!({ "color": swatch.id.get(), "action": action_key(action) }),
                                    )
                                },
                            ),
                        }
                    })
                    .collect(),
                sort: sortable.then(|| Action::new("sort_brush")),
            });
            items.push(ToolItem::Separator);
        }
        for id in registry::file_commands() {
            if self.project().is_some() && saves(*id) {
                continue;
            }
            items.push(self.command_button(*id));
        }
        items.push(ToolItem::Separator);
        for id in registry::history_commands() {
            items.push(self.command_button(*id));
        }
        items.push(ToolItem::Separator);
        for id in registry::canvas_commands() {
            items.push(self.command_button(*id));
        }
        let current = self.tool();
        for tool in registry::tools() {
            items.push(ToolItem::Button {
                icon: tool.icon.to_string(),
                tip: registry::hover(tool.label, tool.shortcuts),
                enabled: true,
                selected: tool.id == current,
                action: Some(Action::with_arg("select_tool", json!(tool_key(tool.id)))),
                menu: menu_rows(
                    registry::tool_menu(),
                    |_| true,
                    |action| {
                        Action::with_arg(
                            "tool_action",
                            json!({ "tool": tool_key(tool.id), "action": action_key(action) }),
                        )
                    },
                ),
            });
        }
        if current == Tool::Select {
            items.push(ToolItem::Separator);
            for item in registry::select_actions() {
                match item {
                    SelectItem::Button(id) if self.command_shown(*id) => {
                        items.push(self.command_button(*id));
                    }
                    SelectItem::Menu(menu)
                        if menu.commands.iter().any(|id| self.command_shown(*id)) =>
                    {
                        items.push(ToolItem::Menu {
                            icon: menu.icon.to_string(),
                            label: menu.label.to_string(),
                            enabled: menu.commands.iter().any(|id| self.command_enabled(*id)),
                            rows: menu
                                .commands
                                .iter()
                                .map(|id| command_row(self, *id, true))
                                .collect(),
                        });
                    }
                    SelectItem::Button(_) | SelectItem::Menu(_) => {}
                }
            }
        } else {
            let bar = registry::tool(current).bar;
            if !bar.is_empty() {
                items.push(ToolItem::Separator);
                self.tool_bar(bar, &mut items);
            }
        }
        let mut trailing = vec![ToolItem::Separator];
        for id in registry::view_commands() {
            if *id == CommandId::Maximize {
                let definition = registry::command(*id);
                trailing.push(ToolItem::Maximize {
                    icon: definition.icon.to_string(),
                    restore_icon: definition
                        .toggle_icon
                        .unwrap_or(definition.icon)
                        .to_string(),
                    tip: registry::hover(definition.label, definition.shortcuts),
                });
            } else {
                trailing.push(self.command_button(*id));
            }
        }
        Toolbar { items, trailing }
    }

    fn command_button(&self, id: CommandId) -> ToolItem {
        let definition = registry::command(id);
        ToolItem::Button {
            icon: definition.icon.to_string(),
            tip: registry::hover(definition.label, definition.shortcuts),
            enabled: self.command_enabled(id),
            selected: false,
            action: Some(command_action(id)),
            menu: Vec::new(),
        }
    }

    /// The current tool's settings, as `toolbar::tool_bar` drew them.
    fn tool_bar(&self, bar: &[registry::BarControl], items: &mut Vec<ToolItem>) {
        for control in bar {
            match control {
                registry::BarControl::Size => {
                    let Some(control) = self.size_control() else {
                        continue;
                    };
                    items.push(ToolItem::Slider {
                        caption: registry::SIZE_LABEL.to_string(),
                        width: SIZE_W,
                        value: i32::from(control.value),
                        min: i32::from(control.min),
                        max: i32::from(control.max),
                        tint: control.tint,
                        set: Action::new("set_size"),
                        finish: Some(Action::new("finish_edit")),
                    });
                }
                registry::BarControl::Divider => items.push(ToolItem::Separator),
                registry::BarControl::Slider {
                    prop,
                    label,
                    min,
                    max,
                } => {
                    let Some(PropValue::Number(value)) = self.bar_value(*prop) else {
                        continue;
                    };
                    items.push(ToolItem::Slider {
                        caption: label.to_string(),
                        width: BAR_SLIDER_W,
                        value: value.round() as i32,
                        min: *min,
                        max: *max,
                        tint: None,
                        set: Action::with_arg("set_bar", json!({ "prop": prop.key() })),
                        finish: Some(Action::new("finish_edit")),
                    });
                }
                registry::BarControl::Choice { prop, options } => {
                    let chosen = match self.bar_value(*prop) {
                        Some(PropValue::Index(index)) => Some(usize::from(index)),
                        _ => None,
                    };
                    for (index, (icon, label)) in options.iter().enumerate() {
                        items.push(ToolItem::Button {
                            icon: icon.to_string(),
                            tip: label.to_string(),
                            enabled: true,
                            selected: chosen == Some(index),
                            action: Some(Action::with_arg(
                                "bar_choice",
                                json!({ "prop": prop.key(), "index": index }),
                            )),
                            menu: Vec::new(),
                        });
                    }
                }
                registry::BarControl::List => {
                    let (on, kind) = self.text_list();
                    let (icon, label) =
                        registry::LIST_KINDS[kind.min(registry::LIST_KINDS.len() - 1)];
                    items.push(ToolItem::Button {
                        icon: icon.to_string(),
                        tip: label.to_string(),
                        enabled: true,
                        selected: on,
                        action: Some(Action::new("toggle_list")),
                        menu: registry::LIST_KINDS
                            .iter()
                            .enumerate()
                            .map(|(index, (icon, label))| MenuEntry {
                                label: label.to_string(),
                                icon: Some(icon.to_string()),
                                enabled: true,
                                divider: false,
                                action: Some(Action::with_arg("choose_list", json!(index))),
                                submenu: Vec::new(),
                                rename: false,
                            })
                            .collect(),
                    });
                }
            }
        }
    }

    /// The panel of `panel::show`: the outliner on the Layers page, the way
    /// back from a color page, and each section's rows with their values.
    pub(crate) fn declare_properties(&self) -> Properties {
        let toggle = registry::command(CommandId::TogglePanel);
        let back = self.back_label().map(|label| Back {
            label,
            action: Action::new("go_back"),
        });
        let layers = self.shows_layers().then(|| self.declare_layers());
        let sections = self
            .panel_sections()
            .into_iter()
            .map(|(title, props)| Section {
                title,
                rows: props
                    .into_iter()
                    .filter_map(|prop| self.declare_row(prop))
                    .collect(),
            })
            .collect();
        Properties {
            open: self.panel_open(),
            toggle: command_action(CommandId::TogglePanel),
            tip: registry::hover(toggle.label, toggle.shortcuts),
            finish: Action::new("finish_edit"),
            back,
            layers,
            sections,
        }
    }

    fn declare_row(&self, id: PropId) -> Option<Row> {
        let desc = schema::prop(id);
        let value = self.panel_property(id)?;
        let kind = match desc.kind {
            PropKind::Toggle => RowKind::Toggle,
            PropKind::Number {
                min,
                max,
                percent,
                places,
            } => RowKind::Number {
                min,
                max,
                percent,
                places,
            },
            PropKind::Text => RowKind::Text,
            PropKind::Choice {
                options,
                segmented,
                preview,
            } => RowKind::Choice {
                options: options.iter().map(|option| option.to_string()).collect(),
                segmented,
                preview: match preview {
                    ChoicePreview::Dash => protocol::ChoicePreview::Dash,
                    ChoicePreview::Cap => protocol::ChoicePreview::Cap,
                    ChoicePreview::Label => protocol::ChoicePreview::Label,
                },
            },
            PropKind::Preset => RowKind::Preset {
                options: self
                    .preset_labels()
                    .iter()
                    .map(|label| label.to_string())
                    .collect(),
                custom: draw::defaults::CUSTOM_PRESET.to_string(),
                sample: self
                    .panel_dynamics()
                    .map(|(tool, dynamics)| protocol::PresetSample {
                        eraser: tool == Tool::Eraser,
                        dynamics: dynamics_to_value(dynamics),
                    }),
            },
            PropKind::Rgb => RowKind::Rgb {
                quick: self.quick_colors(),
            },
            PropKind::Curve => RowKind::Curve,
            PropKind::Palette => {
                let owner = self.palette_node()?;
                RowKind::Palette {
                    owner: owner.get(),
                    colors: self
                        .palette_entries(owner)
                        .iter()
                        .map(|color| self.color_row(owner, color, false))
                        .collect(),
                    sort: Some(Action::new("sort_color")),
                    flags: flag_icons(),
                }
            }
        };
        Some(Row {
            prop: json!(id.key()),
            label: desc.label.to_string(),
            kind,
            value: prop_value(value),
        })
    }

    /// The Layers tree of `outliner::show`, every row with its own actions.
    fn declare_layers(&self) -> Layers {
        let rows = self
            .layers()
            .into_iter()
            .map(|node| {
                let menu = match node.kind {
                    LayerKind::Group => registry::group_row_menu(),
                    LayerKind::Canvas => registry::canvas_row_menu(),
                    LayerKind::Rectangle
                    | LayerKind::Ellipse
                    | LayerKind::Line
                    | LayerKind::Text => registry::object_menu(),
                };
                let has_page = self.object_page_available(node.id);
                let id = node.id.get();
                LayerRow {
                    id,
                    parent: node.parent.map(NodeId::get),
                    depth: node.depth,
                    icon: layer_icon(node.kind).to_string(),
                    name: node.name.clone(),
                    group: node.kind == LayerKind::Group,
                    selected: node.selected,
                    visible: node.visible,
                    locked: node.locked,
                    renamable: matches!(node.kind, LayerKind::Canvas | LayerKind::Group),
                    click: Action::with_arg("select_click", json!({ "node": id })),
                    rename: Action::with_arg("rename_node", json!({ "node": id })),
                    toggle_visible: Action::with_arg(
                        "toggle_node",
                        json!({ "node": id, "prop": PropId::Visible.key() }),
                    ),
                    toggle_locked: Action::with_arg(
                        "toggle_node",
                        json!({ "node": id, "prop": PropId::Locked.key() }),
                    ),
                    menu: menu_rows(
                        menu,
                        |action| action != MenuAction::Properties || has_page,
                        |action| match action {
                            MenuAction::MakePaletteDefault => {
                                Action::with_arg("make_palette_default", json!({ "node": id }))
                            }
                            action => Action::with_arg(
                                "object_action",
                                json!({ "node": id, "action": action_key(action) }),
                            ),
                        },
                    ),
                    colors: if node.kind == LayerKind::Group {
                        Vec::new()
                    } else {
                        node.colors
                            .iter()
                            .map(|color| self.color_row(node.id, color, true))
                            .collect()
                    },
                }
            })
            .collect();
        Layers {
            rows,
            reorder: Action::new("reorder"),
            sort_color: Action::new("sort_color"),
            flags: flag_icons(),
        }
    }

    /// A palette row, in the outliner under its canvas or in the panel's
    /// Palette section, as `outliner::color_row` drew it.
    fn color_row(&self, owner: NodeId, color: &LayerColor, outliner: bool) -> ColorRow {
        let target = json!({ "owner": owner.get(), "color": color.id.get() });
        let open = if outliner {
            Action::with_arg("select_color", target.clone())
        } else {
            Action::with_arg("open_palette_color", json!({ "color": color.id.get() }))
        };
        let menu = if color.fixed {
            registry::role_menu()
        } else {
            registry::color_row_menu()
        };
        let deletable = self.layer_count(owner) > 1;
        let flag = |prop: PropId| {
            Action::with_arg(
                "toggle_color",
                json!({ "owner": owner.get(), "color": color.id.get(), "prop": prop.key() }),
            )
        };
        ColorRow {
            id: color.id.get(),
            name: color.name.clone(),
            rgb: color.rgb,
            visible: color.visible,
            locked: color.locked,
            fixed: color.fixed,
            open: open.clone(),
            rename: (!color.fixed).then(|| Action::with_arg("rename_color", target.clone())),
            toggle_visible: flag(PropId::Visible),
            toggle_locked: (!color.fixed).then(|| flag(PropId::Locked)),
            menu: menu_rows(
                menu,
                |action| action != MenuAction::Delete || deletable,
                |action| match action {
                    MenuAction::Properties => Action::with_arg(
                        "open_color",
                        json!({
                            "owner": owner.get(),
                            "color": color.id.get(),
                            "outliner": outliner,
                        }),
                    ),
                    action => Action::with_arg(
                        "layer_action",
                        json!({
                            "owner": owner.get(),
                            "color": color.id.get(),
                            "action": action_key(action),
                        }),
                    ),
                },
            ),
        }
    }

    /// What `canvas.rs` read from the session to draw and preview: the
    /// current tool's settings for its gesture, the text being typed, and
    /// the object menu.
    pub(crate) fn declare_canvas(&self) -> Canvas {
        let tab = self.active_id().get().to_string();
        let tool = match self.tool {
            Tool::Select => CanvasTool::Select,
            Tool::Brush => self.brush_tool(),
            Tool::Eraser => CanvasTool::Erase {
                canvas: self.active_sketch().highlighted_id().map(NodeId::get),
                size: self.eraser_size,
                dynamics: dynamics_to_value(&self.eraser),
            },
            Tool::Rectangle | Tool::Ellipse | Tool::Line => {
                let settings = self.shape_tool(self.tool).expect("a shape tool");
                let palette: Vec<PaletteColor> = settings
                    .palette
                    .iter()
                    .map(|item| {
                        let mut color = item.color.clone();
                        if color.role.has_stroke() {
                            color.size = item.mark_size();
                        }
                        color
                    })
                    .collect();
                let shape = match shape_kind(self.tool) {
                    Some(draw::document::ShapeKind::Rectangle) => "rectangle",
                    Some(draw::document::ShapeKind::Ellipse) => "ellipse",
                    _ => "line",
                };
                CanvasTool::Shape {
                    shape: shape.to_string(),
                    constrain: settings.constrain,
                    snap: settings.snap,
                    palette: palette_value(&palette),
                    corner_radius: settings.corner_radius,
                }
            }
            Tool::Text => CanvasTool::Text {
                line: f64::from(self.text.style.size) * self.text.style.line_height,
                em: f64::from(self.text.style.size),
            },
        };
        let object_pages = self
            .layers()
            .into_iter()
            .map(|node| node.id)
            .filter(|node| self.object_page_available(*node))
            .map(NodeId::get)
            .collect();
        let properties_row = registry::object_menu()
            .iter()
            .find(|def| def.action == MenuAction::Properties)
            .map(|def| def.label.to_string());
        let eraser_end = (self.tool != Tool::Eraser).then(|| EraserEnd {
            enter: Action::with_arg("select_tool", json!(tool_key(Tool::Eraser))),
            leave: Action::with_arg("select_tool", json!(tool_key(self.tool))),
        });
        Canvas {
            tab,
            tool,
            object_menu: menu_rows(
                registry::object_menu(),
                |_| true,
                |action| Action::with_arg("object_action", json!({ "action": action_key(action) })),
            ),
            object_pages,
            properties_row,
            caret: self.declare_caret(),
            edit_text: Some(Action::new("edit_text")),
            eraser_end,
            escape_clears: matches!(self.tool, Tool::Select | Tool::Text),
            text_handles: self.text_box_handles(),
            metrics: Some(Action::new("text_metrics")),
        }
    }

    /// The brush's next stroke, as `begin` would open it.
    fn brush_tool(&self) -> CanvasTool {
        let sketch = self.active_sketch();
        let canvas = sketch.highlighted_id();
        let source = self.brush_active;
        let ready = canvas.is_none() || sketch.brush_ready(source);
        let item = self.tool_color(Tool::Brush, source);
        let size = item.and_then(|item| (!item.lock_size).then_some(item.next_size));
        let layer = canvas.and_then(|canvas| sketch.layer_for_source(canvas, source));
        let look = item
            .map(|item| {
                let color = &item.color;
                stroke_look_value(color.size, color.cap, color.dash, color.rgb, color.opacity)
            })
            .unwrap_or(Value::Null);
        CanvasTool::Brush {
            canvas: canvas.map(NodeId::get),
            source: source.get(),
            layer: layer.map(ColorId::get),
            size,
            dynamics: dynamics_to_value(&self.brush),
            look,
            ready: ready && item.is_some(),
        }
    }

    fn declare_caret(&self) -> Option<Caret> {
        let draft = self.text_draft.as_ref()?;
        let (id, content, style, palette, frame) = match draft {
            TextDraft::New { frame, content } => {
                let colors: Vec<PaletteColor> = self
                    .text
                    .palette
                    .iter()
                    .map(|item| item.color.clone())
                    .collect();
                (
                    None,
                    content.clone(),
                    Some(style_to_value(&self.text.style)),
                    Some(palette_value(&colors)),
                    Some(box_value(*frame)),
                )
            }
            TextDraft::Existing { id, content, .. } => {
                (Some(id.get()), content.clone(), None, None, None)
            }
        };
        Some(Caret {
            id,
            content,
            generation: self.text_generation,
            style,
            palette,
            frame,
            edit: Action::new("text_edit"),
            commit: Action::new("commit_text"),
            draft_box: Action::new("draft_box"),
        })
    }

    /// The camera the host moved, as `note_view` kept it: dirty only when it moved.
    pub(crate) fn set_view(&mut self, origin: [f64; 2], zoom: f64) {
        self.active_sketch_mut().move_view(|camera, _| {
            camera.origin = origin;
            camera.zoom = zoom;
        });
    }

    /// A text's size as the host laid it out, as the window's paint stored it.
    pub(crate) fn note_text_metrics(&mut self, id: NodeId, width: f64, height: f64, natural: f64) {
        self.set_text_metrics(id, width, height, natural);
    }

    /// Opens a color's page from a palette row's Properties, as
    /// `outliner::color_row` did: from the outliner or from the panel.
    pub(crate) fn open_color_row(&mut self, owner: NodeId, color: ColorId, outliner: bool) {
        if outliner {
            self.select_color(owner, color);
        } else {
            self.open_palette_color(color);
        }
        self.show_panel();
    }
}

fn flag_icons() -> protocol::FlagIcons {
    protocol::FlagIcons {
        eye_open: registry::EYE_OPEN.to_string(),
        eye_closed: registry::EYE_CLOSED.to_string(),
        lock_closed: registry::LOCK_CLOSED.to_string(),
        lock_open: registry::LOCK_OPEN.to_string(),
    }
}

fn layer_icon(kind: LayerKind) -> &'static str {
    match kind {
        LayerKind::Canvas => registry::node_icon(false),
        LayerKind::Group => registry::node_icon(true),
        LayerKind::Rectangle => registry::tool(Tool::Rectangle).icon,
        LayerKind::Ellipse => registry::tool(Tool::Ellipse).icon,
        LayerKind::Line => registry::tool(Tool::Line).icon,
        LayerKind::Text => registry::tool(Tool::Text).icon,
    }
}

/// The Open Recent submenu of `tabs::recent_rows`: each recent file by name,
/// most recent first, then Clear Recent below a divider. Two files with one
/// name also show their folder.
fn recent_rows(recent: &[PathBuf], clear_enabled: bool) -> Vec<MenuEntry> {
    let titles: Vec<String> = recent
        .iter()
        .map(|path| crate::store::file_title(path))
        .collect();
    let mut rows: Vec<MenuEntry> = recent
        .iter()
        .zip(&titles)
        .map(|(path, title)| {
            let shared = titles.iter().filter(|other| *other == title).count() > 1;
            let folder = path
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str());
            let label = match folder {
                Some(folder) if shared => format!("{title} — {folder}"),
                _ => title.clone(),
            };
            MenuEntry {
                label,
                icon: None,
                enabled: true,
                divider: false,
                action: Some(Action::with_arg(
                    "open_recent",
                    json!(path.display().to_string()),
                )),
                submenu: Vec::new(),
                rename: false,
            }
        })
        .collect();
    let clear = registry::command(CommandId::ClearRecent);
    rows.push(MenuEntry {
        label: clear.label.to_string(),
        icon: None,
        enabled: clear_enabled,
        divider: true,
        action: Some(command_action(CommandId::ClearRecent)),
        submenu: Vec::new(),
        rename: false,
    });
    rows
}

fn prop_value(value: PropValue) -> protocol::PropValue {
    match value {
        PropValue::Bool(on) => protocol::PropValue::Bool(on),
        PropValue::Number(number) => protocol::PropValue::Number(number),
        PropValue::Text(text) => protocol::PropValue::Text(text),
        PropValue::Index(index) => protocol::PropValue::Index(index),
        PropValue::Rgb(rgb) => protocol::PropValue::Rgb(rgb),
        PropValue::Curve(points) => protocol::PropValue::Curve(points),
        PropValue::Idle => protocol::PropValue::Idle,
        PropValue::Mixed => protocol::PropValue::Mixed,
    }
}

/// A value the panel or toolbar sent, as the session takes it.
pub(crate) fn session_value(value: protocol::PropValue) -> PropValue {
    match value {
        protocol::PropValue::Bool(on) => PropValue::Bool(on),
        protocol::PropValue::Number(number) => PropValue::Number(number),
        protocol::PropValue::Text(text) => PropValue::Text(text),
        protocol::PropValue::Index(index) => PropValue::Index(index),
        protocol::PropValue::Rgb(rgb) => PropValue::Rgb(rgb),
        protocol::PropValue::Curve(points) => PropValue::Curve(points),
        protocol::PropValue::Idle => PropValue::Idle,
        protocol::PropValue::Mixed => PropValue::Mixed,
    }
}
