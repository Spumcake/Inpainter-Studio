//! The agent loop: one conversation, and each request's turns with the model
//! and the tool calls it makes, routed through the host.
//!
//! The thread waits on one inbox. The host's commands reach it through a
//! small forwarding thread, and each model turn runs on a thread of its own
//! and posts its reply there too, so a stop is heard while the model thinks.

use crate::prompt;
use crate::{Attachment, Command, Config, Endpoint, Entry, Event, MAX_CALLS, Reasoning, Request, Role, Target, now};
use base64::Engine;
use serde_json::{Map, Value, json};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::{NO_KEY, NO_MODEL};
use crate::STOPPED;
const NOT_RUN_STOPPED: &str = "Not run: the person stopped the request.";
const NO_REPLY: &str = "The model ended without a reply.";

fn limit_note() -> String {
    format!("Stopped after {MAX_CALLS} tool calls, the most one request may make.")
}

enum Input {
    Command(Command),
    /// A model turn's answer: the turn's number, and the reply's message.
    Reply(u64, Result<Value, String>),
    /// A piece of a model turn's text, as it arrives.
    Partial(u64, String),
    /// The host dropped its handle.
    Closed,
}

/// What woke a running request.
enum Woke {
    Stop,
    Reply(u64, Result<Value, String>),
    Partial(u64, String),
    Result(protocol::ToolResult),
}

/// The request in progress.
struct Busy {
    group: String,
    model: String,
    effort: Option<String>,
    /// The endpoint as the request began.
    endpoint: Endpoint,
    selected: Option<(String, String)>,
    targets: Vec<Target>,
    stopped: bool,
    calls: usize,
}

struct Agent {
    base_url: String,
    api_key: Option<String>,
    reasoning: Reasoning,
    voice: String,
    refusal: Option<String>,
    wake: Arc<dyn Fn() + Send + Sync>,
    events: Sender<Event>,
    inbox: Receiver<Input>,
    /// Kept so each model turn's thread can post its reply.
    outbox: Sender<Input>,
    /// What the model sees, after the system prompt: OpenAI-style messages
    /// with every tool's full input and output.
    history: Vec<Value>,
    /// Commands that wait until the request in progress ends.
    deferred: VecDeque<Command>,
    requests: u64,
    turns: u64,
    closed: bool,
}

pub fn run(config: Config, commands: Receiver<Command>, events: Sender<Event>) {
    let (outbox, inbox) = channel();
    let forward = outbox.clone();
    let forwarding = std::thread::Builder::new()
        .name(String::from("assistant-commands"))
        .spawn(move || {
            while let Ok(command) = commands.recv() {
                if forward.send(Input::Command(command)).is_err() {
                    return;
                }
            }
            let _ = forward.send(Input::Closed);
        });
    if forwarding.is_err() {
        return;
    }
    let mut agent = Agent {
        base_url: config.base_url,
        api_key: config.api_key,
        reasoning: Reasoning::OpenRouter,
        voice: String::new(),
        refusal: None,
        wake: config.wake,
        events,
        inbox,
        outbox,
        history: Vec::new(),
        deferred: VecDeque::new(),
        requests: 0,
        turns: 0,
        closed: false,
    };
    agent.idle();
}

impl Agent {
    /// The Discussion skill's endpoint and prompt, for every request from
    /// now on.
    fn configure(&mut self, endpoint: Endpoint) {
        self.base_url = endpoint.base_url;
        self.api_key = endpoint.api_key;
        self.reasoning = endpoint.reasoning;
        self.voice = endpoint.voice;
        self.refusal = endpoint.refusal;
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
        (self.wake)();
    }

    fn entry(&self, role: Role, text: impl Into<String>) {
        self.emit_entry(role, text, false);
    }

    /// A notice the host asked for: an aside, never the end of a request.
    fn aside(&self, text: impl Into<String>) {
        self.emit_entry(Role::Note, text, true);
    }

    fn emit_entry(&self, role: Role, text: impl Into<String>, aside: bool) {
        self.emit(Event::Entry(Entry {
            role,
            text: text.into(),
            time: now(),
            attachments: Vec::new(),
            tool: None,
            ok: None,
            cell: None,
            aside,
        }));
    }

    /// Between requests.
    fn idle(&mut self) {
        while !self.closed {
            let command = match self.deferred.pop_front() {
                Some(command) => command,
                None => match self.inbox.recv() {
                    Ok(Input::Command(command)) => command,
                    Ok(Input::Reply(..)) | Ok(Input::Partial(..)) => continue,
                    Ok(Input::Closed) | Err(_) => return,
                },
            };
            match command {
                Command::Send(request) => self.request(request),
                Command::Configure(endpoint) => self.configure(endpoint),
                Command::ListModels => self.list_models(),
                Command::Restore(entries) => self.restore(entries),
                Command::Clear => {
                    self.history.clear();
                    self.emit(Event::Cleared);
                }
                Command::Note(text) => self.aside(text),
                Command::Result(_) | Command::Targets(_) | Command::Stop => {}
            }
        }
    }

    /// Fetches the model list on a thread of its own, so neither a request
    /// nor the window waits on it.
    fn list_models(&self) {
        let base_url = self.base_url.clone();
        let key = self.api_key.clone();
        let tools_only = self.reasoning == Reasoning::OpenRouter;
        let events = self.events.clone();
        let wake = self.wake.clone();
        let started = std::thread::Builder::new()
            .name(String::from("assistant-models"))
            .spawn(move || {
                let models = crate::openrouter::models(&base_url, key.as_deref(), tools_only);
                let _ = events.send(Event::Models(models));
                wake();
            });
        if started.is_err() {
            self.emit(Event::Models(Err(String::from("The model list could not be fetched."))));
        }
    }

    /// Replaces the conversation with saved entries, rebuilding what the
    /// model sees from them.
    fn restore(&mut self, entries: Vec<Entry>) {
        self.history.clear();
        self.emit(Event::Cleared);
        for entry in &entries {
            match entry.role {
                Role::Person => {
                    let attachments: Vec<Value> = entry
                        .attachments
                        .iter()
                        .map(|path| reread(Path::new(path)))
                        .collect();
                    self.history.push(person_message(&entry.text, attachments));
                }
                Role::Assistant => self.append_assistant(&entry.text),
                Role::Tool => {
                    let tool = entry.tool.as_deref().unwrap_or("a tool");
                    let outcome = if entry.ok == Some(true) { "succeeded" } else { "failed" };
                    self.append_assistant(&format!(
                        "[Earlier, the tool `{tool}` was used, and it {outcome}.]"
                    ));
                }
                Role::Note => {}
            }
        }
        for entry in entries {
            self.emit(Event::Entry(entry));
        }
    }

    /// Adds text to the last assistant message when it is plain text, or
    /// starts one.
    fn append_assistant(&mut self, text: &str) {
        if let Some(last) = self.history.last_mut()
            && last["role"] == "assistant"
            && last.get("tool_calls").is_none()
            && let Some(content) = last["content"].as_str()
        {
            last["content"] = json!(format!("{content}\n{text}"));
            return;
        }
        self.history.push(json!({"role": "assistant", "content": text}));
    }

    /// One request, from the person's message to the model's last reply.
    fn request(&mut self, request: Request) {
        self.requests += 1;
        self.emit(Event::Working(true));
        self.emit(Event::Entry(Entry {
            role: Role::Person,
            text: request.text.clone(),
            time: now(),
            attachments: request
                .attachments
                .iter()
                .map(|attachment| attachment.path().display().to_string())
                .collect(),
            tool: None,
            ok: None,
            cell: None,
            aside: false,
        }));
        compact(&mut self.history);
        let parts = request.attachments.iter().map(attachment_part).collect();
        self.history.push(person_message(&request.text, parts));
        let mut busy = Busy {
            group: format!("request-{}-{}", now(), self.requests),
            model: request.model,
            effort: request.effort,
            endpoint: Endpoint {
                base_url: self.base_url.clone(),
                api_key: self.api_key.clone(),
                reasoning: self.reasoning,
                voice: self.voice.clone(),
                refusal: self.refusal.clone(),
            },
            selected: request.selected,
            targets: request.targets,
            stopped: false,
            calls: 0,
        };
        if let Some(note) = self.converse(&mut busy) {
            self.entry(Role::Note, note);
        }
        self.emit(Event::Working(false));
    }

    /// The request's turns. Returns the note it ends with, if any.
    fn converse(&mut self, busy: &mut Busy) -> Option<String> {
        if let Some(refusal) = &busy.endpoint.refusal {
            return Some(refusal.clone());
        }
        // OpenRouter needs a key; a local OpenAI-compatible server may not.
        let key = match busy.endpoint.api_key.clone() {
            Some(key) => key,
            None if busy.endpoint.reasoning == Reasoning::OpenRouter => return Some(String::from(NO_KEY)),
            None => String::new(),
        };
        if busy.model.trim().is_empty() {
            return Some(String::from(NO_MODEL));
        }
        loop {
            let message = match self.turn(&key, busy) {
                Ok(message) => message,
                Err(note) => return note,
            };
            let text = content_text(&message["content"]);
            let calls = tool_calls(&message);
            self.history.push(assistant_message(&message, &calls));
            if !text.trim().is_empty() {
                self.entry(Role::Assistant, text.trim());
            }
            if calls.is_empty() {
                return text.trim().is_empty().then(|| String::from(NO_REPLY));
            }
            let mut pictures: Vec<Value> = Vec::new();
            for (id, name, arguments) in &calls {
                let content = if busy.stopped {
                    tool_error(NOT_RUN_STOPPED)
                } else if busy.calls >= MAX_CALLS {
                    tool_error(&format!(
                        "Not run: the request reached its limit of {MAX_CALLS} tool calls."
                    ))
                } else {
                    busy.calls += 1;
                    match self.call(busy, name, arguments) {
                        Some((content, images)) => {
                            if !images.is_empty() {
                                pictures.push(json!({
                                    "type": "text",
                                    "text": format!("{PICTURES_LEAD}`{name}` returned:"),
                                }));
                                pictures.extend(images.iter().map(|png| image_part("image/png", png)));
                            }
                            content
                        }
                        None => return None,
                    }
                };
                self.history.push(json!({
                    "role": "tool",
                    "tool_call_id": id,
                    "content": content,
                }));
            }
            if !pictures.is_empty() {
                self.history.push(json!({"role": "user", "content": pictures}));
            }
            if busy.stopped {
                return Some(String::from(STOPPED));
            }
            if busy.calls >= MAX_CALLS {
                return Some(limit_note());
            }
        }
    }

    /// One model turn, on its own thread. A stop or a failure ends the
    /// request with the note given; `Err(None)` means the host is gone.
    fn turn(&mut self, key: &str, busy: &mut Busy) -> Result<Value, Option<String>> {
        self.turns += 1;
        let turn = self.turns;
        let mut messages = vec![json!({
            "role": "system",
            "content": prompt::with_voice(prompt::system(&busy.targets, busy.selected.as_ref()), &busy.endpoint.voice),
        })];
        messages.extend(self.history.iter().cloned());
        let mut body = json!({"model": busy.model, "messages": messages});
        for (field, value) in crate::effort_fields(busy.endpoint.reasoning, busy.effort.as_deref()) {
            body[field] = value;
        }
        let tools = prompt::tools(&busy.targets);
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools);
        }
        let base_url = busy.endpoint.base_url.clone();
        let key = key.to_string();
        let outbox = self.outbox.clone();
        let started = std::thread::Builder::new()
            .name(String::from("assistant-turn"))
            .spawn(move || {
                let pieces = outbox.clone();
                let reply = crate::openrouter::chat_streamed(&base_url, &key, &body, |text| {
                    let _ = pieces.send(Input::Partial(turn, text.to_string()));
                });
                let _ = outbox.send(Input::Reply(turn, reply));
            });
        if started.is_err() {
            return Err(Some(String::from("The request could not be started.")));
        }
        loop {
            match self.next(busy) {
                None => return Err(None),
                Some(Woke::Stop) => return Err(Some(String::from(STOPPED))),
                Some(Woke::Reply(number, reply)) if number == turn => {
                    return reply.map_err(Some);
                }
                Some(Woke::Partial(number, text)) if number == turn => self.emit(Event::Partial(text)),
                Some(Woke::Reply(..)) | Some(Woke::Partial(..)) | Some(Woke::Result(_)) => {}
            }
        }
    }

    /// One tool call: checked against the tools offered, then routed through
    /// the host. Returns the tool message's content and the result's
    /// pictures, or `None` when the host is gone.
    fn call(&mut self, busy: &mut Busy, name: &str, arguments: &str) -> Option<(String, Vec<String>)> {
        let routed = route(&busy.targets, name, arguments);
        let (app, tab, input) = match routed {
            Ok(routed) => routed,
            Err(error) => {
                self.tool_entry(name, Some(false));
                return Some((tool_error(&error), Vec::new()));
            }
        };
        let call = format!("{}.{}", busy.group, busy.calls);
        // The host can show the call, as a cell, before it finishes.
        self.tool_entry(name, None);
        self.emit(Event::Call {
            call: call.clone(),
            app,
            tab,
            tool: name.to_string(),
            input,
            group: busy.group.clone(),
        });
        let result = loop {
            match self.next(busy)? {
                Woke::Result(result) if result.call == call => break result,
                // A stop waits for the call in progress.
                Woke::Stop | Woke::Reply(..) | Woke::Partial(..) | Woke::Result(_) => {}
            }
        };
        self.tool_entry(name, Some(result.ok));
        let mut content = json!({"ok": result.ok, "output": result.output});
        if !result.images.is_empty() {
            content["pictures"] = json!(format!(
                "{} picture(s), in the next message.",
                result.images.len()
            ));
        }
        Some((content.to_string(), result.images))
    }

    fn tool_entry(&self, tool: &str, ok: Option<bool>) {
        self.emit(Event::Entry(Entry {
            role: Role::Tool,
            text: String::new(),
            time: now(),
            attachments: Vec::new(),
            tool: Some(tool.to_string()),
            ok,
            cell: None,
            aside: false,
        }));
    }

    /// The next thing a running request must hear. Commands that change the
    /// request apply here, and the rest wait for it to end.
    fn next(&mut self, busy: &mut Busy) -> Option<Woke> {
        loop {
            match self.inbox.recv() {
                Ok(Input::Command(command)) => match command {
                    Command::Stop => {
                        busy.stopped = true;
                        return Some(Woke::Stop);
                    }
                    Command::Result(result) => return Some(Woke::Result(result)),
                    Command::Targets(targets) => busy.targets = targets,
                    Command::Send(_) => {}
                    Command::ListModels => self.list_models(),
                    Command::Configure(endpoint) => self.configure(endpoint),
                    Command::Note(text) => self.aside(text),
                    command @ (Command::Restore(_) | Command::Clear) => {
                        self.deferred.push_back(command)
                    }
                },
                Ok(Input::Reply(turn, reply)) => return Some(Woke::Reply(turn, reply)),
                Ok(Input::Partial(turn, text)) => return Some(Woke::Partial(turn, text)),
                Ok(Input::Closed) | Err(_) => {
                    self.closed = true;
                    return None;
                }
            }
        }
    }
}

/// Checks a call against the targets: the key names an offered tool and the
/// arguments are an object. A tool of an application with tabs takes `tab`.
/// A tool of an application with none does not. Returns the application, the
/// tab (empty when the application has none), and the input without `tab`.
fn route(targets: &[Target], key: &str, arguments: &str) -> Result<(String, String, Value), String> {
    let unknown = || format!("No tool is named `{key}`. Use only the tools offered.");
    let (app, skill, tool) = protocol::split_tool_key(key).ok_or_else(unknown)?;
    let target = targets
        .iter()
        .find(|target| target.app == app)
        .ok_or_else(unknown)?;
    let offered = target
        .skills
        .iter()
        .any(|declared| declared.id == skill && declared.tools.iter().any(|t| t.name == tool));
    if !offered {
        return Err(unknown());
    }
    let mut input = if arguments.trim().is_empty() {
        Map::new()
    } else {
        match serde_json::from_str::<Value>(arguments) {
            Ok(Value::Object(map)) => map,
            _ => return Err(String::from("The arguments were not a JSON object.")),
        }
    };
    if target.tabless {
        input.remove("tab");
        return Ok((app.to_string(), String::new(), Value::Object(input)));
    }
    let tabs = || {
        target
            .tabs
            .iter()
            .map(|tab| format!("`{}`", tab.id))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let tab = match input.remove("tab") {
        Some(Value::String(tab)) if target.tabs.iter().any(|known| known.id == tab) => tab,
        Some(Value::String(tab)) => {
            return Err(format!("There is no tab `{tab}` here. `tab` must be one of {}.", tabs()));
        }
        _ => return Err(format!("`tab` is required, and must be one of {}.", tabs())),
    };
    Ok((app.to_string(), tab, Value::Object(input)))
}

/// A tool message's content for a call that reached no application.
fn tool_error(error: &str) -> String {
    json!({"ok": false, "output": {"error": error}}).to_string()
}

/// The tool calls of a model message: each one's id, name, and arguments.
fn tool_calls(message: &Value) -> Vec<(String, String, String)> {
    let Some(calls) = message.get("tool_calls").and_then(Value::as_array) else {
        return Vec::new();
    };
    calls
        .iter()
        .enumerate()
        .map(|(index, call)| {
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(String::from)
                .unwrap_or_else(|| format!("call_{index}"));
            let name = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let arguments = match call.pointer("/function/arguments") {
                Some(Value::String(text)) => text.clone(),
                Some(Value::Null) | None => String::new(),
                Some(other) => other.to_string(),
            };
            (id, name, arguments)
        })
        .collect()
}

/// The model's message as the history keeps it: its text, its tool calls
/// with the ids the tool messages answer, and any reasoning a provider asks
/// to have passed back.
fn assistant_message(message: &Value, calls: &[(String, String, String)]) -> Value {
    let mut kept = json!({
        "role": "assistant",
        "content": message.get("content").cloned().unwrap_or(Value::Null),
    });
    if !calls.is_empty() {
        let calls: Vec<Value> = calls
            .iter()
            .map(|(id, name, arguments)| {
                json!({
                    "id": id,
                    "type": "function",
                    "function": {"name": name, "arguments": arguments},
                })
            })
            .collect();
        kept["tool_calls"] = Value::Array(calls);
    }
    if let Some(reasoning) = message.get("reasoning_details").filter(|value| !value.is_null()) {
        kept["reasoning_details"] = reasoning.clone();
    }
    kept
}

/// A message's text, whether the content is a string or a list of parts.
fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|part| part["type"] == "text")
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// The person's message: the note that the work may have changed, their
/// text, and their attachments.
fn person_message(text: &str, attachments: Vec<Value>) -> Value {
    let mut content = vec![
        json!({"type": "text", "text": prompt::STALE}),
        json!({"type": "text", "text": text}),
    ];
    content.extend(attachments);
    json!({"role": "user", "content": content})
}

fn image_part(mime: &str, base64: &str) -> Value {
    json!({
        "type": "image_url",
        "image_url": {"url": format!("data:{mime};base64,{base64}")},
    })
}

fn text_file_part(path: &Path, text: &str) -> Value {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    json!({"type": "text", "text": format!("The attached file \"{name}\":\n\n{text}")})
}

fn attachment_part(attachment: &Attachment) -> Value {
    match attachment {
        Attachment::Image { mime, data, .. } => {
            image_part(mime, &base64::engine::general_purpose::STANDARD.encode(data))
        }
        Attachment::Text { path, text } => text_file_part(path, text),
    }
}

/// A saved attachment, read again from its path.
fn reread(path: &Path) -> Value {
    let missing = || {
        json!({
            "type": "text",
            "text": format!("The attached file {} no longer exists.", path.display()),
        })
    };
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mime = match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    };
    let Ok(data) = std::fs::read(path) else {
        return missing();
    };
    match mime {
        Some(mime) => attachment_part(&Attachment::Image {
            path: path.to_path_buf(),
            mime: mime.to_string(),
            data,
        }),
        None => text_file_part(path, &String::from_utf8_lossy(&data)),
    }
}

/// Before a new request, earlier requests' tool outputs and pictures are cut
/// to a stub. They may be stale, and kept whole they would make every later
/// model turn larger and costlier. The calls themselves, and every message
/// of the person and the assistant, stay.
fn compact(history: &mut [Value]) {
    for message in history.iter_mut() {
        match message.get("role").and_then(Value::as_str) {
            Some("tool") => {
                let ok = message
                    .get("content")
                    .and_then(Value::as_str)
                    .and_then(|content| serde_json::from_str::<Value>(content).ok())
                    .and_then(|content| content.get("ok").and_then(Value::as_bool));
                let stub = json!({"ok": ok, "output": EARLIER}).to_string();
                message["content"] = Value::String(stub);
            }
            Some("user") => {
                // The message that carries tools' pictures, as `converse`
                // builds it, not one the person sent.
                let tool_pictures = message
                    .get("content")
                    .and_then(Value::as_array)
                    .is_some_and(|parts| {
                        parts.iter().any(|part| {
                            part.get("text")
                                .and_then(Value::as_str)
                                .is_some_and(|text| text.starts_with(PICTURES_LEAD))
                        })
                    });
                if tool_pictures {
                    message["content"] = Value::String(String::from(EARLIER_PICTURES));
                }
            }
            _ => {}
        }
    }
}

// Each stub says the earlier reply did see what it stands for, so a model
// that reads the history, whichever level it is, never takes a removed
// picture for one that was never looked at and retracts a true answer, as
// the owner saw on 2026-10-30.
const EARLIER: &str = "This tool's output was seen when an earlier request ran and is no longer kept here. The replies after it were based on it. To know what is there now, read or look again.";
const PICTURES_LEAD: &str = "The pictures ";
const EARLIER_PICTURES: &str = "[The pictures the tools returned here were seen when an earlier request ran; they are no longer kept, to save space. The replies after them were based on them. To see what is there now, look again.]";
