//! Window checks for Image cells: the assistant's image skill makes a
//! picture through the stand-in's image API, the cell draws it, the project
//! keeps it, and the filmstrip moves between versions. They spend nothing.

use super::{STAND_IN_PNG, StandIn, Window, both, call_reply, keep_ticking, offered, project_window, scratch, text_reply};
use crate::cells::CardAction;
use eframe::egui::{self, Pos2};
use protocol::{CellPart, CellState};
use serde_json::json;

fn image_cell(window: &Window) -> protocol::Cell {
    window
        .app
        .chat
        .conversation
        .iter()
        .find_map(|entry| entry.cell.clone().filter(|cell| cell.kind == protocol::IMAGE_KIND))
        .expect("the conversation has no Image cell")
}

fn picture_of(cell: &protocol::Cell) -> String {
    cell.output
        .iter()
        .find_map(|part| match part {
            CellPart::Path { path, media_type } if media_type == "image/png" => Some(path.clone()),
            _ => None,
        })
        .expect("the cell has no picture")
}

fn click(window: &mut Window, at: Pos2) {
    window.events.push(egui::Event::PointerMoved(at));
    for pressed in [true, false] {
        window.events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
        window.step();
    }
}

/// Catches an image skill that is only a row, and a picture that is only a
/// path: asked for a picture, the assistant calls the skill; the stand-in
/// is asked for it at high quality on the skill's model; the cell draws the
/// picture, kept in the project under a relative path; the assistant sees
/// exactly that picture; Run again adds a version and a file; and a click on
/// the filmstrip shows the first version again.
#[test]
fn a_picture_is_made_kept_and_shown() {
    let dir = scratch("image-cell");
    let tool = protocol::tool_key(crate::route::HOST_ID, "skills", "image_generation");
    let stand_in = StandIn::start(vec![call_reply(&tool, json!({"prompt": "A girl in a straw hat on a beach at sunset."})), text_reply("Done.")]);
    let mut window = project_window(both(), &stand_in, &dir);
    assert!(window.app.settings.add_image_skill());
    assert!(!window.app.settings.add_image_skill(), "the image skill was added twice");
    window.step();
    window.ask("Make an image of a girl on a beach.");
    window.until(|app| {
        app.chat.conversation.iter().any(|entry| entry.cell.as_ref().is_some_and(|cell| cell.kind == protocol::IMAGE_KIND && cell.state == CellState::Done))
            && !app.chat.working
    });
    assert!(offered(&stand_in.requests()[0]).contains(&tool), "the image skill was not offered");
    let asked = stand_in.pictures();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["model"], crate::workspace::IMAGE_MODEL);
    assert_eq!(asked[0]["quality"], "high");
    assert_eq!(asked[0]["prompt"], "A girl in a straw hat on a beach at sunset.");

    let cell = image_cell(&window);
    assert_eq!(cell.prompt(), "A girl in a straw hat on a beach at sunset.");
    let first = picture_of(&cell);
    assert!(first.starts_with("images/"), "the cell names its picture outside the project: {first}");
    let folder = window.app.project.as_ref().unwrap().folder.clone();
    assert!(folder.join(&first).is_file(), "the picture is not in the project");
    window.until(|_| stand_in.requests().len() >= 2);
    assert!(stand_in.requests()[1].to_string().contains(STAND_IN_PNG), "the assistant did not see the picture");

    // The picture is drawn, centred in the card, and its path is not shown.
    window.step();
    window.step();
    let card = window.app.board.card_rect.expect("the cell was not drawn");
    let drawn: Vec<egui::Rect> = window.pictures.iter().copied().filter(|rect| card.contains_rect(*rect)).collect();
    assert!(drawn.iter().any(|rect| rect.height() > 200.0 && (rect.center().x - card.center().x).abs() < 2.0), "no picture was drawn: {drawn:?}");
    assert!(window.texts.iter().all(|(text, _)| !text.contains("images/")), "the picture's path was drawn as text");

    // Run again: a second version, and a second file beside the first.
    stand_in.script(vec![text_reply("Done.")]);
    window.app.note_cell_actions(vec![CardAction::Run(cell.id.clone())]);
    let id = cell.id.clone();
    window.until(move |app| app.board.running(&id).is_none() && app.chat.conversation.iter().any(|entry| entry.cell.as_ref().is_some_and(|cell| cell.versions.len() == 2)));
    let cell = image_cell(&window);
    let second = picture_of(&cell);
    assert_ne!(first, second);
    assert!(folder.join(&first).is_file() && folder.join(&second).is_file());

    // The filmstrip: two thumbnails; a click on the first shows it.
    window.step();
    window.step();
    let card = window.app.board.card_rect.expect("the cell was not drawn");
    let mut thumbs: Vec<egui::Rect> = window.pictures.iter().copied().filter(|rect| card.contains_rect(*rect) && rect.height() < 60.0).collect();
    thumbs.sort_by(|a, b| a.left().total_cmp(&b.left()));
    assert_eq!(thumbs.len(), 2, "the filmstrip does not show both versions");
    click(&mut window, thumbs[0].center());
    let cell = image_cell(&window);
    assert_eq!(cell.current, Some(0));
    assert_eq!(picture_of(&cell), first);

    // A Run again that fails, here for want of a key, keeps the earlier
    // pictures in reach on the filmstrip.
    let endpoint = window.app.settings.endpoints[0].id.clone();
    window.app.keys.delete(&endpoint);
    window.app.note_cell_actions(vec![CardAction::Run(cell.id.clone())]);
    let id = cell.id.clone();
    window.until(move |app| app.board.running(&id).is_none() && app.chat.conversation.iter().any(|entry| entry.cell.as_ref().is_some_and(|cell| cell.state == CellState::Failed)));
    window.step();
    window.step();
    let card = window.app.board.card_rect.expect("the cell was not drawn");
    let mut thumbs: Vec<egui::Rect> = window.pictures.iter().copied().filter(|rect| card.contains_rect(*rect) && rect.height() < 60.0).collect();
    thumbs.sort_by(|a, b| a.left().total_cmp(&b.left()));
    assert_eq!(thumbs.len(), 2, "a failed run hid the earlier pictures");
    click(&mut window, thumbs[1].center());
    assert_eq!(picture_of(&image_cell(&window)), second);
    window.app.keys.set(&endpoint, "stand-in").unwrap();

    // The session keeps the picture's relative path, never the folder's.
    window.app.write_project(true);
    keep_ticking(&mut window);
    let (_, _, session) = window.here();
    let text = std::fs::read_to_string(crate::workspace::session_path(&folder, &session)).unwrap();
    assert!(text.contains(&first), "the session lost the picture");
    assert!(!text.contains(&folder.display().to_string()), "the session holds an absolute path");
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches Settings flagging the image skill as broken: the panel checks it
/// against the endpoint's image models, where its model is listed, not
/// against the chat models, where it never is.
#[test]
fn settings_do_not_warn_about_the_image_skill() {
    let dir = scratch("image-settings");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    window.app.settings.add_image_skill();
    let image = window.app.settings.image_skill().unwrap().id.clone();
    window.logo(crate::chat::LogoChoice::Settings);
    // The chat list is empty, so the levels warn once both lists are read.
    window.until(|app| !app.settings_panel.unlisted.levels.is_empty());
    for _ in 0..20 {
        window.step();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(!window.app.settings_panel.unlisted.skills.contains(&image), "the image skill was warned about");
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a first start that never adds the image skill, or adds it each
/// time: the window's own first start adds it once.
#[test]
fn the_first_start_adds_the_image_skill_once() {
    let dir = scratch("image-first-start");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    // Settings already hold an endpoint, so no key is read or kept.
    window.app.first_start();
    window.app.first_start();
    let images = window.app.settings.skills.iter().filter(|skill| skill.makes == crate::workspace::Makes::Images).count();
    assert_eq!(images, 1);
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches room left empty under a picture: in the narrow panel beside the
/// workspace, a wide picture is drawn shorter than in the full window, and
/// the filmstrip sits right under it.
#[test]
fn a_picture_in_the_narrow_panel_leaves_no_gap() {
    let dir = scratch("image-narrow");
    let tool = protocol::tool_key(crate::route::HOST_ID, "skills", "image_generation");
    let stand_in = StandIn::start(vec![call_reply(&tool, json!({"prompt": "A wide shore."})), text_reply("Done.")]);
    let mut window = project_window(both(), &stand_in, &dir);
    window.app.settings.add_image_skill();
    window.ask("Make a wide picture.");
    window.until(|app| {
        app.chat.conversation.iter().any(|entry| entry.cell.as_ref().is_some_and(|cell| cell.kind == protocol::IMAGE_KIND && cell.state == CellState::Done))
            && !app.chat.working
    });
    if !window.app.workspace_shown() {
        window.app.toggle_workspace();
    }
    keep_ticking(&mut window);
    window.step();
    let card = window.app.board.card_rect.expect("the cell was not drawn");
    let inside: Vec<egui::Rect> = window.pictures.iter().copied().filter(|rect| card.contains_rect(*rect)).collect();
    let picture = inside.iter().copied().find(|rect| rect.height() > 60.0).expect("no picture was drawn");
    let thumb = inside.iter().copied().find(|rect| rect.height() <= 60.0).expect("no filmstrip was drawn");
    assert!(picture.height() < 300.0, "the wide picture was not fitted to the narrow card: {picture:?}");
    let gap = thumb.top() - picture.bottom();
    assert!(gap <= 16.0, "{gap} points lie empty between the picture and the filmstrip");
    let _ = std::fs::remove_dir_all(dir);
}
