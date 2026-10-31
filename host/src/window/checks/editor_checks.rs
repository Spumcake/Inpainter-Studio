//! The whole loop with the real Image Editor: the window draws its picture,
//! a brush drag reaches its pixels, Undo and Redo reverse and repeat it, and
//! the picture comes back from the project after a restart.

use super::{StandIn, Window, built, keep_ticking, project_window, scratch, spec};
use eframe::egui::{self, Pos2, pos2};
use protocol::{Action, RasterTool};
use serde_json::json;

fn display(window: &Window) -> Option<&crate::process::RasterDisplay> {
    window.app.apps.first()?.running.declared.rasters.values().next()
}

/// One pixel of the picture file the host reads.
fn pixel(window: &Window, x: u32, y: u32) -> [u8; 4] {
    let view = &display(window).expect("no raster tab").view;
    let bytes = std::fs::read(&view.picture).expect("the picture file is missing");
    assert_eq!(bytes.len(), (view.width * view.height * 4) as usize);
    let at = ((y * view.width + x) * 4) as usize;
    [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]
}

fn revision(window: &Window) -> u64 {
    display(window).map_or(0, |display| display.view.revision)
}

fn screen(window: &Window, point: [f64; 2]) -> Pos2 {
    let rect = window.app.raster.shown;
    let camera = display(window).and_then(|display| display.camera).expect("the tab has a camera");
    pos2(
        ((point[0] - camera.origin[0]) * camera.zoom) as f32 + rect.min.x,
        ((point[1] - camera.origin[1]) * camera.zoom) as f32 + rect.min.y,
    )
}

/// The stroke is painted as it is drawn, so the picture changes more than
/// once: waits for a newer picture, then for the stroke to reach `x`, `y`.
fn until_painted(window: &mut Window, before: u64, x: u32, y: u32) {
    window.until(move |app| app.apps[0].running.declared.rasters.values().next().is_some_and(|display| display.view.revision > before));
    until_pixel(window, x, y, |pixel| pixel[0] < 128);
}

/// Waits, ten seconds at most, for the pixel at `x`, `y` to be as `wanted`
/// says: the pictures of a stroke still being painted may come first.
fn until_pixel(window: &mut Window, x: u32, y: u32, wanted: impl Fn([u8; 4]) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !wanted(pixel(window, x, y)) && std::time::Instant::now() < deadline {
        window.step();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn editor(dir: &std::path::Path, stand_in: &StandIn) -> Window {
    let mut window = project_window(vec![spec("image-editor", "Image Editor", &built("image-editor"))], stand_in, dir);
    if !window.app.workspace_shown() {
        window.app.toggle_workspace();
    }
    window.until(|app| app.apps.first().is_some_and(|started| !started.running.declared.rasters.is_empty()));
    for _ in 0..3 {
        window.step();
    }
    window
}

/// Catches an editor that only looks like one: the real Image Editor's new
/// picture is white; a brush drag across it, through the host's camera,
/// darkens the pixels under it; Undo restores them exactly; Redo paints them
/// again; and after a restart the project gives the painted picture back.
#[test]
fn a_brush_drag_paints_undoes_and_comes_back_from_the_project() {
    let dir = scratch("editor-loop");
    let stand_in = StandIn::start(Vec::new());
    let mut window = editor(&dir, &stand_in);
    let view = display(&window).unwrap().view.clone();
    assert_eq!((view.width, view.height), (1920, 1080));
    let (x, y) = (960, 540);
    assert_eq!(pixel(&window, x, y), [255, 255, 255, 255], "a new picture is not white");

    window.app.run_action(&Action::with_arg("tool", json!({ "tool": "brush" })));
    window.until(|app| matches!(app.apps[0].running.declared.rasters.values().next().map(|display| &display.view.tool), Some(RasterTool::Brush { .. })));
    window.step();
    let before = revision(&window);
    let points = [[860.0, 540.0], [920.0, 540.0], [1000.0, 540.0], [1060.0, 540.0]];
    let screens: Vec<Pos2> = points.iter().map(|point| screen(&window, *point)).collect();
    window.events.push(egui::Event::PointerMoved(screens[0]));
    window.step();
    window.events.push(egui::Event::PointerButton { pos: screens[0], button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    window.step();
    for at in &screens[1..] {
        window.events.push(egui::Event::PointerMoved(*at));
        window.step();
    }
    window.events.push(egui::Event::PointerButton { pos: *screens.last().unwrap(), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    window.step();
    until_painted(&mut window, before, x, y);
    let painted = pixel(&window, x, y);
    assert!(painted[0] < 128 && painted[1] < 128 && painted[2] < 128, "the drag did not paint the picture: {painted:?}");
    assert_eq!(pixel(&window, x, 100), [255, 255, 255, 255], "the drag painted far from where it went");

    window.app.run_action(&Action::new("undo"));
    until_pixel(&mut window, x, y, |pixel| pixel == [255, 255, 255, 255]);
    assert_eq!(pixel(&window, x, y), [255, 255, 255, 255], "Undo did not restore the pixels");
    window.app.run_action(&Action::new("redo"));
    until_pixel(&mut window, x, y, |pixel| pixel == painted);
    assert_eq!(pixel(&window, x, y), painted, "Redo did not paint them again");

    // The picture keeps itself in the project.
    keep_ticking(&mut window);
    keep_ticking(&mut window);
    window.app.write_project(true);
    keep_ticking(&mut window);
    let folder = window.app.project.as_ref().unwrap().folder.clone();
    let kept = std::fs::read_dir(&folder).unwrap().flatten().any(|entry| entry.path().extension().is_some_and(|ext| ext == "ipx"));
    assert!(kept, "no picture file was kept in the project");
    drop(window);

    let stand_in = StandIn::start(Vec::new());
    let mut window = editor(&dir, &stand_in);
    window.until(move |app| {
        app.apps[0].running.declared.rasters.values().next().is_some_and(|display| {
            std::fs::read(&display.view.picture).is_ok_and(|bytes| {
                let at = ((y * display.view.width + x) * 4) as usize;
                bytes.len() == (display.view.width * display.view.height * 4) as usize && bytes[at..at + 4] == painted
            })
        })
    });
    let _ = std::fs::remove_dir_all(dir);
}

fn film(window: &Window) -> protocol::Film {
    display(window).and_then(|display| display.view.film.clone()).expect("the tab has no filmstrip")
}

/// The thumbnails drawn on the strip, left to right.
fn thumbnails(window: &Window) -> Vec<egui::Rect> {
    let mut thumbs: Vec<egui::Rect> = window.pictures.iter().copied().filter(|rect| rect.height() < 50.0 && rect.top() > 600.0).collect();
    thumbs.sort_by(|a, b| a.left().total_cmp(&b.left()));
    thumbs
}

fn press(window: &mut Window, at: Pos2, button: egui::PointerButton) {
    window.events.push(egui::Event::PointerMoved(at));
    window.step();
    for pressed in [true, false] {
        window.events.push(egui::Event::PointerButton { pos: at, button, pressed, modifiers: Default::default() });
        window.step();
    }
}

/// Catches a gallery that only looks like one: with the real Image Editor,
/// the + adds a picture that the strip and the canvas show; painting it
/// leaves the first picture white; its thumbnail brings the first back;
/// Rename from its menu renames it in place; and the gallery returns after a
/// restart.
#[test]
fn a_gallery_adds_shows_renames_and_comes_back() {
    let dir = scratch("editor-gallery");
    let stand_in = StandIn::start(Vec::new());
    let mut window = editor(&dir, &stand_in);
    let first = film(&window);
    assert_eq!(first.items.len(), 1);
    let add = first.add[0].action.clone().expect("the + has a New picture row");
    window.app.run_action(&add);
    window.until(|app| app.apps[0].running.declared.rasters.values().next().and_then(|display| display.view.film.as_ref()).is_some_and(|film| film.items.len() == 2 && film.shown == 1));
    for _ in 0..3 {
        window.step();
    }

    window.app.run_action(&Action::with_arg("tool", json!({ "tool": "brush" })));
    window.until(|app| matches!(app.apps[0].running.declared.rasters.values().next().map(|display| &display.view.tool), Some(RasterTool::Brush { .. })));
    window.step();
    let before = revision(&window);
    let screens: Vec<Pos2> = [[860.0, 540.0], [960.0, 540.0], [1060.0, 540.0]].iter().map(|point| screen(&window, *point)).collect();
    window.events.push(egui::Event::PointerMoved(screens[0]));
    window.step();
    window.events.push(egui::Event::PointerButton { pos: screens[0], button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    window.step();
    for at in &screens[1..] {
        window.events.push(egui::Event::PointerMoved(*at));
        window.step();
    }
    window.events.push(egui::Event::PointerButton { pos: screens[2], button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    window.step();
    until_painted(&mut window, before, 960, 540);
    assert!(pixel(&window, 960, 540)[0] < 128, "the new picture was not painted");

    // Its thumbnail brings the first picture back, still white.
    for _ in 0..3 {
        window.step();
    }
    let thumbs = thumbnails(&window);
    assert_eq!(thumbs.len(), 2, "the strip does not show both pictures");
    press(&mut window, thumbs[0].center(), egui::PointerButton::Primary);
    window.until(|app| app.apps[0].running.declared.rasters.values().next().and_then(|display| display.view.film.as_ref()).is_some_and(|film| film.shown == 0));
    window.until(|app| {
        app.apps[0].running.declared.rasters.values().next().is_some_and(|display| {
            std::fs::read(&display.view.picture).is_ok_and(|bytes| {
                let at = ((540 * display.view.width + 960) * 4) as usize;
                bytes.len() == (display.view.width * display.view.height * 4) as usize && bytes[at] == 255
            })
        })
    });

    // Rename, from the thumbnail's menu, in place.
    for _ in 0..3 {
        window.step();
    }
    let thumbs = thumbnails(&window);
    press(&mut window, thumbs[0].center(), egui::PointerButton::Secondary);
    for _ in 0..6 {
        window.step();
    }
    let rename = window.texts.iter().find(|(text, _)| text == "Rename").map(|(_, at)| at.center()).expect("the menu has no Rename");
    press(&mut window, rename, egui::PointerButton::Primary);
    for _ in 0..3 {
        window.step();
    }
    window.events.push(egui::Event::Key { key: egui::Key::A, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND });
    window.step();
    window.events.push(egui::Event::Text(String::from("Beach")));
    window.step();
    window.events.push(egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    window.step();
    window.until(|app| app.apps[0].running.declared.rasters.values().next().and_then(|display| display.view.film.as_ref()).is_some_and(|film| film.items[0].name == "Beach"));

    keep_ticking(&mut window);
    keep_ticking(&mut window);
    window.app.write_project(true);
    keep_ticking(&mut window);
    drop(window);
    let stand_in = StandIn::start(Vec::new());
    let window = editor(&dir, &stand_in);
    let back = film(&window);
    assert_eq!(back.items.len(), 2, "the gallery did not come back");
    assert_eq!(back.items[0].name, "Beach");
    let _ = std::fs::remove_dir_all(dir);
}
