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
    window.until(move |app| app.apps[0].running.declared.rasters.values().next().is_some_and(|display| display.view.revision > before));
    let painted = pixel(&window, x, y);
    assert!(painted[0] < 128 && painted[1] < 128 && painted[2] < 128, "the drag did not paint the picture: {painted:?}");
    assert_eq!(pixel(&window, x, 100), [255, 255, 255, 255], "the drag painted far from where it went");

    let before = revision(&window);
    window.app.run_action(&Action::new("undo"));
    window.until(move |app| app.apps[0].running.declared.rasters.values().next().is_some_and(|display| display.view.revision > before));
    assert_eq!(pixel(&window, x, y), [255, 255, 255, 255], "Undo did not restore the pixels");
    let before = revision(&window);
    window.app.run_action(&Action::new("redo"));
    window.until(move |app| app.apps[0].running.declared.rasters.values().next().is_some_and(|display| display.view.revision > before));
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
