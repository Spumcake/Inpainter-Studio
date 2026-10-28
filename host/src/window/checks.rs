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
use crate::workspace::ModelChoice;
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

/// A sub-application nothing was written for, whose one untitled ledger can
/// be saved. Its quit question, buttons, and file request use words of its
/// own. Like Simple Sketch, it writes and closes the ledger in one step, so
/// no tab report ever shows the ledger with its file.
const LEDGER: &str = r#"#!/bin/sh
dirty=0
echo '{"type":"hello","protocol":1}'
echo '{"type":"tab","id":"l1","label":"Untitled Ledger"}'
echo '{"type":"contents","tab":"l1","kind":"grid"}'
echo '{"type":"end"}'
echo '{"type":"tabs","tabs":[{"id":"l1","label":"Untitled Ledger","saves":true}],"active":"l1"}'
while IFS= read -r line; do
  case "$line" in
    *'"type":"ping"'*) echo '{"type":"pong"}' ;;
    *'"type":"shutdown"'*) exit 0 ;;
    *'"type":"command"'*) dirty=1 ;;
    *'"type":"quit"'*)
      if [ "$dirty" = 1 ]; then
        echo '{"type":"ask","id":"ledger-unwritten","title":"Write the ledger?","buttons":[{"id":"write","label":"Write"},{"id":"toss","label":"Toss"},{"id":"stay","label":"Stay"}],"dismiss":"stay"}'
      else
        echo '{"type":"quit_ready"}'
      fi ;;
    *'"type":"answer"'*'"button":"write"'*) echo '{"type":"file","id":"ledger-file","kind":"save"}' ;;
    *'"type":"answer"'*'"button":"toss"'*|*'"type":"file_result"'*'"path"'*)
      dirty=0
      echo '{"type":"tabs","tabs":[{"id":"l2","label":"Untitled Ledger","saves":true}],"active":"l2"}'
      echo '{"type":"quit_ready"}' ;;
    *'"type":"open"'*'"path"'*)
      path=$(printf '%s' "$line" | sed 's/.*"path":"\([^"]*\)".*/\1/')
      echo "{\"type\":\"tabs\",\"tabs\":[{\"id\":\"l1\",\"label\":\"Untitled Ledger\",\"saves\":true},{\"id\":\"l3\",\"label\":\"Ledger\",\"path\":\"$path\",\"saves\":true}],\"active\":\"l3\"}" ;;
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

/// A folder of the check's own, which it removes when done.
fn scratch(name: &str) -> PathBuf {
    let base = std::env::var_os("TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from);
    let dir = base.join(format!("inpainter-checks-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A scripted OpenRouter: every chat request is recorded, and answered with
/// the next scripted reply.
struct StandIn {
    url: String,
    requests: Arc<Mutex<Vec<Value>>>,
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
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header.trim().is_empty() {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let reply = if request_line.starts_with("GET") {
                    json!({"data": []})
                } else {
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
            replies,
            gate,
        }
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
        app.openrouter_url = stand_in.url.clone();
        app.openrouter_key = Some(String::from("stand-in"));
        app.chat.model = Some(ModelChoice {
            id: String::from("stand-in/model"),
            name: String::from("Stand-in"),
        });
        let mut window = Self {
            ctx,
            app,
            frame: eframe::Frame::_new_kittest(),
            close_next: false,
        texts: Vec::new(),
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
        self.texts = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some((text.galley.text().to_string(), text.visual_bounding_rect())),
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
    names.push(protocol::tool_key(
        crate::route::HOST_ID,
        "prompt",
        "run_text_prompt",
    ));
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

    fn menu(&mut self, choice: crate::chat::ProjectChoice) {
        let Self { app, frame, .. } = self;
        app.project_menu(choice, frame);
        self.step();
    }
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
    app.openrouter_url = stand_in.url.clone();
    app.openrouter_key = Some(String::from("stand-in"));
    app.chat.model = Some(ModelChoice {
        id: String::from("stand-in/model"),
        name: String::from("Stand-in"),
    });
    app.data = Some(data.to_path_buf());
    app.settings = crate::workspace::Settings::load(Some(data.join("settings.json")));
    app.open_current();
    let mut window = Window {
        ctx,
        app,
        frame: eframe::Frame::_new_kittest(),
        close_next: false,
        texts: Vec::new(),
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

/// Saves the open tab through its own Save, answering the file dialog with `path`.
fn save_tab(window: &mut Window, app: usize, path: &Path) {
    let tab = window.app.apps[app].running.tabs().1.clone().unwrap();
    window.app.send(
        app,
        protocol::HostMessage::Command {
            id: String::from("save"),
            tab: Some(tab),
            arg: None,
            node: None,
            viewport: window.app.viewport,
        },
    );
    let start = Instant::now();
    let wanted = path.display().to_string();
    while start.elapsed() < Duration::from_secs(20) {
        window.answer_saves(path);
        for started in &mut window.app.apps {
            started.running.poll();
        }
        window.step();
        let has = window.app.apps[app]
            .running
            .tabs()
            .0
            .iter()
            .any(|tab| tab.path.as_deref() == Some(wanted.as_str()));
        if has {
            keep_ticking(window);
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the tab was never kept as {}", path.display());
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
    let shown = window.app.viewport;
    assert!(
        (shown[0] - hidden[0]).abs() < 2.0 && (shown[1] - hidden[1]).abs() < 2.0,
        "hidden {hidden:?} and shown {shown:?} are different sizes"
    );
    let id = window.app.project.as_ref().unwrap().id.clone();
    let text = std::fs::read_to_string(crate::workspace::project_path(&dir, &id)).unwrap();
    assert!(text.contains("\"workspace_open\": true"), "{text}");
    window.app.toggle_workspace();
    window.step();
    assert!(!window.app.project.as_ref().unwrap().workspace_open);
    assert!(window.app.bar_rect.center().x > 1200.0);
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

/// Rename keeps the id and everything else, and the name at the top changes.
#[test]
fn renaming_changes_only_the_name() {
    let dir = scratch("rename");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let id = window.app.project.as_ref().unwrap().id.clone();
    let before = crate::workspace::ProjectFile::read(&crate::workspace::project_path(&dir, &id)).unwrap();
    window.app.commit_rename("Notes".into());
    window.step();
    assert_eq!(window.app.project.as_ref().unwrap().name, "Notes");
    assert_eq!(window.app.project.as_ref().unwrap().id, id);
    let file = crate::workspace::ProjectFile::read(&crate::workspace::project_path(&dir, &id)).unwrap();
    assert_eq!(file.name, "Notes");
    assert_eq!(file.tabs, before.tabs);
    assert_eq!(file.folders, before.folders);
    assert_eq!(file.conversation, before.conversation);
    let _ = std::fs::remove_dir_all(dir);
}

/// A new project starts empty, after unsaved work is resolved. Dismissing
/// the question cancels it, and the previous project stays as it was.
#[test]
fn a_new_project_waits_for_unsaved_work_and_keeps_the_previous_one() {
    let dir = scratch("new-project");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let id = window.app.project.as_ref().unwrap().id.clone();
    let tab = window.app.apps[0].running.tabs().1.clone().unwrap();
    stand_in.script(vec![
        call_reply(
            "simple-sketch__draw__add_shape",
            json!({"tab": tab, "kind": "ellipse", "from": [0, 0], "to": [40, 40]}),
        ),
        text_reply("Drawn."),
    ]);
    window.ask("Draw a circle.");
    window.menu(crate::chat::ProjectChoice::New);
    window.until(|app| app.apps.iter().any(|started| started.running.declared.ask.is_some()));
    let title = window
        .app
        .apps
        .iter()
        .find_map(|started| started.running.declared.ask.as_ref().map(|ask| ask.title.clone()))
        .unwrap();
    assert_eq!(title, "Keep this sketch?");
    answer(&mut window, "cancel");
    assert!(!window.app.flows.closing_started(), "dismissing the question did not cancel");
    assert_eq!(window.app.project.as_ref().unwrap().id, id);
    assert_eq!(crate::workspace::list_projects(&dir).len(), 1);

    window.menu(crate::chat::ProjectChoice::New);
    window.until(|app| app.apps.iter().any(|started| started.running.declared.ask.is_some()));
    answer(&mut window, "discard");
    window.until(|app| app.project.as_ref().is_some_and(|project| project.id != id));
    assert!(
        window
            .app
            .chat
            .conversation
            .iter()
            .all(|entry| entry.role != Role::Person),
        "the new conversation is not empty"
    );
    let previous = crate::workspace::ProjectFile::read(&crate::workspace::project_path(&dir, &id)).unwrap();
    assert!(previous.conversation.iter().any(|entry| entry.text == "Draw a circle."));
    let _ = std::fs::remove_dir_all(dir);
}

/// Save on the quit question writes the sketch, and Simple Sketch then closes
/// it. The project still holds that path, and switching back reopens it.
#[test]
fn a_sketch_saved_while_quitting_stays_in_the_project() {
    let dir = scratch("quit-save");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let sketch = dir.join("kept.smx");
    save_tab(&mut window, 0, &sketch);
    let tab = window.app.apps[0].running.tabs().1.clone().unwrap();
    stand_in.script(vec![
        call_reply(
            "simple-sketch__draw__add_shape",
            json!({"tab": tab, "kind": "ellipse", "from": [0, 0], "to": [40, 40]}),
        ),
        text_reply("Drawn."),
    ]);
    window.ask("Draw a circle.");
    let id = window.app.project.as_ref().unwrap().id.clone();
    window.menu(crate::chat::ProjectChoice::New);
    window.until(|app| app.apps.iter().any(|started| started.running.declared.ask.is_some()));
    answer(&mut window, "save");
    window.until(|app| {
        app.project.as_ref().is_some_and(|project| project.id != id)
            && app.apps.iter().all(|started| !started.running.pending() && started.running.tabs().1.is_some())
    });
    let wanted = sketch.display().to_string();
    let file = crate::workspace::ProjectFile::read(&crate::workspace::project_path(&dir, &id)).unwrap();
    assert!(
        file.tabs.iter().any(|tab| tab.path.as_deref() == Some(wanted.as_str())),
        "the saved sketch left the project:\n{}",
        file.to_text()
    );
    window.menu(crate::chat::ProjectChoice::Switch(id.clone()));
    window.until(|app| {
        app.project.as_ref().is_some_and(|project| project.id == id)
            && app.apps.iter().any(|started| {
                started
                    .running
                    .tabs()
                    .0
                    .iter()
                    .any(|tab| tab.path.as_deref() == Some(wanted.as_str()))
            })
    });
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a quit that waits for ever: cancelling the save dialog a quit
/// question opened stops the new project, as dismissing the question does,
/// and the project menu works again.
#[test]
fn cancelling_the_save_dialog_while_quitting_stops_the_switch() {
    let dir = scratch("quit-cancel");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let id = window.app.project.as_ref().unwrap().id.clone();
    let tab = window.app.apps[0].running.tabs().1.clone().unwrap();
    stand_in.script(vec![
        call_reply(
            "simple-sketch__draw__add_shape",
            json!({"tab": tab, "kind": "ellipse", "from": [0, 0], "to": [40, 40]}),
        ),
        text_reply("Drawn."),
    ]);
    window.ask("Draw a circle.");
    window.menu(crate::chat::ProjectChoice::New);
    window.until(|app| app.apps.iter().any(|started| started.running.declared.ask.is_some()));
    answer(&mut window, "save");
    window.until(|app| !app.apps[0].running.declared.files.is_empty());
    while let Some(request) = window.app.apps[0].running.declared.files.pop_front() {
        window.app.send(0, protocol::HostMessage::FileResult { id: request.id, path: None });
    }
    keep_ticking(&mut window);
    assert!(!window.app.flows.closing_started(), "the cancelled save left the switch waiting");
    assert_eq!(window.app.project.as_ref().unwrap().id, id);
    // Simple Sketch sent its question again behind the dialog and has no
    // message to take it back, so it may still show. Cancel answers it.
    if window.app.apps.iter().any(|started| started.running.declared.ask.is_some()) {
        answer(&mut window, "cancel");
    }
    assert!(!window.app.flows.closing_started());

    window.menu(crate::chat::ProjectChoice::New);
    window.until(|app| app.apps.iter().any(|started| started.running.declared.ask.is_some()));
    answer(&mut window, "discard");
    window.until(|app| app.project.as_ref().is_some_and(|project| project.id != id));
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a project from before projects had folders: its save dialog
/// starts in a folder that exists.
#[test]
fn a_project_without_a_files_folder_gets_one() {
    let dir = scratch("no-files");
    let stand_in = StandIn::start(Vec::new());
    let window = project_window(both(), &stand_in, &dir);
    let id = window.app.project.as_ref().unwrap().id.clone();
    drop(window);
    let files = crate::workspace::project_files(&dir, &id);
    std::fs::remove_dir_all(&files).unwrap();
    let window = project_window(both(), &stand_in, &dir);
    assert_eq!(window.app.project.as_ref().unwrap().id, id);
    let start = window.app.dialog_folder(None).expect("no folder for the dialog");
    assert_eq!(start, files);
    assert!(files.is_dir(), "the project's folder was not made");
    let _ = std::fs::remove_dir_all(dir);
}

/// Catches a host that knows one sub-application's questions: an untitled
/// document a sub-application nothing was written for saves through the file
/// dialog during New Project stays in the project, and switching back
/// reopens it.
#[test]
fn any_applications_document_saved_while_quitting_stays_in_the_project() {
    let dir = scratch("ledger-save");
    let ledger = ledger_program(&dir);
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(vec![spec("ledger", "Ledger", &ledger)], &stand_in, &dir);
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
    let id = window.app.project.as_ref().unwrap().id.clone();
    window.menu(crate::chat::ProjectChoice::New);
    window.until(|app| app.apps[0].running.declared.ask.is_some());
    answer(&mut window, "write");
    window.until(|app| !app.apps[0].running.declared.files.is_empty());
    let written = dir.join("written.ledger");
    window.answer_saves(&written);
    window.until(|app| {
        app.project.as_ref().is_some_and(|project| project.id != id)
            && app.apps.iter().all(|started| !started.running.pending() && started.running.tabs().1.is_some())
    });
    let wanted = written.display().to_string();
    let file = crate::workspace::ProjectFile::read(&crate::workspace::project_path(&dir, &id)).unwrap();
    assert!(
        file.tabs.iter().any(|tab| tab.path.as_deref() == Some(wanted.as_str())),
        "the written ledger left the project:\n{}",
        file.to_text()
    );
    std::fs::write(&written, "ledger").unwrap();
    window.menu(crate::chat::ProjectChoice::Switch(id.clone()));
    window.until(|app| {
        app.project.as_ref().is_some_and(|project| project.id == id)
            && app.apps[0]
                .running
                .tabs()
                .0
                .iter()
                .any(|tab| tab.path.as_deref() == Some(wanted.as_str()))
    });
    let _ = std::fs::remove_dir_all(dir);
}

/// Switching brings the project back, and a project whose tabs name only one
/// sub-application still runs the others.
#[test]
fn switching_brings_a_project_back_and_every_application_runs() {
    let dir = scratch("switch");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let first = window.app.project.as_ref().unwrap().id.clone();
    window.app.commit_rename("First".into());
    let sketch = dir.join("kept.smx");
    save_tab(&mut window, 0, &sketch);
    window.select("template");
    let papers = dir.join("Papers");
    std::fs::create_dir_all(&papers).unwrap();
    window.app.scripted_paths.push_back(papers.clone());
    window.menu(crate::chat::ProjectChoice::AddFolder);
    keep_ticking(&mut window);
    let sketch_tab = window.app.apps[0].running.tabs().1.clone().unwrap();
    // A pan, as the drawing surface reports it: the host's camera, then the
    // camera message Simple Sketch keeps with the sketch.
    {
        let display = window.app.apps[0]
            .running
            .declared
            .documents
            .get_mut(&sketch_tab)
            .expect("the sketch has a document");
        display.camera.origin = [20.0, 30.0];
        display.camera.zoom = 1.5;
    }
    window.app.send(
        0,
        protocol::HostMessage::Camera {
            tab: sketch_tab.clone(),
            origin: [20.0, 30.0],
            zoom: 1.5,
        },
    );
    window.app.flows.moved = true;
    window.step();
    let before = open_tabs(&window.app);
    assert!(before.iter().any(|tab| tab.3 == Some(([20.0, 30.0], 1.5))));
    let conversation = window.app.chat.conversation.clone();
    let folders = window.app.project.as_ref().unwrap().folders.clone();

    // The pan changed the sketch, so leaving asks, and Save keeps its view.
    window.menu(crate::chat::ProjectChoice::New);
    window.until(|app| app.apps.iter().any(|started| started.running.declared.ask.is_some()));
    answer(&mut window, "save");
    window.until(|app| {
        app.project.as_ref().is_some_and(|project| project.id != first)
            && app.apps.iter().all(|started| !started.running.pending() && started.running.tabs().1.is_some())
    });
    let second = window.app.project.as_ref().unwrap().id.clone();
    window.app.commit_rename("Second".into());

    window.menu(crate::chat::ProjectChoice::Switch(first.clone()));
    window.until(|app| {
        app.project.as_ref().is_some_and(|project| project.id == first)
            && app.apps.iter().all(|started| started.running.tabs().1.is_some())
            && app.chat.conversation.len() >= conversation.len()
    });
    assert_eq!(window.app.project.as_ref().unwrap().name, "First");
    assert_eq!(window.app.apps[window.app.selected].id, "template");
    assert_eq!(window.app.chat.conversation, conversation);
    window.until(|app| open_tabs(app).iter().any(|tab| tab.3 == Some(([20.0, 30.0], 1.5))));
    assert_eq!(open_tabs(&window.app), before, "the tabs did not come back as they were");
    assert_eq!(window.app.project.as_ref().unwrap().folders, folders);
    assert!(window.app.project.as_ref().unwrap().folders.iter().any(|folder| folder == &papers.display().to_string()));
    let ids: Vec<&str> = window.app.apps.iter().map(|started| started.id.as_str()).collect();
    assert_eq!(ids, ["simple-sketch", "template"]);

    // A project file that names only the template still runs Simple Sketch.
    let only = crate::workspace::ProjectFile {
        version: crate::workspace::VERSION,
        name: "Template only".into(),
        folders: Vec::new(),
        tabs: vec![crate::workspace::ProjectTab {
            app: "template".into(),
            path: None,
            label: "Template".into(),
            camera: None,
        }],
        selected: Some(0),
        workspace_open: false,
        conversation: Vec::new(),
    };
    only.write(&crate::workspace::project_path(&dir, "only")).unwrap();
    // Nothing changed since the project came back, so leaving it asks nothing.
    window.menu(crate::chat::ProjectChoice::Switch("only".into()));
    window.until(|app| {
        assert!(
            app.apps.iter().all(|started| started.running.declared.ask.is_none()),
            "a project that came back unchanged asked about its sketch"
        );
        app.project.as_ref().is_some_and(|project| project.id == "only")
            && app.apps.iter().any(|started| started.id == "simple-sketch" && !started.running.declared.skills.is_empty())
    });
    let sketch_running = window
        .app
        .apps
        .iter()
        .find(|started| started.id == "simple-sketch")
        .unwrap();
    assert!(sketch_running.running.tabs().1.is_some());
    let _ = (second, sketch);
    let _ = std::fs::remove_dir_all(dir);
}

/// Closing and opening again comes back to the same project, with nothing saved by hand.
#[test]
fn reopening_comes_back_to_the_same_project() {
    let dir = scratch("reopen");
    let stand_in = StandIn::start(vec![text_reply("Noted.")]);
    let mut window = project_window(both(), &stand_in, &dir);
    window.app.commit_rename("Kept".into());
    let sketch = dir.join("kept.smx");
    save_tab(&mut window, 0, &sketch);
    window.ask("Remember this.");
    window.select("template");
    window.app.toggle_workspace();
    keep_ticking(&mut window);
    let id = window.app.project.as_ref().unwrap().id.clone();
    let conversation = window.app.chat.conversation.clone();
    let open = window.app.project.as_ref().unwrap().workspace_open;
    drop(window);

    let stand_in = StandIn::start(Vec::new());
    let window = project_window(both(), &stand_in, &dir);
    assert_eq!(window.app.project.as_ref().unwrap().id, id);
    assert_eq!(window.app.project.as_ref().unwrap().name, "Kept");
    assert_eq!(window.app.project.as_ref().unwrap().workspace_open, open);
    assert_eq!(window.app.apps[window.app.selected].id, "template");
    assert_eq!(window.app.chat.conversation, conversation);
    let text = std::fs::read_to_string(crate::workspace::project_path(&dir, &id)).unwrap();
    assert!(text.contains(&sketch.display().to_string()), "the project does not hold the path:\n{text}");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&sketch).unwrap()).unwrap();
    // The project file has its own name, view, and conversation. A key of the
    // sketch that is not one of those is the sketch's own content.
    let own = ["version", "name", "folders", "tabs", "selected", "workspace_open", "conversation", "app", "path", "label", "camera", "origin", "zoom", "role", "text", "time", "attachments", "tool", "ok"];
    for key in saved.as_object().unwrap().keys().filter(|key| !own.contains(&key.as_str())) {
        assert!(!text.contains(&format!("\"{key}\"")), "the project holds {key}");
    }
    assert!(!text.contains("\"input\"") && !text.contains("\"output\""));
    drop(window);

    change_on_its_own(&sketch);
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let reopened = window.app.apps[0].running.tabs().1.clone().unwrap();
    window.until(|app| app.apps[0].running.declared.documents.contains_key(&reopened));
    assert!(
        marks(&window.app, &reopened) >= 1,
        "the change saved on its own is not in the reopened tab"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A change is written on its own. Dropping the window, without closing it,
/// still finds the change in the project.
#[test]
fn a_change_is_kept_without_closing() {
    let dir = scratch("kept-quietly");
    let stand_in = StandIn::start(vec![text_reply("Noted.")]);
    let mut window = project_window(both(), &stand_in, &dir);
    window.ask("Remember this.");
    keep_ticking(&mut window);
    let id = window.app.project.as_ref().unwrap().id.clone();
    drop(window);

    let stand_in = StandIn::start(Vec::new());
    let window = project_window(both(), &stand_in, &dir);
    assert_eq!(window.app.project.as_ref().unwrap().id, id);
    assert!(
        window.app.chat.conversation.iter().any(|entry| entry.text == "Remember this."),
        "the project was not written on its own"
    );
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

/// Removing deletes the project's folder and nothing it refers to, after the
/// question. A folder's own remove mark only takes it off the project.
#[test]
fn removing_a_project_leaves_its_folders_and_files() {
    let dir = scratch("remove-project");
    let stand_in = StandIn::start(Vec::new());
    let mut window = project_window(both(), &stand_in, &dir);
    let papers = dir.join("Papers");
    std::fs::create_dir_all(&papers).unwrap();
    let sketch = dir.join("kept.smx");
    save_tab(&mut window, 0, &sketch);
    window.app.scripted_paths.push_back(papers.clone());
    window.menu(crate::chat::ProjectChoice::AddFolder);
    assert!(window.app.project.as_ref().unwrap().folders.iter().any(|folder| folder == &papers.display().to_string()));
    window.menu(crate::chat::ProjectChoice::RemoveFolder(0));
    assert!(papers.is_dir(), "taking a folder off the project removed it");
    assert!(window.app.project.as_ref().unwrap().folders.is_empty());
    window.app.scripted_paths.push_back(papers.clone());
    window.menu(crate::chat::ProjectChoice::AddFolder);
    let id = window.app.project.as_ref().unwrap().id.clone();
    window.menu(crate::chat::ProjectChoice::Remove);
    assert_eq!(window.app.flows.question_shown().as_deref(), Some("Remove this project?"));
    assert!(window.app.flows.question_lines().join(" ").contains("project's files are deleted"));
    window.app.answer_question("cancel");
    assert!(crate::workspace::project_dir(&dir, &id).is_dir());

    window.menu(crate::chat::ProjectChoice::Remove);
    window.app.answer_question("remove");
    // Quit becomes ready and the next frame opens another project, so the
    // check is the project changing, not the ready flag the frame consumes.
    window.until(|app| app.project.as_ref().is_some_and(|project| project.id != id));
    assert!(!crate::workspace::project_dir(&dir, &id).exists());
    assert!(papers.is_dir());
    assert!(sketch.is_file());
    let _ = std::fs::remove_dir_all(dir);
}
