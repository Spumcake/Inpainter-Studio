//! The assistant: one conversation, the agent loop, and the OpenRouter client.
//!
//! It knows no sub-application, skill, or tool by name. Everything it offers
//! the model comes from the skills the running sub-applications declared,
//! passed in with each request. It runs on its own thread, so the window
//! never waits on it. The host sends it [`Command`]s and reads [`Event`]s:
//! it shows each [`Entry`], routes each [`Event::Call`] to the sub-application
//! that owns the tab, and answers with the sub-application's
//! [`protocol::ToolResult`].

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

mod agent;
mod openrouter;
mod prompt;
pub use openrouter::{image_models, make_image, models, status, stream_prompt};

/// A picture's bytes as the standard base64 a tool result carries.
pub fn picture_base64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
#[cfg(test)]
mod tests;

/// The most tool calls one request may make before it stops itself.
pub const MAX_CALLS: usize = 40;

/// Shown when a run is asked for and no key was in the environment at start.
/// The note a stopped request ends with.
pub const STOPPED: &str = "Stopped.";

pub const NO_KEY: &str = "No key is set for the Discussion skill's endpoint. Add one in Settings, under Endpoints.";

/// Shown when a run is asked for and the picker has no model.
pub const NO_MODEL: &str = "No model is chosen.";

/// Where the assistant reaches its models, and how it wakes the window.
pub struct Config {
    /// The OpenRouter API root, `https://openrouter.ai/api/v1`, or a
    /// stand-in's for tests.
    pub base_url: String,
    /// `OPENROUTER_API_KEY`. Never written anywhere.
    pub api_key: Option<String>,
    /// Called after every event, so the window repaints.
    pub wake: Arc<dyn Fn() + Send + Sync>,
}

impl Config {
    /// The key from `OPENROUTER_API_KEY`, and the root from
    /// `INPAINTER_OPENROUTER_URL` when set, for a stand-in.
    pub fn from_env(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let base_url = std::env::var("INPAINTER_OPENROUTER_URL")
            .unwrap_or_else(|_| String::from("https://openrouter.ai/api/v1"));
        let api_key = std::env::var("OPENROUTER_API_KEY")
            .ok()
            .filter(|key| !key.trim().is_empty());
        Self {
            base_url,
            api_key,
            wake,
        }
    }
}

/// One running sub-application, as a request offers it.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// The manifest id.
    pub app: String,
    /// The manifest name, which the model reads.
    pub name: String,
    pub tabs: Vec<TabInfo>,
    pub skills: Vec<protocol::Skill>,
    /// Its tools take no `tab`, such as the host's own. A target that does
    /// not say so offers nothing while it has no open tab.
    pub tabless: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TabInfo {
    pub id: String,
    pub label: String,
}

/// A file attached to a message.
#[derive(Debug, Clone, PartialEq)]
pub enum Attachment {
    /// A picture: PNG, JPEG, WebP, or GIF bytes and their media type.
    Image {
        path: PathBuf,
        mime: String,
        data: Vec<u8>,
    },
    Text {
        path: PathBuf,
        text: String,
    },
}

impl Attachment {
    pub fn path(&self) -> &PathBuf {
        match self {
            Attachment::Image { path, .. } | Attachment::Text { path, .. } => path,
        }
    }
}

/// What the person sent, and everything the model needs to act on it.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub text: String,
    pub attachments: Vec<Attachment>,
    /// The model id, as the endpoint names it.
    pub model: String,
    /// How hard the model thinks: `low`, `medium`, or `high`, or none.
    pub effort: Option<String>,
    /// The selected tab: its application id and tab id.
    pub selected: Option<(String, String)>,
    /// Every running sub-application.
    pub targets: Vec<Target>,
}

/// What the host tells the assistant.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Start a request. Ignored while one runs.
    Send(Request),
    /// A sub-application's answer to an [`Event::Call`].
    Result(protocol::ToolResult),
    /// The running sub-applications changed: their tabs or skills. Offered
    /// from the request's next model turn.
    Targets(Vec<Target>),
    /// End the request after the call in progress.
    Stop,
    /// Fetch the models that can use tools. The answer is [`Event::Models`].
    ListModels,
    /// Replace the conversation with a saved one, as a workspace reopens.
    /// The answer is [`Event::Cleared`], then each entry again.
    Restore(Vec<Entry>),
    /// Empty the conversation. The answer is [`Event::Cleared`].
    Clear,
    /// Add a notice to the conversation, such as a file that could not be
    /// read. Shown and kept, never sent to the model.
    Note(String),
    /// Where requests go from now on, and how the assistant speaks: the
    /// Discussion skill's endpoint, its key, its kind, and its prompt. A
    /// request already running keeps what it began with.
    Configure(Endpoint),
}

/// How an endpoint takes a reasoning effort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reasoning {
    /// OpenRouter: `reasoning: {effort}`.
    OpenRouter,
    /// An OpenAI-compatible server: `reasoning_effort`.
    OpenAi,
}

/// The Discussion skill's endpoint and prompt, as the host sets them.
#[derive(Debug, Clone, PartialEq)]
pub struct Endpoint {
    pub base_url: String,
    pub api_key: Option<String>,
    pub reasoning: Reasoning,
    /// How to speak with the person, sent after Studio's own rules.
    pub voice: String,
    /// Why no request can go to this endpoint, as the host says it, such as
    /// a key it lacks. A request is then answered with this note.
    pub refusal: Option<String>,
}

/// The request body's fields for an effort on this kind of endpoint.
pub fn effort_fields(reasoning: Reasoning, effort: Option<&str>) -> serde_json::Map<String, serde_json::Value> {
    let mut fields = serde_json::Map::new();
    if let Some(effort) = effort {
        match reasoning {
            Reasoning::OpenRouter => {
                fields.insert("reasoning".into(), serde_json::json!({"effort": effort}));
            }
            Reasoning::OpenAi => {
                fields.insert("reasoning_effort".into(), serde_json::json!(effort));
            }
        }
    }
    fields
}

/// What the assistant tells the host.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A conversation entry to show, in order. The workspace keeps these.
    Entry(Entry),
    /// Run a tool. The host sends `invoke` to the sub-application of `app`
    /// and answers with [`Command::Result`] carrying the same `call`.
    Call {
        call: String,
        app: String,
        tab: String,
        /// The tool's key, `<application id>__<skill id>__<tool name>`.
        tool: String,
        input: Value,
        /// The request's id. One request is one undo step.
        group: String,
    },
    /// A piece of the assistant's reply, as the model writes it. The host
    /// shows the pieces until the reply's entry arrives.
    Partial(String),
    /// A request started (true) or ended (false). While true, the host locks
    /// the sub-applications' controls and send becomes stop.
    Working(bool),
    /// The answer to [`Command::ListModels`]: the models, or why not.
    Models(Result<Vec<Model>, String>),
    /// The conversation was emptied.
    Cleared,
}

/// One line of the conversation. It never holds a tool's input or output,
/// which can hold the work itself, so a workspace can keep it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub role: Role,
    pub text: String,
    /// Seconds since the Unix epoch.
    pub time: u64,
    /// The paths of the files attached to a person's message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    /// For a tool entry: the tool's key, and whether the call succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    /// A cell this entry shows. Set while a prompt runs, and kept with its result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<protocol::Cell>,
    /// A notice that is not the end of a request, such as one the host makes
    /// about a file. It never marks a reply as failed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub aside: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The person's message.
    Person,
    /// The assistant's reply.
    Assistant,
    /// A tool the assistant called. Kept, not shown.
    Tool,
    /// A notice in the conversation: no key, a failure, a stop, a missing file.
    Note,
}

/// A model the picker offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub id: String,
    pub name: String,
    /// The model accepts pictures.
    pub images: bool,
}

/// The handle the host holds. Dropping it ends the assistant's thread.
pub struct Assistant {
    commands: Sender<Command>,
    events: Receiver<Event>,
}

impl Assistant {
    pub fn spawn(config: Config) -> Self {
        let (commands, command_rx) = channel();
        let (event_tx, events) = channel();
        std::thread::Builder::new()
            .name(String::from("assistant"))
            .spawn(move || run(config, command_rx, event_tx))
            .expect("the assistant thread starts");
        Self { commands, events }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
}

/// The assistant's thread: the conversation and the agent loop.
fn run(config: Config, commands: Receiver<Command>, events: Sender<Event>) {
    agent::run(config, commands, events);
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}
