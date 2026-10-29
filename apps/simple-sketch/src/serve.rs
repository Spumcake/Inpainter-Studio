//! Simple Sketch as a sub-application: the protocol loop on standard input and
//! output, around the session Simple Sketch 1.0.1 drove from its window. The
//! startup, the preferences and recovery upkeep, and the panic hook are the
//! ones `canvas.rs` ran; the window is the host's. Each host message runs the
//! session call the window's widget made, and the declarations that changed
//! are sent back. The assistant's `invoke` runs one of the skills in
//! `skills.rs`; a workspace's `open` and `keep` open a file in a tab and give
//! a tab a file through Simple Sketch's own Save.
//!
//! Inside a project, which the host names with `project`, every sketch is
//! written into the project's folder by itself, a second after it changes,
//! and before a tab closes, a `keep` is answered, or the host quits. Save,
//! Save As, recovery copies, and their questions are not used there.

use crate::ask::Chrome;
use crate::look::Looker;
use crate::registry::Tool;
use crate::session::{
    FileIntent, RecoveryOffer, SaveResult, Session, action_named, command_named, session_value,
    tool_named,
};
use draw::document::{ColorId, NodeId, SketchId};
use draw::gesture::{Gesture, GestureKind};
use draw::{PropId, PropValue};
use protocol::{
    AppMessage, Ask, Canvas, ContentsKind, DocumentCamera, FileFilter, FileKind, HostMessage, Keys,
    MenuEntry, PanelSide, Properties, SelectOp, Tabs, Toolbar, ToolResult,
};
use serde_json::Value;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a change waits before it is written, so a burst of edits is
/// written once.
const WRITE_AFTER: Duration = Duration::from_secs(1);
/// How long a failed write waits before it is tried again by itself.
const RETRY_AFTER: Duration = Duration::from_secs(5);
/// How often the loop wakes with no message, to write what is due.
const TICK: Duration = Duration::from_millis(250);

/// A file dialog Simple Sketch asked the host for, and is waiting on.
enum Waiting {
    Save {
        id: SketchId,
        copy: bool,
        close_after: bool,
    },
    Open,
}

/// What was last sent of each declaration, so only a change is sent again.
#[derive(Default)]
struct Sent {
    document: Option<(String, Value, Vec<u64>, Option<u64>)>,
    tabs: Option<Tabs>,
    shown: Vec<SketchId>,
    keys: Option<Keys>,
    toolbar: Option<Toolbar>,
    menu: Option<Vec<MenuEntry>>,
    properties: Option<Properties>,
    canvas: Option<Canvas>,
    ask: Option<Ask>,
}

/// One Simple Sketch session behind the protocol. `run` feeds it standard
/// input; a test can feed it the same messages in-process.
pub struct Server {
    session: Session,
    chrome: Chrome,
    waiting: Option<(String, Waiting)>,
    next_request: u64,
    /// The host's contents area, once it is known.
    viewport: Option<[f64; 2]>,
    sent: Sent,
    /// Draws the looking tool's pictures.
    looker: Looker,
    /// Started with `--restore` and no `open` yet: the session's opening
    /// blank is not a tab, and its recovery offers wait for a tab to show.
    hidden: bool,
    deferred: Vec<RecoveryOffer>,
    /// A `keep` waiting on Simple Sketch's own Save: the host's tab and the sketch.
    keeping: Option<(String, SketchId)>,
    /// The current project's folder from the host.
    project: Option<PathBuf>,
    /// Sketches with work not yet in their file, since when.
    unwritten_since: HashMap<SketchId, Instant>,
    /// Sketches whose last write failed: why, and when.
    write_failed: HashMap<SketchId, (String, Instant)>,
}

impl Server {
    /// The session `canvas.rs` started, with its preferences, recent files,
    /// and recovery offers read from `data_dir` when there is one.
    pub fn new(data_dir: Option<&std::path::Path>) -> Self {
        Self::start(data_dir, false)
    }

    /// As `new`, or, with `restore`, opening no sketch of its own until the
    /// host sends `open`, as when a workspace is reopened. Recovery offers
    /// then wait until that first tab exists.
    pub fn start(data_dir: Option<&std::path::Path>, restore: bool) -> Self {
        let mut session = Session::new();
        let mut chrome = Chrome::new();
        let mut deferred = Vec::new();
        if let Some(dir) = data_dir {
            session.set_data_dir(dir);
            let (tools, recent) = crate::prefs::load_dir(dir);
            session.restore_tools(tools);
            session.set_recent(recent);
            if restore {
                deferred = session.recovery_offers();
            } else {
                chrome.set_recoveries(session.recovery_offers());
            }
        }
        Self {
            session,
            chrome,
            waiting: None,
            next_request: 1,
            viewport: None,
            sent: Sent::default(),
            looker: Looker::new(),
            hidden: restore,
            deferred,
            keeping: None,
            project: None,
            unwritten_since: HashMap::new(),
            write_failed: HashMap::new(),
        }
    }

    /// The declaration, with the skills, then the open sketch's document and
    /// everything the host draws for it.
    pub fn greeting(&mut self) -> Vec<AppMessage> {
        let mut messages = vec![AppMessage::Hello {
            protocol: protocol::PROTOCOL_VERSION,
        }];
        let tabs = if self.hidden {
            Vec::new()
        } else {
            self.session.tabs()
        };
        for tab in tabs {
            let id = tab_id(tab.id);
            messages.push(AppMessage::Tab {
                id: id.clone(),
                label: tab.name.to_string(),
            });
            messages.push(AppMessage::Contents {
                tab: id.clone(),
                kind: ContentsKind::Canvas,
            });
            messages.push(AppMessage::Panel {
                tab: id,
                side: PanelSide::Right,
            });
        }
        messages.push(AppMessage::Skills {
            skills: crate::skills::skills(),
        });
        messages.push(AppMessage::End);
        messages.extend(self.updates(true));
        messages
    }

    /// One message from the host, and every reply to it. `None` means stop.
    pub fn handle(&mut self, message: HostMessage) -> Option<Vec<AppMessage>> {
        let mut replies = Vec::new();
        let mut camera = false;
        let mut last = Vec::new();
        if is_hand(&message) {
            // A change by hand ends a request's undo step.
            self.session.close_groups();
        }
        if self.hidden && is_tab_work(&message) {
            return Some(Vec::new());
        }
        match message {
            HostMessage::Shutdown => return None,
            HostMessage::Ping => return Some(vec![AppMessage::Pong]),
            HostMessage::Invoke {
                call,
                tab,
                tool,
                input,
                group,
            } => last = self.invoke(call, &tab, &tool, &input, &group),
            HostMessage::Project { folder } => {
                let folder = PathBuf::from(folder);
                self.session.set_project(&folder);
                self.project = Some(folder);
                // In a project every change is in its file, so there is
                // nothing to recover.
                self.chrome.set_recoveries(Vec::new());
                self.deferred.clear();
            }
            // A path outside the project opens nothing.
            HostMessage::Open { path: Some(path) } if !self.allowed_project_path(std::path::Path::new(&path)) => {}
            HostMessage::Open { path } => {
                let known: Vec<SketchId> = if self.hidden {
                    Vec::new()
                } else {
                    self.session.tabs().iter().map(|tab| tab.id).collect()
                };
                self.open(path);
                // A tab new since the greeting is declared as the greeting's
                // were, before the `tabs` that lists it.
                if !self.hidden {
                    for tab in self.session.tabs() {
                        if !known.contains(&tab.id) {
                            replies.push(AppMessage::Contents {
                                tab: tab_id(tab.id),
                                kind: ContentsKind::Canvas,
                            });
                            replies.push(AppMessage::Panel {
                                tab: tab_id(tab.id),
                                side: PanelSide::Right,
                            });
                        }
                    }
                }
            }
            HostMessage::Keep { tab } if self.project.is_some() => {
                let kept = match sketch_named(&self.session, Some(&tab)).filter(|_| !self.hidden) {
                    None => Ok(None),
                    Some(id) => self.write(id),
                };
                last.push(match kept {
                    Ok(path) => AppMessage::Kept { tab, path: path.map(path_text), error: None },
                    Err(error) => AppMessage::Kept { tab, path: None, error: Some(error) },
                });
            }
            HostMessage::Keep { tab } => match sketch_named(&self.session, Some(&tab)).filter(|_| !self.hidden) {
                None => last.push(AppMessage::Kept {
                    tab,
                    path: None,
                    error: None,
                }),
                Some(id) => match self.session.file_path(id) {
                    Some(path) => last.push(AppMessage::Kept {
                        tab,
                        path: Some(path_text(path)),
                        error: None,
                    }),
                    None => {
                        self.session.request_save_dialog(id, false);
                        self.keeping = Some((tab, id));
                        if let Some(request) = self.file_request() {
                            replies.push(request);
                        }
                    }
                },
            },
            HostMessage::Viewport { viewport } => {
                let first = self.viewport.is_none();
                self.note_viewport(viewport);
                if first && let Some(viewport) = self.viewport {
                    self.session.present_view(viewport);
                    camera = true;
                }
            }
            HostMessage::Command {
                id,
                tab,
                arg,
                node,
                viewport,
            } => {
                self.note_viewport(viewport);
                self.command(&id, tab.as_deref(), arg.as_ref(), node, viewport);
                if let Some(request) = self.file_request() {
                    replies.push(request);
                }
            }
            HostMessage::Gesture { tab, gesture } => {
                // A gesture is always answered with the document, even when
                // it changed nothing, so the host's preview of it can end.
                self.sent.document = None;
                activate(&mut self.session, Some(&tab));
                if let Some(gesture) = Gesture::from_value(&gesture) {
                    replay(&mut self.session, gesture);
                }
            }
            HostMessage::Select { tab, op } => {
                activate(&mut self.session, Some(&tab));
                select(&mut self.session, op);
            }
            HostMessage::Camera { tab, origin, zoom } => {
                activate(&mut self.session, Some(&tab));
                self.session.set_view(origin, zoom);
            }
            HostMessage::Property {
                prop,
                value,
                preview,
            } => {
                if let (Some(id), Some(value)) =
                    (prop.as_str().and_then(PropId::from_key), prop_value(&value))
                {
                    if preview {
                        self.session.preview_panel_property(id, value);
                    } else {
                        self.session.set_panel_property(id, value);
                    }
                }
            }
            HostMessage::Answer { id, button } => {
                self.sent.ask = None;
                self.chrome.answer(&mut self.session, &id, &button);
                if let Some(request) = self.file_request() {
                    replies.push(request);
                }
            }
            HostMessage::FileResult { id, path } => {
                camera = self.file_result(&id, path);
            }
            HostMessage::Quit => {
                if self.project.is_some() {
                    // Everything is written first, so the quit asks nothing.
                    for id in self.session.unwritten(true) {
                        let _ = self.write(id);
                    }
                }
                self.chrome.note_quit(&mut self.session)
            }
            HostMessage::Claim { id, path } => replies.push(AppMessage::Claimed {
                id,
                yours: crate::store::readable_smx(std::path::Path::new(&path))
                    && self.allowed_project_path(std::path::Path::new(&path)),
            }),
            HostMessage::EditCell { .. }
            | HostMessage::CellResult { .. }
            | HostMessage::InsertCell { .. }
            | HostMessage::SelectCell { .. }
            | HostMessage::SetVersion { .. }
            | HostMessage::MoveCell { .. }
            | HostMessage::DeleteCell { .. }
            | HostMessage::ShowSource { .. } => {}
        }
        if self.project.is_some() {
            self.write_due(Instant::now());
        } else {
            self.session.poll_recovery(Instant::now());
            self.session.publish_emergency();
        }
        persist_tools(&mut self.session);
        replies.extend(self.updates(camera));
        replies.extend(last);
        if let Some(kept) = self.resolve_keep() {
            replies.push(kept);
        }
        Some(replies)
    }

    /// One tool of a skill on `tab`, as one undo step with the rest of
    /// `group`. A tab other than the one shown is worked on and its document
    /// sent, and the shown tab stays shown. Returns that document, if any,
    /// and the result.
    fn invoke(
        &mut self,
        call: String,
        tab: &str,
        tool: &str,
        input: &Value,
        group: &str,
    ) -> Vec<AppMessage> {
        let answer = |ok: bool, output: Value, images: Vec<String>| {
            AppMessage::ToolResult(ToolResult {
                call: call.clone(),
                ok,
                output,
                images,
            })
        };
        let refuse = |reason: String| answer(false, serde_json::json!({ "error": reason }), Vec::new());
        let Some(sketch) = sketch_named(&self.session, Some(tab)).filter(|_| !self.hidden) else {
            return vec![refuse(format!("Simple Sketch has no tab {tab}"))];
        };
        let Some((_, skill, name)) = protocol::split_tool_key(tool) else {
            return vec![refuse(format!("{tool} is not a tool key"))];
        };
        let shown = self.session.active_id();
        self.session.activate(sketch);
        self.session.assist_settle();
        self.session.join_group(sketch, group);
        let outcome = crate::skills::Desk {
            session: &mut self.session,
            looker: &mut self.looker,
            viewport: self.viewport,
        }
        .run(skill, name, input);
        let mut out = Vec::new();
        if shown != sketch {
            let record = self.session.active_sketch();
            out.push(AppMessage::Document {
                tab: tab_id(record.id()),
                sketch: record.sketch.snapshot_value(record.id(), record.name()),
                selection: record.selection().iter().map(|node| node.get()).collect(),
                active_canvas: record.active_canvas().map(NodeId::get),
                camera: None,
            });
            self.session.activate(shown);
        }
        out.push(answer(outcome.ok, outcome.output, outcome.images));
        out
    }

    /// A workspace's `open`: that file in a tab, or the sketch Simple Sketch
    /// opens of its own at start, a new untitled one.
    fn open(&mut self, path: Option<String>) {
        match (self.hidden, path) {
            (true, Some(path)) => {
                if self.session.open_initial(std::path::Path::new(&path)) {
                    self.reveal();
                }
            }
            (true, None) => self.reveal(),
            (false, Some(path)) => {
                self.session.open_path(std::path::Path::new(&path));
            }
            (false, None) => {
                self.session.new_sketch();
            }
        }
    }

    /// The first tab of a restored workspace exists: recovery can be offered.
    fn reveal(&mut self) {
        self.hidden = false;
        self.chrome.set_recoveries(std::mem::take(&mut self.deferred));
    }

    /// Writes, inside a project, every sketch whose change is a second old,
    /// and tries a failed one again after a while. Words still being typed
    /// wait until the text box closes.
    fn write_due(&mut self, now: Instant) {
        let unwritten = self.session.unwritten(false);
        self.unwritten_since.retain(|id, _| unwritten.contains(id));
        for id in unwritten {
            let since = *self.unwritten_since.entry(id).or_insert(now);
            if now.duration_since(since) < WRITE_AFTER {
                continue;
            }
            if let Some((_, failed)) = self.write_failed.get(&id)
                && now.duration_since(*failed) < RETRY_AFTER
            {
                continue;
            }
            let _ = self.write(id);
        }
    }

    /// Writes one sketch into the project's folder. A failure is shown once,
    /// when it is new, and is kept for a `keep` to report.
    fn write(&mut self, id: SketchId) -> Result<Option<PathBuf>, String> {
        let result = self.session.write_into_project(id);
        match &result {
            Ok(_) => {
                self.unwritten_since.remove(&id);
                self.write_failed.remove(&id);
            }
            Err(error) => {
                let repeated = self.write_failed.contains_key(&id);
                if repeated {
                    // Shown already: do not ask again each time.
                    let _ = self.session.take_file_error();
                }
                self.write_failed.insert(id, (error.clone(), Instant::now()));
            }
        }
        result
    }

    /// The loop's wake with no message: writes what is due, and sends what
    /// that changed, such as a tab's new name.
    pub fn tick(&mut self) -> Vec<AppMessage> {
        if self.project.is_none() {
            return Vec::new();
        }
        self.write_due(Instant::now());
        self.updates(false)
    }

    /// The answer to a `keep` once its Save is done or cancelled.
    fn resolve_keep(&mut self) -> Option<AppMessage> {
        let (_, id) = self.keeping.as_ref()?;
        if self.waiting.is_some() || self.chrome.replacing() {
            return None;
        }
        let path = self.session.file_path(*id).map(path_text);
        let (tab, _) = self.keeping.take()?;
        Some(AppMessage::Kept {
            tab,
            path,
            error: None,
        })
    }

    fn note_viewport(&mut self, viewport: [f64; 2]) {
        if viewport[0] > 1.0 && viewport[1] > 1.0 {
            self.viewport = Some(viewport);
        }
    }

    fn allowed_project_path(&self, path: &std::path::Path) -> bool {
        let Some(project) = &self.project else {
            return true;
        };
        let project = project.canonicalize().unwrap_or_else(|_| project.clone());
        let candidate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            project.join(path)
        };
        let candidate = candidate.canonicalize().unwrap_or(candidate);
        candidate.starts_with(&project)
    }

    /// Inside a project, a closing tab's work is written first, so closing
    /// asks nothing. A sketch that could not be written still asks.
    fn write_before_closing(&mut self, id: SketchId) {
        if self.project.is_some() && self.session.unwritten(true).contains(&id) {
            let _ = self.write(id);
        }
    }

    /// A declared control's action, as the window's widget ran it.
    fn command(
        &mut self,
        id: &str,
        tab: Option<&str>,
        arg: Option<&Value>,
        node: Option<u64>,
        viewport: [f64; 2],
    ) {
        match id {
            "select_tab" => activate(&mut self.session, tab),
            "close_tab" => {
                if let Some(id) = sketch_named(&self.session, tab) {
                    self.write_before_closing(id);
                    let _ = self.chrome.close(&mut self.session, id);
                }
            }
            "close" => {
                activate(&mut self.session, tab);
                let id = self.session.active_id();
                self.write_before_closing(id);
                let _ = self.chrome.close(&mut self.session, id);
            }
            "save" if self.project.is_some() => {
                activate(&mut self.session, tab);
                let id = self.session.active_id();
                let _ = self.write(id);
            }
            "save_as" | "save_copy" if self.project.is_some() => {}
            "open_recent" => {
                let allowed = arg
                    .and_then(Value::as_str)
                    .is_some_and(|path| self.allowed_project_path(std::path::Path::new(path)));
                if !allowed {
                    return;
                }
                activate(&mut self.session, tab);
                run_action(&mut self.session, id, arg, node, viewport);
            }
            _ => {
                activate(&mut self.session, tab);
                run_action(&mut self.session, id, arg, node, viewport);
            }
        }
    }

    /// The dialog the session's last command asked for, as a `file` request.
    fn file_request(&mut self) -> Option<AppMessage> {
        let intent = self.session.take_file_intent()?;
        let request = format!("file-{}", self.next_request);
        self.next_request += 1;
        let folder = self.session.dialog_directory().map(path_text);
        let filter = Some(FileFilter {
            name: "Sketch".to_string(),
            extensions: vec!["smx".to_string()],
        });
        Some(match intent {
            FileIntent::Save {
                id,
                copy,
                close_after,
            } => {
                let name = Some(self.session.suggested_save_name(id));
                self.waiting = Some((
                    request.clone(),
                    Waiting::Save {
                        id,
                        copy,
                        close_after,
                    },
                ));
                AppMessage::File {
                    id: request,
                    kind: FileKind::Save,
                    name,
                    folder,
                    filter,
                }
            }
            FileIntent::Open => {
                self.waiting = Some((request.clone(), Waiting::Open));
                AppMessage::File {
                    id: request,
                    kind: FileKind::Open,
                    name: None,
                    folder,
                    filter,
                }
            }
        })
    }

    /// The path the host's dialog returned, as `resolve_dialogs` handled it.
    /// Returns whether a sketch was opened, which brings its camera.
    fn file_result(&mut self, id: &str, path: Option<String>) -> bool {
        let Some((request, wait)) = self.waiting.take() else {
            return false;
        };
        if request != id {
            self.waiting = Some((request, wait));
            return false;
        }
        let Some(path) = path.map(PathBuf::from) else {
            if let Waiting::Save {
                close_after: true, ..
            } = wait
            {
                self.chrome.cancel_save_as();
            }
            return false;
        };
        if !self.allowed_project_path(&path) {
            if let Waiting::Save {
                close_after: true, ..
            } = wait
            {
                self.chrome.cancel_save_as();
            }
            return false;
        }
        let session = &mut self.session;
        session.note_dialog_folder(&path);
        match wait {
            Waiting::Save {
                id,
                copy,
                close_after,
            } => {
                match session.save_path(id, &path, copy, false) {
                    SaveResult::Wrote => {
                        if close_after {
                            self.chrome.complete_save(session, id);
                        }
                    }
                    SaveResult::NeedsReplace(path) => {
                        self.chrome.ask_replace(id, path, copy, close_after);
                    }
                    SaveResult::Failed => {
                        if close_after {
                            self.chrome.cancel_save_as();
                        }
                    }
                }
                false
            }
            Waiting::Open => session.open_path(&path),
        }
    }

    /// The document and each declaration that changed since it was last sent.
    /// A sketch shown for the first time is presented in the host's view and
    /// brings its camera.
    fn updates(&mut self, camera: bool) -> Vec<AppMessage> {
        let mut out = Vec::new();
        if self.hidden {
            // No tab yet: only a question, such as a file that could not
            // be opened, or the end of a quit.
            self.chrome_updates(&mut out);
            return out;
        }
        let active = self.session.active_id();
        let first_view = !self.sent.shown.contains(&active);
        if first_view && let Some(viewport) = self.viewport {
            self.session.present_view(viewport);
        }
        let camera = camera || first_view;

        let record = self.session.active_sketch();
        let state = (
            tab_id(record.id()),
            record.sketch.snapshot_value(record.id(), record.name()),
            record
                .selection()
                .iter()
                .map(|node| node.get())
                .collect::<Vec<_>>(),
            record.active_canvas().map(NodeId::get),
        );
        // The host keeps the camera while it shows the tab, so a pan alone
        // does not send the document back.
        let mut seen = state.clone();
        if let Some(object) = seen.1.as_object_mut() {
            object.remove("camera");
        }
        if camera || self.sent.document.as_ref() != Some(&seen) {
            let view = record.camera();
            out.push(AppMessage::Document {
                tab: state.0.clone(),
                sketch: state.1.clone(),
                selection: state.2.clone(),
                active_canvas: state.3,
                camera: camera.then_some(DocumentCamera {
                    origin: view.origin,
                    zoom: view.zoom,
                }),
            });
            self.sent.document = Some(seen);
        }

        let tabs = self.session.declare_tabs();
        if self.sent.tabs.as_ref() != Some(&tabs) {
            out.push(AppMessage::Tabs(tabs.clone()));
            self.sent.tabs = Some(tabs);
        }
        if first_view {
            self.sent.shown.push(active);
        }
        let keys = self.session.declare_keys();
        if self.sent.keys.as_ref() != Some(&keys) {
            out.push(AppMessage::Keys(keys.clone()));
            self.sent.keys = Some(keys);
        }
        let toolbar = self.session.declare_toolbar();
        if self.sent.toolbar.as_ref() != Some(&toolbar) {
            out.push(AppMessage::Toolbar(toolbar.clone()));
            self.sent.toolbar = Some(toolbar);
        }
        let menu = self.session.declare_menu();
        if self.sent.menu.as_ref() != Some(&menu) {
            out.push(AppMessage::Menu { rows: menu.clone() });
            self.sent.menu = Some(menu);
        }
        let properties = self.session.declare_properties();
        if self.sent.properties.as_ref() != Some(&properties) {
            out.push(AppMessage::Properties(properties.clone()));
            self.sent.properties = Some(properties);
        }
        let canvas = self.session.declare_canvas();
        if self.sent.canvas.as_ref() != Some(&canvas) {
            out.push(AppMessage::Canvas(canvas.clone()));
            self.sent.canvas = Some(canvas);
        }
        self.chrome_updates(&mut out);
        out
    }

    fn chrome_updates(&mut self, out: &mut Vec<AppMessage>) {
        let ask = self.chrome.current(&mut self.session);
        if ask != self.sent.ask {
            if let Some(ask) = &ask {
                out.push(AppMessage::Ask(ask.clone()));
            }
            self.sent.ask = ask;
        }
        if self.chrome.take_quit_ready() {
            out.push(AppMessage::QuitReady);
        }
    }
}

pub fn run() {
    install_panic_hook();
    let data_dir = data_dir_from_args();
    let restore = std::env::args().any(|arg| arg == protocol::RESTORE_FLAG);
    let mut server = Server::start(data_dir.as_deref(), restore);
    for message in server.greeting() {
        send(&message);
    }
    // Lines arrive on their own thread, so the loop also wakes with no
    // message and writes a project's changes that are due.
    let (lines, incoming) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else {
                break;
            };
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    loop {
        let line = match incoming.recv_timeout(TICK) {
            Ok(line) => line,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                for reply in &server.tick() {
                    send(reply);
                }
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let Ok(message) = protocol::parse_host_message(&line) else {
            continue;
        };
        let Some(replies) = server.handle(message) else {
            break;
        };
        for reply in &replies {
            send(reply);
        }
    }
    // The host is gone: whatever a project has not written yet is written.
    for id in server.session.unwritten(true) {
        if server.project.is_some() {
            let _ = server.write(id);
        }
    }
    server.session.finish_size_edit();
    persist_tools(&mut server.session);
}

/// The folder `--data-dir` names. Without it, as in a test with no host,
/// Simple Sketch writes no preferences and no recovery copies.
fn data_dir_from_args() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--data-dir" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

fn install_panic_hook() {
    use std::sync::Once;
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            crate::store::flush_emergency();
            previous(info);
        }));
    });
}

fn persist_tools(session: &mut Session) {
    if session.prefs_dirty()
        && let Some(dir) = session.data_dir()
        && crate::prefs::save_tools(dir, &session.tool_snapshot())
    {
        session.clear_prefs_dirty();
    }
}

/// A message from the person's own hand: it ends a request's undo step.
/// Text metrics and choosing a tab are not edits, and the host sends them
/// while a request runs.
fn is_hand(message: &HostMessage) -> bool {
    match message {
        HostMessage::Command { id, .. } => !matches!(id.as_str(), "text_metrics" | "select_tab"),
        HostMessage::Gesture { .. } | HostMessage::Select { .. } | HostMessage::Property { .. } => {
            true
        }
        _ => false,
    }
}

/// A message about a tab, which waits for one while none is open.
fn is_tab_work(message: &HostMessage) -> bool {
    matches!(
        message,
        HostMessage::Command { .. }
            | HostMessage::Gesture { .. }
            | HostMessage::Select { .. }
            | HostMessage::Camera { .. }
            | HostMessage::Property { .. }
    )
}

fn sketch_named(session: &Session, tab: Option<&str>) -> Option<SketchId> {
    let id = tab?.parse::<u64>().ok()?;
    session
        .tabs()
        .iter()
        .map(|tab| tab.id)
        .find(|item| item.get() == id)
}

fn activate(session: &mut Session, tab: Option<&str>) {
    if let Some(found) = sketch_named(session, tab) {
        session.activate(found);
    }
}

fn number(arg: Option<&Value>, key: &str) -> Option<u64> {
    arg?.get(key)?.as_u64().filter(|id| *id != 0)
}

fn text<'a>(arg: Option<&'a Value>, key: &str) -> Option<&'a str> {
    arg?.get(key)?.as_str()
}

fn node_of(arg: Option<&Value>, key: &str) -> Option<NodeId> {
    number(arg, key).map(NodeId::new)
}

fn color_of(arg: Option<&Value>, key: &str) -> Option<ColorId> {
    number(arg, key).map(ColorId::new)
}

fn position(arg: Option<&Value>) -> Option<usize> {
    arg?.get("value")?.as_u64().map(|value| value as usize)
}

fn flag(arg: Option<&Value>, key: &str) -> bool {
    arg.and_then(|arg| arg.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// A declared action by its id: what the window's toolbar, panel, outliner,
/// menus, keys, and canvas called on the session.
fn run_action(
    session: &mut Session,
    id: &str,
    arg: Option<&Value>,
    node: Option<u64>,
    viewport: [f64; 2],
) {
    let node = node.filter(|node| *node != 0).map(NodeId::new);
    match id {
        "select_tool" => {
            if let Some(tool) = arg.and_then(Value::as_str).and_then(tool_named) {
                session.set_tool(tool);
            }
        }
        "object_properties" => {
            if let Some(node) = arg
                .and_then(Value::as_str)
                .and_then(|arg| arg.parse::<u64>().ok())
                .filter(|node| *node != 0)
            {
                session.open_object_page(NodeId::new(node));
            }
        }
        "object_action" => {
            if let (Some(node), Some(action)) = (
                node.or_else(|| node_of(arg, "node")),
                text(arg, "action").and_then(action_named),
            ) {
                session.object_action(node, action);
            }
        }
        "edit_text" => {
            if let Some(node) = node {
                session.edit_text(node);
            }
        }
        "swatch_click" => {
            if let Some(color) = color_of(arg, "color") {
                session.swatch_click(color);
            }
        }
        "swatch_action" => {
            if let (Some(color), Some(action)) = (
                color_of(arg, "color"),
                text(arg, "action").and_then(action_named),
            ) {
                session.swatch_action(color, action);
            }
        }
        "sort_brush" => {
            if let (Some(color), Some(to)) = (color_of(arg, "color"), position(arg)) {
                let _ = session.move_brush_color(color, to);
            }
        }
        "tool_action" => {
            if let (Some(tool), Some(action)) = (
                text(arg, "tool").and_then(tool_named),
                text(arg, "action").and_then(action_named),
            ) {
                session.tool_action(tool, action);
            }
        }
        "set_size" => {
            let value = arg
                .and_then(|arg| arg.get("value"))
                .and_then(Value::as_i64)
                .map(|value| value.clamp(0, 255) as u8);
            if let Some(value) = value {
                if flag(arg, "live") {
                    let _ = session.preview_size(value);
                } else {
                    let _ = session.set_size(value);
                }
            }
        }
        "set_bar" => {
            let value = arg.and_then(|arg| arg.get("value")).and_then(Value::as_f64);
            if let (Some(prop), Some(value)) = (text(arg, "prop").and_then(PropId::from_key), value)
            {
                session.set_bar_value(prop, PropValue::Number(value), !flag(arg, "live"));
            }
        }
        "bar_choice" => {
            if let (Some(prop), Some(index)) = (
                text(arg, "prop").and_then(PropId::from_key),
                number_any(arg, "index"),
            ) {
                session.set_bar_value(prop, PropValue::Index(index as u8), true);
            }
        }
        "finish_edit" => session.finish_size_edit(),
        "toggle_list" => session.toggle_text_list(),
        "choose_list" => {
            if let Some(index) = arg.and_then(Value::as_u64) {
                session.choose_text_list(index as usize);
            }
        }
        "go_back" => session.go_back(),
        "make_palette_default" => {
            if let Some(node) = node_of(arg, "node") {
                session.make_palette_default(node);
            }
        }
        "layer_action" => {
            if let (Some(owner), Some(color), Some(action)) = (
                node_of(arg, "owner"),
                color_of(arg, "color"),
                text(arg, "action").and_then(action_named),
            ) {
                session.layer_action(owner, color, action);
            }
        }
        "select_color" => {
            if let (Some(owner), Some(color)) = (node_of(arg, "owner"), color_of(arg, "color")) {
                session.select_color(owner, color);
            }
        }
        "open_palette_color" => {
            if let Some(color) = color_of(arg, "color") {
                session.open_palette_color(color);
            }
        }
        "open_color" => {
            if let (Some(owner), Some(color)) = (node_of(arg, "owner"), color_of(arg, "color")) {
                session.open_color_row(owner, color, flag(arg, "outliner"));
            }
        }
        "rename_node" => {
            if let (Some(node), Some(name)) = (node_of(arg, "node"), text(arg, "value")) {
                session.rename_node(node, name);
            }
        }
        "rename_color" => {
            if let (Some(owner), Some(color), Some(name)) = (
                node_of(arg, "owner"),
                color_of(arg, "color"),
                text(arg, "value"),
            ) {
                session.rename_color(owner, color, name);
            }
        }
        "toggle_node" => {
            if let (Some(node), Some(prop)) = (
                node_of(arg, "node"),
                text(arg, "prop").and_then(PropId::from_key),
            ) {
                session.toggle_node(node, prop);
            }
        }
        "toggle_color" => {
            if let (Some(owner), Some(color), Some(prop)) = (
                node_of(arg, "owner"),
                color_of(arg, "color"),
                text(arg, "prop").and_then(PropId::from_key),
            ) {
                session.toggle_color_flag(owner, color, prop);
            }
        }
        "reorder" => {
            if let (Some(node), Some(to)) = (node_of(arg, "node"), position(arg)) {
                session.reorder_node(node, to);
            }
        }
        "sort_color" => {
            if let (Some(color), Some(to)) = (color_of(arg, "color"), position(arg)) {
                let _ = session.move_color(color, to);
            }
        }
        "select_click" => {
            if let Some(node) = node_of(arg, "node") {
                session.select_click(node, flag(arg, "value"));
            }
        }
        "text_edit" => {
            if let Some(content) = text(arg, "value") {
                session.set_text_draft(content.to_string());
            }
        }
        "commit_text" => session.commit_text(),
        "draft_box" => {
            if let Some(frame) = arg
                .and_then(|arg| arg.get("value"))
                .and_then(draw::document::snapshot::box_from_value)
            {
                session.set_draft_box(frame);
            }
        }
        "text_metrics" => {
            let values = arg
                .and_then(|arg| arg.get("value"))
                .and_then(Value::as_array)
                .map(|values| values.iter().filter_map(Value::as_f64).collect::<Vec<_>>());
            if let (Some(node), Some(values)) = (node_of(arg, "node"), values)
                && let [width, height, natural] = values[..]
            {
                session.note_text_metrics(node, width, height, natural);
            }
        }
        "open_recent" => {
            if let Some(path) = arg.and_then(Value::as_str) {
                session.open_path(std::path::Path::new(path));
            }
        }
        _ => {
            if let Some(command) = command_named(id) {
                let shift = flag(arg, "value");
                let _ = session.perform_shifted(command, viewport, shift);
            }
        }
    }
}

fn number_any(arg: Option<&Value>, key: &str) -> Option<u64> {
    arg?.get(key)?.as_u64()
}

/// A click, marquee, or cancelled drag, as the select tool applied it.
fn select(session: &mut Session, op: SelectOp) {
    let ids = |ids: Vec<u64>| -> Vec<NodeId> {
        ids.into_iter()
            .filter(|id| *id != 0)
            .map(NodeId::new)
            .collect()
    };
    match op {
        SelectOp::Click { node, add } if node != 0 => session.select_click(NodeId::new(node), add),
        SelectOp::Click { .. } => {}
        SelectOp::Clear => session.clear_selection(),
        SelectOp::Marquee {
            min,
            max,
            add,
            base,
        } => session.marquee_select(min, max, add, &ids(base)),
        SelectOp::Replace { nodes } => session.replace_selection(ids(nodes)),
    }
}

/// Replays a finished gesture through the session's own gesture path, so the
/// commit is the one Simple Sketch 1.0.1 made. The session's own tool and
/// state decide the canvas and color; the gesture brings its samples, its
/// drag, or the transforms the host previewed.
fn replay(session: &mut Session, gesture: Gesture) {
    match gesture.kind {
        GestureKind::Brush { .. } | GestureKind::Erase { .. } => {
            let tool = if matches!(gesture.kind, GestureKind::Brush { .. }) {
                Tool::Brush
            } else {
                Tool::Eraser
            };
            if session.tool() != tool {
                session.set_tool(tool);
            }
            if session.begin().is_err() {
                return;
            }
            for sample in gesture.samples {
                let _ = session.add_sample(sample);
            }
            let _ = session.commit();
        }
        GestureKind::Shape {
            kind,
            origin,
            pointer,
            shift,
        } => {
            let tool = match kind {
                draw::document::ShapeKind::Rectangle => Tool::Rectangle,
                draw::document::ShapeKind::Ellipse => Tool::Ellipse,
                draw::document::ShapeKind::Line => Tool::Line,
            };
            if session.tool() != tool {
                session.set_tool(tool);
            }
            if session.begin_shape(origin).is_err() {
                return;
            }
            let _ = session.move_shape(pointer, shift);
            let _ = session.commit();
        }
        GestureKind::Text {
            origin, pointer, ..
        } => {
            if session.tool() != Tool::Text {
                session.set_tool(Tool::Text);
            }
            if session.begin_text(origin).is_err() {
                return;
            }
            let _ = session.move_text(pointer);
            let _ = session.end_text(pointer != origin);
        }
        GestureKind::Transform { before, after } => {
            session.restore_locals(&after);
            session.commit_preview(&before);
        }
        GestureKind::TextFrame {
            node,
            before,
            after,
        } => {
            session.restore_text_box(node, after);
            session.commit_text_box(node, before);
        }
    }
}

/// A property value: the panel's form, or a bare number, flag, or word.
fn prop_value(value: &Value) -> Option<PropValue> {
    if let Ok(value) = serde_json::from_value::<protocol::PropValue>(value.clone()) {
        return Some(session_value(value));
    }
    if let Some(number) = value.as_f64() {
        Some(PropValue::Number(number))
    } else if let Some(on) = value.as_bool() {
        Some(PropValue::Bool(on))
    } else {
        value.as_str().map(|text| PropValue::Text(text.to_string()))
    }
}

fn send(message: &AppMessage) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(protocol::encode_app_message(message).as_bytes());
    let _ = stdout.flush();
}

fn tab_id(id: SketchId) -> String {
    id.get().to_string()
}

fn path_text(path: PathBuf) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("simple-sketch-project-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch folder");
        dir
    }

    fn command(id: &str, tab: Option<String>) -> HostMessage {
        HostMessage::Command { id: id.to_string(), tab, arg: None, node: None, viewport: [800.0, 600.0] }
    }

    /// A server inside `folder`, its one tab, and that tab changed.
    fn changed_in(folder: &std::path::Path) -> (Server, String) {
        let mut server = Server::new(None);
        let _ = server.greeting();
        let _ = server.handle(HostMessage::Project { folder: folder.display().to_string() });
        let tab = tab_id(server.session.active_id());
        let _ = server.handle(command("new_canvas", Some(tab.clone())));
        (server, tab)
    }

    fn smx_files(folder: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(folder)
            .expect("the folder")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".smx"))
            .collect();
        names.sort();
        names
    }

    /// Catches a sketch that waits for Save, or one written outside the
    /// project: a second after a change, it is a file in the project's folder.
    #[test]
    fn a_change_is_written_into_the_project_by_itself() {
        let folder = scratch("written");
        let (mut server, _) = changed_in(&folder);
        server.write_due(Instant::now());
        assert!(smx_files(&folder).is_empty(), "written before a second passed");
        server.write_due(Instant::now() + WRITE_AFTER * 2);
        assert_eq!(smx_files(&folder), vec!["Untitled 1.smx".to_string()]);
        assert!(server.session.unwritten(true).is_empty(), "the sketch still counts as unsaved");
        let _ = std::fs::remove_dir_all(folder);
    }

    /// Catches two sessions writing one file: a second sketch named the same
    /// as one already in the folder gets the next free name.
    #[test]
    fn a_new_sketch_never_replaces_another_sessions_file() {
        let folder = scratch("names");
        let (mut first, _) = changed_in(&folder);
        first.write_due(Instant::now() + WRITE_AFTER * 2);
        let before = std::fs::read(folder.join("Untitled 1.smx")).expect("the first file");
        let (mut second, _) = changed_in(&folder);
        let _ = second.handle(command("new_canvas", None));
        second.write_due(Instant::now() + WRITE_AFTER * 2);
        assert_eq!(smx_files(&folder), vec!["Untitled 1.smx".to_string(), "Untitled 2.smx".to_string()]);
        assert_eq!(std::fs::read(folder.join("Untitled 1.smx")).expect("the first file"), before);
        let _ = std::fs::remove_dir_all(folder);
    }

    /// Catches a `keep` that opens a save dialog or reports nothing: in a
    /// project it writes and names the file, an untouched tab has nothing to
    /// keep, and a write that fails says so.
    #[test]
    fn keep_writes_and_reports_a_failure() {
        let folder = scratch("keep");
        let (mut server, tab) = changed_in(&folder);
        let replies = server.handle(HostMessage::Keep { tab: tab.clone() }).expect("replies");
        let kept = replies.iter().find_map(|reply| match reply {
            AppMessage::Kept { path, error, .. } => Some((path.clone(), error.clone())),
            _ => None,
        });
        let expected = folder.canonicalize().expect("the folder").join("Untitled 1.smx");
        assert_eq!(kept, Some((Some(path_text(expected)), None)));
        assert!(!replies.iter().any(|reply| matches!(reply, AppMessage::File { .. })), "a save dialog opened");

        let mut untouched = Server::new(None);
        let _ = untouched.greeting();
        let _ = untouched.handle(HostMessage::Project { folder: folder.display().to_string() });
        let blank = tab_id(untouched.session.active_id());
        let replies = untouched.handle(HostMessage::Keep { tab: blank }).expect("replies");
        assert!(replies.iter().any(|reply| matches!(reply, AppMessage::Kept { path: None, error: None, .. })));

        let locked = scratch("locked");
        let (mut failing, tab) = changed_in(&locked);
        let mut permissions = std::fs::metadata(&locked).expect("the folder").permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&locked, permissions.clone()).expect("read-only");
        let replies = failing.handle(HostMessage::Keep { tab }).expect("replies");
        permissions.set_readonly(false);
        let _ = std::fs::set_permissions(&locked, permissions);
        assert!(
            replies.iter().any(|reply| matches!(reply, AppMessage::Kept { error: Some(_), .. })),
            "a failed write was reported as kept"
        );
        assert!(!failing.session.unwritten(true).is_empty(), "the change was lost");
        let _ = std::fs::remove_dir_all(folder);
        let _ = std::fs::remove_dir_all(locked);
    }

    /// Catches Save offered where every change is written by itself.
    #[test]
    fn a_project_offers_no_save() {
        let folder = scratch("menu");
        let (mut server, _) = changed_in(&folder);
        let rows: Vec<String> = server.session.declare_menu().into_iter().map(|row| row.label).collect();
        assert!(!rows.iter().any(|label| label.starts_with("Save")), "{rows:?}");
        let _ = std::fs::remove_dir_all(folder);
    }
}
