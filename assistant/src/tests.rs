//! The agent loop against a scripted stand-in for OpenRouter: a tiny HTTP
//! server on 127.0.0.1 that records every request and answers from a script.
//! No network, no money.

use crate::*;
use protocol::{Skill, Tool, ToolResult};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One request the stand-in received.
#[derive(Clone)]
struct Received {
    method: String,
    path: String,
    body: Value,
}

type Script = dyn Fn(usize, &Received) -> (u16, String) + Send + Sync;

/// The stand-in: its root URL and every chat request it received.
struct StandIn {
    url: String,
    chats: Arc<Mutex<Vec<Value>>>,
}

impl StandIn {
    /// `script(n, request)` answers the n-th chat request (from 0), and
    /// every other request too.
    fn start(script: impl Fn(usize, &Received) -> (u16, String) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}/api/v1", listener.local_addr().expect("addr"));
        let chats = Arc::new(Mutex::new(Vec::new()));
        let recorded = chats.clone();
        let script: Arc<Script> = Arc::new(script);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let recorded = recorded.clone();
                let script = script.clone();
                std::thread::spawn(move || serve(stream, &recorded, &*script));
            }
        });
        Self { url, chats }
    }

    fn chats(&self) -> Vec<Value> {
        self.chats.lock().expect("lock").clone()
    }
}

fn serve(stream: std::net::TcpStream, chats: &Mutex<Vec<Value>>, script: &Script) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut length = 0;
    let mut chunked = false;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
            break;
        }
        let lower = header.to_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
        if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
            chunked = true;
        }
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            let mut size = String::new();
            reader.read_line(&mut size).expect("chunk size");
            let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
            let mut chunk = vec![0; size + 2];
            reader.read_exact(&mut chunk).expect("chunk");
            if size == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..size]);
        }
    } else {
        body.resize(length, 0);
        reader.read_exact(&mut body).expect("body");
    }
    let received = Received {
        method,
        path,
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    };
    let index = if received.path.ends_with("/chat/completions") {
        let mut chats = chats.lock().expect("lock");
        chats.push(received.body.clone());
        chats.len() - 1
    } else {
        0
    };
    let (status, reply) = script(index, &received);
    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 {status} Scripted\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    );
}

fn text_reply(text: &str) -> (u16, String) {
    (
        200,
        json!({"choices": [{"message": {"role": "assistant", "content": text}}]}).to_string(),
    )
}

fn calls_reply(calls: &[(&str, Value)]) -> (u16, String) {
    let calls: Vec<Value> = calls
        .iter()
        .enumerate()
        .map(|(index, (name, arguments))| {
            json!({
                "id": format!("model-call-{index}"),
                "type": "function",
                "function": {"name": name, "arguments": arguments.to_string()},
            })
        })
        .collect();
    (
        200,
        json!({"choices": [{"message": {"role": "assistant", "content": null, "tool_calls": calls}}]})
            .to_string(),
    )
}

fn tool(name: &str) -> Tool {
    Tool {
        name: name.to_string(),
        description: format!("Does {name}."),
        input: json!({
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"],
        }),
    }
}

/// Made-up applications, as tests only.
fn targets() -> Vec<Target> {
    vec![
        Target {
            app: String::from("fixture-notes"),
            name: String::from("Fixture Notes"),
            tabs: vec![
                TabInfo { id: String::from("n1"), label: String::from("Notes One") },
                TabInfo { id: String::from("n2"), label: String::from("Notes Two") },
            ],
            tabless: false,
            skills: vec![
                Skill {
                    id: String::from("reading"),
                    name: String::from("Reading notes"),
                    description: String::from("Reads the notes."),
                    instructions: String::from("Read before you write."),
                    tools: vec![tool("list"), tool("look")],
                },
                Skill {
                    id: String::from("writing"),
                    name: String::from("Writing notes"),
                    description: String::from("Writes notes."),
                    instructions: String::new(),
                    tools: vec![tool("add")],
                },
            ],
        },
        Target {
            app: String::from("fixture-labels"),
            name: String::from("Fixture Labels"),
            tabs: vec![TabInfo { id: String::from("l1"), label: String::from("Labels") }],
            tabless: false,
            skills: vec![Skill {
                id: String::from("naming"),
                name: String::from("Naming"),
                description: String::from("Renames a tab."),
                instructions: String::new(),
                tools: vec![tool("rename")],
            }],
        },
    ]
}

fn request(text: &str, targets: Vec<Target>) -> Request {
    Request {
        text: text.to_string(),
        attachments: Vec::new(),
        model: String::from("fixture/model"),
        selected: Some((String::from("fixture-notes"), String::from("n1"))),
        targets,
    }
}

fn spawn(url: &str, key: Option<&str>) -> Assistant {
    Assistant::spawn(Config {
        base_url: url.to_string(),
        api_key: key.map(String::from),
        wake: Arc::new(|| {}),
    })
}

/// Every event until the request ends, answering each with `on`.
fn drive(assistant: &Assistant, mut on: impl FnMut(&Event, &Assistant)) -> Vec<Event> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut events = Vec::new();
    loop {
        assert!(Instant::now() < deadline, "the request did not end: {events:?}");
        match assistant.try_recv() {
            Some(event) => {
                on(&event, assistant);
                let done = event == Event::Working(false);
                events.push(event);
                if done {
                    return events;
                }
            }
            None => std::thread::sleep(Duration::from_millis(2)),
        }
    }
}

/// The next `count` events.
fn take(assistant: &Assistant, count: usize) -> Vec<Event> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut events = Vec::new();
    while events.len() < count {
        assert!(Instant::now() < deadline, "too few events: {events:?}");
        match assistant.try_recv() {
            Some(event) => events.push(event),
            None => std::thread::sleep(Duration::from_millis(2)),
        }
    }
    events
}

/// Answers every call with success and the given output.
fn answer(output: Value, images: Vec<String>) -> impl FnMut(&Event, &Assistant) {
    move |event, assistant| {
        if let Event::Call { call, .. } = event {
            assistant.send(Command::Result(ToolResult {
                call: call.clone(),
                ok: true,
                output: output.clone(),
                images: images.clone(),
            }));
        }
    }
}

fn entries(events: &[Event]) -> Vec<Entry> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Entry(entry) => Some(entry.clone()),
            _ => None,
        })
        .collect()
}

fn notes(events: &[Event]) -> Vec<String> {
    entries(events)
        .into_iter()
        .filter(|entry| entry.role == Role::Note)
        .map(|entry| entry.text)
        .collect()
}

fn offered(chat: &Value) -> Vec<String> {
    let mut names: Vec<String> = chat["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .map(|tool| tool["function"]["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn declared(targets: &[Target]) -> Vec<String> {
    let mut keys = Vec::new();
    for target in targets {
        for skill in &target.skills {
            for tool in &skill.tools {
                keys.push(protocol::tool_key(&target.app, &skill.id, &tool.name));
            }
        }
    }
    keys.sort();
    keys
}

#[test]
fn the_tools_offered_are_exactly_the_declared_ones_with_their_tabs() {
    let stand_in = StandIn::start(|_, _| text_reply("Hello."));
    let assistant = spawn(&stand_in.url, Some("test-key"));
    assistant.send(Command::Send(request("hi", targets())));
    let events = drive(&assistant, |_, _| {});
    assert_eq!(events.first(), Some(&Event::Working(true)));
    assert_eq!(events.last(), Some(&Event::Working(false)));
    let chats = stand_in.chats();
    assert_eq!(chats.len(), 1);
    let chat = &chats[0];
    assert_eq!(chat["model"], "fixture/model");
    assert_eq!(offered(chat), declared(&targets()));
    for tool in chat["tools"].as_array().expect("tools") {
        let name = tool["function"]["name"].as_str().expect("name");
        let parameters = &tool["function"]["parameters"];
        let tabs = if name.starts_with("fixture-notes__") { json!(["n1", "n2"]) } else { json!(["l1"]) };
        assert_eq!(parameters["properties"]["tab"]["enum"], tabs, "{name}");
        assert_eq!(parameters["properties"]["text"]["type"], "string");
        assert_eq!(parameters["required"], json!(["tab", "text"]));
    }
    let system = chat["messages"][0]["content"].as_str().expect("system");
    assert!(system.starts_with(crate::prompt::SYSTEM));
    for words in [
        "Fixture Notes",
        "\"Notes One\", tab `n1` (selected)",
        "\"Notes Two\", tab `n2`\n",
        "Reading notes",
        "Reads the notes.",
        "Read before you write.",
        "Fixture Labels",
    ] {
        assert!(system.contains(words), "{words} missing from {system}");
    }
    let person = &chat["messages"][1];
    assert_eq!(person["role"], "user");
    assert_eq!(person["content"][0]["text"], crate::prompt::STALE);
    assert_eq!(person["content"][1]["text"], "hi");
    let entries = entries(&events);
    assert_eq!(entries[0].role, Role::Person);
    assert_eq!(entries[1].role, Role::Assistant);
    assert_eq!(entries[1].text, "Hello.");

    // With one application, only its tools.
    let only = vec![targets().remove(1)];
    assistant.send(Command::Send(request("again", only.clone())));
    drive(&assistant, |_, _| {});
    assert_eq!(offered(&stand_in.chats()[1]), declared(&only));
}

#[test]
fn calls_are_routed_with_the_group_and_results_reach_the_model() {
    let stand_in = StandIn::start(|index, _| match index {
        0 => calls_reply(&[
            ("fixture-notes__writing__add", json!({"tab": "n2", "text": "secret words"})),
            ("fixture-labels__naming__rename", json!({"tab": "l1", "text": "New"})),
        ]),
        2 => calls_reply(&[("fixture-notes__reading__list", json!({"tab": "n1", "text": "x"}))]),
        _ => text_reply("Done."),
    });
    let assistant = spawn(&stand_in.url, Some("test-key"));
    assistant.send(Command::Send(request("add a note", targets())));
    let mut calls = Vec::new();
    let events = drive(&assistant, |event, assistant| {
        if let Event::Call { call, app, tab, tool, input, group } = event {
            calls.push((app.clone(), tab.clone(), tool.clone(), input.clone(), group.clone()));
            // Sending again while working is ignored.
            assistant.send(Command::Send(request("ignored", targets())));
            assistant.send(Command::Result(ToolResult {
                call: call.clone(),
                ok: true,
                output: json!({"made": format!("id-for-{tool}")}),
                images: Vec::new(),
            }));
        }
    });
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "fixture-notes");
    assert_eq!(calls[0].1, "n2");
    assert_eq!(calls[0].2, "fixture-notes__writing__add");
    assert_eq!(calls[0].3, json!({"text": "secret words"}));
    assert_eq!(calls[1].0, "fixture-labels");
    assert_eq!(calls[1].1, "l1");
    assert!(!calls[0].4.is_empty());
    assert_eq!(calls[0].4, calls[1].4, "one request is one group");

    let chats = stand_in.chats();
    assert_eq!(chats.len(), 2);
    let messages = chats[1]["messages"].as_array().expect("messages");
    let tools: Vec<&Value> = messages.iter().filter(|m| m["role"] == "tool").collect();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0]["tool_call_id"], "model-call-0");
    assert_eq!(tools[1]["tool_call_id"], "model-call-1");
    assert!(tools[0]["content"].as_str().expect("content").contains("id-for-fixture-notes__writing__add"));
    let assistant_turn = messages.iter().find(|m| m["role"] == "assistant").expect("turn");
    assert_eq!(assistant_turn["tool_calls"].as_array().map(Vec::len), Some(2));

    let entries = entries(&events);
    let finished: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.role == Role::Tool && entry.ok.is_some())
        .collect();
    assert_eq!(finished.len(), 2);
    assert_eq!(entries.iter().filter(|e| e.role == Role::Person).count(), 1);
    // A finished tool entry holds the key and whether it worked, never input or output.
    // A call also records a pending entry first, so the host can show it while it runs.
    for entry in entries.iter().filter(|entry| entry.role == Role::Tool) {
        assert!(entry.text.is_empty());
        let saved = serde_json::to_string(entry).expect("json");
        assert!(!saved.contains("secret words") && !saved.contains("id-for") && !saved.contains("New"), "{saved}");
    }
    assert_eq!(finished[0].ok, Some(true));
    assert_eq!(finished[0].tool.as_deref(), Some("fixture-notes__writing__add"));

    // A second request has a different group.
    assistant.send(Command::Send(request("more", targets())));
    let mut second = None;
    drive(&assistant, |event, assistant| {
        if let Event::Call { call, group, .. } = event {
            second = Some(group.clone());
            assistant.send(Command::Result(ToolResult {
                call: call.clone(),
                ok: true,
                output: json!({}),
                images: Vec::new(),
            }));
        }
    });
    assert_ne!(second.expect("a second call"), calls[0].4);
}

#[test]
fn an_image_result_becomes_an_image_part() {
    let stand_in = StandIn::start(|index, _| match index {
        0 => calls_reply(&[("fixture-notes__reading__look", json!({"tab": "n1", "text": "all"}))]),
        _ => text_reply("I see it."),
    });
    let assistant = spawn(&stand_in.url, Some("test-key"));
    assistant.send(Command::Send(request("look", targets())));
    drive(&assistant, answer(json!({"seen": true}), vec![String::from("iVBORw0KGgo=")]));
    let chats = stand_in.chats();
    let messages = chats[1]["messages"].as_array().expect("messages");
    let tool_at = messages.iter().position(|m| m["role"] == "tool").expect("tool");
    let pictures = &messages[tool_at + 1];
    assert_eq!(pictures["role"], "user");
    let parts = pictures["content"].as_array().expect("parts");
    let image = parts.iter().find(|part| part["type"] == "image_url").expect("image part");
    assert_eq!(image["image_url"]["url"], "data:image/png;base64,iVBORw0KGgo=");
    assert!(!messages[tool_at]["content"].as_str().expect("text").contains("iVBORw0KGgo"));
}

/// Catches a conversation that carries every earlier read and picture into
/// every later turn: the next request sees earlier tool outputs and pictures
/// only as stubs, and still sees what was said.
#[test]
fn earlier_tool_outputs_and_pictures_are_cut_from_later_requests() {
    let stand_in = StandIn::start(|index, _| match index {
        0 => calls_reply(&[("fixture-notes__reading__look", json!({"tab": "n1", "text": "all"}))]),
        1 => text_reply("I see it."),
        _ => text_reply("Again."),
    });
    let assistant = spawn(&stand_in.url, Some("test-key"));
    assistant.send(Command::Send(request("look", targets())));
    drive(&assistant, answer(json!({"secret_detail": 41}), vec![String::from("iVBORw0KGgo=")]));
    assistant.send(Command::Send(request("and now?", targets())));
    drive(&assistant, answer(json!({}), Vec::new()));
    let chats = stand_in.chats();
    let later = chats[2].to_string();
    assert!(!later.contains("secret_detail"), "an earlier output was resent: {later}");
    assert!(!later.contains("iVBORw0KGgo"), "an earlier picture was resent");
    assert!(later.contains("I see it."), "the earlier reply was lost");
    assert!(later.contains("fixture-notes__reading__look"), "the earlier call was lost");
}

#[test]
fn a_request_stops_after_the_call_limit() {
    let stand_in = StandIn::start(|_, _| {
        calls_reply(&[("fixture-notes__reading__list", json!({"tab": "n1", "text": "x"}))])
    });
    let assistant = spawn(&stand_in.url, Some("test-key"));
    assistant.send(Command::Send(request("forever", targets())));
    let events = drive(&assistant, answer(json!({}), Vec::new()));
    let calls = events.iter().filter(|event| matches!(event, Event::Call { .. })).count();
    assert_eq!(calls, MAX_CALLS);
    assert_eq!(stand_in.chats().len(), MAX_CALLS);
    let notes = notes(&events);
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("40"), "{notes:?}");
}

#[test]
fn stop_ends_the_request_after_the_call_in_progress() {
    let stand_in = StandIn::start(|_, _| {
        calls_reply(&[
            ("fixture-notes__reading__list", json!({"tab": "n1", "text": "x"})),
            ("fixture-notes__writing__add", json!({"tab": "n1", "text": "y"})),
        ])
    });
    let assistant = spawn(&stand_in.url, Some("test-key"));
    assistant.send(Command::Send(request("go", targets())));
    let events = drive(&assistant, |event, assistant| {
        if let Event::Call { call, .. } = event {
            assistant.send(Command::Stop);
            assistant.send(Command::Result(ToolResult {
                call: call.clone(),
                ok: true,
                output: json!({}),
                images: Vec::new(),
            }));
        }
    });
    let calls = events.iter().filter(|event| matches!(event, Event::Call { .. })).count();
    assert_eq!(calls, 1, "the second call never runs");
    assert_eq!(stand_in.chats().len(), 1);
    assert_eq!(notes(&events), ["Stopped."]);

    // The next request still answers both of the model's calls.
    assistant.send(Command::Send(request("again", targets())));
    drive(&assistant, |event, assistant| {
        if let Event::Call { .. } = event {
            assistant.send(Command::Stop);
        }
        answer(json!({}), Vec::new())(event, assistant);
    });
    let chats = stand_in.chats();
    let tools = chats[1]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter(|m| m["role"] == "tool")
        .count();
    assert_eq!(tools, 2);
}

#[test]
fn stop_ends_the_request_while_the_model_thinks() {
    let stand_in = StandIn::start(|_, _| {
        std::thread::sleep(Duration::from_secs(3));
        text_reply("Too late.")
    });
    let assistant = spawn(&stand_in.url, Some("test-key"));
    assistant.send(Command::Send(request("go", targets())));
    let started = Instant::now();
    let mut stopped = false;
    let events = drive(&assistant, |event, assistant| {
        if *event == Event::Working(true) && !stopped {
            stopped = true;
            std::thread::sleep(Duration::from_millis(100));
            assistant.send(Command::Stop);
        }
    });
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(notes(&events), ["Stopped."]);
    assert!(entries(&events).iter().all(|entry| entry.role != Role::Assistant));
}

#[test]
fn with_no_key_a_request_says_so() {
    let assistant = spawn("http://127.0.0.1:9/api/v1", None);
    assistant.send(Command::Send(request("hi", targets())));
    let events = drive(&assistant, |_, _| {});
    assert_eq!(events.len(), 4);
    assert_eq!(events[0], Event::Working(true));
    let notes = notes(&events);
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("No OpenRouter key") && notes[0].contains("OPENROUTER_API_KEY"));
}

#[test]
fn failures_become_notes() {
    let stand_in = StandIn::start(|index, _| match index {
        0 => (402, json!({"error": {"code": 402, "message": "Insufficient credits"}}).to_string()),
        1 => (200, String::from("not json at all")),
        2 => (200, json!({"error": {"code": 502, "message": "Provider exploded"}}).to_string()),
        _ => (200, json!({"choices": []}).to_string()),
    });
    let assistant = spawn(&stand_in.url, Some("test-key"));
    let mut seen = Vec::new();
    for _ in 0..4 {
        assistant.send(Command::Send(request("hi", targets())));
        seen.push(notes(&drive(&assistant, |_, _| {})));
    }
    assert!(seen[0][0].contains("402") && seen[0][0].contains("Insufficient credits"), "{seen:?}");
    assert!(seen[1][0].contains("not JSON"), "{seen:?}");
    assert!(seen[2][0].contains("Provider exploded"), "{seen:?}");
    assert!(seen[3][0].contains("no answer"), "{seen:?}");

    let unreachable = spawn("http://127.0.0.1:1/api/v1", Some("test-key"));
    unreachable.send(Command::Send(request("hi", targets())));
    let notes = notes(&drive(&unreachable, |_, _| {}));
    assert!(notes[0].contains("Could not reach OpenRouter"), "{notes:?}");
}

#[test]
fn unknown_tools_and_bad_tabs_never_reach_an_application() {
    let stand_in = StandIn::start(|index, _| match index {
        0 => calls_reply(&[
            ("fixture-notes__reading__made_up", json!({"tab": "n1", "text": "x"})),
            ("nowhere__at__all", json!({"tab": "n1"})),
            ("fixture-labels__naming__rename", json!({"tab": "n1", "text": "x"})),
            ("fixture-labels__naming__rename", json!({"text": "x"})),
        ]),
        _ => text_reply("Sorry."),
    });
    let assistant = spawn(&stand_in.url, Some("test-key"));
    assistant.send(Command::Send(request("go", targets())));
    let events = drive(&assistant, |_, _| {});
    assert!(!events.iter().any(|event| matches!(event, Event::Call { .. })));
    let chats = stand_in.chats();
    let tools: Vec<String> = chats[1]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["content"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(tools.len(), 4);
    for content in &tools {
        let content: Value = serde_json::from_str(content).expect("json");
        assert_eq!(content["ok"], false);
    }
    assert!(tools[2].contains("`l1`"), "{}", tools[2]);
}

#[test]
fn targets_replace_the_tools_from_the_next_turn() {
    let stand_in = StandIn::start(|index, _| match index {
        0 => calls_reply(&[("fixture-notes__reading__list", json!({"tab": "n1", "text": "x"}))]),
        _ => text_reply("Done."),
    });
    let assistant = spawn(&stand_in.url, Some("test-key"));
    assistant.send(Command::Send(request("go", targets())));
    let fewer = vec![targets().remove(1)];
    let replacement = fewer.clone();
    drive(&assistant, move |event, assistant| {
        if let Event::Call { call, .. } = event {
            assistant.send(Command::Targets(replacement.clone()));
            assistant.send(Command::Result(ToolResult {
                call: call.clone(),
                ok: true,
                output: json!({}),
                images: Vec::new(),
            }));
        }
    });
    let chats = stand_in.chats();
    assert_eq!(offered(&chats[0]), declared(&targets()));
    assert_eq!(offered(&chats[1]), declared(&fewer));
}

#[test]
fn restore_rebuilds_the_conversation_and_clear_empties_it() {
    let folder = std::env::temp_dir().join(format!("assistant-restore-{}", std::process::id()));
    std::fs::create_dir_all(&folder).expect("folder");
    let attached = folder.join("brief.txt");
    std::fs::write(&attached, "attached words").expect("write");
    let missing = folder.join("gone.png");

    let stand_in = StandIn::start(|_, _| text_reply("Hello again."));
    let assistant = spawn(&stand_in.url, Some("test-key"));
    let saved = vec![
        Entry {
            role: Role::Person,
            text: String::from("first request"),
            time: 1,
            attachments: vec![attached.display().to_string(), missing.display().to_string()],
            tool: None,
            ok: None,
            cell: None,
            aside: false,
        },
        Entry {
            role: Role::Tool,
            text: String::new(),
            time: 2,
            attachments: Vec::new(),
            tool: Some(String::from("fixture-notes__writing__add")),
            ok: Some(true),
            cell: None,
            aside: false,
        },
        Entry {
            role: Role::Assistant,
            text: String::from("first reply"),
            time: 3,
            attachments: Vec::new(),
            tool: None,
            ok: None,
            cell: None,
            aside: false,
        },
        Entry {
            role: Role::Note,
            text: String::from("a note"),
            time: 4,
            attachments: Vec::new(),
            tool: None,
            ok: None,
            cell: None,
            aside: false,
        },
    ];
    assistant.send(Command::Restore(saved.clone()));
    let events = take(&assistant, 5);
    assert_eq!(events[0], Event::Cleared);
    assert_eq!(entries(&events), saved);

    assistant.send(Command::Send(request("second request", targets())));
    drive(&assistant, |_, _| {});
    let chats = stand_in.chats();
    let messages = chats[0]["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 4, "{messages:?}");
    let first = messages[1].to_string();
    assert!(first.contains("first request") && first.contains("attached words") && first.contains("brief.txt"));
    assert!(first.contains("no longer exists"));
    assert_eq!(messages[2]["role"], "assistant");
    let reply = messages[2]["content"].as_str().expect("text");
    assert!(reply.contains("fixture-notes__writing__add") && reply.contains("first reply"));
    assert!(!messages.iter().any(|m| m.to_string().contains("a note")));
    assert!(messages[3].to_string().contains("second request"));

    assistant.send(Command::Clear);
    assert_eq!(take(&assistant, 1), [Event::Cleared]);
    assistant.send(Command::Send(request("fresh", targets())));
    drive(&assistant, |_, _| {});
    let chats = stand_in.chats();
    assert_eq!(chats[1]["messages"].as_array().map(Vec::len), Some(2));
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn attachments_go_to_the_model_as_parts() {
    let stand_in = StandIn::start(|_, _| text_reply("Seen."));
    let assistant = spawn(&stand_in.url, Some("test-key"));
    let mut sent = request("use these", targets());
    sent.attachments = vec![
        Attachment::Image {
            path: "/pictures/guide.png".into(),
            mime: String::from("image/png"),
            data: vec![1, 2, 3],
        },
        Attachment::Text {
            path: "/words/brief.md".into(),
            text: String::from("the brief"),
        },
    ];
    assistant.send(Command::Send(sent));
    let events = drive(&assistant, |_, _| {});
    assert_eq!(entries(&events)[0].attachments, ["/pictures/guide.png", "/words/brief.md"]);
    let chats = stand_in.chats();
    let parts = chats[0]["messages"][1]["content"].as_array().expect("parts").clone();
    assert_eq!(parts[2]["image_url"]["url"], "data:image/png;base64,AQID");
    assert!(parts[3]["text"].as_str().expect("text").contains("brief.md"));
    assert!(parts[3]["text"].as_str().expect("text").contains("the brief"));
}

#[test]
fn the_model_list_keeps_only_models_that_use_tools() {
    let stand_in = StandIn::start(|_, received| {
        assert_eq!(received.method, "GET");
        assert!(received.path.starts_with("/api/v1/models"));
        (
            200,
            json!({
                "data": [
                    {"id": "b/see", "name": "Zeta Vision", "supported_parameters": ["tools", "temperature"],
                     "architecture": {"input_modalities": ["text", "image"]}},
                    {"id": "a/talk", "name": "Alpha Talk", "supported_parameters": ["temperature"],
                     "architecture": {"input_modalities": ["text"]}},
                    {"id": "c/text", "name": "beta Text", "supported_parameters": ["tools"],
                     "architecture": {"input_modalities": ["text"]}},
                ],
                "links": {"next": null},
            })
            .to_string(),
        )
    });
    let assistant = spawn(&stand_in.url, None);
    assistant.send(Command::ListModels);
    let events = take(&assistant, 1);
    assert_eq!(
        events[0],
        Event::Models(Ok(vec![
            Model { id: String::from("c/text"), name: String::from("beta Text"), images: false },
            Model { id: String::from("b/see"), name: String::from("Zeta Vision"), images: true },
        ]))
    );

    let failing = StandIn::start(|_, _| (500, json!({"error": {"code": 500, "message": "down"}}).to_string()));
    let assistant = spawn(&failing.url, None);
    assistant.send(Command::ListModels);
    match &take(&assistant, 1)[0] {
        Event::Models(Err(error)) => assert!(error.contains("down"), "{error}"),
        other => panic!("{other:?}"),
    }
}

/// The assistant knows no application by name: its code and prompts never
/// mention one, nor any kind of work one does.
#[test]
fn the_source_names_no_application_or_work() {
    let words = [
        "simple sketch",
        "simple-sketch",
        "simple_sketch",
        "sketch",
        "template",
        "canvas",
        "stroke",
        "shape",
        "circle",
        "ellipse",
        "rectangle",
    ];
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0;
    for file in std::fs::read_dir(&folder).expect("src") {
        let path = file.expect("entry").path();
        if path.extension().is_none_or(|extension| extension != "rs") || path.ends_with("tests.rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read");
        let code = match source.find("#[cfg(test)]\nmod tests {") {
            Some(tests) => &source[..tests],
            None => &source[..],
        };
        let code = code.to_lowercase();
        for word in words {
            assert!(!code.contains(word), "{} mentions {word:?}", path.display());
        }
        checked += 1;
    }
    assert!(checked >= 4);
}

/// Catches every application with no open tab offered as if its tools took
/// none: only a target that says its tools take no `tab` is offered that
/// way, and one with no tab otherwise offers nothing.
#[test]
fn only_a_tabless_target_offers_tools_without_a_tab() {
    let make = |app: &str, tabless: bool| Target {
        app: String::from(app),
        name: String::from(app),
        tabs: Vec::new(),
        tabless,
        skills: vec![Skill {
            id: String::from("making"),
            name: String::from("Making"),
            description: String::from("Makes."),
            instructions: String::new(),
            tools: vec![tool("make")],
        }],
    };
    let offered = crate::prompt::tools(&[make("fixture-closed", false), make("fixture-own", true)]);
    let names: Vec<&str> = offered
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["fixture-own__making__make"]);
    assert!(offered[0]["function"]["parameters"]["properties"].get("tab").is_none());
}

/// Catches a tool call read from one streamed event only: OpenRouter sends
/// the name and the arguments in fragments, and they arrive whole.
#[test]
fn a_tool_call_in_fragments_arrives_whole() {
    let events = [
        json!({"choices": [{"delta": {"role": "assistant", "tool_calls": [{"index": 0, "id": "c1", "type": "function", "function": {"name": "fixture-notes__writing__add", "arguments": ""}}]}}]}),
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "{\"tab\":\"n1\","}}]}}]}),
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "\"text\":\"North\"}"}}]}}]}),
    ];
    let body: String = events.iter().map(|event| format!("data: {event}\n\n")).collect::<String>() + "data: [DONE]\n";
    let message = crate::openrouter::assemble(&body).expect("the stream was not read");
    let call = &message["tool_calls"][0];
    assert_eq!(call["id"], "c1");
    assert_eq!(call["function"]["name"], "fixture-notes__writing__add");
    let arguments: Value = serde_json::from_str(call["function"]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(arguments, json!({"tab": "n1", "text": "North"}));
}
