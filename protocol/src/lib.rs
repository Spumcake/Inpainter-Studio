//! Messages between the host and a sub-application.
//!
//! JSON Lines: one JSON object per line, ended by `\n`. A trailing `\r` is
//! stripped. Every object has a `type` field naming the message. Strings are
//! JSON strings, so a label may hold spaces, quotes, or any other text.

use serde::{Deserialize, Serialize};

mod cell;
mod declare;
mod skills;
pub use cell::*;
pub use declare::*;
pub use skills::*;

/// The version this crate speaks. A sub-application's first message says
/// which version it speaks; the host accepts only this one.
pub const PROTOCOL_VERSION: u32 = 1;

/// What a sub-application writes to the host, on its standard output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppMessage {
    /// Always first. Names the protocol version the sub-application speaks.
    Hello { protocol: u32 },
    /// One document the sub-application has open. `label` is the name of the
    /// file it manages or, when it manages no file, the sub-application's name.
    Tab { id: String, label: String },
    /// What the host draws in that tab's contents area.
    Contents { tab: String, kind: ContentsKind },
    /// That tab uses the host's properties panel, on this side.
    Panel { tab: String, side: PanelSide },
    /// The whole document after a change. `camera` is present when a sketch
    /// is opened or created.
    Document {
        tab: String,
        sketch: serde_json::Value,
        selection: Vec<u64>,
        active_canvas: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        camera: Option<DocumentCamera>,
    },
    /// A file dialog for the host to open.
    File {
        id: String,
        kind: FileKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<FileFilter>,
    },
    /// The declaration is complete.
    End,
    /// Every shortcut, replacing the ones declared before.
    Keys(Keys),
    /// The selected tab's toolbar.
    Toolbar(Toolbar),
    /// The hamburger menu of this sub-application's tabs.
    Menu { rows: Vec<MenuEntry> },
    /// The selected tab's properties panel.
    Properties(Properties),
    /// The tab list, which replaces the tabs reported before.
    Tabs(Tabs),
    /// The drawing surface's current tool and the text being typed.
    Canvas(Canvas),
    /// A question for the host to show. The host answers once.
    Ask(Ask),
    /// Every unsaved document has been resolved after `quit`. The host may exit.
    QuitReady,
    /// The answer to `ping`.
    Pong,
    /// Every skill the sub-application provides to the assistant, replacing
    /// those sent before.
    Skills { skills: Vec<Skill> },
    /// The answer to one `invoke`, after any message the call caused.
    #[serde(rename = "result")]
    ToolResult(ToolResult),
    /// The answer to `keep`: the file that tab now has, or none when the
    /// person cancelled.
    Kept {
        tab: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// The whole cell list of a tab that keeps cells, after any change.
    Cells {
        tab: String,
        cells: Vec<Cell>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selected: Option<String>,
    },
    /// Asks the host to run that cell, as the toolbar's Run does.
    RunCell { tab: String, cell: String },
    /// Asks the host to stop that cell. The host keeps what had arrived.
    StopCell { tab: String, cell: String },
    /// Whether this application can open `path` as its own work.
    Claimed { id: String, yours: bool },
}

/// What the host writes to a sub-application, on its standard input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostMessage {
    /// A menu row, toolbar button, shortcut, the plus, a tab's close mark,
    /// selecting a tab, or an outliner action. `new_sketch` creates a sketch.
    Command {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arg: Option<serde_json::Value>,
        /// The object a canvas menu row was chosen on.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<u64>,
        viewport: [f64; 2],
    },
    /// One finished gesture.
    Gesture {
        tab: String,
        gesture: serde_json::Value,
    },
    /// A click, a marquee, or a cancelled drag on the drawing surface. The
    /// host shows the selection the next `document` states.
    Select { tab: String, op: SelectOp },
    /// The tab's camera after a pan, zoom, or fit. Not a history step.
    Camera {
        tab: String,
        origin: [f64; 2],
        zoom: f64,
    },
    /// The contents area's size, once it is first known.
    Viewport { viewport: [f64; 2] },
    /// The button the person chose on an `ask`.
    Answer { id: String, button: String },
    /// A panel or toolbar edit. `preview` is true while a slider is held.
    Property {
        prop: serde_json::Value,
        value: serde_json::Value,
        preview: bool,
    },
    /// The path the host's file dialog returned. Absent when the person cancelled.
    FileResult {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// The window's close was pressed. The window stays until `quit_ready`.
    Quit,
    /// Sent every second. The answer is `pong`.
    Ping,
    /// Run one tool of a declared skill on that tab. `tool` is the tool's
    /// key, `<application id>__<skill id>__<tool name>`. Every change made
    /// under one `group`, the assistant's request, is one undo step.
    Invoke {
        call: String,
        tab: String,
        tool: String,
        input: serde_json::Value,
        group: String,
    },
    /// Open that file in a new tab or, with no path, the tab the
    /// sub-application opens of its own at start. Used to restore a workspace.
    Open {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// Make sure that tab has a file, through the sub-application's own Save.
    /// The answer is `kept`.
    Keep { tab: String },
    /// The window is closing. Exit.
    Shutdown,
    /// The person's edit of a cell's prompt, when it loses focus and before
    /// a run. Never per keystroke. One undo step for the cell's owner.
    EditCell {
        tab: String,
        cell: String,
        input: Vec<CellPart>,
    },
    /// A run finished, failed, or was stopped. One undo step.
    CellResult {
        tab: String,
        cell: String,
        endpoint: String,
        model: String,
        state: CellState,
        output: Vec<CellPart>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// A dragged cell, already given a new id and `from` by the host.
    InsertCell {
        tab: String,
        index: usize,
        cell: Cell,
    },
    /// The cell the person selected.
    SelectCell { tab: String, cell: String },
    /// Asks whether `path` is this application's own work.
    Claim { id: String, path: String },
}

/// What a selection message does, as the drawing surface's select tool does it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SelectOp {
    /// A click on an object. `add` toggles it in the selection.
    Click { node: u64, add: bool },
    /// A click on nothing.
    Clear,
    /// Everything a marquee touches, added to `base` when `add`.
    Marquee {
        min: [f64; 2],
        max: [f64; 2],
        add: bool,
        base: Vec<u64>,
    },
    /// Puts a selection back, as Escape during a drag does.
    Replace { nodes: Vec<u64> },
}

/// The camera sent with a document when a sketch is opened or created.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentCamera {
    pub origin: [f64; 2],
    pub zoom: f64,
}

/// Which file dialog the host should open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Open,
    Save,
}

/// A shared component the host draws in a contents area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentsKind {
    Grid,
    /// The drawing surface: the grid, the document, and its tools.
    Canvas,
    /// A list of cells. The host draws them; the sub-application keeps them.
    Cells,
}

/// Where the properties panel sits. Only the right side has a reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelSide {
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabReport {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentsReport {
    pub tab: String,
    pub kind: ContentsKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelReport {
    pub tab: String,
    pub side: PanelSide,
}

/// Everything a sub-application has declared. The host draws only a
/// declaration that `accept` has returned `Ready` for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub tabs: Vec<TabReport>,
    pub contents: Vec<ContentsReport>,
    pub panels: Vec<PanelReport>,
    greeted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accept {
    More,
    Ready,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: String,
}

impl Declaration {
    pub fn empty() -> Self {
        Self {
            tabs: Vec::new(),
            contents: Vec::new(),
            panels: Vec::new(),
            greeted: false,
        }
    }

    fn knows_tab(&self, id: &str) -> bool {
        self.tabs.iter().any(|tab| tab.id == id)
    }

    /// Adds one message. `Hello` with this crate's version must come first,
    /// and only once. A tab id is new; contents and a panel name a known tab,
    /// at most one of each per tab.
    pub fn accept(&mut self, message: AppMessage) -> Accept {
        if !self.greeted {
            return match message {
                AppMessage::Hello { protocol } if protocol == PROTOCOL_VERSION => {
                    self.greeted = true;
                    Accept::More
                }
                _ => Accept::Failed,
            };
        }
        match message {
            AppMessage::Hello { .. } => Accept::Failed,
            AppMessage::Tab { id, label } => {
                if self.knows_tab(&id) {
                    Accept::Failed
                } else {
                    self.tabs.push(TabReport { id, label });
                    Accept::More
                }
            }
            AppMessage::Contents { tab, kind } => {
                let vacant = self.contents.iter().all(|contents| contents.tab != tab);
                if self.knows_tab(&tab) && vacant {
                    self.contents.push(ContentsReport { tab, kind });
                    Accept::More
                } else {
                    Accept::Failed
                }
            }
            AppMessage::Panel { tab, side } => {
                let vacant = self.panels.iter().all(|panel| panel.tab != tab);
                if self.knows_tab(&tab) && vacant {
                    self.panels.push(PanelReport { tab, side });
                    Accept::More
                } else {
                    Accept::Failed
                }
            }
            // Later messages. They do not change the declaration the window draws.
            AppMessage::Document { .. }
            | AppMessage::File { .. }
            | AppMessage::Keys(_)
            | AppMessage::Toolbar(_)
            | AppMessage::Menu { .. }
            | AppMessage::Properties(_)
            | AppMessage::Tabs(_)
            | AppMessage::Canvas(_)
            | AppMessage::Ask(_)
            | AppMessage::QuitReady
            | AppMessage::Pong
            | AppMessage::Skills { .. }
            | AppMessage::ToolResult(_)
            | AppMessage::Kept { .. }
            | AppMessage::Cells { .. }
            | AppMessage::RunCell { .. }
            | AppMessage::StopCell { .. }
            | AppMessage::Claimed { .. } => Accept::More,
            AppMessage::End => Accept::Ready,
        }
    }
}

fn strip(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

pub fn parse_app_message(line: &str) -> Result<AppMessage, ParseError> {
    let line = strip(line);
    let refused = || ParseError {
        line: line.to_string(),
    };
    let message: AppMessage = serde_json::from_str(line).map_err(|_| refused())?;
    // A running cell is the host's to draw, never a sub-application's to say.
    if let AppMessage::Cells { cells, .. } = &message
        && cells.iter().any(|cell| cell.state == CellState::Running)
    {
        return Err(refused());
    }
    Ok(message)
}

pub fn parse_host_message(line: &str) -> Result<HostMessage, ParseError> {
    let line = strip(line);
    serde_json::from_str(line).map_err(|_| ParseError {
        line: line.to_string(),
    })
}

/// One line, including the trailing newline. Compact JSON escapes any
/// newline inside a string, so the message stays on one line.
pub fn encode_app_message(message: &AppMessage) -> String {
    let mut line = serde_json::to_string(message).expect("an app message always encodes");
    line.push('\n');
    line
}

pub fn encode_host_message(message: &HostMessage) -> String {
    let mut line = serde_json::to_string(message).expect("a host message always encodes");
    line.push('\n');
    line
}

/// The file that registers a sub-application with the host. Each one lives
/// in its own folder under `apps/`: `apps/<id>/manifest.json`.
pub const MANIFEST_FILE: &str = "manifest.json";

/// What the host reads about a sub-application before it starts the program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Lowercase letters, digits, and hyphens. It is the folder name under
    /// `apps/` and the value the host's `--app` option takes.
    pub id: String,
    /// The sub-application's name as people see it.
    pub name: String,
    /// The program's file name, found beside the host's executable.
    pub executable: String,
    /// The protocol version the program speaks. Must be `PROTOCOL_VERSION`.
    pub protocol: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError(pub String);

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Reads a manifest and checks the rules the host relies on.
pub fn parse_manifest(text: &str) -> Result<Manifest, ManifestError> {
    let manifest: Manifest =
        serde_json::from_str(text).map_err(|error| ManifestError(error.to_string()))?;
    let id_ok = !manifest.id.is_empty()
        && manifest
            .id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !id_ok {
        return Err(ManifestError(format!(
            "id {:?} must be lowercase letters, digits, and hyphens",
            manifest.id
        )));
    }
    if manifest.executable.is_empty() || manifest.executable.contains(['/', '\\']) {
        return Err(ManifestError(format!(
            "executable {:?} must be a file name, not a path",
            manifest.executable
        )));
    }
    if manifest.protocol != PROTOCOL_VERSION {
        return Err(ManifestError(format!(
            "protocol {} is not this host's version {PROTOCOL_VERSION}",
            manifest.protocol
        )));
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_text(id: &str, executable: &str, protocol: u32) -> String {
        serde_json::json!({
            "id": id,
            "name": "Sketch",
            "executable": executable,
            "protocol": protocol,
        })
        .to_string()
    }

    /// Catches a framing that splits on spaces and drops the rest of the label.
    #[test]
    fn tab_label_with_spaces_round_trips() {
        let label = "Canvas 1".to_string();
        let message = AppMessage::Tab {
            id: "canvas-1".to_string(),
            label: label.clone(),
        };
        let encoded = encode_app_message(&message);
        let line = encoded
            .strip_suffix('\n')
            .expect("encoded message ends with one newline");
        let parsed = parse_app_message(line).expect("label with spaces parses as one message");
        assert_eq!(parsed, message);

        let mut declaration = Declaration::empty();
        assert_eq!(
            declaration.accept(AppMessage::Hello { protocol: 1 }),
            Accept::More
        );
        assert_eq!(declaration.accept(parsed), Accept::More);
        assert_eq!(declaration.tabs[0].label, label);
    }

    /// Catches a declaration the host would draw without agreeing the version.
    #[test]
    fn declaration_fails_without_hello_version_1() {
        let mut declaration = Declaration::empty();
        assert_eq!(
            declaration.accept(AppMessage::Tab {
                id: "canvas-1".to_string(),
                label: "Canvas 1".to_string(),
            }),
            Accept::Failed
        );

        let mut declaration = Declaration::empty();
        assert_eq!(
            declaration.accept(AppMessage::Hello { protocol: 2 }),
            Accept::Failed
        );
    }

    /// Catches an id that is not lowercase letters, digits, and hyphens.
    #[test]
    fn manifest_refuses_id_outside_lowercase_digits_and_hyphens() {
        let text = manifest_text("Sketch", "sketch", 1);
        assert!(parse_manifest(&text).is_err(), "uppercase id was accepted");
    }

    /// Catches an executable that is a path.
    #[test]
    fn manifest_refuses_executable_path() {
        for executable in ["bin/sketch", "bin\\sketch"] {
            let text = manifest_text("sketch", executable, 1);
            assert!(
                parse_manifest(&text).is_err(),
                "path executable {executable:?} was accepted"
            );
        }
    }

    /// Catches a protocol version other than 1.
    #[test]
    fn manifest_refuses_protocol_other_than_1() {
        let text = manifest_text("sketch", "sketch", 2);
        assert!(parse_manifest(&text).is_err(), "protocol 2 was accepted");
    }

    /// Catches a result sent under another type name than `result`, or a
    /// tool key that splits wrongly at an application id's hyphen.
    #[test]
    fn invoke_and_result_round_trip() {
        let key = tool_key("simple-sketch", "make", "add_shape");
        assert_eq!(split_tool_key(&key), Some(("simple-sketch", "make", "add_shape")));
        assert_eq!(split_tool_key("simple-sketch__make"), None);

        let invoke = HostMessage::Invoke {
            call: "c1".to_string(),
            tab: "7".to_string(),
            tool: key,
            input: serde_json::json!({"kind": "ellipse"}),
            group: "r1".to_string(),
        };
        let line = encode_host_message(&invoke);
        assert_eq!(parse_host_message(&line).unwrap(), invoke);

        let result = AppMessage::ToolResult(ToolResult {
            call: "c1".to_string(),
            ok: true,
            output: serde_json::json!({"id": 12}),
            images: Vec::new(),
        });
        let line = encode_app_message(&result);
        assert!(line.contains("\"type\":\"result\""), "{line}");
        assert_eq!(parse_app_message(&line).unwrap(), result);
    }

    /// Catches a tab list from before 0.3, without `path` or `saves`, that
    /// no longer parses.
    #[test]
    fn tabs_without_workspace_fields_still_parse() {
        let line = r#"{"type":"tabs","tabs":[{"id":"1","label":"Untitled 1"}],"active":"1"}"#;
        let AppMessage::Tabs(tabs) = parse_app_message(line).unwrap() else {
            panic!("not a tab list");
        };
        assert_eq!(tabs.tabs[0].path, None);
        assert!(!tabs.tabs[0].saves);
    }

    /// Catches `open` without a path being refused: a workspace reopens a
    /// tab that has no file this way.
    #[test]
    fn open_without_path_parses() {
        assert_eq!(
            parse_host_message(r#"{"type":"open"}"#).unwrap(),
            HostMessage::Open { path: None }
        );
    }

    fn round_trip_app(message: AppMessage) {
        let line = encode_app_message(&message);
        assert_eq!(parse_app_message(&line).unwrap(), message);
    }

    fn round_trip_host(message: HostMessage) {
        let line = encode_host_message(&message);
        assert_eq!(parse_host_message(&line).unwrap(), message);
    }

    /// A cell whose prompt and result hold a file, and one of a kind this
    /// build does not run, both survive a round trip. Dropping either would
    /// lose a later kind.
    /// Catches a cells message from a later version refused whole: a part
    /// and a state this build does not know are kept, and the rest reads.
    #[test]
    fn a_later_part_and_state_still_read() {
        let line = r#"{"type":"cells","tab":"book","cells":[{"id":"x","kind":"text","input":[{"text":"north"},{"image_ref":3}],"state":"queued"}]}"#;
        let AppMessage::Cells { cells, .. } = parse_app_message(line).expect("a later cell was refused") else {
            panic!("not a cells message");
        };
        assert_eq!(cells[0].state, CellState::Other);
        assert_eq!(cells[0].input.len(), 2);
        assert!(matches!(cells[0].input[1], CellPart::Other(_)));
        assert_eq!(cells[0].prompt(), "north");
    }

    #[test]
    fn a_cell_with_a_path_and_an_unknown_kind_round_trip() {
        let path = Cell {
            id: "11111111-1111-4111-8111-111111111111".into(),
            from: Some("00000000-0000-4000-8000-000000000000".into()),
            kind: "text".into(),
            input: vec![
                CellPart::Text { text: "Describe this.".into() },
                CellPart::Path {
                    path: "/tmp/still.png".into(),
                    media_type: "image/png".into(),
                },
            ],
            endpoint: Some("openrouter".into()),
            model: Some("stand-in/model".into()),
            output: vec![CellPart::Path {
                path: "/tmp/out.png".into(),
                media_type: "image/png".into(),
            }],
            state: CellState::Done,
            error: None,
        };
        let unknown = Cell {
            id: "22222222-2222-4222-8222-222222222222".into(),
            from: None,
            kind: "plot".into(),
            input: vec![CellPart::Text { text: "a rising line".into() }],
            endpoint: None,
            model: None,
            output: Vec::new(),
            state: CellState::Idle,
            error: None,
        };
        round_trip_app(AppMessage::Cells {
            tab: "book".into(),
            cells: vec![path.clone(), unknown.clone()],
            selected: Some(unknown.id.clone()),
        });
        round_trip_host(HostMessage::InsertCell {
            tab: "book".into(),
            index: 1,
            cell: unknown,
        });
        round_trip_host(HostMessage::CellResult {
            tab: "book".into(),
            cell: path.id.clone(),
            endpoint: "openrouter".into(),
            model: "stand-in/model".into(),
            state: CellState::Failed,
            output: path.output.clone(),
            error: Some("The model failed.".into()),
        });
        assert!(parse_app_message(r#"{"type":"cells","tab":"book","cells":[{"id":"x","kind":"plot","state":"running"}]}"#).is_err());
    }

    /// Every cell message round-trips, so a field cannot be dropped quietly.
    #[test]
    fn cell_messages_round_trip() {
        round_trip_app(AppMessage::RunCell {
            tab: "book".into(),
            cell: "c1".into(),
        });
        round_trip_app(AppMessage::StopCell {
            tab: "book".into(),
            cell: "c1".into(),
        });
        round_trip_app(AppMessage::Claimed {
            id: "claim-1".into(),
            yours: true,
        });
        round_trip_host(HostMessage::EditCell {
            tab: "book".into(),
            cell: "c1".into(),
            input: vec![CellPart::Text { text: "A lighthouse.".into() }],
        });
        round_trip_host(HostMessage::SelectCell {
            tab: "book".into(),
            cell: "c1".into(),
        });
        round_trip_host(HostMessage::Claim {
            id: "claim-1".into(),
            path: "/tmp/book.smx".into(),
        });
    }
}
