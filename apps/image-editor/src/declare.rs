//! What the Image Editor declares for the host to draw: the toolbar and its
//! settings row, as the approved views show them, the Layers page, the
//! menu, the keys, and the tab list.

use crate::paint::Combine;
use crate::picture::{Blend, Node};
use crate::session::{Session, ToolKind};
use protocol::{
    Action, Binding, Chord, ChoicePreview, FlagIcons, HeadingAction, IconToggle, Keys, LayerRow, Layers, MenuEntry,
    PanelPage, PropValue, Properties, Row, RowKind, Section, TabEntry, Tabs, ToolItem, Toolbar,
};
use serde_json::json;

/// The crop ratios: their words and width over height, none for Free.
pub const RATIOS: [(&str, Option<f64>); 5] = [
    ("Free", None),
    ("1:1", Some(1.0)),
    ("4:3", Some(4.0 / 3.0)),
    ("3:2", Some(1.5)),
    ("16:9", Some(16.0 / 9.0)),
];

/// The zoom menu's rows: a word and the zoom it sets, none for Fit.
const ZOOMS: [(&str, Option<f64>); 4] = [("Fit", None), ("50%", Some(0.5)), ("100%", Some(1.0)), ("200%", Some(2.0))];

/// The four locks, in the order the Lock row shows them.
const LOCK_ICONS: [(&str, &str); 4] = [
    ("view-grid", "Lock transparency"),
    ("edit-pencil", "Lock pixels"),
    ("drag", "Lock position"),
    ("lock", "Lock all"),
];

const SLIDER_W: f32 = 110.0;

/// Each tool's icon, name, and key.
fn tool_look(tool: ToolKind) -> (&'static str, &'static str, &'static str) {
    match tool {
        ToolKind::Move => ("cursor-pointer", "Move", "V"),
        ToolKind::Marquee => ("square-dashed", "Rectangle select", "M"),
        ToolKind::Lasso => ("chat-bubble-empty", "Lasso select", "L"),
        ToolKind::Crop => ("crop", "Crop", "C"),
        ToolKind::Eyedropper => ("color-picker", "Eyedropper", "I"),
        // Drawn by the host in Iconoir's style; Iconoir has no brush.
        ToolKind::Brush => ("brush", "Brush", "B"),
        ToolKind::Eraser => ("erase", "Eraser", "E"),
        ToolKind::Hand => ("drag-hand-gesture", "Hand", "H"),
        ToolKind::Zoom => ("search", "Zoom", "Z"),
    }
}

fn button(icon: &str, tip: &str, action: Action, enabled: bool, selected: bool) -> ToolItem {
    ToolItem::Button {
        icon: icon.into(),
        tip: tip.into(),
        enabled,
        selected,
        action: Some(action),
        menu: Vec::new(),
    }
}

fn row(label: &str, action: Action) -> MenuEntry {
    MenuEntry {
        label: label.into(),
        icon: None,
        enabled: true,
        divider: false,
        action: Some(action),
        submenu: Vec::new(),
        rename: false,
    }
}

/// The toolbar: open and export, undo and redo, the tools, then the zoom
/// menu, zoom out, zoom in, fit, and full screen, with the chosen tool's
/// settings in the row under it.
pub fn toolbar(session: &Session) -> Toolbar {
    let image = session.active_image();
    let can_undo = image.is_some_and(|image| image.doc.can_undo());
    let can_redo = image.is_some_and(|image| image.doc.can_redo());
    let mut export = button("floppy-disk", "Export as PNG", Action::new("export_png"), image.is_some(), false);
    if let ToolItem::Button { menu, .. } = &mut export {
        *menu = vec![row("Export PNG", Action::new("export_png")), row("Export JPEG", Action::new("export_jpeg"))];
    }
    let mut items = vec![
        button("folder", "Open (Ctrl+O)", Action::new("open"), true, false),
        export,
        ToolItem::Separator,
        button("undo", "Undo (Ctrl+Z)", Action::new("undo"), can_undo, false),
        button("redo", "Redo (Ctrl+Shift+Z)", Action::new("redo"), can_redo, false),
        ToolItem::Separator,
    ];
    for tool in ToolKind::ALL {
        let (icon, name, key) = tool_look(tool);
        items.push(button(
            icon,
            &format!("{name} ({key})"),
            Action::with_arg("tool", json!({ "tool": tool.key() })),
            true,
            session.tools.tool == tool,
        ));
    }
    let zoom = image.and_then(|image| image.camera.as_ref()).map_or(1.0, |camera| camera.zoom);
    let trailing = vec![
        ToolItem::Separator,
        ToolItem::Menu {
            icon: String::new(),
            label: format!("{}%", (zoom * 100.0).round()),
            enabled: true,
            rows: ZOOMS
                .iter()
                .map(|(label, zoom)| match zoom {
                    Some(zoom) => row(label, Action::with_arg("canvas.zoom", json!({ "zoom": zoom }))),
                    None => row(label, Action::new("canvas.fit")),
                })
                .collect(),
        },
        ToolItem::Separator,
        button("zoom-out", "Zoom out (Ctrl+-)", Action::new("canvas.zoom_out"), true, false),
        button("zoom-in", "Zoom in (Ctrl++)", Action::new("canvas.zoom_in"), true, false),
        button("frame", "Fit (Ctrl+0)", Action::new("canvas.fit"), true, false),
        ToolItem::Maximize { icon: "maximize".into(), restore_icon: "reduce".into(), tip: "Full screen".into() },
    ];
    let (options, options_trailing) = settings_row(session);
    Toolbar { items, trailing, options, options_trailing }
}

/// The chosen tool's settings, as brush.png, selection.png, and crop.png
/// show them.
fn settings_row(session: &Session) -> (Vec<ToolItem>, Vec<ToolItem>) {
    let tools = &session.tools;
    let slider = |caption: &str, value: u32, min: i32, max: i32, suffix: &str, tool: &str, setting: &str| ToolItem::Slider {
        caption: caption.into(),
        // The host lays the caption, the track, and the value out within the
        // width, as Simple Sketch's Size control is: the track keeps its own
        // width beside them.
        width: SLIDER_W + caption.chars().count() as f32 * 7.5 + 70.0,
        value: value as i32,
        min,
        max,
        tint: None,
        set: Action::with_arg("set_brush", json!({ "tool": tool, "setting": setting })),
        finish: Some(Action::new("finish_brush")),
        suffix: suffix.into(),
    };
    let caption = |text: &str| vec![ToolItem::Label { text: text.into(), strong: false }];
    // Why the chosen tool cannot change the active layer, after its
    // settings; the gesture is still refused.
    let reason = |erase: bool, moving: bool| {
        let doc = &session.active_image()?.doc;
        if moving { doc.move_refusal() } else { doc.paint_refusal(erase) }
            .map(|text| ToolItem::Label { text, strong: true })
    };
    match tools.tool {
        ToolKind::Brush | ToolKind::Eraser => {
            let (settings, key) = if tools.tool == ToolKind::Brush { (tools.brush, "brush") } else { (tools.eraser, "eraser") };
            let mut row = Vec::new();
            if tools.tool == ToolKind::Brush {
                row.push(ToolItem::Color { rgb: tools.rgb, tip: "Brush colour".into(), set: Action::new("set_color") });
                row.push(ToolItem::Separator);
            }
            row.push(slider("Size", settings.size, 1, 500, "px", key, "size"));
            row.push(slider("Hardness", settings.hardness, 0, 100, "%", key, "hardness"));
            row.push(slider("Opacity", settings.opacity, 1, 100, "%", key, "opacity"));
            row.push(ToolItem::Separator);
            row.push(ToolItem::Switch {
                caption: "Pen pressure".into(),
                on: settings.pressure,
                action: Action::with_arg("toggle_pressure", json!({ "tool": key })),
            });
            if let Some(reason) = reason(tools.tool == ToolKind::Eraser, false) {
                row.push(ToolItem::Separator);
                row.push(reason);
            }
            (row, Vec::new())
        }
        ToolKind::Marquee | ToolKind::Lasso => (
            vec![
                ToolItem::Segmented {
                    options: Combine::ALL.iter().map(|how| how.label().to_string()).collect(),
                    selected: Combine::ALL.iter().position(|how| *how == tools.combine).unwrap_or(0),
                    set: Action::new("select_mode"),
                },
                ToolItem::Separator,
                ToolItem::Number {
                    caption: "Feather".into(),
                    value: tools.feather,
                    min: 0.0,
                    max: 1000.0,
                    suffix: "px".into(),
                    set: Action::new("feather"),
                },
            ],
            vec![
                ToolItem::Push { label: "Deselect".into(), dark: false, enabled: true, action: Action::new("deselect") },
                ToolItem::Push { label: "Invert".into(), dark: false, enabled: true, action: Action::new("invert") },
            ],
        ),
        ToolKind::Crop => {
            let image = session.active_image();
            let size = image
                .map(|image| match image.crop {
                    Some(rect) => (rect[2].round() as u32, rect[3].round() as u32),
                    None => (image.doc.picture.width, image.doc.picture.height),
                })
                .unwrap_or((0, 0));
            (
                vec![
                    ToolItem::Choice {
                        caption: "Ratio".into(),
                        options: RATIOS.iter().map(|(label, _)| label.to_string()).collect(),
                        selected: tools.ratio,
                        set: Action::new("crop_ratio"),
                    },
                    ToolItem::Label { text: format!("{} × {}", size.0, size.1), strong: true },
                ],
                vec![
                    ToolItem::Push { label: "Cancel".into(), dark: false, enabled: true, action: Action::new("crop_cancel") },
                    ToolItem::Push { label: "Apply".into(), dark: true, enabled: image.is_some(), action: Action::new("crop_apply") },
                ],
            )
        }
        ToolKind::Move => {
            let mut row = caption("Drag to move the active layer.");
            row.extend(reason(false, true));
            (row, Vec::new())
        }
        ToolKind::Eyedropper => (caption("Click the picture to take its colour for the brush."), Vec::new()),
        ToolKind::Hand => (caption("Drag to pan."), Vec::new()),
        ToolKind::Zoom => (caption("Click to zoom in, Alt-click to zoom out."), Vec::new()),
    }
}

/// The menu. In a project every change is written by itself, so Save and
/// Save As are not offered.
pub fn menu(session: &Session) -> Vec<MenuEntry> {
    let mut rows = vec![
        row("New image", Action::new("new_image")),
        row("Open…", Action::new("open")),
        row("Open as layer…", Action::new("open_layer")),
    ];
    if session.project.is_none() {
        rows.push(row("Save", Action::new("save")));
        rows.push(row("Save As…", Action::new("save_as")));
    }
    let mut export = row("Export PNG…", Action::new("export_png"));
    export.divider = true;
    rows.push(export);
    rows.push(row("Export JPEG…", Action::new("export_jpeg")));
    let mut undo = row("Undo", Action::new("undo"));
    undo.divider = true;
    undo.enabled = session.active_image().is_some_and(|image| image.doc.can_undo());
    rows.push(undo);
    let mut redo = row("Redo", Action::new("redo"));
    redo.enabled = session.active_image().is_some_and(|image| image.doc.can_redo());
    rows.push(redo);
    let mut deselect = row("Deselect", Action::new("deselect"));
    deselect.divider = true;
    rows.push(deselect);
    rows.push(row("Invert selection", Action::new("invert")));
    rows
}

/// The keys. The tools take Photoshop's letters; F4 stays the host's.
pub fn keys() -> Keys {
    let chord = |key: &str, command: bool, shift: Option<bool>| Chord { key: key.into(), command, shift, alt: false };
    let bind = |chord: Chord, action: Action| Binding { chord, then: Vec::new(), action };
    let mut bindings = vec![
        bind(chord("Z", true, Some(false)), Action::new("undo")),
        bind(chord("Z", true, Some(true)), Action::new("redo")),
        bind(chord("Y", true, Some(false)), Action::new("redo")),
        bind(chord("N", true, Some(false)), Action::new("new_image")),
        bind(chord("O", true, Some(false)), Action::new("open")),
        bind(chord("D", true, Some(false)), Action::new("deselect")),
        bind(chord("I", true, Some(true)), Action::new("invert")),
        bind(chord("Enter", false, Some(false)), Action::new("crop_apply")),
        bind(chord("Escape", false, Some(false)), Action::new("crop_cancel")),
        bind(chord("BracketLeft", false, Some(false)), Action::new("size_down")),
        bind(chord("BracketRight", false, Some(false)), Action::new("size_up")),
        bind(chord("Plus", true, None), Action::new("canvas.zoom_in")),
        bind(chord("Equals", true, None), Action::new("canvas.zoom_in")),
        bind(chord("Minus", true, None), Action::new("canvas.zoom_out")),
        bind(chord("Digit0", true, Some(false)), Action::new("canvas.fit")),
    ];
    for tool in ToolKind::ALL {
        let (_, _, key) = tool_look(tool);
        bindings.push(bind(chord(key, false, Some(false)), Action::with_arg("tool", json!({ "tool": tool.key() }))));
    }
    Keys { bindings, copy: None, cut: None, paste: None }
}

/// The panel: the Layers page, with the active layer's settings above the
/// list of layers.
pub fn properties(session: &Session) -> Option<Properties> {
    let image = session.active_image()?;
    let picture = &image.doc.picture;
    let active = image.doc.active.and_then(|id| picture.find(id));
    let mut sections = Vec::new();
    if let Some(node) = active {
        let mut rows = vec![
            Row {
                prop: json!("blend"),
                label: "Blend Mode".into(),
                kind: RowKind::Choice {
                    options: Blend::ALL.iter().map(|blend| blend.label().to_string()).collect(),
                    segmented: false,
                    preview: ChoicePreview::Label,
                },
                value: PropValue::Index(node.blend.index() as u8),
            },
            Row {
                prop: json!("opacity"),
                label: "Opacity".into(),
                kind: RowKind::Number { min: 0.0, max: 1.0, percent: true, places: 0 },
                value: PropValue::Number(f64::from(node.opacity)),
            },
        ];
        // A group has no pixels of its own for Fill to scale.
        if !node.is_group() {
            rows.push(Row {
                prop: json!("fill"),
                label: "Fill".into(),
                kind: RowKind::Number { min: 0.0, max: 1.0, percent: true, places: 0 },
                value: PropValue::Number(f64::from(node.fill)),
            });
        }
        rows.push(Row {
            prop: json!("locks"),
            label: "Lock:".into(),
            kind: RowKind::Icons {
                icons: LOCK_ICONS
                    .iter()
                    .enumerate()
                    .map(|(index, (icon, tip))| IconToggle { icon: icon.to_string(), tip: tip.to_string(), on: node.locks.get(index) })
                    .collect(),
            },
            value: PropValue::Idle,
        });
        sections.push(Section { title: String::new(), rows });
    }
    let mut rows = Vec::new();
    for node in picture.nodes.iter().rev() {
        layer_rows(session, node, None, 0, &mut rows);
    }
    Some(Properties {
        open: session.panel_open,
        toggle: Action::new("toggle_panel"),
        tip: "Layers (F4)".into(),
        finish: Action::new("finish_edit"),
        back: None,
        layers: Some(Layers {
            rows,
            reorder: Action::new("reorder"),
            sort_color: Action::new("sort_color"),
            nest: true,
            flags: FlagIcons {
                eye_open: "eye".into(),
                eye_closed: "eye-closed".into(),
                lock_closed: "lock".into(),
                lock_open: "lock-slash".into(),
            },
            actions: vec![
                HeadingAction { icon: "plus".into(), tip: "New layer".into(), enabled: true, action: Action::new("add_layer") },
                HeadingAction { icon: "folder".into(), tip: "New group".into(), enabled: true, action: Action::new("add_group") },
                HeadingAction {
                    icon: "trash".into(),
                    tip: "Delete layer".into(),
                    enabled: image.doc.active.is_some() && picture.nodes.iter().map(|node| picture.ids_within(node.id).len()).sum::<usize>() > 1,
                    action: Action::new("delete_layer"),
                },
            ],
        }),
        sections,
        pages: vec![
            PanelPage { label: "Layers".into(), active: true, enabled: true, action: None },
            // The Properties and Adjustments pages wait for a later step.
            PanelPage { label: "Properties".into(), active: false, enabled: false, action: None },
            PanelPage { label: "Adjustments".into(), active: false, enabled: false, action: None },
        ],
    })
}

/// A node's row, then its children's, top first, as the list shows them.
fn layer_rows(session: &Session, node: &Node, parent: Option<u64>, depth: u32, rows: &mut Vec<LayerRow>) {
    let Some(image) = session.active_image() else {
        return;
    };
    let id = node.id;
    let target = json!({ "node": id });
    let mut menu = vec![MenuEntry {
        label: "Rename".into(),
        icon: None,
        enabled: true,
        divider: false,
        action: None,
        submenu: Vec::new(),
        rename: true,
    }];
    if node.is_group() {
        menu.push(row("Ungroup", Action::with_arg("ungroup", target.clone())));
    }
    menu.push(row("Delete", Action::with_arg("delete_layer", target.clone())));
    rows.push(LayerRow {
        id,
        parent,
        depth,
        icon: if node.is_group() { "folder".into() } else { "media-image".into() },
        name: node.name.clone(),
        group: node.is_group(),
        selected: image.doc.active == Some(id),
        visible: node.visible,
        locked: node.locks.all,
        renamable: true,
        click: Action::with_arg("layer_click", target.clone()),
        rename: Action::with_arg("layer_rename", target.clone()),
        toggle_visible: Action::with_arg("layer_visible", target.clone()),
        toggle_locked: Action::with_arg("layer_lock", target),
        menu,
        colors: Vec::new(),
        thumbnail: image.thumbs.get(&id).map(|(_, thumb)| thumb.clone()),
    });
    if let Some(children) = &node.children {
        for child in children.iter().rev() {
            layer_rows(session, child, Some(id), depth + 1, rows);
        }
    }
}

pub fn tabs(session: &Session) -> Option<Tabs> {
    let active = session.active_image()?;
    Some(Tabs {
        tabs: session
            .images
            .iter()
            .map(|image| TabEntry {
                id: image.id.clone(),
                label: image.label.clone(),
                dirty: image.dirty,
                close_waits: false,
                path: image.path.as_ref().map(|path| path.display().to_string()),
                saves: true,
            })
            .collect(),
        active: active.id.clone(),
        new_tab: Some(Action::new("new_image")),
        new_tab_tip: "New image".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(items: &[ToolItem]) -> Vec<String> {
        items
            .iter()
            .map(|item| match item {
                ToolItem::Separator => "|".to_string(),
                ToolItem::Color { .. } => "colour".to_string(),
                ToolItem::Slider { caption, suffix, .. } => format!("{caption} {suffix}"),
                ToolItem::Switch { caption, .. } => caption.clone(),
                ToolItem::Segmented { options, .. } => options.join("/"),
                ToolItem::Number { caption, suffix, .. } => format!("{caption} {suffix}"),
                ToolItem::Choice { caption, options, .. } => format!("{caption}: {}", options.join("/")),
                ToolItem::Label { text, .. } => format!("label {text}"),
                ToolItem::Push { label, dark, .. } => format!("{label}{}", if *dark { " (dark)" } else { "" }),
                ToolItem::Button { icon, .. } => icon.clone(),
                ToolItem::Menu { label, .. } => format!("menu {label}"),
                ToolItem::Maximize { .. } => "maximize".to_string(),
                _ => "other".to_string(),
            })
            .collect()
    }

    /// Catches a settings row that drifts from the approved views: brush.png,
    /// selection.png, and crop.png, and the toolbar they share.
    #[test]
    fn the_settings_rows_follow_the_views() {
        let mut session = Session::launch();
        let bar = toolbar(&session);
        assert_eq!(
            kinds(&bar.items),
            [
                "folder", "floppy-disk", "|", "undo", "redo", "|", "cursor-pointer", "square-dashed",
                "chat-bubble-empty", "crop", "color-picker", "brush", "erase", "drag-hand-gesture", "search",
            ]
        );
        // A new picture is fitted: 1920 wide in 1200 less the margins is 58%.
        assert_eq!(kinds(&bar.trailing), ["|", "menu 58%", "|", "zoom-out", "zoom-in", "frame", "maximize"]);
        let ToolItem::Menu { rows, .. } = &bar.trailing[1] else { panic!() };
        let actions: Vec<_> = rows.iter().map(|row| row.action.clone().unwrap()).collect();
        assert_eq!(actions[0], Action::new("canvas.fit"));
        assert_eq!(actions[2], Action::with_arg("canvas.zoom", json!({ "zoom": 1.0 })));
        assert_eq!(
            kinds(&bar.options),
            ["colour", "|", "Size px", "Hardness %", "Opacity %", "|", "Pen pressure"]
        );
        let ToolItem::Slider { value, .. } = &bar.options[2] else { panic!() };
        assert_eq!(*value, 24);

        session.choose_tool(ToolKind::Eraser);
        assert_eq!(kinds(&toolbar(&session).options), ["Size px", "Hardness %", "Opacity %", "|", "Pen pressure"]);

        for tool in [ToolKind::Marquee, ToolKind::Lasso] {
            session.choose_tool(tool);
            let bar = toolbar(&session);
            assert_eq!(kinds(&bar.options), ["New/Add/Subtract/Intersect", "|", "Feather px"]);
            assert_eq!(kinds(&bar.options_trailing), ["Deselect", "Invert"]);
        }

        session.choose_tool(ToolKind::Crop);
        let bar = toolbar(&session);
        assert_eq!(kinds(&bar.options), ["Ratio: Free/1:1/4:3/3:2/16:9", "label 1920 × 1080"]);
        assert_eq!(kinds(&bar.options_trailing), ["Cancel", "Apply (dark)"]);

        for tool in [ToolKind::Move, ToolKind::Eyedropper] {
            session.choose_tool(tool);
            let bar = toolbar(&session);
            assert_eq!(bar.options.len(), 1);
            assert!(matches!(bar.options[0], ToolItem::Label { .. }));
        }
    }

    /// Catches a Layers page that drifts from the mockup: three pages with
    /// only Layers enabled, Blend Mode, Opacity, Fill, and four locks above
    /// the list, the heading's add, group, and delete, and thumbnails.
    #[test]
    fn the_layers_page_follows_the_mockup() {
        let mut session = Session::launch();
        let _ = session.emit();
        let panel = properties(&session).unwrap();
        let pages: Vec<(String, bool)> = panel.pages.iter().map(|page| (page.label.clone(), page.enabled)).collect();
        assert_eq!(
            pages,
            [("Layers".to_string(), true), ("Properties".to_string(), false), ("Adjustments".to_string(), false)]
        );
        let labels: Vec<&str> = panel.sections[0].rows.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(labels, ["Blend Mode", "Opacity", "Fill", "Lock:"]);
        let RowKind::Icons { icons } = &panel.sections[0].rows[3].kind else { panic!("the locks are not icons") };
        assert_eq!(icons.len(), 4);
        let layers = panel.layers.unwrap();
        let actions: Vec<&str> = layers.actions.iter().map(|action| action.icon.as_str()).collect();
        assert_eq!(actions, ["plus", "folder", "trash"]);
        assert_eq!(layers.rows.len(), 1);
        assert_eq!(layers.rows[0].name, "Background");
        let thumbnail = layers.rows[0].thumbnail.as_ref().expect("no thumbnail");
        assert_eq!((thumbnail.width, thumbnail.height), (56, 32));
        let bytes = std::fs::read(&thumbnail.path).unwrap();
        assert_eq!(bytes.len(), 56 * 32 * 4);
        assert!(bytes.chunks(4).all(|pixel| pixel == [255, 255, 255, 255]));
    }

    /// Catches a key that the host keeps for itself, or two keys on one chord.
    #[test]
    fn keys_are_unique_and_leave_f4() {
        let keys = keys();
        let mut seen = std::collections::HashSet::new();
        for binding in &keys.bindings {
            assert_ne!(binding.chord.key, "F4");
            let chord = &binding.chord;
            assert!(seen.insert((chord.key.clone(), chord.command, chord.shift, chord.alt)), "{chord:?} twice");
        }
    }
}
