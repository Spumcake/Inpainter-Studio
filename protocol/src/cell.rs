//! A cell: one promptable task, its prompt and its result.
//!
//! The host draws every cell. A sub-application that keeps cells stores them
//! and never draws them. `running` is not a stored state: only the host draws
//! it, while a run is in progress.

use serde::{Deserialize, Serialize};

/// One cell. A kind this build does not know still parses: `kind` is a string,
/// and parts this build does not show stay in `input` and `output`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    /// Unique everywhere, so a later version can name any cell in any book.
    pub id: String,
    /// When the cell was dragged in, the id of the cell it was copied from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// `"text"` in this version. Any other string is kept and drawn generically.
    pub kind: String,
    /// The prompt. A text cell has one text part. Later kinds add path parts.
    #[serde(default)]
    pub input: Vec<CellPart>,
    /// Which endpoint ran it. Set when it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// That endpoint's model id. Set when it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The reasoning effort it ran at, such as `"medium"`, if one was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The result. Empty until a run finishes, fails, or is stopped.
    #[serde(default)]
    pub output: Vec<CellPart>,
    pub state: CellState,
    /// A sentence when the cell failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Every run, oldest first. A run always exists: nothing removes one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<CellVersion>,
    /// The version the cell shows. The top-level `endpoint`, `model`, `effort`,
    /// `output`, `state`, and `error` always equal it; only `input` may
    /// differ, as an edit not yet run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<usize>,
}

/// One run of a cell, kept for good.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellVersion {
    /// Unique everywhere, so a later version can name one run as context.
    pub id: String,
    /// The prompt that ran.
    #[serde(default)]
    pub input: Vec<CellPart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default)]
    pub output: Vec<CellPart>,
    /// Done, failed, or stopped.
    pub state: CellState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Seconds since the Unix epoch.
    #[serde(default)]
    pub time: u64,
}

/// The kind of a cell that sends its prompt to a language model.
pub const TEXT_KIND: &str = "text";
/// The kind of a cell that makes a picture from its prompt.
pub const IMAGE_KIND: &str = "image";
/// The kind of a cell of Markdown, which runs by showing its source as rich
/// text, calls nothing, and has no versions.
pub const MARKDOWN_KIND: &str = "markdown";

/// One part of a cell's prompt or result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CellPart {
    Text { text: String },
    Path { path: String, media_type: String },
    /// A part of a shape a later version adds. It is kept as it was, so a
    /// cells message or a book holding one never fails to read.
    Other(serde_json::Value),
}

/// What a sub-application stores. The host draws `running` itself and never
/// sends it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CellState {
    Idle,
    Done,
    Failed,
    Stopped,
    /// Only the host draws a cell as running. A sub-application that sends
    /// it is refused when its message is read.
    Running,
    /// A state a later version adds, read as a state this build cannot name.
    #[serde(other)]
    Other,
}

impl Cell {
    /// A text cell that has not run, with one prompt part.
    pub fn text(id: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            from: None,
            kind: String::from("text"),
            input: vec![CellPart::Text { text: prompt.into() }],
            endpoint: None,
            model: None,
            effort: None,
            output: Vec::new(),
            state: CellState::Idle,
            error: None,
            versions: Vec::new(),
            current: None,
        }
    }

    /// A Markdown cell showing its source.
    pub fn markdown(id: impl Into<String>, source: impl Into<String>) -> Self {
        let mut cell = Self::text(id, source);
        cell.kind = String::from(MARKDOWN_KIND);
        cell
    }

    /// Makes the top level agree with the current version, as the rule
    /// between the two copies says. A current index past the last version
    /// points at the last one.
    pub fn agree(&mut self) {
        if self.versions.is_empty() {
            self.current = None;
            return;
        }
        let index = self.current.unwrap_or(self.versions.len() - 1).min(self.versions.len() - 1);
        self.current = Some(index);
        let version = &self.versions[index];
        self.endpoint = version.endpoint.clone();
        self.model = version.model.clone();
        self.effort = version.effort.clone();
        self.output = version.output.clone();
        self.state = version.state;
        self.error = version.error.clone();
    }

    /// Makes version `index` current: its prompt and result both show,
    /// replacing an edit not yet run.
    pub fn show_version(&mut self, index: usize) -> bool {
        let Some(version) = self.versions.get(index) else {
            return false;
        };
        self.input = version.input.clone();
        self.current = Some(index);
        self.agree();
        true
    }

    /// The text parts of the prompt, joined.
    pub fn prompt(&self) -> String {
        parts_text(&self.input)
    }

    /// The text parts of the result, joined.
    pub fn result_text(&self) -> String {
        parts_text(&self.output)
    }
}

fn parts_text(parts: &[CellPart]) -> String {
    parts
        .iter()
        .filter_map(|part| match part {
            CellPart::Text { text } => Some(text.as_str()),
            CellPart::Path { .. } | CellPart::Other(_) => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// A new cell id. It is a UUID, so two cells never share one.
pub fn cell_id() -> String {
    let mut bytes = [0u8; 16];
    let mut file = std::fs::File::open("/dev/urandom").expect("urandom");
    std::io::Read::read_exact(&mut file, &mut bytes).expect("urandom");
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8],
        bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}
