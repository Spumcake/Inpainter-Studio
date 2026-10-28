//! Window checks for cells. They drive the real window, the real programs,
//! and the scripted stand-in. Nothing here writes a project by hand or
//! answers a question it was not expecting.

use super::{StandIn, Window, answer, built, call_reply, keep_ticking, project_window, scratch, spec, text_reply};
use crate::chat::ChatEvent;
use eframe::egui::{self, Pos2};
use protocol::{CellState, HostMessage};
use std::path::Path;

const PIECE: &str = "He kept the light through the storm.";

/// A cell-keeping application with its own ids. The host was not written for it.
const SLATE: &str = r#"#!/bin/sh
echo '{"type":"hello","protocol":1}'
echo '{"type":"tab","id":"s1","label":"Slate"}'
echo '{"type":"contents","tab":"s1","kind":"cells"}'
echo '{"type":"end"}'
echo '{"type":"skills","skills":[{"id":"slate","name":"Slate","description":"Keeps one slate.","instructions":"Mark the slate.","tools":[{"name":"mark","description":"Mark it.","input":{"type":"object","properties":{"mark":{"type":"string"}},"required":["mark"]}}]}]}'
echo '{"type":"tabs","tabs":[{"id":"s1","label":"Slate","saves":true}],"active":"s1"}'
echo '{"type":"cells","tab":"s1","cells":[{"id":"slate-1","kind":"text","input":[{"text":"A slate line."}],"state":"idle"}],"selected":"slate-1"}'
while IFS= read -r line; do
  case "$line" in
    *'"type":"ping"'*) echo '{"type":"pong"}' ;;
    *'"type":"shutdown"'*) exit 0 ;;
    *'"type":"quit"'*) echo '{"type":"quit_ready"}' ;;
    *'"type":"claim"'*)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
      path=$(printf '%s' "$line" | sed -n 's/.*"path":"\([^"]*\)".*/\1/p')
      mark=$(head -c 5 "$path" 2>/dev/null || true)
      if [ "$mark" = "SLATE" ]; then
        echo "{\"type\":\"claimed\",\"id\":\"$id\",\"yours\":true}"
      else
        echo "{\"type\":\"claimed\",\"id\":\"$id\",\"yours\":false}"
      fi ;;
    *'"type":"open"'*)
      path=$(printf '%s' "$line" | sed -n 's/.*"path":"\([^"]*\)".*/\1/p')
      echo "{\"type\":\"tabs\",\"tabs\":[{\"id\":\"s1\",\"label\":\"Slate\",\"saves\":true},{\"id\":\"s2\",\"label\":\"Claimed\",\"path\":\"$path\",\"saves\":true}],\"active\":\"s2\"}"
      echo '{"type":"contents","tab":"s2","kind":"cells"}'
      echo '{"type":"cells","tab":"s2","cells":[],"selected":null}' ;;
    *'"id":"jot"'*)
      echo '{"type":"cells","tab":"s1","cells":[{"id":"slate-1","kind":"text","input":[{"text":"A slate line."}],"state":"idle"},{"id":"slate-2","kind":"text","input":[{"text":""}],"state":"idle"}],"selected":"slate-2"}' ;;
    *'"id":"go"'*)
      echo '{"type":"run_cell","tab":"s1","cell":"slate-1"}' ;;
    *'"id":"halt"'*)
      echo '{"type":"stop_cell","tab":"s1","cell":"slate-1"}' ;;
    *'"type":"cell_result"'*)
      text=$(printf '%s' "$line" | sed -n 's/.*"text":"\([^"]*\)".*/\1/p' | head -n 1)
      echo "{\"type\":\"cells\",\"tab\":\"s1\",\"cells\":[{\"id\":\"slate-1\",\"kind\":\"text\",\"input\":[{\"text\":\"A slate line.\"}],\"output\":[{\"text\":\"$text\"}],\"state\":\"done\"}],\"selected\":\"slate-1\"}" ;;
    *'"type":"insert_cell"'*)
      prompt=$(printf '%s' "$line" | sed -n 's/.*"input":\[{"text":"\([^"]*\)".*/\1/p' | head -n 1)
      from=$(printf '%s' "$line" | sed -n 's/.*"from":"\([^"]*\)".*/\1/p' | head -n 1)
      echo "{\"type\":\"cells\",\"tab\":\"s1\",\"cells\":[{\"id\":\"slate-1\",\"kind\":\"text\",\"input\":[{\"text\":\"A slate line.\"}],\"state\":\"idle\"},{\"id\":\"dropped\",\"kind\":\"text\",\"from\":\"$from\",\"input\":[{\"text\":\"$prompt\"}],\"state\":\"done\"}],\"selected\":\"dropped\"}" ;;
    *'"type":"edit_cell"'*|*'"type":"select_cell"'*) ;;
  esac
done
"#;

fn slate_program(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("slate");
    std::fs::write(&path, SLATE).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn apps() -> Vec<super::super::AppSpec> {
    vec![
        spec("simple-sketch", "Simple Sketch", &built("simple-sketch")),
        spec("template", "Template", &built("template")),
        spec("mission-control", "Mission Control", &built("mission-control")),
    ]
}

fn command(window: &mut Window, app_id: &str, id: &str) {
    let index = window.app.apps.iter().position(|started| started.id == app_id).unwrap();
    let tab = window.app.apps[index].running.tabs().1.clone();
    window.app.send(
        index,
        HostMessage::Command {
            id: id.to_string(),
            tab,
            arg: None,
            node: None,
            viewport: [1280.0, 800.0],
        },
    );
    window.step();
}

fn book_cells(app: &crate::window::HostWindow) -> Vec<protocol::Cell> {
    app.apps
        .iter()
        .find(|started| started.id == "mission-control")
        .and_then(|started| started.running.declared.cells.values().next())
        .map(|(cells, _)| cells.clone())
        .unwrap_or_default()
}

fn conversation_cell(app: &crate::window::HostWindow) -> protocol::Cell {
    app.chat
        .conversation
        .iter()
        .find_map(|entry| entry.cell.clone())
        .expect("the conversation has no cell")
}

fn show_workspace(window: &mut Window) {
    if !window.app.workspace_shown() {
        window.app.toggle_workspace();
        window.step();
    }
}

fn type_prompt(window: &mut Window, cell: &str, text: &str) {
    show_workspace(window);
    let id = egui::Id::new(("cell-prompt", cell));
    window.ctx.memory_mut(|memory| memory.request_focus(id));
    window.step();
    window.events.push(egui::Event::Text(text.to_string()));
    window.step();
    window.ctx.memory_mut(|memory| memory.surrender_focus(id));
    window.step();
}

/// C-1 and the streaming half of C-2. The stand-in answers in parts.
#[test]
fn the_assistant_makes_a_cell_and_the_result_arrives_in_parts() {
    let dir = scratch("cell-run");
    let full = PIECE;
    let mid = full.char_indices().nth(full.chars().count() / 2).unwrap().0;
    let stand_in = StandIn::start(vec![
        call_reply(
            "inpainter__prompt__run_text_prompt",
            serde_json::json!({"prompt": "A lighthouse keeper."}),
        ),
        text_reply(full),
        text_reply("Done."),
    ]);
    let mut window = project_window(apps(), &stand_in, &dir);
    window.app.chat.input = "Write a two-line logline for a film about a lighthouse keeper.".into();
    let ctx = window.ctx.clone();
    let before = window.app.chat.conversation.len();
    window.app.chat_event(ChatEvent::Send, &ctx, &mut window.frame);
    window.until(|app| {
        app.chat.conversation[before..]
            .iter()
            .any(|entry| entry.cell.as_ref().is_some_and(|cell| !cell.result_text().is_empty()))
    });
    let partial = conversation_cell(&window.app);
    assert_eq!(partial.prompt(), "A lighthouse keeper.");
    assert_eq!(partial.result_text(), &full[..mid], "the result did not arrive in parts");
    assert!(window.app.board.running(&partial.id).is_some(), "a filling cell is not running");
    let requests = stand_in.requests();
    let cell_call = &requests[1];
    assert_eq!(cell_call["stream"], true);
    assert!(cell_call.get("tools").is_none(), "a cell run was offered tools");
    let messages = cell_call["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], "A lighthouse keeper.");
    window.until(|app| {
        app.chat.conversation.iter().any(|entry| {
            entry.cell.as_ref().is_some_and(|cell| cell.state == CellState::Done && cell.result_text() == full)
        }) && app.chat.conversation.iter().any(|entry| entry.text == "Done.")
    });
    let person = window.app.chat.conversation.iter().position(|entry| entry.role == assistant::Role::Person).unwrap();
    let cell = window.app.chat.conversation.iter().position(|entry| entry.cell.is_some()).unwrap();
    let reply = window.app.chat.conversation.iter().position(|entry| entry.text == "Done.").unwrap();
    assert!(person < cell && cell < reply, "the cell is not under the request, before the reply");
    // On screen, the reply is drawn below the card, never over it.
    window.step();
    window.step();
    let card = window.app.board.card_rect.expect("the cell was not drawn");
    let (_, drawn) = window
        .texts
        .iter()
        .find(|(text, _)| text == "Done.")
        .cloned()
        .expect("the reply was not drawn");
    assert!(drawn.top() >= card.bottom(), "the reply {drawn:?} is drawn over the cell {card:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// C-2. A failure shows its reason, and stopping keeps what had arrived.
#[test]
fn a_cell_fails_with_the_reason_and_stops_with_what_arrived() {
    let dir = scratch("cell-fail");
    let stand_in = StandIn::start(vec![
        call_reply(
            "inpainter__prompt__run_text_prompt",
            serde_json::json!({"prompt": "A dark shore."}),
        ),
        text_reply("I could not run it."),
    ]);
    let mut window = project_window(apps(), &stand_in, &dir);
    window.app.openrouter_key = None;
    window.app.chat.input = "Write about a dark shore.".into();
    let ctx = window.ctx.clone();
    window.app.chat_event(ChatEvent::Send, &ctx, &mut window.frame);
    window.until(|app| {
        app.chat.conversation.iter().any(|entry| {
            entry.cell.as_ref().is_some_and(|cell| cell.state == CellState::Failed)
        })
    });
    let failed = conversation_cell(&window.app);
    assert_eq!(failed.error.as_deref(), Some(assistant::NO_KEY));
    assert!(failed.result_text().is_empty());
    window.until(|app| !app.chat.working);

    let full = PIECE;
    let mid = full.char_indices().nth(full.chars().count() / 2).unwrap().0;
    stand_in.script(vec![
        call_reply(
            "inpainter__prompt__run_text_prompt",
            serde_json::json!({"prompt": "Keep going."}),
        ),
        text_reply(full),
    ]);
    window.app.openrouter_key = Some(String::from("stand-in"));
    // The model pauses after its first part, so Stop lands mid-answer.
    stand_in.hold();
    window.app.chat.input = "Write more.".into();
    let ctx = window.ctx.clone();
    window.app.chat_event(ChatEvent::Send, &ctx, &mut window.frame);
    window.until(|app| {
        app.chat.conversation.iter().any(|entry| {
            entry.cell.as_ref().is_some_and(|cell| cell.prompt() == "Keep going." && !cell.result_text().is_empty())
        })
    });
    let ctx = window.ctx.clone();
    window.app.chat_event(ChatEvent::Stop, &ctx, &mut window.frame);
    window.until(|app| {
        app.chat.conversation.iter().any(|entry| {
            entry.cell.as_ref().is_some_and(|cell| cell.prompt() == "Keep going." && cell.state == CellState::Stopped)
        }) && !app.chat.working
    });
    let stopped = window
        .app
        .chat
        .conversation
        .iter()
        .find_map(|entry| entry.cell.as_ref().filter(|cell| cell.prompt() == "Keep going."))
        .unwrap()
        .clone();
    assert_eq!(stopped.result_text(), &full[..mid]);
    assert_ne!(stopped.result_text(), full);
    stand_in.release();
    let _ = std::fs::remove_dir_all(dir);
}

/// C-1. The result shows while the model is still writing: the stand-in
/// sends its first part and waits, and the cell shows that part, running,
/// before the rest exists.
#[test]
fn a_cells_result_shows_while_the_model_is_still_writing() {
    let dir = scratch("cell-live");
    let full = PIECE;
    let mid = full.char_indices().nth(full.chars().count() / 2).unwrap().0;
    let stand_in = StandIn::start(vec![
        call_reply(
            "inpainter__prompt__run_text_prompt",
            serde_json::json!({"prompt": "A logline about a lighthouse keeper."}),
        ),
        text_reply(full),
        text_reply("Done."),
    ]);
    let mut window = project_window(apps(), &stand_in, &dir);
    stand_in.hold();
    window.app.chat.input = "Write a logline.".into();
    let ctx = window.ctx.clone();
    window.app.chat_event(ChatEvent::Send, &ctx, &mut window.frame);
    let first = &full[..mid];
    let shows_first = |app: &crate::window::HostWindow| {
        app.board
            .runs
            .iter()
            .any(|run| !run.settled && run.text == first)
    };
    window.until(shows_first);
    // Time passes and nothing more arrives: the model has not written it.
    std::thread::sleep(std::time::Duration::from_millis(300));
    window.step();
    assert!(shows_first(&window.app), "the rest arrived before the model sent it");
    stand_in.release();
    window.until(|app| {
        app.chat.conversation.iter().any(|entry| {
            entry.cell.as_ref().is_some_and(|cell| cell.state == CellState::Done && cell.result_text() == full)
        })
    });
    let _ = std::fs::remove_dir_all(dir);
}

/// C-3. Mission Control's book, through its own commands.
#[test]
fn a_mission_book_adds_runs_and_undoes_a_cell() {
    let dir = scratch("book");
    let stand_in = StandIn::start(vec![text_reply(PIECE)]);
    let mut window = project_window(apps(), &stand_in, &dir);
    window.select("mission-control");
    show_workspace(&mut window);
    let label = window
        .app
        .apps
        .iter()
        .find(|started| started.id == "mission-control")
        .unwrap()
        .running
        .tabs()
        .0
        .iter()
        .next()
        .unwrap()
        .label
        .clone();
    assert_eq!(label, "Untitled Mission Book");
    assert!(book_cells(&window.app).is_empty());
    command(&mut window, "mission-control", "add_cell");
    window.until(|app| book_cells(app).len() == 1);
    command(&mut window, "mission-control", "undo");
    window.until(|app| book_cells(app).is_empty());
    command(&mut window, "mission-control", "redo");
    window.until(|app| book_cells(app).len() == 1);
    let id = book_cells(&window.app)[0].id.clone();
    type_prompt(&mut window, &id, "A keeper at the lamp.");
    window.until(|app| book_cells(app).iter().any(|cell| cell.prompt() == "A keeper at the lamp."));
    command(&mut window, "mission-control", "undo");
    window.until(|app| book_cells(app).iter().any(|cell| cell.prompt().is_empty()));
    command(&mut window, "mission-control", "redo");
    window.until(|app| book_cells(app).iter().any(|cell| cell.prompt() == "A keeper at the lamp."));
    command(&mut window, "mission-control", "run");
    let mut saw_part = window.app.board.runs.iter().any(|run| !run.text.is_empty() && run.text != PIECE);
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(20) {
        for started in &mut window.app.apps {
            started.running.poll();
        }
        window.step();
        if window.app.board.runs.iter().any(|run| !run.text.is_empty() && run.text != PIECE) {
            saw_part = true;
        }
        if book_cells(&window.app).iter().any(|cell| cell.result_text() == PIECE) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(saw_part, "the book's result did not arrive in parts");
    window.until(|app| {
        app.apps.iter().any(|started| {
            started.running.declared.cells.values().any(|(cells, _)| {
                cells.iter().any(|cell| cell.result_text() == PIECE && cell.state == CellState::Done)
            })
        })
    });
    let ran = book_cells(&window.app);
    assert_eq!(ran[0].prompt(), "A keeper at the lamp.");
    assert_eq!(ran[0].model.as_deref(), Some("stand-in/model"));
    command(&mut window, "mission-control", "undo");
    window.until(|app| {
        app.apps.iter().any(|started| {
            started.running.declared.cells.values().any(|(cells, _)| {
                cells.iter().any(|cell| cell.output.is_empty() && cell.prompt() == "A keeper at the lamp.")
            })
        })
    });
    command(&mut window, "mission-control", "redo");
    window.until(|app| {
        app.apps.iter().any(|started| {
            started.running.declared.cells.values().any(|(cells, _)| {
                cells.iter().any(|cell| cell.result_text() == PIECE)
            })
        })
    });
    command(&mut window, "mission-control", "delete_cell");
    window.until(|app| book_cells(app).is_empty());
    command(&mut window, "mission-control", "undo");
    window.until(|app| book_cells(app).iter().any(|cell| cell.result_text() == PIECE));
    command(&mut window, "mission-control", "redo");
    window.until(|app| book_cells(app).is_empty());
    command(&mut window, "mission-control", "new_book");
    window.until(|app| {
        app.apps.iter().any(|started| {
            started.id == "mission-control" && started.running.tabs().0.len() == 2
        })
    });
    let _ = std::fs::remove_dir_all(dir);
}

/// C-4. A conversation cell dragged onto a book is copied onto its end.
#[test]
fn dragging_a_cell_onto_a_book_copies_it() {
    let dir = scratch("drag");
    let stand_in = StandIn::start(vec![
        call_reply(
            "inpainter__prompt__run_text_prompt",
            serde_json::json!({"prompt": "Drag me."}),
        ),
        text_reply(PIECE),
        text_reply("Done."),
    ]);
    let mut window = project_window(apps(), &stand_in, &dir);
    window.app.chat.input = "Make a cell to drag.".into();
    let ctx = window.ctx.clone();
    window.app.chat_event(ChatEvent::Send, &ctx, &mut window.frame);
    window.until(|app| {
        app.chat.conversation.iter().any(|entry| {
            entry.cell.as_ref().is_some_and(|cell| cell.state == CellState::Done)
        })
    });
    let source = conversation_cell(&window.app);
    window.select("mission-control");
    show_workspace(&mut window);
    let from = window.app.board.card_rect.expect("the conversation cell was not drawn");
    let onto = window.app.board.drop_rect.expect("the book was not drawn");
    let handle = Pos2::new(from.left() + 24.0, from.top() + 8.0);
    let drop = onto.center();
    window.events.push(egui::Event::PointerMoved(handle));
    window.events.push(egui::Event::PointerButton {
        pos: handle,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    window.step();
    window.events.push(egui::Event::PointerMoved(drop));
    window.step();
    window.events.push(egui::Event::PointerButton {
        pos: drop,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    window.step();
    window.until(|app| {
        app.apps.iter().any(|started| {
            started.running.declared.cells.values().any(|(cells, _)| {
                cells.iter().any(|cell| cell.from.as_deref() == Some(source.id.as_str()))
            })
        })
    });
    let copied = book_cells(&window.app).into_iter().find(|cell| cell.from.is_some()).unwrap();
    assert_eq!(copied.prompt(), "Drag me.");
    assert_eq!(copied.result_text(), PIECE);
    assert_eq!(copied.model.as_deref(), Some("stand-in/model"));
    assert!(conversation_cell(&window.app).id == source.id);
    window.select("template");
    window.step();
    let before = book_cells(&window.app).len();
    let from = window.app.board.card_rect.expect("the cell was not drawn");
    let handle = Pos2::new(from.left() + 24.0, from.top() + 8.0);
    let miss = Pos2::new(900.0, 400.0);
    window.events.push(egui::Event::PointerMoved(handle));
    window.events.push(egui::Event::PointerButton {
        pos: handle,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    window.step();
    window.events.push(egui::Event::PointerMoved(miss));
    window.events.push(egui::Event::PointerButton {
        pos: miss,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    window.step();
    window.step();
    assert_eq!(book_cells(&window.app).len(), before, "a drop on a tab without cells changed the book");
    let _ = std::fs::remove_dir_all(dir);
}

/// C-5 and C-6. The book's file, the project's folder, and the conversation's cell.
#[test]
fn a_book_and_a_conversation_cell_come_back_with_the_project() {
    let dir = scratch("round");
    let stand_in = StandIn::start(vec![
        call_reply(
            "inpainter__prompt__run_text_prompt",
            serde_json::json!({"prompt": "Remember this."}),
        ),
        text_reply(PIECE),
        text_reply("Done."),
        text_reply(PIECE),
    ]);
    let mut window = project_window(apps(), &stand_in, &dir);
    let id = window.app.project.as_ref().unwrap().id.clone();
    let files = crate::workspace::project_files(&dir, &id);
    assert!(files.is_dir());
    window.app.chat.input = "Remember a cell.".into();
    let ctx = window.ctx.clone();
    window.app.chat_event(ChatEvent::Send, &ctx, &mut window.frame);
    window.until(|app| {
        app.chat.conversation.iter().any(|entry| entry.cell.as_ref().is_some_and(|cell| cell.state == CellState::Done))
            && !app.chat.working
    });
    window.select("mission-control");
    show_workspace(&mut window);
    command(&mut window, "mission-control", "add_cell");
    window.until(|app| book_cells(app).len() == 1);
    let cell = book_cells(&window.app)[0].id.clone();
    type_prompt(&mut window, &cell, "Saved from the book.");
    window.until(|app| book_cells(app).iter().any(|cell| cell.prompt() == "Saved from the book."));
    command(&mut window, "mission-control", "run");
    window.until(|app| book_cells(app).iter().any(|cell| cell.result_text() == PIECE));
    command(&mut window, "mission-control", "save");
    window.until(|app| {
        app.apps.iter().any(|started| started.id == "mission-control" && !started.running.declared.files.is_empty())
    });
    let book = files.join("keeper.smx");
    window.answer_saves(&book);
    window.until(|app| {
        app.apps.iter().any(|started| {
            started.id == "mission-control"
                && started.running.tabs().0.iter().any(|tab| tab.path.as_deref() == Some(book.to_str().unwrap()))
        })
    });
    let saved = std::fs::read_to_string(&book).unwrap();
    assert!(saved.contains("Saved from the book."));
    assert!(saved.contains(PIECE));
    keep_ticking(&mut window);
    let project = crate::workspace::ProjectFile::read(&crate::workspace::project_path(&dir, &id)).unwrap();
    assert!(project.conversation.iter().any(|entry| {
        entry.cell.as_ref().is_some_and(|cell| cell.prompt() == "Remember this." && cell.result_text() == PIECE)
    }));
    window.until(|app| !app.chat.working);
    window.menu(crate::chat::ProjectChoice::New);
    assert!(
        window.app.flows.closing_started(),
        "new did not start closing"
    );
    window.until(|app| {
        let asked = app.apps.iter().any(|started| started.running.declared.ask.is_some());
        let ready = app.project.as_ref().is_some_and(|project| project.id != id)
            && app.apps.iter().all(|started| !started.running.pending() && started.running.tabs().1.is_some())
            && !app.chat.working;
        asked || ready
    });
    let asked: Vec<String> = window
        .app
        .apps
        .iter()
        .filter_map(|started| started.running.declared.ask.as_ref().map(|ask| ask.title.clone()))
        .collect();
    assert!(asked.is_empty(), "leaving a saved project asked {asked:?}");
    window.menu(crate::chat::ProjectChoice::Switch(id.clone()));
    window.until(|app| {
        app.project.as_ref().is_some_and(|project| project.id == id)
            && app.chat.conversation.iter().any(|entry| {
                entry.cell.as_ref().is_some_and(|cell| cell.prompt() == "Remember this." && cell.result_text() == PIECE)
            })
            && app.apps.iter().any(|started| {
                started.id == "mission-control"
                    && started.running.declared.cells.values().any(|(cells, _)| {
                        cells.iter().any(|cell| cell.prompt() == "Saved from the book." && cell.result_text() == PIECE)
                    })
            })
    });

    // Closing Studio and starting it again brings the same back.
    window.logo(crate::chat::LogoChoice::Quit);
    window.until(|app| app.apps.iter().all(|started| started.running.declared.quit_ready));
    window.step();
    window.step();
    drop(window);
    let window = project_window(apps(), &stand_in, &dir);
    assert_eq!(window.app.project.as_ref().unwrap().id, id);
    let mut window = window;
    window.until(|app| {
        app.chat.conversation.iter().any(|entry| {
            entry.cell.as_ref().is_some_and(|cell| cell.prompt() == "Remember this." && cell.result_text() == PIECE)
        }) && app.apps.iter().any(|started| {
            started.id == "mission-control"
                && started.running.tabs().0.iter().any(|tab| tab.path.as_deref() == Some(book.to_str().unwrap()))
                && started.running.declared.cells.values().any(|(cells, _)| {
                    cells.iter().any(|cell| cell.prompt() == "Saved from the book." && cell.result_text() == PIECE)
                })
        })
    });
    let _ = std::fs::remove_dir_all(dir);
}

/// C-5. Closing a book with changes asks first.
#[test]
fn closing_a_changed_book_asks_first() {
    let dir = scratch("close-book");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(apps(), &stand_in, &dir);
    window.select("mission-control");
    show_workspace(&mut window);
    command(&mut window, "mission-control", "add_cell");
    window.until(|app| book_cells(app).len() == 1);
    command(&mut window, "mission-control", "close_tab");
    window.until(|app| {
        app.apps.iter().any(|started| {
            started.running.declared.ask.as_ref().is_some_and(|ask| ask.title == "Keep this book?")
        })
    });
    answer(&mut window, "cancel");
    window.until(|app| book_cells(app).len() == 1 && app.apps.iter().all(|started| started.running.declared.ask.is_none()));
    let _ = std::fs::remove_dir_all(dir);
}

/// C-8. The stand-in's own commands get drawing, Run, and a dropped cell.
#[test]
fn a_stand_in_that_keeps_cells_gets_drawing_run_and_drag() {
    let dir = scratch("slate");
    let program = slate_program(&dir);
    let stand_in = StandIn::start(vec![
        text_reply(PIECE),
        call_reply(
            "inpainter__prompt__run_text_prompt",
            serde_json::json!({"prompt": "Onto the slate."}),
        ),
        text_reply(PIECE),
        text_reply("Done."),
    ]);
    let mut window = project_window(
        vec![spec("slate", "Slate", &program)],
        &stand_in,
        &dir,
    );
    window.select("slate");
    show_workspace(&mut window);
    let slate = window.app.board.drop_rect.expect("the stand-in's cells were not drawn");
    // Run, clicked in the stand-in's own card, as a person clicks it.
    let run = window
        .texts
        .iter()
        .find(|(text, rect)| text == "Generate" && slate.contains_rect(*rect))
        .map(|(_, rect)| rect.center())
        .expect("the stand-in's card has no Generate button");
    window.events.push(egui::Event::PointerMoved(run));
    window.events.push(egui::Event::PointerButton {
        pos: run,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    window.step();
    window.events.push(egui::Event::PointerButton {
        pos: run,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    window.step();
    window.until(|app| {
        app.apps[0].running.declared.cells.values().any(|(cells, _)| {
            cells.iter().any(|cell| cell.id == "slate-1" && cell.result_text() == PIECE)
        })
    });
    window.app.chat.input = "A cell for the slate.".into();
    let ctx = window.ctx.clone();
    window.app.chat_event(ChatEvent::Send, &ctx, &mut window.frame);
    window.until(|app| app.chat.conversation.iter().any(|entry| entry.cell.as_ref().is_some_and(|cell| cell.state == CellState::Done)));
    let source = conversation_cell(&window.app);
    show_workspace(&mut window);
    let from = window.app.board.card_rect.expect("the cell was not drawn");
    let onto = window.app.board.drop_rect.expect("the slate was not drawn");
    let handle = Pos2::new(from.left() + 24.0, from.top() + 8.0);
    window.events.push(egui::Event::PointerMoved(handle));
    window.events.push(egui::Event::PointerButton {
        pos: handle,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    window.step();
    window.events.push(egui::Event::PointerMoved(onto.center()));
    window.step();
    window.events.push(egui::Event::PointerButton {
        pos: onto.center(),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    window.step();
    window.until(|app| {
        app.apps[0].running.declared.cells.values().any(|(cells, _)| {
            cells.iter().any(|cell| cell.from.as_deref() == Some(source.id.as_str()) && cell.prompt() == "Onto the slate.")
        })
    });
    // One frame draws the same cell in the conversation and in the stand-in:
    // its kind and its prompt, painted by the same code in both places.
    window.step();
    window.step();
    let onto = window.app.board.drop_rect.expect("the slate was not drawn");
    let card = window.app.board.card_rect.expect("the cell was not drawn");
    for wanted in ["Onto the slate.", "LLM"] {
        let in_slate = window.texts.iter().any(|(text, rect)| text == wanted && onto.contains_rect(*rect));
        let in_chat = window.texts.iter().any(|(text, rect)| text == wanted && card.contains_rect(*rect));
        assert!(in_slate && in_chat, "{wanted:?} was not drawn in both places: slate {in_slate}, conversation {in_chat}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// C-9. The application that claims a file is the one that opens it.
#[test]
fn the_application_that_claims_a_file_opens_it() {
    let dir = scratch("claim");
    let program = slate_program(&dir);
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(
        vec![
            spec("simple-sketch", "Simple Sketch", &built("simple-sketch")),
            spec("mission-control", "Mission Control", &built("mission-control")),
            spec("slate", "Slate", &program),
        ],
        &stand_in,
        &dir,
    );
    let id = window.app.project.as_ref().unwrap().id.clone();
    let files = crate::workspace::project_files(&dir, &id);
    let sketch = files.join("view.smx");
    super::save_tab(&mut window, 0, &sketch);
    let book = files.join("story.smx");
    std::fs::write(
        &book,
        r#"{"version":1,"cells":[{"id":"kept","kind":"text","input":[{"text":"From the book."}],"state":"idle"}]}"#,
    )
    .unwrap();
    let slate = files.join("board.smx");
    std::fs::write(&slate, "SLATE board").unwrap();

    window.select("simple-sketch");
    command(&mut window, "simple-sketch", "open");
    window.until(|app| app.apps.iter().any(|started| !started.running.declared.files.is_empty()));
    window.answer_saves(&book);
    window.until(|app| book_cells(app).iter().any(|cell| cell.prompt() == "From the book."));
    assert!(window.app.apps.iter().find(|started| started.id == "simple-sketch").unwrap().running.tabs().0.iter().all(|tab| {
        tab.path.as_deref() != Some(book.to_str().unwrap())
    }));

    window.select("mission-control");
    command(&mut window, "mission-control", "open");
    window.until(|app| app.apps.iter().any(|started| started.id == "mission-control" && !started.running.declared.files.is_empty()));
    window.answer_saves(&sketch);
    window.until(|app| {
        app.apps.iter().any(|started| {
            started.id == "simple-sketch" && started.running.tabs().0.iter().any(|tab| tab.path.as_deref() == Some(sketch.to_str().unwrap()))
        })
    });

    window.select("simple-sketch");
    command(&mut window, "simple-sketch", "open");
    window.until(|app| app.apps.iter().any(|started| started.id == "simple-sketch" && !started.running.declared.files.is_empty()));
    window.answer_saves(&slate);
    window.until(|app| {
        app.apps.iter().any(|started| {
            started.id == "slate" && started.running.tabs().0.iter().any(|tab| tab.path.as_deref() == Some(slate.to_str().unwrap()))
        })
    });
    let _ = std::fs::remove_dir_all(dir);
}

/// The assistant's own reply shows while the model is still writing: the
/// stand-in sends its first part and waits, and the conversation shows that
/// part before the rest exists; then the reply's entry replaces it.
#[test]
fn the_assistants_reply_shows_while_the_model_is_still_writing() {
    let dir = scratch("reply-live");
    let full = PIECE;
    let mid = full.char_indices().nth(full.chars().count() / 2).unwrap().0;
    let stand_in = StandIn::start(vec![text_reply(full)]);
    let mut window = project_window(apps(), &stand_in, &dir);
    stand_in.hold();
    window.app.chat.input = "Say something.".into();
    let ctx = window.ctx.clone();
    window.app.chat_event(ChatEvent::Send, &ctx, &mut window.frame);
    let first = &full[..mid];
    window.until(|app| app.chat.working && app.chat.live == first);
    std::thread::sleep(std::time::Duration::from_millis(300));
    window.step();
    assert_eq!(window.app.chat.live, first, "the rest arrived before the model sent it");
    stand_in.release();
    window.until(|app| {
        !app.chat.working
            && app.chat.live.is_empty()
            && app.chat.conversation.iter().any(|entry| entry.role == assistant::Role::Assistant && entry.text == full)
    });
    let _ = std::fs::remove_dir_all(dir);
}

/// A message sent while a request runs waits as queued, and is sent when the
/// request ends.
#[test]
fn a_message_sent_while_a_request_runs_is_sent_after_it() {
    let dir = scratch("queue");
    let stand_in = StandIn::start(vec![text_reply("First answer."), text_reply("Second answer.")]);
    let mut window = project_window(apps(), &stand_in, &dir);
    stand_in.hold();
    window.app.chat.input = "First question.".into();
    let ctx = window.ctx.clone();
    window.app.chat_event(ChatEvent::Send, &ctx, &mut window.frame);
    window.until(|app| app.chat.working && !app.chat.live.is_empty());
    window.app.chat.input = "Second question.".into();
    window.app.chat_event(ChatEvent::Send, &ctx, &mut window.frame);
    assert_eq!(window.app.chat.queued.len(), 1, "the second message was not queued");
    assert_eq!(stand_in.requests().len(), 1, "the second message went out while the first ran");
    stand_in.release();
    window.until(|app| {
        app.chat.queued.is_empty()
            && !app.chat.working
            && app.chat.conversation.iter().any(|entry| entry.text == "Second answer.")
    });
    let order: Vec<&str> = window
        .app
        .chat
        .conversation
        .iter()
        .filter(|entry| entry.role != assistant::Role::Tool)
        .map(|entry| entry.text.as_str())
        .collect();
    assert_eq!(order, ["First question.", "First answer.", "Second question.", "Second answer."]);
    let _ = std::fs::remove_dir_all(dir);
}
