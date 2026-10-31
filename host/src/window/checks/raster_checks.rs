//! Window checks for raster tabs, against a stand-in sub-application that
//! no part of the host was written for. It declares a tab of pixels, writes a
//! known picture as raw RGBA, and logs every line the host sends it, so each
//! check reads what the host drew and what it sent. They spend nothing.

use super::{StandIn, Window, scratch, start};
use crate::window::Started;
use eframe::egui::{self, Color32, Pos2, Rect, pos2};
use protocol::{
    Action, AppMessage, ChoicePreview, DocumentCamera, Film, FilmItem, FlagIcons, HeadingAction, IconToggle,
    LayerRow, Layers, MenuEntry, PanelPage, PropValue, Properties, RasterTool, RasterView, Row, RowKind, Section,
    TabEntry, Tabs, Thumbnail, ToolItem, Toolbar,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The stand-in: it prints its greeting, then logs each line it is sent.
/// A command `tool.<name>` is answered with the lines in `<name>.jsonl`, the
/// view and toolbar of that tool.
const EASEL: &str = r#"#!/bin/sh
dir="__DIR__"
cat "$dir/greeting.jsonl"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$dir/log"
  case "$line" in
    *'"type":"ping"'*) echo '{"type":"pong"}' ;;
    *'"type":"shutdown"'*) exit 0 ;;
    *'"type":"quit"'*) echo '{"type":"quit_ready"}' ;;
    *'"id":"tool.'*)
      tool=$(printf '%s' "$line" | sed 's/.*"id":"tool\.\([a-z]*\)".*/\1/')
      cat "$dir/$tool.jsonl" ;;
  esac
done
"#;

/// The picture's size, and the camera the view carries: the picture is
/// drawn 50 points in from the contents' corner, 10 points to a pixel.
const W: u32 = 40;
const H: u32 = 20;
const ORIGIN: [f64; 2] = [-5.0, -5.0];
const ZOOM: f64 = 10.0;
const BRUSH: [u8; 3] = [200, 30, 40];
const CHIP: [u8; 3] = [20, 20, 22];

fn line(message: AppMessage) -> String {
    protocol::encode_app_message(&message)
}

fn view(dir: &Path, tool: RasterTool) -> RasterView {
    RasterView {
        tab: String::from("r1"),
        width: W,
        height: H,
        picture: dir.join("picture.rgba").display().to_string(),
        revision: 1,
        patch: None,
        tool,
        selection: vec![vec![[10.0, 0.0], [30.0, 0.0], [30.0, 20.0], [10.0, 20.0]]],
        crop: Some([0.0, 0.0, 20.0, 10.0]),
        bounds: Some([0.0, 0.0, 40.0, 20.0]),
        camera: Some(DocumentCamera { origin: ORIGIN, zoom: ZOOM }),
        film: None,
    }
}

fn brush() -> RasterTool {
    RasterTool::Brush { rgb: BRUSH, size: 1.0, hardness: 1.0, opacity: 1.0, pressure: true }
}

/// The settings row of each kind of tool, as the views show them.
fn toolbar(kind: &str) -> Toolbar {
    let (options, options_trailing) = match kind {
        "select" => (
            vec![
                ToolItem::Segmented {
                    options: ["New", "Add", "Subtract", "Intersect"].map(String::from).to_vec(),
                    selected: 0,
                    set: Action::new("set_mode"),
                },
                ToolItem::Separator,
                ToolItem::Number {
                    caption: String::from("Feather"),
                    value: 0.0,
                    min: 0.0,
                    max: 100.0,
                    suffix: String::from("px"),
                    set: Action::new("set_feather"),
                },
            ],
            vec![ToolItem::Push { label: String::from("Deselect"), dark: false, enabled: true, action: Action::new("deselect") }],
        ),
        "crop" => (
            vec![
                ToolItem::Choice {
                    caption: String::from("Ratio"),
                    options: ["Free", "1:1", "16:9"].map(String::from).to_vec(),
                    selected: 0,
                    set: Action::new("set_ratio"),
                },
                ToolItem::Label { text: String::from("3200 × 1800"), strong: true },
            ],
            vec![
                ToolItem::Push { label: String::from("Later"), dark: false, enabled: false, action: Action::new("later") },
                ToolItem::Push { label: String::from("Cancel"), dark: false, enabled: true, action: Action::new("cancel_crop") },
                ToolItem::Push { label: String::from("Apply"), dark: true, enabled: true, action: Action::new("apply_crop") },
            ],
        ),
        _ => (
            vec![
                ToolItem::Color { rgb: CHIP, tip: String::from("Colour"), set: Action::new("set_color") },
                ToolItem::Separator,
                ToolItem::Slider {
                    caption: String::from("Size"),
                    width: 190.0,
                    value: 24,
                    min: 1,
                    max: 200,
                    tint: None,
                    set: Action::new("set_size"),
                    finish: Some(Action::new("finish_size")),
                    suffix: String::from("px"),
                },
                ToolItem::Switch { caption: String::from("Pen pressure"), on: true, action: Action::new("toggle_pressure") },
            ],
            Vec::new(),
        ),
    };
    Toolbar { items: Vec::new(), trailing: Vec::new(), options, options_trailing }
}

/// A row of the Layers list. A layer has a picture; a group has none.
fn layer_row(dir: &Path, id: u64, parent: Option<u64>, depth: u32, name: &str, group: bool) -> LayerRow {
    LayerRow {
        id,
        parent,
        depth,
        icon: String::from(if group { "folder" } else { "frame" }),
        name: String::from(name),
        group,
        selected: true,
        visible: true,
        locked: false,
        renamable: true,
        click: Action::new("select_layer"),
        rename: Action::new("rename_layer"),
        toggle_visible: Action::new("toggle_visible"),
        toggle_locked: Action::new("toggle_lock"),
        menu: Vec::new(),
        colors: Vec::new(),
        thumbnail: (!group).then(|| Thumbnail { path: dir.join("thumb.rgba").display().to_string(), width: W, height: H, revision: 1 }),
    }
}

fn properties(rows: Vec<LayerRow>) -> Properties {
    let heading = |icon: &str, tip: &str, id: &str| HeadingAction {
        icon: icon.to_string(),
        tip: tip.to_string(),
        enabled: true,
        action: Action::new(id),
    };
    let lock = |icon: &str| IconToggle { icon: icon.to_string(), tip: icon.to_string(), on: false };
    Properties {
        open: true,
        toggle: Action::new("toggle_panel"),
        tip: String::from("Layers"),
        finish: Action::new("finish_edit"),
        back: None,
        layers: Some(Layers {
            rows,
            reorder: Action::new("reorder"),
            sort_color: Action::new("sort_color"),
            nest: true,
            flags: FlagIcons {
                eye_open: String::from("eye"),
                eye_closed: String::from("eye-closed"),
                lock_closed: String::from("lock"),
                lock_open: String::from("lock-slash"),
            },
            actions: vec![
                heading("plus", "Add a layer", "add_layer"),
                heading("folder", "Group", "group_layers"),
                heading("trash", "Delete", "delete_layer"),
            ],
        }),
        sections: vec![
            Section {
                title: String::new(),
                rows: vec![Row {
                    prop: json!("blend"),
                    label: String::from("Blend Mode"),
                    kind: RowKind::Choice {
                        options: vec![String::from("Normal"), String::from("Multiply")],
                        segmented: false,
                        preview: ChoicePreview::Label,
                    },
                    value: PropValue::Index(0),
                }],
            },
            Section {
                title: String::new(),
                rows: vec![Row {
                    prop: json!("lock"),
                    label: String::from("Lock:"),
                    kind: RowKind::Icons { icons: vec![lock("square-dashed"), lock("edit-pencil"), lock("drag"), lock("lock")] },
                    value: PropValue::Idle,
                }],
            },
        ],
        pages: vec![
            PanelPage { label: String::from("Layers"), active: true, enabled: true, action: None },
            PanelPage { label: String::from("Properties"), active: false, enabled: false, action: Some(Action::new("page_properties")) },
            PanelPage { label: String::from("Adjustments"), active: false, enabled: true, action: Some(Action::new("page_adjustments")) },
        ],
    }
}

/// The picture: opaque red, but for one clear pixel in its corner, so it
/// is drawn over squares.
fn picture_bytes() -> Vec<u8> {
    let mut bytes = Vec::new();
    for index in 0..W * H {
        bytes.extend_from_slice(if index == 0 { &[0, 0, 0, 0] } else { &[220, 40, 40, 255] });
    }
    bytes
}

/// Starts the stand-in in a window. Returns the window and its folder.
fn easel(name: &str) -> (Window, PathBuf, StandIn) {
    easel_with(name, |dir| vec![layer_row(dir, 1, None, 0, "Background", false)])
}

/// The stand-in with these rows in its Layers list.
fn easel_with(name: &str, rows: impl FnOnce(&Path) -> Vec<LayerRow>) -> (Window, PathBuf, StandIn) {
    easel_of(name, rows, false)
}

/// The film's pictures: their names and their thumbnails' sizes, a wide one,
/// a tall one, and a square one.
const PICTURES: [(&str, u32, u32); 3] = [("Harbor", 32, 18), ("Tower", 18, 32), ("Tile", 20, 20)];

/// A gallery of the three, the second shown, whose rows and tiles send
/// actions only this stand-in declares. Its view leaves the camera to the
/// host, so the host frames the picture above the strip.
fn film(dir: &Path) -> Film {
    let row = |label: &str, id: &str, arg: Option<Value>| MenuEntry {
        label: label.to_string(),
        icon: None,
        enabled: true,
        divider: false,
        action: Some(Action { id: id.to_string(), arg }),
        submenu: Vec::new(),
        rename: false,
    };
    Film {
        items: PICTURES
            .iter()
            .enumerate()
            .map(|(index, (name, width, height))| FilmItem {
                name: name.to_string(),
                thumbnail: Thumbnail { path: dir.join(format!("film{index}.rgba")).display().to_string(), width: *width, height: *height, revision: 1 },
                menu: vec![
                    row("Duplicate", "copy_item", Some(json!({ "item": index }))),
                    row("Rename", "rename_item", Some(json!({ "item": index }))),
                    row("Delete", "remove_item", Some(json!({ "item": index }))),
                ],
            })
            .collect(),
        shown: 1,
        select: Action::new("show_item"),
        reorder: Action::with_arg("order_items", json!({ "strip": true })),
        add: vec![row("New picture", "new_item", None), row("Open PNG or JPEG…", "open_item", None)],
        focus: Action::new("strip_focus"),
    }
}

/// The stand-in with a filmstrip of three pictures.
fn filmed(name: &str) -> (Window, PathBuf, StandIn) {
    easel_of(name, |dir| vec![layer_row(dir, 1, None, 0, "Background", false)], true)
}

fn easel_of(name: &str, rows: impl FnOnce(&Path) -> Vec<LayerRow>, filmed: bool) -> (Window, PathBuf, StandIn) {
    let dir = scratch(&format!("raster-{name}"));
    std::fs::write(dir.join("picture.rgba"), picture_bytes()).unwrap();
    std::fs::write(dir.join("thumb.rgba"), picture_bytes()).unwrap();
    for (index, (_, width, height)) in PICTURES.iter().enumerate() {
        std::fs::write(dir.join(format!("film{index}.rgba")), vec![200; (width * height * 4) as usize]).unwrap();
    }
    let declared = |dir: &Path, tool: RasterTool| {
        let mut view = view(dir, tool);
        if filmed {
            view.film = Some(film(dir));
            view.camera = None;
        }
        view
    };
    let greeting = [
        String::from("{\"type\":\"hello\",\"protocol\":1}\n"),
        String::from("{\"type\":\"tab\",\"id\":\"r1\",\"label\":\"Picture\"}\n"),
        String::from("{\"type\":\"contents\",\"tab\":\"r1\",\"kind\":\"raster\"}\n"),
        String::from("{\"type\":\"panel\",\"tab\":\"r1\",\"side\":\"right\"}\n"),
        String::from("{\"type\":\"end\"}\n"),
        String::from("{\"type\":\"skills\",\"skills\":[{\"id\":\"easel\",\"name\":\"Easel\",\"description\":\"Shows a picture.\",\"instructions\":\"Nothing to do.\",\"tools\":[{\"name\":\"look\",\"description\":\"Look.\",\"input\":{\"type\":\"object\",\"properties\":{}}}]}]}\n"),
        line(AppMessage::Tabs(Tabs {
            tabs: vec![TabEntry { id: String::from("r1"), label: String::from("Picture"), dirty: false, close_waits: false, path: None, saves: false }],
            active: String::from("r1"),
            new_tab: None,
            new_tab_tip: String::new(),
        })),
        line(AppMessage::Properties(properties(rows(&dir)))),
        line(AppMessage::Raster(declared(&dir, brush()))),
        line(AppMessage::Toolbar(toolbar("brush"))),
    ];
    std::fs::write(dir.join("greeting.jsonl"), greeting.concat()).unwrap();
    for (name, tool, bar) in [
        ("brush", brush(), "brush"),
        ("eraser", RasterTool::Erase { size: 2.0, hardness: 1.0, opacity: 1.0, pressure: false }, "brush"),
        ("marquee", RasterTool::Marquee, "select"),
        ("lasso", RasterTool::Lasso, "select"),
        ("move", RasterTool::Move, "brush"),
        ("crop", RasterTool::Crop, "crop"),
        ("eyedropper", RasterTool::Eyedropper, "brush"),
        ("hand", RasterTool::Hand, "brush"),
        ("zoom", RasterTool::Zoom, "brush"),
    ] {
        let lines = [line(AppMessage::Raster(declared(&dir, tool))), line(AppMessage::Toolbar(toolbar(bar)))];
        std::fs::write(dir.join(format!("{name}.jsonl")), lines.concat()).unwrap();
    }
    let program = dir.join("easel");
    std::fs::write(&program, EASEL.replace("__DIR__", &dir.display().to_string())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let stand_in = StandIn::start(Vec::new());
    // A script written a moment ago can be busy for another check's
    // fork, so starting it is tried again.
    let mut started: Started = start("easel", "Easel", &program);
    for _ in 0..20 {
        if started.running.answers() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        started = start("easel", "Easel", &program);
    }
    let mut window = Window::new(vec![started], &stand_in);
    window.until(|app| app.apps[0].running.declared.rasters.contains_key("r1") && app.apps[0].running.declared.properties.is_some());
    window.step();
    window.step();
    (window, dir, stand_in)
}

/// Every line the host sent the stand-in, in order.
fn sent(dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(dir.join("log"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn of_type(dir: &Path, kind: &str) -> Vec<Value> {
    sent(dir).into_iter().filter(|line| line["type"] == kind).collect()
}

/// Every finished gesture: the parts of a stroke sent while it is drawn
/// are left out.
fn gestures(dir: &Path) -> Vec<Value> {
    all_gestures(dir).into_iter().filter(|gesture| gesture["kind"] != "stroke_part").collect()
}

fn all_gestures(dir: &Path) -> Vec<Value> {
    of_type(dir, "raster_gesture").into_iter().map(|line| line["gesture"].clone()).collect()
}

fn commands(dir: &Path, id: &str) -> Vec<Value> {
    of_type(dir, "command").into_iter().filter(|line| line["id"] == id).collect()
}

fn raster(window: &Window) -> &crate::process::RasterDisplay {
    &window.app.apps[0].running.declared.rasters["r1"]
}

/// Where a picture point is on screen, through the host's camera.
fn screen(window: &Window, point: [f64; 2]) -> Pos2 {
    let rect = window.app.raster.shown;
    let camera = raster(window).camera.expect("the tab has a camera");
    pos2(
        ((point[0] - camera.origin[0]) * camera.zoom) as f32 + rect.min.x,
        ((point[1] - camera.origin[1]) * camera.zoom) as f32 + rect.min.y,
    )
}

/// Chooses a tool through the stand-in, as a toolbar button does.
fn use_tool(window: &mut Window, name: &str, is: fn(&RasterTool) -> bool) {
    window.app.run_action(&Action::new(format!("tool.{name}")));
    window.until(move |app| is(&app.apps[0].running.declared.rasters["r1"].view.tool));
    window.step();
}

fn press(window: &mut Window, at: Pos2, pressed: bool) {
    window.events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
}

/// A drag through picture points: pressed at the first, moved through the
/// rest, released at the last. `during` runs on the frame before release.
fn drag(window: &mut Window, points: &[[f64; 2]], during: impl FnOnce(&Window)) {
    let screens: Vec<Pos2> = points.iter().map(|point| screen(window, *point)).collect();
    window.events.push(egui::Event::PointerMoved(screens[0]));
    window.step();
    press(window, screens[0], true);
    window.step();
    for at in &screens[1..] {
        window.events.push(egui::Event::PointerMoved(*at));
        window.step();
    }
    during(window);
    press(window, *screens.last().unwrap(), false);
    window.step();
}

fn click(window: &mut Window, at: Pos2) {
    window.events.push(egui::Event::PointerMoved(at));
    window.step();
    press(window, at, true);
    window.step();
    press(window, at, false);
    window.step();
}

/// Frames enough for a menu to fade in.
fn settle(window: &mut Window) {
    for _ in 0..12 {
        window.step();
    }
}

fn text_at(window: &Window, text: &str) -> Rect {
    window
        .texts
        .iter()
        .filter(|(shown, _)| shown == text)
        .map(|(_, rect)| *rect)
        .next_back()
        .unwrap_or_else(|| panic!("{text:?} was not drawn"))
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-3
}

fn point(value: &Value) -> [f64; 2] {
    [value[0].as_f64().unwrap(), value[1].as_f64().unwrap()]
}

/// Rects painted with a texture of this name, and where.
fn textured(window: &Window, name: &str) -> Vec<Rect> {
    let manager = window.ctx.tex_manager();
    let manager = manager.read();
    window
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) => rect.brush.as_ref().map(|brush| (rect.rect, brush.fill_texture_id)),
            egui::Shape::Mesh(mesh) => Some((mesh.calc_bounds(), mesh.texture_id)),
            _ => None,
        })
        .filter(|(_, id)| manager.meta(*id).is_some_and(|meta| meta.name == name))
        .map(|(rect, _)| rect)
        .collect()
}

fn texture_size(window: &Window, name: &str) -> Option<[usize; 2]> {
    let manager = window.ctx.tex_manager();
    let manager = manager.read();
    manager.allocated().find(|(_, meta)| meta.name == name).map(|(_, meta)| meta.size)
}

fn segments(window: &Window, color: Color32) -> Vec<[Pos2; 2]> {
    window
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::LineSegment { points, stroke } if stroke.color == color => Some(*points),
            _ => None,
        })
        .collect()
}

fn discs(window: &Window, color: Color32) -> usize {
    window
        .shapes
        .iter()
        .filter(|clipped| matches!(&clipped.shape, egui::Shape::Circle(dot) if dot.fill == color))
        .count()
}

/// Ends the window, whose stand-in exits when its input closes, and
/// removes the check's folder.
fn close(window: Window, dir: PathBuf) {
    drop(window);
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a raster tab that is not drawn, or drawn somewhere its camera
/// does not put it: the stand-in's raw picture becomes a texture of its own
/// size, at the place the view's camera says, over squares for its clear
/// pixel, with its size under it; the selection's outline is a white edge
/// with black dashes along it; and the crop box dims what lies outside it
/// and is divided in thirds.
#[test]
fn the_picture_its_selection_and_its_crop_box_are_drawn_where_the_camera_puts_them() {
    let (mut window, dir, _stand_in) = easel("drawn");
    let name = format!("raster:{}", dir.join("picture.rgba").display());
    assert_eq!(texture_size(&window, &name), Some([W as usize, H as usize]), "the picture was not read whole");
    let rect = window.app.raster.shown;
    let expected = Rect::from_min_size(rect.min + egui::vec2(50.0, 50.0), egui::vec2(400.0, 200.0));
    let drawn = textured(&window, &name);
    assert!(drawn.iter().any(|at| (at.min - expected.min).length() < 0.01 && (at.max - expected.max).length() < 0.01), "the picture is not where the camera puts it: {drawn:?}, not {expected:?}");
    assert!(textured(&window, "raster-checker").iter().any(|at| *at == expected), "no squares under the clear pixel");
    // No size caption under the picture, as the owner set on 2026-10-30.
    assert!(window.texts.iter().all(|(text, _)| text != "40 × 20"), "the picture's size is drawn under it");

    // The selection: a white edge along its outline, and black dashes on it.
    let outline = Rect::from_two_pos(screen(&window, [10.0, 0.0]), screen(&window, [30.0, 20.0]));
    let white = window.shapes.iter().any(|clipped| match &clipped.shape {
        egui::Shape::Path(path) => {
            path.stroke.color == egui::epaint::ColorMode::Solid(Color32::WHITE)
                && path.points.first() == Some(&outline.left_top())
                && path.points.len() == 5
        }
        _ => false,
    });
    assert!(white, "the selection's edge was not drawn");
    let dashes = segments(&window, Color32::BLACK);
    assert!(dashes.len() >= 20, "the selection has no marching dashes: {}", dashes.len());
    assert!(dashes.iter().flatten().all(|at| outline.expand(0.5).contains(*at) && !outline.shrink(0.5).contains(*at)), "a dash is off the selection's edge");

    // The crop box: the outside dimmed, and thirds across it.
    let frame = Rect::from_two_pos(screen(&window, [0.0, 0.0]), screen(&window, [20.0, 10.0]));
    let dimmed = window.shapes.iter().filter(|clipped| matches!(&clipped.shape, egui::Shape::Rect(shown) if shown.fill == Color32::from_black_alpha(110))).count();
    assert!(dimmed >= 2, "the outside of the crop box is not dimmed");
    let thirds = segments(&window, Color32::from_white_alpha(190));
    let x = frame.left() + frame.width() / 3.0;
    assert!(thirds.iter().any(|[a, b]| (a.x - x).abs() < 0.01 && (b.x - x).abs() < 0.01), "the crop box has no thirds");

    // The move tool's box: eight handles in blue on the layer's bounds.
    let handles = |window: &Window| -> Vec<Rect> {
        window
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(shown) if shown.stroke.color == Color32::from_rgb(37, 99, 235) && shown.rect.width() < 10.0 => Some(shown.rect),
                _ => None,
            })
            .collect()
    };
    assert!(handles(&window).is_empty(), "the layer's box shows without the move tool");
    use_tool(&mut window, "move", |tool| *tool == RasterTool::Move);
    let shown = handles(&window);
    assert_eq!(shown.len(), 8, "the move tool does not show the layer's handles");
    assert!(shown.iter().any(|handle| (handle.center() - expected.min).length() < 0.01));
    close(window, dir);
}

/// Catches gestures that are not sent, sent more than once, or sent in
/// screen points: each tool's drag or click on the stand-in's picture is
/// one `raster_gesture` in picture pixels, through the camera. The brush
/// sends its points as they come, in order, and then the whole stroke; it
/// draws no trail of its own, so the stroke seen is the picture's.
#[test]
fn each_tool_sends_one_gesture_in_picture_pixels() {
    let (mut window, dir, _stand_in) = easel("gestures");

    // The brush: one paint drag, its points sent as they come.
    let mut trail = 0;
    drag(&mut window, &[[5.0, 5.0], [10.0, 6.0], [15.0, 8.0], [25.0, 12.0]], |window| {
        trail += discs(window, Color32::from_rgb(BRUSH[0], BRUSH[1], BRUSH[2]));
    });
    assert_eq!(trail, 0, "the host drew a trail of its own over the picture");
    window.until(|_| !gestures(&dir).is_empty());
    let sent = gestures(&dir);
    assert_eq!(sent.len(), 1, "one drag sent {} gestures", sent.len());
    assert_eq!(sent[0]["kind"], "stroke");
    let points = sent[0]["points"].as_array().unwrap();
    assert!(points.len() >= 4, "the drag lost its points: {points:?}");
    let mut streamed: Vec<Value> = Vec::new();
    for part in all_gestures(&dir).iter().filter(|gesture| gesture["kind"] == "stroke_part") {
        assert_eq!(part["from"].as_u64(), Some(streamed.len() as u64), "a part of the stroke came out of order");
        streamed.extend(part["points"].as_array().unwrap().iter().cloned());
    }
    assert_eq!(&streamed, points, "the parts sent while drawing are not the stroke laid");
    let first = &points[0];
    let last = points.last().unwrap();
    assert!(near(first[0].as_f64().unwrap(), 5.0) && near(first[1].as_f64().unwrap(), 5.0) && near(first[2].as_f64().unwrap(), 1.0), "{first}");
    assert!(near(last[0].as_f64().unwrap(), 25.0) && near(last[1].as_f64().unwrap(), 12.0), "{last}");

    // The marquee: a rectangle, corner to corner.
    use_tool(&mut window, "marquee", |tool| *tool == RasterTool::Marquee);
    drag(&mut window, &[[12.0, 3.0], [15.0, 6.0], [18.0, 9.0]], |_| {});
    window.until(|_| gestures(&dir).len() >= 2);
    let rect = &gestures(&dir)[1];
    assert_eq!(rect["kind"], "rect");
    assert_eq!((point(&rect["from"]), point(&rect["to"])), ([12.0, 3.0], [18.0, 9.0]));

    // The lasso: the outline drawn.
    use_tool(&mut window, "lasso", |tool| *tool == RasterTool::Lasso);
    drag(&mut window, &[[5.0, 5.0], [10.0, 5.0], [10.0, 10.0], [5.0, 10.0]], |_| {});
    window.until(|_| gestures(&dir).len() >= 3);
    let lasso = &gestures(&dir)[2];
    assert_eq!(lasso["kind"], "lasso");
    let outline: Vec<[f64; 2]> = lasso["points"].as_array().unwrap().iter().map(point).collect();
    assert_eq!(outline, vec![[5.0, 5.0], [10.0, 5.0], [10.0, 10.0], [5.0, 10.0]]);

    // Move: from the press to the release, with the layer's box following.
    use_tool(&mut window, "move", |tool| *tool == RasterTool::Move);
    let corner = screen(&window, [0.0, 0.0]);
    let mut followed = false;
    drag(&mut window, &[[20.0, 10.0], [22.0, 11.0], [24.0, 12.0]], |window| {
        followed = window.shapes.iter().any(|clipped| matches!(&clipped.shape, egui::Shape::Rect(shown) if shown.stroke.color == Color32::from_rgb(37, 99, 235) && (shown.rect.center() - (corner + egui::vec2(40.0, 20.0))).length() < 0.01));
    });
    assert!(followed, "the layer's box did not follow the move");
    window.until(|_| gestures(&dir).len() >= 4);
    let moved = &gestures(&dir)[3];
    assert_eq!(moved["kind"], "drag");
    assert_eq!((point(&moved["from"]), point(&moved["to"])), ([20.0, 10.0], [24.0, 12.0]));

    // Crop: a new box dragged outside the shown one, then the shown box's
    // right edge dragged out, each on whole pixels.
    use_tool(&mut window, "crop", |tool| *tool == RasterTool::Crop);
    drag(&mut window, &[[32.0, 16.0], [20.0, 9.0], [10.2, 4.4]], |_| {});
    window.until(|_| gestures(&dir).len() >= 5);
    let crop = &gestures(&dir)[4];
    assert_eq!(crop["kind"], "crop");
    assert_eq!(crop["rect"], json!([10.0, 4.0, 22.0, 12.0]));
    drag(&mut window, &[[20.0, 5.0], [25.0, 5.0], [30.0, 5.0]], |_| {});
    window.until(|_| gestures(&dir).len() >= 6);
    assert_eq!(gestures(&dir)[5]["rect"], json!([0.0, 0.0, 30.0, 10.0]), "dragging the box's edge did not move it");

    // The eyedropper: a click picks there.
    use_tool(&mut window, "eyedropper", |tool| *tool == RasterTool::Eyedropper);
    let at = screen(&window, [7.0, 8.0]);
    click(&mut window, at);
    window.until(|_| gestures(&dir).len() >= 7);
    let pick = &gestures(&dir)[6];
    assert_eq!((pick["kind"].clone(), point(&pick["at"])), (json!("pick"), [7.0, 8.0]));

    // The hand and the magnifier move the camera and send no gesture.
    use_tool(&mut window, "hand", |tool| *tool == RasterTool::Hand);
    let before = raster(&window).camera.unwrap();
    drag(&mut window, &[[10.0, 10.0], [8.0, 9.0], [6.0, 8.0]], |_| {});
    let after = raster(&window).camera.unwrap();
    assert!(near(after.origin[0], before.origin[0] + 4.0) && near(after.origin[1], before.origin[1] + 2.0), "the hand did not pan: {after:?}");
    use_tool(&mut window, "zoom", |tool| *tool == RasterTool::Zoom);
    let at = screen(&window, [20.0, 10.0]);
    click(&mut window, at);
    assert!(near(raster(&window).camera.unwrap().zoom, 2.0 * ZOOM), "the magnifier did not zoom in");
    window.until(|_| of_type(&dir, "camera").iter().any(|camera| camera["zoom"] == json!(2.0 * ZOOM)));
    assert_eq!(gestures(&dir).len(), 7, "the hand or the magnifier sent a gesture");
    close(window, dir);
}

/// Catches toolbar zoom commands that act only on a drawing: Fit frames the
/// picture, `canvas.zoom` sets a zoom by number, and each change is reported.
#[test]
fn the_toolbar_zooms_and_fits_a_raster_tab() {
    let (mut window, dir, _stand_in) = easel("zoom");
    window.app.run_action(&Action::with_arg("canvas.zoom", json!({"zoom": 1})));
    window.step();
    assert!(near(raster(&window).camera.unwrap().zoom, 1.0));
    window.until(|_| of_type(&dir, "camera").iter().any(|camera| camera["zoom"] == json!(1.0)));
    window.app.run_action(&Action::new("canvas.fit"));
    window.step();
    let rect = window.app.raster.shown;
    let fitted = raster(&window).camera.unwrap();
    let zoom = ((f64::from(rect.width()) - 80.0) / f64::from(W)).min((f64::from(rect.height()) - 80.0) / f64::from(H));
    assert!(near(fitted.zoom, zoom), "Fit chose {} for {zoom}", fitted.zoom);
    let middle = screen(&window, [f64::from(W) / 2.0, f64::from(H) / 2.0]);
    assert!((middle - rect.center()).length() < 0.01, "Fit did not center the picture");
    window.app.run_action(&Action::new("canvas.zoom_in"));
    assert!(near(raster(&window).camera.unwrap().zoom, zoom * 1.25));
    let log = dir.clone();
    window.until(move |_| of_type(&log, "camera").iter().any(|camera| near(camera["zoom"].as_f64().unwrap(), zoom * 1.25)));
    close(window, dir);
}

/// Catches settings-row items and panel pieces that draw and do nothing:
/// each one the stand-in declares sends its action with its value, and one
/// that waits sends nothing.
#[test]
fn the_settings_row_and_the_panel_send_their_actions() {
    let (mut window, dir, _stand_in) = easel("controls");

    // The colour chip opens Studio's swatches; one is chosen.
    let chip = window
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Circle(dot) if dot.fill == Color32::from_rgb(CHIP[0], CHIP[1], CHIP[2]) => Some(dot.center),
            _ => None,
        })
        .expect("the colour chip was not drawn");
    click(&mut window, chip);
    settle(&mut window);
    let red = draw::defaults::PALETTE[1].rgb;
    let swatch = window
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Circle(dot) if dot.fill == Color32::from_rgb(red[0], red[1], red[2]) => Some(dot.center),
            _ => None,
        })
        .expect("the chip opened no swatches");
    click(&mut window, swatch);
    window.until(|_| !commands(&dir, "set_color").is_empty());
    assert_eq!(commands(&dir, "set_color")[0]["arg"]["value"], json!(red));

    // The slider, with its unit, and the switch.
    let value = text_at(&window, "24");
    let unit = text_at(&window, " px");
    assert!(unit.left() > value.right() && unit.left() - value.right() < 8.0, "the slider's value has no unit after it");
    let size = text_at(&window, "Size");
    click(&mut window, pos2(size.right() + 30.0, size.center().y));
    window.until(|_| !commands(&dir, "set_size").is_empty() && !commands(&dir, "finish_size").is_empty());
    let at = text_at(&window, "Pen pressure").center();
    click(&mut window, at);
    window.until(|_| !commands(&dir, "toggle_pressure").is_empty());

    // The selection tools' row: a segment, a typed number, and a button.
    use_tool(&mut window, "marquee", |tool| *tool == RasterTool::Marquee);
    let at = text_at(&window, "Add").center();
    click(&mut window, at);
    window.until(|_| !commands(&dir, "set_mode").is_empty());
    assert_eq!(commands(&dir, "set_mode")[0]["arg"]["value"], json!(1));
    let at = text_at(&window, "0 px").center();
    click(&mut window, at);
    window.step();
    window.events.push(egui::Event::Text(String::from("4")));
    window.step();
    window.events.push(egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    window.step();
    window.until(|_| !commands(&dir, "set_feather").is_empty());
    assert_eq!(commands(&dir, "set_feather")[0]["arg"]["value"].as_f64(), Some(4.0));
    let at = text_at(&window, "Deselect").center();
    click(&mut window, at);
    window.until(|_| !commands(&dir, "deselect").is_empty());

    // The crop row: a dropdown, a strong label, and Cancel and Apply at the
    // right, beside a button that waits.
    use_tool(&mut window, "crop", |tool| *tool == RasterTool::Crop);
    assert!(window.texts.iter().any(|(text, _)| text == "3200 × 1800"));
    let apply = text_at(&window, "Apply");
    assert!(apply.right() < window.app.raster.shown.right(), "the row's right group is not above the contents");
    let at = text_at(&window, "Free").center();
    click(&mut window, at);
    settle(&mut window);
    let at = text_at(&window, "16:9").center();
    click(&mut window, at);
    window.until(|_| !commands(&dir, "set_ratio").is_empty());
    assert_eq!(commands(&dir, "set_ratio")[0]["arg"]["value"], json!(2));
    let at = text_at(&window, "Later").center();
    click(&mut window, at);
    click(&mut window, apply.center());
    window.until(|_| !commands(&dir, "apply_crop").is_empty());
    assert!(commands(&dir, "later").is_empty(), "a button that waits was sent");

    // The panel: page tabs, the lock icons, the heading's buttons, and the
    // layer's picture.
    let at = text_at(&window, "Properties").center();
    click(&mut window, at);
    let at = text_at(&window, "Adjustments").center();
    click(&mut window, at);
    window.until(|_| !commands(&dir, "page_adjustments").is_empty());
    assert!(commands(&dir, "page_properties").is_empty(), "a page that waits was sent");
    let lock = text_at(&window, "Lock:");
    let s = crate::widgets::Scale::PANEL;
    let third = pos2(lock.right() + s.of(14.0) + 2.0 * s.of(40.0) + s.of(17.0), lock.center().y);
    click(&mut window, third);
    window.until(|_| !of_type(&dir, "property").is_empty());
    let property = &of_type(&dir, "property")[0];
    assert_eq!((property["prop"].clone(), property["value"].clone()), (json!("lock"), json!({"index": 2})));
    let heading = window
        .texts
        .iter()
        .filter(|(text, _)| text == "Layers")
        .map(|(_, rect)| *rect)
        .max_by(|a, b| a.top().total_cmp(&b.top()))
        .unwrap();
    let panel = Rect::from_min_max(pos2(window.app.raster.shown.right(), 0.0), pos2(1280.0, 800.0));
    let right = panel.right() - 16.0;
    click(&mut window, pos2(right - 17.0, heading.center().y));
    click(&mut window, pos2(right - 3.0 * 34.0 + 17.0, heading.center().y));
    window.until(|_| !commands(&dir, "delete_layer").is_empty() && !commands(&dir, "add_layer").is_empty());
    assert!(commands(&dir, "group_layers").is_empty());
    let thumb = format!("raster:{}", dir.join("thumb.rgba").display());
    let drawn = textured(&window, &thumb);
    assert!(drawn.iter().any(|at| panel.contains_rect(*at) && at.width() > 20.0), "the layer's picture was not drawn: {drawn:?}");
    close(window, dir);
}

/// A drag in the window from one screen point to another, through a few
/// points between, so it is a drag and not a click.
fn drag_on_screen(window: &mut Window, from: Pos2, to: Pos2) {
    window.events.push(egui::Event::PointerMoved(from));
    window.step();
    press(window, from, true);
    window.step();
    for step in 1..=6 {
        window.events.push(egui::Event::PointerMoved(from + (to - from) * (step as f32 / 6.0)));
        window.step();
    }
    window.step();
    press(window, to, false);
    window.step();
}

/// Catches a Layers list that cannot put a layer into a group or take one
/// out: a row dropped on the middle of a group's row is sent to the top of
/// that group, a group's layer dropped between two top-level rows is sent
/// to the top level, between them, and one dropped below the list to the
/// bottom of the top level.
#[test]
fn layers_are_dragged_into_and_out_of_groups() {
    let (mut window, dir, _stand_in) = easel_with("nest", |dir| {
        vec![
            layer_row(dir, 2, None, 0, "Top layer", false),
            layer_row(dir, 10, None, 0, "Folder", true),
            layer_row(dir, 11, Some(10), 1, "Inner layer", false),
            layer_row(dir, 1, None, 0, "Background", false),
        ]
    });
    let folder = text_at(&window, "Folder").center();
    let from = text_at(&window, "Background").center();
    drag_on_screen(&mut window, from, folder);
    window.until(|_| !commands(&dir, "reorder").is_empty());
    assert_eq!(commands(&dir, "reorder")[0]["arg"], json!({ "node": 1, "value": 0, "parent": 10 }), "the drop on the group's middle did not put the layer in it");

    // Just under the top of the group's row is between it and the row above.
    let from = text_at(&window, "Inner layer").center();
    drag_on_screen(&mut window, from, pos2(folder.x, folder.y - 52.0 / 2.0 + 4.0));
    window.until(|_| commands(&dir, "reorder").len() > 1);
    assert_eq!(commands(&dir, "reorder")[1]["arg"], json!({ "node": 11, "value": 1, "parent": null }), "the drop between top-level rows did not take the layer out of its group");

    // Below the last row is the bottom of the top level.
    let below = text_at(&window, "Background").center() + egui::vec2(0.0, 52.0 / 2.0 + 6.0);
    let from = text_at(&window, "Inner layer").center();
    drag_on_screen(&mut window, from, below);
    window.until(|_| commands(&dir, "reorder").len() > 2);
    assert_eq!(commands(&dir, "reorder")[2]["arg"], json!({ "node": 11, "value": 3, "parent": null }), "the drop below the list did not reach the bottom");
    close(window, dir);
}

// The filmstrip.

/// Where each of the film's thumbnails was drawn, in order.
fn thumbnails(window: &Window, dir: &Path) -> Vec<Rect> {
    (0..PICTURES.len())
        .map(|index| {
            let name = format!("raster:{}", dir.join(format!("film{index}.rgba")).display());
            textured(window, &name).into_iter().next_back().unwrap_or_else(|| panic!("thumbnail {index} was not drawn"))
        })
        .collect()
}

/// Rectangles drawn with an edge of this colour.
fn edged(window: &Window, color: Color32) -> Vec<Rect> {
    window
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(shown) if shown.stroke.color == color && shown.stroke.width > 0.0 => Some(shown.rect),
            _ => None,
        })
        .collect()
}

/// The ids of the commands sent, in order.
fn command_ids(dir: &Path) -> Vec<String> {
    of_type(dir, "command").iter().filter_map(|line| line["id"].as_str().map(String::from)).collect()
}

fn press_with(window: &mut Window, at: Pos2, button: egui::PointerButton, pressed: bool) {
    window.events.push(egui::Event::PointerButton { pos: at, button, pressed, modifiers: Default::default() });
}

const BLUE: Color32 = Color32::from_rgb(37, 99, 235);

/// Catches a filmstrip that is not drawn, is drawn over the picture, or
/// does not show which picture is shown: a white strip under a hairline
/// along the bottom of the working area, each thumbnail its own picture 44
/// points tall at its proportions, in order, the shown one outlined in blue
/// and the others not, and a bordered + tile after them. The picture's area
/// ends where the strip begins, the host's first camera and Fit frame the
/// picture there, and commands report that area as their viewport.
#[test]
fn the_filmstrip_is_drawn_under_the_picture_with_the_shown_one_outlined() {
    let (mut window, dir, _stand_in) = filmed("film-drawn");
    let area = window.app.raster.shown;
    let strip = window
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(shown) if shown.fill == Color32::WHITE && (shown.rect.top() - area.bottom()).abs() < 0.01 && (shown.rect.width() - area.width()).abs() < 0.01 => Some(shown.rect),
            _ => None,
        })
        .expect("no white strip under the picture's area");
    assert!((strip.height() - 68.0).abs() < 0.01, "the strip is {} points tall", strip.height());
    let hairline = window.shapes.iter().any(|clipped| matches!(&clipped.shape, egui::Shape::LineSegment { points, stroke } if stroke.color == crate::widgets::BORDER && (points[0].y - strip.top()).abs() < 1.0 && (points[1].x - points[0].x).abs() >= strip.width() - 0.5));
    assert!(hairline, "the strip has no hairline above it");

    let thumbs = thumbnails(&window, &dir);
    for (index, thumb) in thumbs.iter().enumerate() {
        let (_, width, height) = PICTURES[index];
        assert!(strip.contains_rect(*thumb), "thumbnail {index} is off the strip: {thumb:?}");
        assert!((thumb.height() - 44.0).abs() < 0.5, "thumbnail {index} is {} tall", thumb.height());
        let proportions = (thumb.width() / thumb.height(), width as f32 / height as f32);
        assert!((proportions.0 - proportions.1).abs() < 0.05, "thumbnail {index} is not at its picture's proportions: {proportions:?}");
        if index > 0 {
            assert!(thumb.left() > thumbs[index - 1].right(), "the thumbnails are out of order");
        }
    }
    let blue = edged(&window, BLUE);
    let outlined = |thumb: &Rect| blue.iter().any(|edge| edge.contains_rect(*thumb) && edge.width() - thumb.width() <= 6.5);
    assert!(outlined(&thumbs[1]), "the shown picture is not outlined in blue");
    assert!(!outlined(&thumbs[0]) && !outlined(&thumbs[2]), "a picture not shown is outlined in blue");
    let add = edged(&window, crate::widgets::BORDER)
        .into_iter()
        .find(|tile| strip.contains_rect(*tile) && tile.left() > thumbs[2].right() && (tile.height() - 44.0).abs() < 0.5)
        .expect("no bordered + tile after the thumbnails");
    assert!(add.left() - thumbs[2].right() < 20.0, "the + tile is not at the end of the thumbnails");

    // The first camera and Fit frame the picture above the strip.
    let name = format!("raster:{}", dir.join("picture.rgba").display());
    let fitted = |window: &Window| {
        let picture = textured(window, &name).into_iter().next_back().expect("the picture was not drawn");
        let rect = window.app.raster.shown;
        assert!(picture.bottom() <= strip.top() - 39.0, "the picture reaches the strip: {picture:?}, strip at {}", strip.top());
        assert!((picture.center() - rect.center()).length() < 0.5, "the picture is not centered above the strip");
        let zoom = ((f64::from(rect.width()) - 80.0) / f64::from(W)).min((f64::from(rect.height()) - 80.0) / f64::from(H));
        assert!(near(raster(window).camera.unwrap().zoom, zoom), "the camera was not fitted to the area above the strip");
    };
    fitted(&window);
    window.app.run_action(&Action::with_arg("canvas.zoom", json!({"zoom": 1})));
    window.step();
    window.app.run_action(&Action::new("canvas.fit"));
    window.step();
    fitted(&window);
    window.app.run_action(&Action::new("deselect"));
    window.until(|_| !commands(&dir, "deselect").is_empty());
    let viewport = &commands(&dir, "deselect")[0]["viewport"];
    assert_eq!(viewport, &json!([area.width(), area.height()]), "a command's viewport is not the area above the strip");
    close(window, dir);
}

/// Catches a filmstrip that draws and does nothing, or sends the wrong
/// thing: a click on a thumbnail sends `focus` and then `select` with its
/// index, and paints nothing on the picture; a right-click opens that
/// thumbnail's rows above it, on the menus' card, and a row sends its own
/// action; the + tile opens its rows above it; a thumbnail dragged along
/// the strip shows where it will land and sends `reorder` with `to` counted
/// after it is taken out; a drag that ends where it began sends nothing;
/// and resting on a thumbnail shows its name.
#[test]
fn the_filmstrip_sends_focus_select_its_menus_and_reorder() {
    let (mut window, dir, _stand_in) = filmed("film-actions");
    let thumbs = thumbnails(&window, &dir);

    // A click: focus, then select.
    click(&mut window, thumbs[2].center());
    window.until(|_| !commands(&dir, "show_item").is_empty());
    let ids = command_ids(&dir);
    let focus = ids.iter().position(|id| id == "strip_focus").expect("the press sent no focus");
    let select = ids.iter().position(|id| id == "show_item").unwrap();
    assert!(focus < select, "focus was not sent before select: {ids:?}");
    assert_eq!(commands(&dir, "show_item")[0]["arg"], json!({ "value": 2 }));
    assert_eq!(commands(&dir, "show_item")[0]["tab"], json!("r1"));
    assert!(gestures(&dir).is_empty(), "a click on the strip painted the picture");

    // Resting on a thumbnail shows its name. A pointer moved just after a
    // click shows none, so the move waits a moment as a person's would.
    settle(&mut window);
    window.events.push(egui::Event::PointerMoved(thumbs[0].center()));
    for _ in 0..50 {
        window.step();
    }
    assert!(window.texts.iter().any(|(text, _)| text == "Harbor"), "resting on a thumbnail does not show its name");

    // A right-click opens the thumbnail's rows above it.
    let at = thumbs[0].center();
    window.events.push(egui::Event::PointerMoved(at));
    window.step();
    press_with(&mut window, at, egui::PointerButton::Secondary, true);
    window.step();
    press_with(&mut window, at, egui::PointerButton::Secondary, false);
    window.step();
    settle(&mut window);
    let rename = text_at(&window, "Rename");
    assert!(text_at(&window, "Duplicate").bottom() < thumbs[0].top() && text_at(&window, "Delete").bottom() < thumbs[0].top(), "the thumbnail's rows are not above it");
    assert!((text_at(&window, "Duplicate").left() - thumbs[0].left()).abs() < 40.0, "the thumbnail's rows are not over it");
    assert_eq!(command_ids(&dir).iter().filter(|id| *id == "strip_focus").count(), 2, "a right-click did not send focus");
    click(&mut window, rename.center());
    window.until(|_| !commands(&dir, "rename_item").is_empty());
    assert_eq!(commands(&dir, "rename_item")[0]["arg"], json!({ "item": 0 }));
    assert!(commands(&dir, "copy_item").is_empty() && commands(&dir, "remove_item").is_empty());
    assert_eq!(commands(&dir, "show_item").len(), 1, "the right-click or its row selected a picture");

    // The + tile opens its rows above it.
    let add = edged(&window, crate::widgets::BORDER)
        .into_iter()
        .find(|tile| tile.left() > thumbs[2].right() && (tile.height() - 44.0).abs() < 0.5)
        .expect("no + tile");
    click(&mut window, add.center());
    settle(&mut window);
    let new = text_at(&window, "New picture");
    let open = text_at(&window, "Open PNG or JPEG…");
    assert!(new.bottom() < add.top() && open.bottom() < add.top(), "the + tile's rows are not above it");
    click(&mut window, open.center());
    window.until(|_| !commands(&dir, "open_item").is_empty());
    assert!(commands(&dir, "new_item").is_empty());

    // A drag past the last thumbnail lands last; one before the first lands
    // first; each shows a blue line where it will land while it is held.
    let line = |window: &Window| segments(window, BLUE).into_iter().filter(|[a, b]| (a.x - b.x).abs() < 0.01 && (a.y - b.y).abs() > 40.0).map(|[a, _]| a.x).next();
    window.events.push(egui::Event::PointerMoved(thumbs[0].center()));
    window.step();
    press(&mut window, thumbs[0].center(), true);
    window.step();
    for step in 1..=8 {
        let to = thumbs[2].right() - 4.0;
        let x = thumbs[0].center().x + (to - thumbs[0].center().x) * step as f32 / 8.0;
        window.events.push(egui::Event::PointerMoved(pos2(x, thumbs[0].center().y)));
        window.step();
    }
    let shown_at = line(&window);
    press(&mut window, pos2(thumbs[2].right() - 4.0, thumbs[0].center().y), false);
    window.step();
    let landed = shown_at.expect("the drag showed no line where it would land");
    assert!(landed > thumbs[2].right() && landed < add.left(), "the line is not after the last thumbnail: {landed}");
    window.until(|_| !commands(&dir, "order_items").is_empty());
    assert_eq!(commands(&dir, "order_items")[0]["arg"], json!({ "strip": true, "from": 0, "to": 2 }));
    drag_on_screen(&mut window, thumbs[2].center(), pos2(thumbs[0].left() + 4.0, thumbs[0].center().y));
    window.until(|_| commands(&dir, "order_items").len() > 1);
    assert_eq!(commands(&dir, "order_items")[1]["arg"], json!({ "strip": true, "from": 2, "to": 0 }));
    drag_on_screen(&mut window, thumbs[1].center(), thumbs[1].center() + egui::vec2(8.0, 0.0));
    for _ in 0..10 {
        window.step();
    }
    assert_eq!(commands(&dir, "order_items").len(), 2, "a drag that ended where it began was sent");
    assert_eq!(commands(&dir, "show_item").len(), 1, "a drag selected a picture");
    assert!(gestures(&dir).is_empty(), "the strip's pointer reached the picture");
    close(window, dir);
}
