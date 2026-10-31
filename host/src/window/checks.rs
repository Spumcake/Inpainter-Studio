//! The mission's assistant checks, run against real sub-application
//! processes and a scripted stand-in for OpenRouter, so they spend nothing.
//!
//! The stand-in sub-application below declares a skill that nothing in the
//! host or the assistant was written for. These checks need `cargo build`
//! first, because they start the built `template` program.

use super::{HostWindow, Started};
use crate::chat::ChatEvent;
use crate::process::Running;
use crate::tablet::Tablet;
use assistant::{Assistant, Config, Role};
use eframe::App;
use eframe::egui::{self, Pos2, Rect};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A sub-application with one skill no source names: it stamps a glyph.
const QUILL: &str = r#"#!/bin/sh
echo '{"type":"hello","protocol":1}'
echo '{"type":"tab","id":"q1","label":"Quill Board"}'
echo '{"type":"contents","tab":"q1","kind":"grid"}'
echo '{"type":"end"}'
echo '{"type":"skills","skills":[{"id":"quill","name":"Quill","description":"Stamps one glyph on the board.","instructions":"Pass the glyph to stamp.","tools":[{"name":"stamp_glyph","description":"Stamp a glyph.","input":{"type":"object","properties":{"glyph":{"type":"string"}},"required":["glyph"]}}]}]}'
while IFS= read -r line; do
  case "$line" in
    *'"type":"ping"'*) echo '{"type":"pong"}' ;;
    *'"type":"shutdown"'*) exit 0 ;;
    *'"type":"quit"'*) echo '{"type":"quit_ready"}' ;;
    *'"type":"invoke"'*)
      call=$(printf '%s' "$line" | sed 's/.*"call":"\([^"]*\)".*/\1/')
      echo "{\"type\":\"result\",\"call\":\"$call\",\"ok\":true,\"output\":{\"stamped\":\"quill-mark-7\"}}" ;;
  esac
done
"#;

/// A sub-application nothing was written for, with ids of its own. It is
/// told the project's folder, and writes its one ledger there by itself
/// after any change, reporting the file, as every activity must.
const LEDGER: &str = r#"#!/bin/sh
folder=""
echo '{"type":"hello","protocol":1}'
echo '{"type":"tab","id":"l1","label":"Untitled Ledger"}'
echo '{"type":"contents","tab":"l1","kind":"grid"}'
echo '{"type":"end"}'
echo '{"type":"tabs","tabs":[{"id":"l1","label":"Untitled Ledger","saves":true}],"active":"l1"}'
while IFS= read -r line; do
  case "$line" in
    *'"type":"ping"'*) echo '{"type":"pong"}' ;;
    *'"type":"shutdown"'*) exit 0 ;;
    *'"type":"project"'*) folder=$(printf '%s' "$line" | sed 's/.*"folder":"\([^"]*\)".*/\1/') ;;
    *'"type":"command"'*)
      printf 'ledger' > "$folder/Ledger.ledger"
      echo "{\"type\":\"tabs\",\"tabs\":[{\"id\":\"l1\",\"label\":\"Ledger\",\"path\":\"$folder/Ledger.ledger\",\"saves\":true}],\"active\":\"l1\"}" ;;
    *'"type":"quit"'*) echo '{"type":"quit_ready"}' ;;
  esac
done
"#;

fn ledger_program(dir: &Path) -> PathBuf {
    let path = dir.join("ledger");
    std::fs::write(&path, LEDGER).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn built(program: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../target/debug")
        .join(program);
    assert!(path.exists(), "{} is missing: run `cargo build` first", path.display());
    path
}

fn quill_program(dir: &Path) -> PathBuf {
    let path = dir.join("quill");
    std::fs::write(&path, QUILL).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

mod cells_checks;
mod settings_checks;
mod image_checks;
mod editor_checks;
mod raster_checks;
mod edit_checks;

/// A folder of the check's own, which it removes when done.
fn scratch(name: &str) -> PathBuf {
    let base = std::env::var_os("TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from);
    let dir = base.join(format!("inpainter-checks-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A scripted OpenRouter: every chat request is recorded, and answered with
/// the next scripted reply.
/// The picture the stand-in makes: a 6 by 4 PNG.
const STAND_IN_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAYAAAAECAIAAAAiZtkUAAAAHklEQVR4nGMU0ThxKSCBAQkwPqvQYEAFTAwYAIsQAAvOBBOk9STxAAAAAElFTkSuQmCC";

struct StandIn {
    url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    /// Each request's `Authorization` header, if it had one, in order.
    keys: Arc<Mutex<Vec<Option<String>>>>,
    /// What a model list is answered with.
    models: Arc<Mutex<Value>>,
    /// Each picture asked for, by its request's body. Each is answered with
    /// `STAND_IN_PNG`.
    pictures: Arc<Mutex<Vec<Value>>>,
    replies: Arc<Mutex<std::collections::VecDeque<Value>>>,
    /// While held, a streamed text reply sends its first part, flushes, and
    /// waits for `release` before it sends the rest, as a slow model does.
    gate: Arc<(Mutex<Gate>, std::sync::Condvar)>,
}

#[derive(Default)]
struct Gate {
    held: bool,
}

impl StandIn {
    fn start(replies: Vec<Value>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let keys = Arc::new(Mutex::new(Vec::new()));
        let seen_keys = keys.clone();
        let models = Arc::new(Mutex::new(json!({"data": []})));
        let listed = models.clone();
        let pictures = Arc::new(Mutex::new(Vec::new()));
        let pictured = pictures.clone();
        let replies = Arc::new(Mutex::new(replies.into_iter().collect::<std::collections::VecDeque<_>>()));
        let script = replies.clone();
        let gate = Arc::new((Mutex::new(Gate::default()), std::sync::Condvar::new()));
        let held = gate.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut length = 0;
                let mut authorization = None;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header.trim().is_empty() {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap();
                        }
                        if name.eq_ignore_ascii_case("authorization") {
                            authorization = Some(value.trim().to_string());
                        }
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                // OpenRouter's image API: the picture as base64, and its
                // catalogue of image models.
                let path = request_line.split_whitespace().nth(1).unwrap_or_default().to_string();
                if path.ends_with("/images") || path.ends_with("/images/models") {
                    let reply = if path.ends_with("/images") {
                        pictured.lock().unwrap().push(serde_json::from_slice::<Value>(&body).unwrap());
                        json!({"data": [{"b64_json": STAND_IN_PNG}]})
                    } else {
                        json!({"data": [{"id": crate::workspace::IMAGE_MODEL, "name": "OpenAI: GPT Image 2.5"}]})
                    };
                    let text = reply.to_string();
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                        text.len()
                    );
                    continue;
                }
                let reply = if request_line.starts_with("GET") {
                    listed.lock().unwrap().clone()
                } else {
                    seen_keys.lock().unwrap().push(authorization);
                    seen.lock()
                        .unwrap()
                        .push(serde_json::from_slice(&body).unwrap());
                    script
                        .lock()
                        .unwrap()
                        .pop_front()
                        .unwrap_or_else(|| text_reply("Nothing more."))
                };
                let (body, content_type) = if request_line.starts_with("GET") {
                    (reply.to_string(), "application/json")
                } else {
                    streamed(&reply)
                };
                // No length: the body ends when the connection closes, so
                // each part reaches the reader as it is written.
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\nconnection: close\r\n\r\n"
                );
                let parts: Vec<&str> = body.split_inclusive("\n\n").collect();
                for (index, part) in parts.iter().enumerate() {
                    let _ = stream.write_all(part.as_bytes());
                    let _ = stream.flush();
                    if index == 0 && parts.len() > 2 && content_type == "text/event-stream" {
                        let (lock, wake) = &*held;
                        let mut state = lock.lock().unwrap();
                        while state.held {
                            state = wake.wait(state).unwrap();
                        }
                    }
                }
            }
        });
        Self {
            url,
            requests,
            keys,
            models,
            pictures,
            replies,
            gate,
        }
    }

    /// The `Authorization` header of each request, in order.
    fn keys(&self) -> Vec<Option<String>> {
        self.keys.lock().unwrap().clone()
    }

    /// Every picture asked for, by its request's body.
    fn pictures(&self) -> Vec<Value> {
        self.pictures.lock().unwrap().clone()
    }

    /// A model list asked for is answered with `list`.
    fn list_models(&self, list: Value) {
        *self.models.lock().unwrap() = list;
    }

    /// Streamed replies pause after their first part until `release`.
    fn hold(&self) {
        self.gate.0.lock().unwrap().held = true;
    }

    fn release(&self) {
        self.gate.0.lock().unwrap().held = false;
        self.gate.1.notify_all();
    }

    /// Replaces the replies still to come.
    fn script(&self, replies: Vec<Value>) {
        *self.replies.lock().unwrap() = replies.into_iter().collect();
    }

    fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
}

/// A chat reply as server-sent events. Text is two parts, so a cell shows
/// it arriving. A tool call is one event carrying the whole message.
fn streamed(reply: &Value) -> (String, &'static str) {
    let Some(message) = reply.pointer("/choices/0/message") else {
        return (reply.to_string(), "application/json");
    };
    if message.get("tool_calls").is_some() {
        let body = format!(
            "data: {}\n\ndata: [DONE]\n",
            json!({ "choices": [{ "message": message }] })
        );
        return (body, "text/event-stream");
    }
    let Some(text) = message.get("content").and_then(Value::as_str) else {
        return (reply.to_string(), "application/json");
    };
    let count = text.chars().count();
    let Some(mid) = text.char_indices().nth(count / 2).map(|(index, _)| index).filter(|_| count >= 2) else {
        return (reply.to_string(), "application/json");
    };
    let (first, second) = text.split_at(mid);
    let body = format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n",
        json!({ "choices": [{ "delta": { "role": "assistant", "content": first } }] }),
        json!({ "choices": [{ "delta": { "content": second } }] }),
    );
    (body, "text/event-stream")
}

fn text_reply(text: &str) -> Value {
    json!({"choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]})
}

fn call_reply(name: &str, arguments: Value) -> Value {
    json!({"choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [
        {"id": "t1", "type": "function", "function": {"name": name, "arguments": arguments.to_string()}}
    ]}, "finish_reason": "tool_calls"}]})
}

/// The real window, its frames run by hand, with the assistant pointed at
/// the stand-in.
struct Window {
    ctx: egui::Context,
    app: HostWindow,
    frame: eframe::Frame,
    /// The logo's Quit asked the window to close; the next frame honors it.
    close_next: bool,
    /// Pointer and text events for the next frame, as a person would send them.
    events: Vec<egui::Event>,
    /// The texts the last frame painted, with where each was drawn.
    texts: Vec<(String, Rect)>,
    /// Where the last frame painted pictures, larger than the logo.
    pictures: Vec<Rect>,
    /// Everything the last frame painted.
    shapes: Vec<egui::epaint::ClippedShape>,
}

impl Window {
    fn new(apps: Vec<Started>, stand_in: &StandIn) -> Self {
        let ctx = egui::Context::default();
        let mut app = HostWindow::with_apps(&ctx, apps, Tablet::none());
        let wake_ctx = ctx.clone();
        app.assistant = Assistant::spawn(Config {
            base_url: stand_in.url.clone(),
            api_key: Some(String::from("stand-in")),
            wake: Arc::new(move || wake_ctx.request_repaint()),
        });
        use_stand_in(&mut app, &stand_in.url);
        let mut window = Self {
            ctx,
            app,
            frame: eframe::Frame::_new_kittest(),
            close_next: false,
        texts: Vec::new(),
        pictures: Vec::new(),
        shapes: Vec::new(),
            events: Vec::new(),
        };
        // Every application here declares skills just after its greeting.
        window.until(|app| {
            app.apps
                .iter()
                .all(|started| !started.running.pending() && !started.running.declared.skills.is_empty())
        });
        window
    }

    fn step(&mut self) {
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1280.0, 800.0))),
            ..Default::default()
        };
        if self.close_next {
            self.close_next = false;
            input
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .events
                .push(egui::ViewportEvent::Close);
        }
        input.events.append(&mut self.events);
        let Self { ctx, app, frame, .. } = self;
        let mut output = ctx.run_ui(input, |ui| app.ui(ui, frame));
        output.textures_delta.clear();
        self.shapes = output.shapes.clone();
        self.texts = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some((text.galley.text().to_string(), text.visual_bounding_rect())),
                _ => None,
            })
            .collect();
        self.pictures = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect) if rect.brush.is_some() && rect.rect.width() > 30.0 => Some(rect.rect),
                _ => None,
            })
            .collect();
        for viewport in output.viewport_output.values() {
            if viewport
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Close))
            {
                self.close_next = true;
            }
        }
    }

    fn until(&mut self, done: impl Fn(&HostWindow) -> bool) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(20) {
            for started in &mut self.app.apps {
                started.running.poll();
            }
            self.step();
            if done(&self.app) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let state: Vec<String> = self
            .app
            .apps
            .iter()
            .map(|started| {
                format!(
                    "{} pending={} exited={} tabs={:?} quit_ready={} ask={:?}",
                    started.id,
                    started.running.pending(),
                    started.running.exited(),
                    started.running.tabs(),
                    started.running.declared.quit_ready,
                    started.running.declared.ask.as_ref().map(|ask| &ask.title),
                )
            })
            .collect();
        let entries: Vec<String> = self
            .app
            .chat
            .conversation
            .iter()
            .map(|entry| {
                format!(
                    "{:?} ok={:?} tool={:?} cell={} text={:?}",
                    entry.role,
                    entry.ok,
                    entry.tool,
                    entry.cell.is_some(),
                    entry.text.chars().take(40).collect::<String>()
                )
            })
            .collect();
        panic!(
            "the window never got there: project {:?}, question {:?}, working {}, {entries:?}, {state:#?}",
            self.app.project.as_ref().map(|project| &project.id),
            self.app.flows.question_shown(),
            self.app.chat.working,
        );
    }

    /// Types `text` in the chat and sends it, as the person does.
    fn ask(&mut self, text: &str) {
        self.app.chat.input = text.to_string();
        let ctx = self.ctx.clone();
        let Self { app, frame, .. } = self;
        let before = app.chat.conversation.len();
        app.chat_event(ChatEvent::Send, &ctx, frame);
        // A short request can start and end between two frames, so wait for
        // its reply rather than for the working state.
        self.until(move |app| {
            !app.chat.working
                && app.chat.conversation[before..]
                    .iter()
                    .any(|entry| matches!(entry.role, Role::Assistant | Role::Note))
        });
    }

    fn select(&mut self, app_id: &str) {
        let index = self
            .app
            .apps
            .iter()
            .position(|started| started.id == app_id)
            .unwrap();
        self.app.selected = index;
        self.step();
    }
}

fn start(id: &str, name: &str, program: &Path) -> Started {
    Started {
        id: id.to_string(),
        name: name.to_string(),
        executable: Some(program.to_path_buf()),
        running: Running::start(program, None, false),
    }
}

fn offered(request: &Value) -> Vec<String> {
    let mut names: Vec<String> = request["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .map(|tool| tool["function"]["name"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn declared(app: &HostWindow) -> Vec<String> {
    let mut names: Vec<String> = app
        .apps
        .iter()
        .flat_map(|started| {
            started.running.declared.skills.iter().flat_map(move |skill| {
                skill
                    .tools
                    .iter()
                    .map(move |tool| protocol::tool_key(&started.id, &skill.id, &tool.name))
            })
        })
        .collect();
    for tool in ["run_text_prompt", crate::route::EDIT_PICTURE] {
        names.push(protocol::tool_key(crate::route::HOST_ID, "prompt", tool));
    }
    names.sort();
    names
}

/// Catches an assistant whose knowledge of an application is written into
/// its prompts or code: a skill nothing was written for is offered, chosen,
/// routed to its own process, and its answer reaches the model.
#[test]
fn an_unknown_skill_is_offered_routed_and_answered() {
    let dir = scratch("unknown-skill");
    let quill = quill_program(&dir);
    let stand_in = StandIn::start(vec![
        call_reply("quillwork__quill__stamp_glyph", json!({"tab": "q1", "glyph": "*"})),
        text_reply("Stamped."),
    ]);
    let mut window = Window::new(
        vec![
            start("template", "Template", &built("template")),
            start("quillwork", "Quillwork", &quill),
        ],
        &stand_in,
    );
    window.select("template");
    window.ask("Stamp a star on the quill board.");

    let requests = stand_in.requests();
    assert_eq!(requests.len(), 2, "the model was asked twice: {requests:#?}");
    // Exactly the declared tools of every running application, no more.
    assert_eq!(offered(&requests[0]), declared(&window.app));
    assert!(offered(&requests[0]).contains(&"quillwork__quill__stamp_glyph".to_string()));
    // The quill program's own answer is what the model read next.
    assert!(
        requests[1].to_string().contains("quill-mark-7"),
        "the result never reached the model"
    );
    let last = window.app.chat.conversation.last().unwrap();
    assert_eq!((last.role, last.text.as_str()), (Role::Assistant, "Stamped."));
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a host that offers tools by an application's id: a stand-in
/// registered as `simple-sketch` is offered with its own skill only.
#[test]
fn a_stand_in_under_a_known_id_offers_only_its_own_skill() {
    let dir = scratch("known-id");
    let quill = quill_program(&dir);
    let stand_in = StandIn::start(vec![text_reply("Nothing to do.")]);
    let mut window = Window::new(vec![start("simple-sketch", "Simple Sketch", &quill)], &stand_in);
    window.ask("What can you do?");
    let requests = stand_in.requests();
    assert_eq!(
        offered(&requests[0]),
        vec![
            "inpainter__prompt__edit_picture".to_string(),
            "inpainter__prompt__run_text_prompt".to_string(),
            "simple-sketch__quill__stamp_glyph".to_string(),
        ]
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// The marks the host draws for a tab: what Simple Sketch last sent it.
fn marks(app: &HostWindow, tab: &str) -> usize {
    app.apps[0].running.declared.documents[tab].sketch.scene(None).len()
}

/// Catches an assistant that works on its own copy, or a host that keeps a
/// history: the assistant's two shapes arrive as Simple Sketch's documents,
/// and one undo, sent as the person's own, takes the whole request back.
#[test]
fn the_assistants_work_is_simple_sketchs_and_one_undo_takes_it_back() {
    let stand_in = StandIn::start(Vec::new());
    let mut window = Window::new(
        vec![start("simple-sketch", "Simple Sketch", &built("simple-sketch"))],
        &stand_in,
    );
    let tab = window.app.apps[0].running.tabs().1.expect("a sketch is open");
    window.until(|app| app.apps[0].running.declared.documents.contains_key(&tab));
    let before = marks(&window.app, &tab);
    stand_in.script(vec![
        call_reply(
            "simple-sketch__draw__add_shape",
            json!({"tab": tab, "kind": "ellipse", "from": [0, 0], "to": [100, 100]}),
        ),
        call_reply(
            "simple-sketch__draw__add_shape",
            json!({"tab": tab, "kind": "rectangle", "from": [140, 0], "to": [240, 100]}),
        ),
        text_reply("Done."),
    ]);
    window.ask("Create a circle and a square next to each other.");
    let after = marks(&window.app, &tab);
    assert!(after >= before + 2, "the shapes never reached the host: {before} then {after}");
    let requests = stand_in.requests();
    assert!(requests[1].to_string().contains("\\\"ok\\\":true"), "the ellipse failed");

    // The person's undo, as the toolbar sends it.
    let index = 0;
    window.app.send(
        index,
        protocol::HostMessage::Command {
            id: String::from("undo"),
            tab: Some(tab.clone()),
            arg: None,
            node: None,
            viewport: [1280.0, 800.0],
        },
    );
    let expected = before;
    window.until(move |app| marks(app, &tab) == expected);
}

impl Window {
    /// Answers the save dialogs the sub-applications asked for, as the
    /// person would, with `path`.
    fn answer_saves(&mut self, path: &Path) {
        for app in 0..self.app.apps.len() {
            while let Some(request) = self.app.apps[app].running.declared.files.pop_front() {
                self.app.deliver_file(app, request, Some(path.to_path_buf()));
            }
        }
    }

    fn logo(&mut self, choice: crate::chat::LogoChoice) {
        let ctx = self.ctx.clone();
        let Self { app, frame, .. } = self;
        app.logo(choice, &ctx, frame);
        self.step();
    }

    fn menu(&mut self, choice: crate::sessions::SessionChoice) {
        let Self { app, frame, .. } = self;
        app.session_choice(choice, frame);
        self.step();
    }

    /// The open project's id and folder, and the open session's id.
    fn here(&self) -> (String, PathBuf, String) {
        let project = self.app.project.as_ref().expect("a project is open");
        (project.id.clone(), project.folder.clone(), project.session.clone())
    }

    /// Steps until the open session is `session`, and every sub-application
    /// has reported its tabs. Fails if anything asks a question meanwhile.
    fn settle_on(&mut self, session: &str) {
        let session = session.to_string();
        self.until(move |app| {
            let asked: Vec<String> = app
                .apps
                .iter()
                .filter_map(|started| started.running.declared.ask.as_ref().map(|ask| ask.title.clone()))
                .collect();
            assert!(asked.is_empty(), "switching asked {asked:?}");
            app.project.as_ref().is_some_and(|project| project.session == session)
                && app.flows.idle()
                && app.apps.iter().all(|started| !started.running.pending() && started.running.tabs().1.is_some())
        });
        keep_ticking(self);
    }
}

/// Settings as a first start makes them, on the stand-in: its endpoint, with
/// a key, and every level on its model.
fn use_stand_in(app: &mut HostWindow, url: &str) -> String {
    let endpoint = app
        .settings
        .first_start(url)
        .or_else(|| app.settings.endpoints.first().map(|endpoint| endpoint.id.clone()))
        .expect("an endpoint");
    app.settings.update(|settings| {
        for level in &mut settings.levels {
            level.model = String::from("stand-in/model");
        }
    });
    app.keys.set(&endpoint, "stand-in").expect("the key is kept");
    endpoint
}

/// A window on a fresh data folder: every given sub-application is started
/// inside the project the folder holds, which is a new one when it is empty.
fn project_window(specs: Vec<super::AppSpec>, stand_in: &StandIn, data: &Path) -> Window {
    let _ = std::fs::create_dir_all(data);
    let ctx = egui::Context::default();
    let mut app = HostWindow::with_apps(&ctx, Vec::new(), Tablet::none());
    app.launch = specs;
    let wake_ctx = ctx.clone();
    app.assistant = Assistant::spawn(Config {
        base_url: stand_in.url.clone(),
        api_key: Some(String::from("stand-in")),
        wake: Arc::new(move || wake_ctx.request_repaint()),
    });
    app.data = Some(data.to_path_buf());
    app.settings = crate::workspace::Settings::load(Some(data.join("settings.json")));
    use_stand_in(&mut app, &stand_in.url);
    app.open_current();
    let mut window = Window {
        ctx,
        app,
        frame: eframe::Frame::_new_kittest(),
        close_next: false,
        texts: Vec::new(),
        pictures: Vec::new(),
        shapes: Vec::new(),
        events: Vec::new(),
    };
    window.until(|app| {
        app.project.is_some()
            && app.apps.iter().all(|started| {
                !started.running.pending() && started.running.tabs().1.is_some()
            })
    });
    keep_ticking(&mut window);
    window
}

/// Steps long enough that a changed project writes itself. A forced write
/// would hide the absence of that once-a-second keep.
fn keep_ticking(window: &mut Window) {
    let start = Instant::now();
    let wait = super::flows::WRITE_EVERY + Duration::from_millis(50);
    while start.elapsed() < wait {
        window.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    window.step();
}

/// The open tabs, in order: sub-application, path, label, and camera.
fn open_tabs(app: &HostWindow) -> Vec<(String, Option<String>, String, Option<([f64; 2], f64)>)> {
    let mut tabs = Vec::new();
    for started in &app.apps {
        let (reported, _) = started.running.tabs();
        for tab in reported {
            let camera = started
                .running
                .declared
                .documents
                .get(&tab.id)
                .map(|display| (display.camera.origin, display.camera.zoom));
            tabs.push((started.id.clone(), tab.path, tab.label, camera));
        }
    }
    tabs
}

fn spec(id: &str, name: &str, program: &Path) -> super::AppSpec {
    super::AppSpec {
        id: id.to_string(),
        name: name.to_string(),
        executable: Some(program.to_path_buf()),
    }
}

fn both() -> Vec<super::AppSpec> {
    vec![
        spec("simple-sketch", "Simple Sketch", &built("simple-sketch")),
        spec("template", "Template", &built("template")),
    ]
}

/// Changes a saved sketch with no window: a separate Simple Sketch opens
/// it, adds a line, and saves it.
fn change_on_its_own(sketch: &Path) {
    use std::process::{Command as Program, Stdio};
    let mut child = Program::new(built("simple-sketch"))
        .arg(protocol::RESTORE_FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut say = |message: protocol::HostMessage| {
        input
            .write_all(protocol::encode_host_message(&message).as_bytes())
            .unwrap();
        input.flush().unwrap();
    };
    let mut wait = |found: &dyn Fn(&protocol::AppMessage) -> bool| -> protocol::AppMessage {
        for line in lines.by_ref() {
            let message = protocol::parse_app_message(&line.unwrap()).unwrap();
            if found(&message) {
                return message;
            }
        }
        panic!("Simple Sketch stopped before answering");
    };
    wait(&|message| matches!(message, protocol::AppMessage::End));
    say(protocol::HostMessage::Open {
        path: Some(sketch.display().to_string()),
    });
    let protocol::AppMessage::Tabs(tabs) =
        wait(&|message| matches!(message, protocol::AppMessage::Tabs(tabs) if !tabs.tabs.is_empty()))
    else {
        unreachable!()
    };
    let tab = tabs.active.clone();
    say(protocol::HostMessage::Invoke {
        call: String::from("c1"),
        tab: tab.clone(),
        tool: String::from("simple-sketch__draw__add_shape"),
        input: json!({"kind": "line", "from": [0, 200], "to": [240, 200]}),
        group: String::from("g1"),
    });
    let protocol::AppMessage::ToolResult(result) =
        wait(&|message| matches!(message, protocol::AppMessage::ToolResult(_)))
    else {
        unreachable!()
    };
    assert!(result.ok, "the line was refused: {:?}", result.output);
    say(protocol::HostMessage::Command {
        id: String::from("save"),
        tab: Some(tab),
        arg: None,
        node: None,
        viewport: [1280.0, 800.0],
    });
    wait(&|message| {
        matches!(message, protocol::AppMessage::Tabs(tabs) if tabs.tabs.iter().all(|tab| !tab.dirty))
    });
    say(protocol::HostMessage::Shutdown);
    child.wait().unwrap();
}

/// Changes the open tab and saves it through its own Save. In a project
/// that writes into the project's folder with no dialog; returns the file.
fn save_tab(window: &mut Window, app: usize) -> PathBuf {
    let tab = window.app.apps[app].running.tabs().1.clone().unwrap();
    for id in ["new_canvas", "save"] {
        window.app.send(
            app,
            protocol::HostMessage::Command {
                id: String::from(id),
                tab: Some(tab.clone()),
                arg: None,
                node: None,
                viewport: window.app.viewport,
            },
        );
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(20) {
        for started in &mut window.app.apps {
            started.running.poll();
        }
        window.step();
        let written = window.app.apps[app]
            .running
            .tabs()
            .0
            .into_iter()
            .find(|entry| entry.id == tab)
            .and_then(|entry| entry.path);
        if let Some(path) = written {
            keep_ticking(window);
            return PathBuf::from(path);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the tab was never written");
}

fn answer(window: &mut Window, button: &str) {
    let Some(index) = window
        .app
        .apps
        .iter()
        .position(|started| started.running.declared.ask.is_some())
    else {
        panic!("no question is showing");
    };
    let ask = window.app.apps[index].running.declared.ask.clone().unwrap();
    window.app.apps[index].running.declared.ask = None;
    window.app.note_answer(&ask, button);
    window.app.send(
        index,
        protocol::HostMessage::Answer {
            id: ask.id,
            button: button.to_string(),
        },
    );
    window.step();
}

/// First launch: an empty data folder opens on "Untitled Project", with the
/// workspace hidden and every installed sub-application running behind it.
#[test]
fn a_new_data_folder_opens_on_an_untitled_project() {
    let dir = scratch("first");
    let stand_in = StandIn::start(vec![text_reply("Hello.")]);
    let mut window = project_window(both(), &stand_in, &dir);
    let project = window.app.project.as_ref().unwrap();
    assert_eq!(project.name, "Untitled Project");
    assert!(!project.workspace_open);
    assert!(window.app.chat.input_centered, "the empty conversation's input is not centred");
    assert!(window.app.chat.conversation.iter().all(|entry| entry.role != Role::Person));
    let sketch = window
        .app
        .apps
        .iter()
        .find(|started| started.id == "simple-sketch")
        .expect("Simple Sketch is running");
    assert!(sketch.running.tabs().1.is_some(), "Simple Sketch has no tab");
    assert!(!sketch.running.declared.skills.is_empty());
    assert!(
        window.app.bar_rect.center().x > 1200.0,
        "the bar is not at the right edge: {:?}",
        window.app.bar_rect
    );
    window.ask("Hello.");
    assert!(!window.app.chat.input_centered, "the input stayed in the middle");
    assert!(window.app.chat.centered_column, "messages are not in a centred column");
    let _ = std::fs::remove_dir_all(dir);
}

/// The bar shows the workspace and hides it, and the project remembers which.
#[test]
fn the_bar_shows_and_hides_the_workspace() {
    let dir = scratch("bar");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    assert!(!window.app.project.as_ref().unwrap().workspace_open);
    let hidden = window.app.viewport;
    assert!(hidden[0] > 100.0 && hidden[1] > 100.0, "a hidden workspace has no size: {hidden:?}");
    window.app.toggle_workspace();
    window.step();
    assert!(window.app.project.as_ref().unwrap().workspace_open);
    // Open, the bar sits inside the workspace panel, right of the edge.
    let edge = crate::chat::width(1280.0, None);
    assert!(window.app.bar_rect.left() > edge, "the bar is inside the conversation: {:?}", window.app.bar_rect);
    let shown = window.app.viewport;
    assert!(
        (shown[0] - hidden[0]).abs() < 2.0 && (shown[1] - hidden[1]).abs() < 2.0,
        "hidden {hidden:?} and shown {shown:?} are different sizes"
    );
    let (_, folder, session) = window.here();
    let text = std::fs::read_to_string(crate::workspace::session_path(&folder, &session)).unwrap();
    assert!(text.contains("\"workspace_open\": true"), "{text}");
    window.app.toggle_workspace();
    window.step();
    assert!(!window.app.project.as_ref().unwrap().workspace_open);
    assert!(window.app.bar_rect.center().x > 1200.0);
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a bar that a tool underneath also takes: with Simple Sketch's
/// brush, a real press on the bar over the canvas, moving a little as a pen
/// does, closes the workspace and draws nothing.
#[test]
fn clicking_the_bar_over_the_canvas_draws_nothing() {
    let dir = scratch("bar-click");
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
    // A press that moves a little, as a pen's does.
    let press = |window: &mut Window, at: Pos2| {
        window.events.push(egui::Event::PointerMoved(at));
        window.step();
        window.events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
        window.step();
        for step in 1..=4 {
            window.events.push(egui::Event::PointerMoved(at + egui::vec2(0.0, step as f32 * 2.0)));
            window.step();
        }
        let end = at + egui::vec2(0.0, 8.0);
        window.events.push(egui::Event::PointerButton { pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
        window.step();
        keep_ticking(window);
    };
    // The same press on the open canvas draws, so the check below can tell.
    let bar = window.app.bar_rect.center();
    let before = marks(&window.app, &tab);
    press(&mut window, bar + egui::vec2(200.0, 0.0));
    window.until(|app| marks(app, &tab) > before);
    let before = marks(&window.app, &tab);
    press(&mut window, bar);
    assert!(!window.app.project.as_ref().unwrap().workspace_open, "the click did not reach the bar");
    assert_eq!(marks(&window.app, &tab), before, "the press on the bar drew");
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a press that closes the session menu and also reaches the
/// drawing beneath: with the menu open over Simple Sketch's brush, a press
/// on the canvas outside the menu closes it and draws nothing.
#[test]
fn a_press_that_closes_the_session_menu_draws_nothing() {
    let dir = scratch("menu-away");
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
    window.app.sessions.open = true;
    window.app.sessions.raise = true;
    keep_ticking(&mut window);
    let at = Pos2::new(1100.0, 500.0);
    window.events.push(egui::Event::PointerMoved(at));
    window.step();
    window.events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    window.step();
    for step in 1..=4 {
        window.events.push(egui::Event::PointerMoved(at + egui::vec2(0.0, step as f32 * 3.0)));
        window.step();
    }
    window.events.push(egui::Event::PointerButton { pos: at + egui::vec2(0.0, 12.0), button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    window.step();
    keep_ticking(&mut window);
    assert!(!window.app.sessions.open, "the press outside did not close the menu");
    assert_eq!(marks(&window.app, &tab), before, "the press that closed the menu drew");
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches the overlay left over the panel: after a press outside closes
/// the session menu, reopening it puts the panel back on top, so the
/// panel, not the overlay, is what a pointer inside it reaches.
#[test]
fn the_reopened_session_menu_is_above_its_overlay() {
    let dir = scratch("menu-reopen");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let press = |window: &mut Window, at: Pos2| {
        window.events.push(egui::Event::PointerMoved(at));
        window.step();
        for pressed in [true, false] {
            window.events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
            window.step();
        }
    };
    let open = |window: &mut Window| {
        window.app.sessions.open = true;
        window.app.sessions.raise = true;
        window.step();
        window.step();
    };
    open(&mut window);
    press(&mut window, Pos2::new(100.0, 700.0));
    assert!(!window.app.sessions.open, "the press outside did not close the menu");
    open(&mut window);
    let panel = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("session-panel"));
    assert_eq!(window.ctx.layer_id_at(Pos2::new(900.0, 500.0)), Some(panel), "the overlay is over the reopened panel");
    let _ = std::fs::remove_dir_all(dir);
}

/// Reach with the workspace hidden matches reach with it shown, including a
/// picture of the visible area and a new canvas, which need that size.
#[test]
fn reach_is_the_same_with_the_workspace_hidden() {
    let hidden = reach(false);
    let shown = reach(true);
    assert_eq!(hidden.picture_ok, shown.picture_ok);
    assert!(hidden.picture_ok, "the picture was refused: {}", hidden.picture);
    assert_eq!(hidden.canvas_ok, shown.canvas_ok);
    assert!(hidden.canvas_ok, "the new canvas was refused: {}", hidden.canvas);
    assert!(
        (hidden.viewport[0] - shown.viewport[0]).abs() < 2.0
            && (hidden.viewport[1] - shown.viewport[1]).abs() < 2.0,
        "hidden {:?} shown {:?}",
        hidden.viewport,
        shown.viewport
    );
}

struct Reach {
    picture_ok: bool,
    picture: String,
    canvas_ok: bool,
    canvas: String,
    viewport: [f64; 2],
}

fn reach(show: bool) -> Reach {
    let dir = scratch(if show { "reach-shown" } else { "reach-hidden" });
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(
        vec![spec("simple-sketch", "Simple Sketch", &built("simple-sketch"))],
        &stand_in,
        &dir,
    );
    if show {
        window.app.toggle_workspace();
        window.step();
    } else {
        assert!(!window.app.project.as_ref().unwrap().workspace_open);
    }
    let tab = window.app.apps[0].running.tabs().1.clone().unwrap();
    window.until(|app| app.apps[0].running.declared.documents.contains_key(&tab));
    window.until(|app| app.viewport[0] > 100.0);
    let viewport = window.app.viewport;
    stand_in.script(vec![
        call_reply(
            "simple-sketch__look__look_at",
            json!({"tab": tab, "visible_area": true, "size": 128}),
        ),
        call_reply("simple-sketch__edit__new_canvas", json!({"tab": tab})),
        text_reply("Done."),
    ]);
    window.ask("Picture what I see, then add a canvas.");
    let requests = stand_in.requests();
    assert!(requests.len() >= 3, "the model was not asked enough: {requests:#?}");
    let picture = requests[1].to_string();
    let canvas = requests[2].to_string();
    let _ = std::fs::remove_dir_all(dir);
    Reach {
        picture_ok: picture.contains("\\\"ok\\\":true"),
        picture,
        canvas_ok: canvas.contains("\\\"ok\\\":true"),
        canvas,
        viewport,
    }
}

/// The intent guide's project test: two sessions of one project, each with a
/// Simple Sketch tab and the template's, switched back and forth and after a
/// restart, each bring back their own conversation, tabs, order, and
/// selection, with nothing asked and nothing saved by hand. Every activity
/// is a file in the project's folder, and the session holds only its path
/// relative to that folder.
#[test]
fn each_session_keeps_its_own_conversation_and_tabs() {
    use crate::sessions::SessionChoice;
    let dir = scratch("sessions");
    let stand_in = StandIn::start(vec![text_reply("First, noted."), text_reply("Second, noted.")]);
    let mut window = project_window(both(), &stand_in, &dir);
    let (project, folder, first) = window.here();
    window.ask("Remember the first.");
    let sketch = save_tab(&mut window, 0);
    assert!(sketch.starts_with(&folder), "the sketch was written outside the project: {}", sketch.display());
    window.select("template");
    keep_ticking(&mut window);
    let first_tabs = open_tabs(&window.app);
    let first_conversation = window.app.chat.conversation.clone();

    window.menu(SessionChoice::NewSession { project: project.clone() });
    window.until(|app| app.project.as_ref().is_some_and(|open| open.session != first));
    let second = window.here().2;
    window.settle_on(&second);
    assert_eq!(window.app.project.as_ref().unwrap().session_name, "Untitled Session 2");
    assert!(window.app.chat.conversation.iter().all(|entry| entry.role != Role::Person), "the new session is not empty");
    window.ask("Remember the second.");
    keep_ticking(&mut window);
    let second_conversation = window.app.chat.conversation.clone();

    window.menu(SessionChoice::Open { project: project.clone(), session: first.clone() });
    window.settle_on(&first);
    window.until(|app| app.chat.conversation.len() >= first_conversation.len());
    assert_eq!(window.app.chat.conversation, first_conversation);
    assert_eq!(window.app.apps[window.app.selected].id, "template");
    let paths = |tabs: &[(String, Option<String>, String, Option<([f64; 2], f64)>)]| {
        tabs.iter().map(|tab| (tab.0.clone(), tab.1.clone())).collect::<Vec<_>>()
    };
    window.until(|app| app.apps[0].running.tabs().0.iter().any(|tab| tab.path.is_some()));
    assert_eq!(paths(&open_tabs(&window.app)), paths(&first_tabs), "the first session's tabs did not come back");

    let text = std::fs::read_to_string(crate::workspace::session_path(&folder, &first)).unwrap();
    let file = crate::workspace::SessionFile::parse(&text).unwrap();
    let relative = sketch.strip_prefix(&folder).unwrap().display().to_string();
    assert!(file.tabs.iter().any(|tab| tab.path.as_deref() == Some(relative.as_str())), "{text}");
    assert!(!text.contains(&folder.display().to_string()), "the session holds an absolute path:\n{text}");
    drop(window);

    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    assert_eq!(window.here().2, first, "a restart did not come back to the last session");
    window.until(|app| app.chat.conversation.len() >= first_conversation.len());
    assert_eq!(window.app.chat.conversation, first_conversation);
    window.menu(SessionChoice::Open { project, session: second.clone() });
    window.settle_on(&second);
    window.until(|app| app.chat.conversation.len() >= second_conversation.len());
    assert_eq!(window.app.chat.conversation, second_conversation);
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a project made somewhere other than chosen, or one made over
/// another folder's files: New Project makes the folder it names, with its
/// first session, and refuses a folder that holds something.
#[test]
fn a_new_project_goes_where_chosen_and_never_over_other_files() {
    use crate::sessions::SessionChoice;
    let dir = scratch("new-project");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let (first, _, _) = window.here();
    let full = dir.join("Full");
    std::fs::create_dir_all(&full).unwrap();
    std::fs::write(full.join("notes.txt"), "mine").unwrap();
    window.menu(SessionChoice::CreateProject { name: "Full".into(), location: full.display().to_string() });
    assert!(window.app.sessions.form_error.is_some(), "a folder with files in it was taken");
    assert_eq!(window.here().0, first);
    assert!(!full.join(".inpainter").exists());

    let lighthouse = dir.join("Studio projects").join("Lighthouse");
    window.menu(SessionChoice::CreateProject { name: "Lighthouse".into(), location: lighthouse.display().to_string() });
    window.until(|app| app.project.as_ref().is_some_and(|open| open.id != first));
    let session = window.here().2;
    window.settle_on(&session);
    let open = window.app.project.as_ref().unwrap();
    assert_eq!(open.name, "Lighthouse");
    assert_eq!(open.folder, lighthouse);
    assert_eq!(open.session_name, "Untitled Session");
    assert!(lighthouse.join(".inpainter").join("project.json").is_file());
    assert_eq!(std::fs::read_to_string(full.join("notes.txt")).unwrap(), "mine");
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a pin that does nothing: projects list the most recently used
/// first, and a pinned one goes above them.
#[test]
fn a_pinned_project_is_listed_first() {
    use crate::sessions::SessionChoice;
    let dir = scratch("pin");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let (first, _, _) = window.here();
    assert!(window.app.sessions.loaded && !window.app.sessions.recent.is_empty(), "the menu's lists were not read ahead");
    let second = dir.join("Second");
    window.menu(SessionChoice::CreateProject { name: "Second".into(), location: second.display().to_string() });
    window.until(|app| app.project.as_ref().is_some_and(|open| open.id != first));
    let session = window.here().2;
    window.settle_on(&session);
    window.app.sessions.loaded = false;
    window.menu(SessionChoice::Opened);
    assert!(window.app.sessions.loaded, "opening the menu did not read its lists");
    assert_ne!(window.app.sessions.projects[0].id, first, "the most recently used project is not first");
    window.menu(SessionChoice::PinProject { project: first.clone(), pinned: true });
    assert_eq!(window.app.sessions.projects[0].id, first, "the pinned project is not first");
    assert!(window.app.sessions.projects[0].pinned);
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a lock that holds nothing: a locked project refuses Rename,
/// Archive, Delete, and its sessions' Delete, and Unlock frees them again.
#[test]
fn a_locked_project_holds_its_name_and_its_sessions() {
    use crate::sessions::SessionChoice;
    let dir = scratch("lock");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let (project, folder, session) = window.here();
    window.menu(SessionChoice::LockProject { project: project.clone(), locked: true });
    let name = window.app.project.as_ref().unwrap().name.clone();
    window.menu(SessionChoice::RenameProject { project: project.clone(), name: "Changed".into() });
    window.menu(SessionChoice::ArchiveProject { project: project.clone(), archived: true });
    window.menu(SessionChoice::DeleteProject { project: project.clone() });
    window.menu(SessionChoice::DeleteSession { project: project.clone(), session: session.clone() });
    keep_ticking(&mut window);
    assert_eq!(window.here().0, project, "a locked project was left");
    assert_eq!(window.app.project.as_ref().unwrap().name, name, "a locked project was renamed");
    let row = window.app.sessions.projects.iter().find(|row| row.id == project).unwrap().clone();
    assert!(row.locked && !row.archived);
    assert!(crate::workspace::SessionFile::read(&folder, &session).unwrap().trashed_at.is_none());
    assert!(window.app.chat.conversation.iter().any(|entry| entry.text.contains("locked")), "nothing said why");
    window.menu(SessionChoice::LockProject { project: project.clone(), locked: false });
    window.menu(SessionChoice::RenameProject { project: project.clone(), name: "Changed".into() });
    assert_eq!(window.app.project.as_ref().unwrap().name, "Changed");
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches an archive that deletes or still shows: an archived project
/// leaves the Projects list and Recent but stays in All Projects, the open
/// one gives way to another, and Unarchive brings it back.
#[test]
fn an_archived_project_leaves_the_lists_and_comes_back() {
    use crate::sessions::SessionChoice;
    let dir = scratch("archive");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let (project, folder, session) = window.here();
    window.menu(SessionChoice::ArchiveProject { project: project.clone(), archived: true });
    window.until(|app| app.project.as_ref().is_some_and(|open| open.id != project));
    let other = window.here().2;
    window.settle_on(&other);
    window.app.refresh_sessions();
    let row = window.app.sessions.projects.iter().find(|row| row.id == project).expect("an archived project left All Projects");
    assert!(row.archived);
    assert!(folder.join(".inpainter").is_dir(), "archiving removed the project");
    assert!(!window.app.sessions.trash.iter().any(|row| row.name == "Untitled Project" && matches!(&row.item, crate::sessions::Trashed::Project(id) if *id == project)));
    window.menu(SessionChoice::EmptyTrash);
    window.app.answer_question("empty");
    assert!(folder.join(".inpainter").is_dir(), "Empty Trash touched an archived project");
    window.menu(SessionChoice::ArchiveProject { project: project.clone(), archived: false });
    assert!(window.app.sessions.projects.iter().any(|row| row.id == project && !row.archived));
    assert!(window.app.sessions.recent.iter().any(|row| row.id == session));
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a delete that loses work: a deleted session and a deleted
/// project go to Trash and come back as they were, and Empty Trash asks
/// first, then keeps a deleted session's activities in its project.
#[test]
fn deleted_sessions_and_projects_come_back_from_trash() {
    use crate::sessions::{SessionChoice, Trashed};
    let dir = scratch("trash");
    let stand_in = StandIn::start(vec![text_reply("Noted.")]);
    let mut window = project_window(both(), &stand_in, &dir);
    let (project, folder, first) = window.here();
    window.ask("Keep this.");
    let sketch = save_tab(&mut window, 0);
    window.menu(SessionChoice::DeleteSession { project: project.clone(), session: first.clone() });
    window.until(|app| app.project.as_ref().is_some_and(|open| open.session != first));
    let other = window.here().2;
    window.settle_on(&other);
    window.app.refresh_sessions();
    assert!(window.app.sessions.trash.iter().any(|row| row.item == Trashed::Session { project: project.clone(), id: first.clone() }));
    assert!(!window.app.sessions.recent.iter().any(|row| row.id == first));
    window.menu(SessionChoice::Restore(Trashed::Session { project: project.clone(), id: first.clone() }));
    assert!(window.app.sessions.recent.iter().any(|row| row.id == first));
    window.menu(SessionChoice::Open { project: project.clone(), session: first.clone() });
    window.settle_on(&first);
    assert!(window.app.chat.conversation.iter().any(|entry| entry.text == "Keep this."));

    window.menu(SessionChoice::DeleteProject { project: project.clone() });
    window.until(|app| app.project.as_ref().is_some_and(|open| open.id != project));
    let made = window.here().2;
    window.settle_on(&made);
    assert!(folder.is_dir(), "deleting the project removed its folder");
    window.menu(SessionChoice::Restore(Trashed::Project(project.clone())));
    assert!(window.app.sessions.projects.iter().any(|row| row.id == project));

    window.menu(SessionChoice::DeleteSession { project: project.clone(), session: other.clone() });
    window.menu(SessionChoice::EmptyTrash);
    assert_eq!(window.app.flows.question_shown().as_deref(), Some("Empty Trash?"));
    assert!(window.app.flows.question_lines().join(" ").contains("system's trash"));
    window.app.answer_question("cancel");
    assert!(crate::workspace::session_path(&folder, &other).is_file());
    window.menu(SessionChoice::EmptyTrash);
    window.app.answer_question("empty");
    assert!(!crate::workspace::session_path(&folder, &other).exists());
    assert!(sketch.is_file(), "emptying a session's Trash removed its project's work");
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a file from outside the project opened in place: Open's file
/// from elsewhere is imported as a copy after the person says so, the
/// sub-application opens the copy, and the original is unchanged.
#[test]
fn an_outside_file_is_imported_never_opened_in_place() {
    let dir = scratch("import");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let (_, folder, _) = window.here();
    let elsewhere = dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let outside = save_tab(&mut window, 0);
    std::fs::rename(&outside, elsewhere.join("Shared.smx")).unwrap();
    let outside = elsewhere.join("Shared.smx");
    let before = std::fs::read(&outside).unwrap();
    let tab = window.app.apps[0].running.tabs().1.clone().unwrap();
    let open = |window: &mut Window| {
        window.app.send(
            0,
            protocol::HostMessage::Command { id: "open".into(), tab: Some(tab.clone()), arg: None, node: None, viewport: [800.0, 600.0] },
        );
        window.until(|app| !app.apps[0].running.declared.files.is_empty());
        let request = window.app.apps[0].running.declared.files.pop_front().unwrap();
        window.app.deliver_file(0, request, Some(outside.clone()));
    };
    open(&mut window);
    assert_eq!(window.app.flows.question_shown().as_deref(), Some("Import a copy into this project?"));
    window.app.answer_question("cancel");
    keep_ticking(&mut window);
    assert!(!window.app.apps[0].running.tabs().0.iter().any(|entry| entry.path.as_deref() == Some(outside.to_str().unwrap())));

    open(&mut window);
    window.app.answer_question("import");
    let copy = folder.join("Shared.smx");
    window.until(|app| app.apps[0].running.tabs().0.iter().any(|entry| entry.path.as_deref() == Some(copy.to_str().unwrap())));
    assert!(!window.app.apps.iter().any(|started| started.running.tabs().0.iter().any(|entry| entry.path.as_deref() == Some(outside.to_str().unwrap()))));
    assert_eq!(std::fs::read(&outside).unwrap(), before, "the original was changed");
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches an attachment the session points to outside the project: an
/// attached file is copied into the project's attachments, and the chip
/// holds the copy.
#[test]
fn an_attachment_is_copied_into_the_project() {
    let dir = scratch("attach");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let (_, folder, _) = window.here();
    let note = dir.join("note.txt");
    std::fs::write(&note, "from outside").unwrap();
    window.app.scripted_paths.push_back(note.clone());
    let ctx = window.ctx.clone();
    let Window { app, frame, .. } = &mut window;
    app.chat_event(crate::chat::ChatEvent::Attach, &ctx, frame);
    let chip = window.app.chat.chips.first().cloned().expect("a chip");
    assert_eq!(chip, folder.join("attachments").join("note.txt"));
    assert_eq!(std::fs::read_to_string(&chip).unwrap(), "from outside");
    // What was attached in one session does not follow into another.
    let (project, _, first) = window.here();
    window.app.chat.input = "Half a thought".into();
    window.menu(crate::sessions::SessionChoice::NewSession { project });
    window.until(|app| app.project.as_ref().is_some_and(|open| open.session != first));
    assert!(window.app.chat.chips.is_empty(), "an attachment followed into another session");
    assert!(window.app.chat.input.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a change that only Save keeps: a sketch changed by hand and
/// left at once, before it was written, is written by the switch, and the
/// session it was left in holds its file.
#[test]
fn a_change_left_at_once_is_written_by_the_switch() {
    use crate::sessions::SessionChoice;
    let dir = scratch("left-at-once");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let (project, folder, first) = window.here();
    let tab = window.app.apps[0].running.tabs().1.clone().unwrap();
    window.app.send(
        0,
        protocol::HostMessage::Command { id: "new_canvas".into(), tab: Some(tab), arg: None, node: None, viewport: [800.0, 600.0] },
    );
    window.menu(SessionChoice::NewSession { project });
    window.until(|app| app.project.as_ref().is_some_and(|open| open.session != first));
    let second = window.here().2;
    window.settle_on(&second);
    let file = crate::workspace::SessionFile::read(&folder, &first).unwrap();
    let written: Vec<String> = file.tabs.iter().filter_map(|tab| tab.path.clone()).collect();
    assert_eq!(written, vec!["Untitled 1.smx".to_string()], "the change left at once is not in its session");
    assert!(folder.join("Untitled 1.smx").is_file());
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a switch that loses work it could not write: with the project's
/// folder read-only, a changed sketch stops the switch, says so, and stays
/// open with its change.
#[test]
fn a_change_that_cannot_be_written_stops_the_switch() {
    use crate::sessions::SessionChoice;
    let dir = scratch("unwritable");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let (project, folder, first) = window.here();
    window.menu(SessionChoice::NewSession { project: project.clone() });
    window.until(|app| app.project.as_ref().is_some_and(|open| open.session != first));
    let second = window.here().2;
    window.settle_on(&second);
    let tab = window.app.apps[0].running.tabs().1.clone().unwrap();
    let mut permissions = std::fs::metadata(&folder).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&folder, permissions.clone()).unwrap();
    window.app.send(
        0,
        protocol::HostMessage::Command { id: "new_canvas".into(), tab: Some(tab), arg: None, node: None, viewport: [800.0, 600.0] },
    );
    window.menu(SessionChoice::Open { project, session: first });
    window.until(|app| app.flows.idle() && app.chat.conversation.iter().any(|entry| entry.text.contains("could not be written")));
    permissions.set_readonly(false);
    std::fs::set_permissions(&folder, permissions).unwrap();
    assert_eq!(window.here().2, second, "the switch went on");
    assert!(window.app.apps[0].running.tabs().0.iter().any(|tab| tab.dirty), "the change was lost");
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches projects from before 0.7 left behind: at start, an old project
/// in the data folder moves into a folder of its own, its conversation
/// becomes its first session, and its sketch saved elsewhere is copied in
/// and reopens from the copy.
#[test]
fn an_old_project_moves_into_a_folder_with_nothing_lost() {
    let dir = scratch("migrate");
    let elsewhere = dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let sketch = save_tab(&mut window, 0);
    std::fs::rename(&sketch, elsewhere.join("Harbor.smx")).unwrap();
    drop(window);
    let _ = std::fs::remove_file(dir.join("projects.json"));
    let _ = std::fs::remove_dir_all(dir.join("Inpainter"));
    let old = dir.join("projects").join("old-one");
    std::fs::create_dir_all(&old).unwrap();
    let file = serde_json::json!({
        "version": 1,
        "name": "Harbor",
        "tabs": [{"app": "simple-sketch", "path": elsewhere.join("Harbor.smx").display().to_string(), "label": "Harbor"}],
        "selected": 0,
        "workspace_open": true,
        "conversation": [{"role": "person", "text": "The harbor at night.", "time": 1}]
    });
    std::fs::write(old.join("project.json"), file.to_string()).unwrap();

    let stand_in = StandIn::start(Vec::new());
    let window = project_window(both(), &stand_in, &dir);
    let (_, folder, _) = window.here();
    assert_eq!(folder, dir.join("Inpainter").join("Harbor"));
    assert_eq!(window.app.project.as_ref().unwrap().name, "Harbor");
    assert!(window.app.chat.conversation.iter().any(|entry| entry.text == "The harbor at night."));
    let copy = folder.join("Harbor.smx");
    assert!(copy.is_file(), "the sketch was not copied in");
    assert!(elsewhere.join("Harbor.smx").is_file(), "the original moved");
    assert!(window.app.apps[0].running.tabs().0.iter().any(|entry| entry.path.as_deref() == Some(copy.to_str().unwrap())));
    assert!(dir.join("projects-before-0.7").join("old-one").join("project.json").is_file(), "no backup was kept");
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a host that knows its sub-applications by name: one nothing
/// was written for, with ids of its own, is told the project's folder, and
/// its work lands there.
#[test]
fn any_application_is_told_the_project_and_saves_into_it() {
    let dir = scratch("ledger");
    let ledger = ledger_program(&dir);
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(vec![spec("ledger", "Ledger", &ledger)], &stand_in, &dir);
    let (_, folder, _) = window.here();
    window.app.send(
        0,
        protocol::HostMessage::Command {
            id: String::from("scribble"),
            tab: Some(String::from("l1")),
            arg: None,
            node: None,
            viewport: [800.0, 600.0],
        },
    );
    let written = folder.join("Ledger.ledger");
    window.until(|app| app.apps[0].running.tabs().0.iter().any(|tab| tab.path.as_deref() == Some(written.to_str().unwrap())));
    assert_eq!(std::fs::read_to_string(&written).unwrap(), "ledger");
    let _ = std::fs::remove_dir_all(dir);
}

/// A change is written on its own. Dropping the window, without closing it,
/// still finds the change in the session.
#[test]
fn a_change_is_kept_without_closing() {
    let dir = scratch("kept-quietly");
    let stand_in = StandIn::start(vec![text_reply("Noted.")]);
    let mut window = project_window(both(), &stand_in, &dir);
    window.ask("Remember this.");
    keep_ticking(&mut window);
    let session = window.here().2;
    drop(window);

    let stand_in = StandIn::start(Vec::new());
    let window = project_window(both(), &stand_in, &dir);
    assert_eq!(window.here().2, session);
    assert!(
        window.app.chat.conversation.iter().any(|entry| entry.text == "Remember this."),
        "the session was not written on its own"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A sketch changed by Simple Sketch with no host shows when the session
/// reopens, and the session holds the sketch's path, never its content.
#[test]
fn a_change_made_with_no_host_shows_when_the_session_reopens() {
    let dir = scratch("reopen");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let (_, folder, session) = window.here();
    let sketch = save_tab(&mut window, 0);
    drop(window);
    let text = std::fs::read_to_string(crate::workspace::session_path(&folder, &session)).unwrap();
    assert!(!text.contains("\"nodes\"") && !text.contains("\"strokes\""), "the session holds the sketch:\n{text}");

    change_on_its_own(&sketch);
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let reopened = window.app.apps[0].running.tabs().1.clone().unwrap();
    window.until(|app| app.apps[0].running.declared.documents.contains_key(&reopened));
    assert!(marks(&window.app, &reopened) >= 1, "the change saved on its own is not in the reopened tab");
    let _ = std::fs::remove_dir_all(dir);
}

/// Quit starts the same close as the window's own close button.
#[test]
fn quit_closes_like_the_window_button() {
    let dir = scratch("quit");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    window.logo(crate::chat::LogoChoice::Quit);
    window.step();
    assert!(window.app.flows.closing_started());
    assert!(window.app.flows.question_shown().is_none());
    let _ = std::fs::remove_dir_all(dir);
}
