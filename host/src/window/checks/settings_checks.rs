//! Window checks for Settings: what is set reaches every request, a skill
//! is a tool that runs on its own endpoint, no key lands in a file Studio
//! writes, and the panel opens and closes as the session menu does. They
//! drive the real window and scripted stand-ins, so they spend nothing.

use super::{StandIn, Window, both, call_reply, keep_ticking, marks, offered, project_window, scratch, text_reply};
use crate::cells::CardAction;
use crate::chat::{ChatEvent, LogoChoice};
use crate::settings_panel::{Page, SettingsChoice};
use crate::workspace::{EndpointKind, SkillSetting};
use eframe::egui::{self, Pos2};
use protocol::CellState;
use serde_json::json;
use std::path::Path;

impl Window {
    /// Makes a change in Settings, as the panel reports it.
    fn setting(&mut self, choice: SettingsChoice) {
        let Self { app, frame, .. } = self;
        app.apply_setting(choice, frame);
        self.step();
    }

    /// Adds an OpenAI-compatible endpoint with no key at `url`. Returns its id.
    fn add_local(&mut self, url: &str) -> String {
        self.setting(SettingsChoice::SaveEndpoint {
            id: None,
            kind: EndpointKind::Openai,
            name: String::from("Local"),
            address: url.to_string(),
            key: None,
        });
        self.app.settings.endpoints.iter().find(|endpoint| endpoint.name == "Local").expect("added").id.clone()
    }
}

/// Catches settings that change nothing: the Discussion prompt, the chosen
/// level's model and effort, and Discussion's endpoint each reach the next
/// request, and a local server with no key is sent none.
#[test]
fn settings_reach_the_assistants_requests() {
    let dir = scratch("settings-requests");
    let stand_in = StandIn::start(vec![text_reply("One."), text_reply("Two.")]);
    let local = StandIn::start(vec![text_reply("Three.")]);
    let mut window = project_window(both(), &stand_in, &dir);

    let mut discussion = window.app.settings.discussion().expect("built in").clone();
    discussion.prompt = String::from("Answer as a lighthouse keeper.");
    window.setting(SettingsChoice::SaveSkill(discussion.clone()));
    window.ask("First.");
    let sent = stand_in.requests();
    let system = sent[0]["messages"][0]["content"].as_str().unwrap().to_string();
    assert!(system.ends_with("Answer as a lighthouse keeper."), "{system}");
    assert!(system.contains("Never say you did something you did not do"), "a fixed rule was lost");
    assert_eq!(sent[0]["reasoning"]["effort"], "medium", "the default level is Haiku Medium");
    assert_eq!(stand_in.keys()[0].as_deref(), Some("Bearer stand-in"));

    let mut sonnet = window.app.settings.levels.iter().find(|level| level.name == "Sonnet High").unwrap().clone();
    sonnet.model = String::from("stand-in/sonnet");
    window.setting(SettingsChoice::SaveLevel(sonnet.clone()));
    let ctx = window.ctx.clone();
    {
        let Window { app, frame, .. } = &mut window;
        app.chat_event(ChatEvent::Level(sonnet.id.clone()), &ctx, frame);
    }
    window.step();
    window.ask("Second.");
    let sent = stand_in.requests();
    assert_eq!(sent[1]["model"], "stand-in/sonnet");
    assert_eq!(sent[1]["reasoning"]["effort"], "high");

    let local_id = window.add_local(&local.url);
    discussion.endpoint = Some(local_id.clone());
    window.setting(SettingsChoice::SaveSkill(discussion));
    window.ask("Third.");
    assert_eq!(stand_in.requests().len(), 2, "a request still went to the old endpoint");
    let sent = local.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["model"], "stand-in/sonnet");
    assert_eq!(sent[0]["reasoning_effort"], "high");
    assert_eq!(local.keys()[0], None, "a keyless server was sent a key");
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a skill that is only a row: the assistant is offered it as a
/// tool, calling it runs a cell on the skill's endpoint with the skill's
/// prompt, its result returns, and Run again stays on that endpoint. The
/// endpoint's own model list names its models.
#[test]
fn a_skill_runs_as_a_cell_on_its_own_endpoint() {
    let dir = scratch("settings-skill");
    let tool = protocol::tool_key(crate::route::HOST_ID, "skills", "story_notes");
    let stand_in = StandIn::start(vec![call_reply(&tool, json!({"input": "a beat sheet"})), text_reply("Done.")]);
    let local = StandIn::start(vec![text_reply("Beat one.")]);
    local.list_models(json!({"data": [{"id": "llama3.2"}]}));
    let mut window = project_window(both(), &stand_in, &dir);
    let local_id = window.add_local(&local.url);
    window.setting(SettingsChoice::SaveSkill(SkillSetting {
        id: String::from("story"),
        name: String::from("Story notes"),
        description: String::from("Drafts beat sheets"),
        endpoint: Some(local_id.clone()),
        model: Some(String::from("llama3.2")),
        prompt: String::from("You write beat sheets."),
        makes: crate::workspace::Makes::Text,
    }));
    window.ask("Make a beat sheet.");
    window.until(|app| {
        app.chat.conversation.iter().any(|entry| entry.cell.as_ref().is_some_and(|cell| cell.state == CellState::Done))
            && !app.chat.working
    });
    assert!(offered(&stand_in.requests()[0]).contains(&tool), "the skill was not offered");
    let sent = local.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["model"], "llama3.2");
    assert_eq!(sent[0]["messages"][0]["content"], "You write beat sheets.\n\na beat sheet");
    assert_eq!(local.keys()[0], None, "a keyless server was sent a key");
    let cell = window.app.chat.conversation.iter().find_map(|entry| entry.cell.clone()).unwrap();
    assert_eq!(cell.endpoint.as_deref(), Some(local_id.as_str()));
    assert_eq!(cell.result_text(), "Beat one.");
    window.until(|_| stand_in.requests().len() >= 2);
    assert!(stand_in.requests()[1].to_string().contains("Beat one."), "the result did not return");

    local.script(vec![text_reply("Beat two.")]);
    window.app.note_cell_actions(vec![CardAction::Run(cell.id.clone())]);
    window.until(|app| {
        app.chat.conversation.iter().any(|entry| entry.cell.as_ref().is_some_and(|cell| cell.result_text() == "Beat two."))
    });
    assert_eq!(local.requests().len(), 2);
    assert_eq!(stand_in.requests().len(), 2, "Run again moved to the Discussion endpoint");

    let endpoint = window.app.settings.endpoint(&local_id).unwrap().clone();
    let ctx = window.ctx.clone();
    let start = std::time::Instant::now();
    let listed = loop {
        if let Some(Ok(list)) = window.app.probes.models(&ctx, &endpoint, None) {
            break list.iter().map(|model| model.id.clone()).collect::<Vec<_>>();
        }
        assert!(start.elapsed().as_secs() < 10, "the model list never came");
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert_eq!(listed, ["llama3.2"]);
    let _ = std::fs::remove_dir_all(dir);
}

/// Every file under `dir` that holds `text`.
fn files_holding(dir: &Path, text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_holding(&path, text));
        } else if std::fs::read(&path).is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(text)) {
            found.push(path.display().to_string());
        }
    }
    found
}

/// Catches a key written where it does not belong: after an endpoint is
/// added with a key and a request runs, the key is only in the private
/// keys file, readable by the person alone; never in settings, a session,
/// a project, or the conversation.
#[test]
fn no_key_is_written_where_studio_keeps_its_files() {
    let dir = scratch("settings-keys");
    let stand_in = StandIn::start(vec![text_reply("Kept.")]);
    let mut window = project_window(both(), &stand_in, &dir);
    window.app.keys = crate::keys::Keys::new(Some(dir.clone()));
    let first = window.app.settings.endpoints[0].clone();
    window.setting(SettingsChoice::SaveEndpoint {
        id: Some(first.id.clone()),
        kind: first.kind,
        name: first.name.clone(),
        address: first.address.clone(),
        key: Some(String::from("sk-never-in-a-file")),
    });
    window.ask("Anything.");
    assert_eq!(stand_in.keys()[0].as_deref(), Some("Bearer sk-never-in-a-file"));
    window.app.write_project(true);
    keep_ticking(&mut window);
    let keys_file = dir.join("keys.json").display().to_string();
    assert_eq!(files_holding(&dir, "sk-never-in-a-file"), [keys_file.clone()]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&keys_file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a panel that does not open from the logo, or a press that closes
/// it reaching what is underneath: Settings opens on General with its four
/// pages, and a press outside, or Escape, closes it and draws nothing.
#[test]
fn settings_open_from_the_logo_and_close_without_drawing() {
    let dir = scratch("settings-panel");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    window.select("simple-sketch");
    window.app.toggle_workspace();
    keep_ticking(&mut window);
    let tab = window.app.apps[0].running.tabs().1.clone().unwrap();
    window.app.send(
        0,
        protocol::HostMessage::Command { id: "select_tool".into(), tab: Some(tab.clone()), arg: Some(json!("brush")), node: None, viewport: [800.0, 600.0] },
    );
    keep_ticking(&mut window);
    window.until(|app| app.apps[0].running.declared.documents.contains_key(&tab));
    let before = marks(&window.app, &tab);

    window.logo(LogoChoice::Settings);
    window.step();
    assert!(window.app.settings_panel.open);
    assert_eq!(window.app.settings_panel.page, Page::General);
    let drawn: Vec<String> = window.texts.iter().map(|(text, _)| text.clone()).collect();
    for wanted in ["General", "Endpoints", "Skills", "Discussion", "Project location"] {
        assert!(drawn.iter().any(|text| text == wanted), "{wanted} is not drawn: {drawn:?}");
    }
    let panel = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("settings-panel"));
    let inside = window.texts.iter().find(|(text, _)| text == "Project location").unwrap().1.center();
    assert_eq!(window.ctx.layer_id_at(inside), Some(panel), "the overlay is over the panel");

    let at = Pos2::new(1100.0, 700.0);
    window.events.push(egui::Event::PointerMoved(at));
    window.step();
    for pressed in [true, false] {
        window.events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
        window.step();
    }
    keep_ticking(&mut window);
    assert!(!window.app.settings_panel.open, "the press outside did not close Settings");
    assert_eq!(marks(&window.app, &tab), before, "the press that closed Settings drew");

    window.logo(LogoChoice::Settings);
    window.step();
    window.events.push(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    window.step();
    assert!(!window.app.settings_panel.open, "Escape did not close Settings");
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a level shared by every session: a session keeps the level it
/// chose, and a new session starts on the default.
#[test]
fn each_session_keeps_its_own_level() {
    use crate::sessions::SessionChoice;
    let dir = scratch("settings-level");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let (project, _, first) = window.here();
    let sonnet = window.app.settings.levels.iter().find(|level| level.name == "Sonnet High").unwrap().id.clone();
    let ctx = window.ctx.clone();
    {
        let Window { app, frame, .. } = &mut window;
        app.chat_event(ChatEvent::Level(sonnet.clone()), &ctx, frame);
    }
    window.step();
    assert_eq!(window.app.chat.levels[window.app.chat.current.unwrap()].name, "Sonnet High");
    keep_ticking(&mut window);

    window.menu(SessionChoice::NewSession { project: project.clone() });
    window.until(|app| app.project.as_ref().is_some_and(|open| open.session != first));
    let second = window.here().2;
    window.settle_on(&second);
    window.step();
    assert_eq!(window.app.chat.levels[window.app.chat.current.unwrap()].name, "Haiku Medium");

    window.menu(SessionChoice::Open { project, session: first.clone() });
    window.settle_on(&first);
    window.step();
    assert_eq!(window.app.chat.levels[window.app.chat.current.unwrap()].name, "Sonnet High");
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a project location that is shown and forgotten: General's
/// Change is remembered, New Project starts there, and so does the
/// project Studio makes by itself.
#[test]
fn the_project_location_is_remembered_and_used() {
    let dir = scratch("settings-root");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let chosen = dir.join("Elsewhere");
    window.app.scripted_paths.push_back(chosen.clone());
    window.setting(SettingsChoice::ChooseProjectsRoot);
    let stored = crate::workspace::Settings::load(Some(dir.join("settings.json")));
    assert_eq!(stored.projects_root.as_deref(), Some(chosen.display().to_string().as_str()));
    assert_eq!(window.app.projects_root(), Some(chosen.clone()));
    window.app.sessions.start_new_project(&window.app.sessions.form_root.clone());
    assert!(Path::new(&window.app.sessions.form_location).starts_with(&chosen), "{}", window.app.sessions.form_location);
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches Run again that forgets how a cell ran: a cell made at Haiku
/// Medium runs again at Haiku Medium after the picker moves to Sonnet High,
/// as the owner set on 2026-10-29. A new cell takes the picker's level.
#[test]
fn run_again_repeats_the_model_and_effort_it_ran_at() {
    let dir = scratch("settings-rerun");
    let tool = protocol::tool_key(crate::route::HOST_ID, "prompt", "run_text_prompt");
    let stand_in = StandIn::start(vec![
        call_reply(&tool, json!({"prompt": "A keeper."})),
        text_reply("He kept it."),
        text_reply("Done."),
    ]);
    let mut window = project_window(both(), &stand_in, &dir);
    window.ask("Write about a keeper.");
    window.until(|app| {
        app.chat.conversation.iter().any(|entry| entry.cell.as_ref().is_some_and(|cell| cell.state == CellState::Done))
            && !app.chat.working
    });
    let cell = window.app.chat.conversation.iter().find_map(|entry| entry.cell.clone()).unwrap();
    assert_eq!(cell.effort.as_deref(), Some("medium"));
    let mut sonnet = window.app.settings.levels.iter().find(|level| level.name == "Sonnet High").unwrap().clone();
    sonnet.model = String::from("stand-in/sonnet");
    window.setting(SettingsChoice::SaveLevel(sonnet.clone()));
    let ctx = window.ctx.clone();
    {
        let Window { app, frame, .. } = &mut window;
        app.chat_event(ChatEvent::Level(sonnet.id.clone()), &ctx, frame);
    }
    window.step();
    let before = stand_in.requests().len();
    stand_in.script(vec![text_reply("He kept it again.")]);
    window.app.note_cell_actions(vec![CardAction::Run(cell.id.clone())]);
    window.until(|app| {
        app.chat.conversation.iter().any(|entry| entry.cell.as_ref().is_some_and(|cell| cell.result_text() == "He kept it again."))
    });
    let again = &stand_in.requests()[before];
    assert_eq!(again["model"], "stand-in/model");
    assert_eq!(again["reasoning"]["effort"], "medium", "Run again lost the effort it ran at");
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a chevron that chooses nothing: it opens the composer's level
/// picker, and the level chosen there is the model and effort Run again
/// sends.
#[test]
fn a_cells_chevron_chooses_its_level() {
    let dir = scratch("settings-chevron");
    let tool = protocol::tool_key(crate::route::HOST_ID, "prompt", "run_text_prompt");
    let stand_in = StandIn::start(vec![call_reply(&tool, json!({"prompt": "A keeper."})), text_reply("He kept it."), text_reply("Done.")]);
    stand_in.list_models(json!({"data": [{"id": "stand-in/model", "name": "Stand-in"}, {"id": "stand-in/other", "name": "Other"}]}));
    let mut window = project_window(both(), &stand_in, &dir);
    window.ask("Write about a keeper.");
    window.until(|app| {
        app.chat.conversation.iter().any(|entry| entry.cell.as_ref().is_some_and(|cell| cell.state == CellState::Done))
            && !app.chat.working
    });
    let cell = window.app.chat.conversation.iter().find_map(|entry| entry.cell.clone()).unwrap();
    let at = egui::Rect::from_min_size(Pos2::new(900.0, 200.0), egui::vec2(30.0, 30.0));
    window.app.note_cell_actions(vec![CardAction::Models(cell.id.clone(), at)]);
    window.step();
    window.until(|app| app.model_menu.is_some());
    assert_eq!(window.app.cell_level(&cell.id, &cell), window.app.chat.current, "the picker does not show the cell's level");
    let mut sonnet = window.app.settings.levels.iter().find(|level| level.name == "Sonnet High").unwrap().clone();
    sonnet.model = String::from("stand-in/sonnet");
    window.setting(SettingsChoice::SaveLevel(sonnet.clone()));
    window.app.choose_cell_level(&cell.id, &sonnet.id);
    window.app.model_menu = None;
    let before = stand_in.requests().len();
    stand_in.script(vec![text_reply("Again.")]);
    window.app.note_cell_actions(vec![CardAction::Run(cell.id.clone())]);
    window.until(|app| app.chat.conversation.iter().any(|entry| entry.cell.as_ref().is_some_and(|cell| cell.result_text() == "Again.")));
    let again = &stand_in.requests()[before];
    assert_eq!(again["model"], "stand-in/sonnet", "the chosen level was not used");
    assert_eq!(again["reasoning"]["effort"], "high");
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches an older cell whose picker shows nothing: a cell that ran
/// before cells kept their effort, on Claude Haiku with the word
/// "openrouter" for its endpoint, shows the session's level, and Run again
/// runs at that level's effort.
#[test]
fn an_older_cell_counts_as_on_the_level_of_its_model() {
    let dir = scratch("settings-older");
    let tool = protocol::tool_key(crate::route::HOST_ID, "prompt", "run_text_prompt");
    let stand_in = StandIn::start(vec![call_reply(&tool, json!({"prompt": "A keeper."})), text_reply("He kept it."), text_reply("Done.")]);
    let mut window = project_window(both(), &stand_in, &dir);
    window.ask("Write about a keeper.");
    window.until(|app| {
        app.chat.conversation.iter().any(|entry| entry.cell.as_ref().is_some_and(|cell| cell.state == CellState::Done))
            && !app.chat.working
    });
    // As a cell from before: no effort, and the old word for its endpoint.
    let cell = {
        let cell = window.app.chat.conversation.iter_mut().find_map(|entry| entry.cell.as_mut()).unwrap();
        cell.effort = None;
        cell.endpoint = Some(String::from("openrouter"));
        cell.clone()
    };
    assert_eq!(window.app.cell_level(&cell.id, &cell), window.app.chat.current, "the picker shows no level");
    let before = stand_in.requests().len();
    stand_in.script(vec![text_reply("Again.")]);
    window.app.note_cell_actions(vec![CardAction::Run(cell.id.clone())]);
    let id = cell.id.clone();
    window.until(move |app| {
        app.board.running(&id).is_none()
            && app.chat.conversation.iter().any(|entry| entry.cell.as_ref().is_some_and(|cell| cell.result_text() == "Again."))
    });
    assert_eq!(stand_in.requests()[before]["reasoning"]["effort"], "medium");
    let ran = window.app.chat.conversation.iter().find_map(|entry| entry.cell.clone()).unwrap();
    assert_eq!(ran.effort.as_deref(), Some("medium"), "the run did not record its effort: {ran:?}");
    let _ = std::fs::remove_dir_all(dir);
}
