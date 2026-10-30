//! A sub-application's process. The host draws a declaration only after
//! `end`. Later messages replace what the sub-application declared: its tabs,
//! documents, keys, toolbar, menu, panel, drawing surface, and questions.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use draw::document::{Camera, NodeId, Sketch};
use protocol::{
    Accept, AppMessage, Ask, Canvas, Declaration, FileFilter, FileKind, HostMessage, Keys,
    MenuEntry, Properties, RasterView, Skill, TabEntry, Tabs, Toolbar, ToolResult,
};

/// How long a closing window waits for the sub-application to exit after
/// `shutdown` before it stops the process.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
/// A ping goes out this often.
const PING_EVERY: Duration = Duration::from_secs(1);
/// With no answer for this long, the sub-application is not responding.
const NO_ANSWER: Duration = Duration::from_secs(3);

/// What the reader thread sends. The frame thread is the only writer of the
/// declaration that is drawn.
enum PipeEvent {
    Line(AppMessage),
    Invalid,
    Closed,
}

/// A document as the host shows it: the copy it replaces whole on each
/// `document`, and the camera the host keeps while the tab is shown.
pub struct Display {
    pub sketch: Sketch,
    pub camera: Camera,
    /// Counts the documents this tab has received, so a preview held for a
    /// sent gesture ends when the answer arrives.
    pub version: u64,
}

/// A raster tab as the host shows it: the latest view, and the camera the
/// host keeps while the tab is shown.
pub struct RasterDisplay {
    pub view: RasterView,
    /// None until the tab is first drawn, which fits the picture to it.
    pub camera: Option<Camera>,
    /// Counts the views this tab has received, so a preview held for a sent
    /// gesture ends when the answer arrives.
    pub version: u64,
}

/// A file dialog a sub-application asked for.
pub struct FileRequest {
    pub id: String,
    pub kind: FileKind,
    pub name: Option<String>,
    pub folder: Option<String>,
    pub filter: Option<FileFilter>,
}

/// Everything declared after the greeting.
#[derive(Default)]
pub struct Declared {
    pub tabs: Option<Tabs>,
    pub documents: HashMap<String, Display>,
    /// The raster tabs' pictures, by tab.
    pub rasters: HashMap<String, RasterDisplay>,
    pub keys: Keys,
    pub toolbar: Option<Toolbar>,
    pub menu: Vec<MenuEntry>,
    pub properties: Option<Properties>,
    pub canvas: Option<Canvas>,
    pub ask: Option<Ask>,
    pub files: VecDeque<FileRequest>,
    pub quit_ready: bool,
    /// The skills it provides to the assistant, as last declared.
    pub skills: Vec<Skill>,
    /// Counts the `skills` messages, so a change is noticed.
    pub skills_version: u64,
    /// Answers to `invoke` not yet passed to the assistant.
    pub results: VecDeque<ToolResult>,
    /// Answers to `keep`: the tab, and its file or none when cancelled.
    pub kept: VecDeque<(String, Option<String>, Option<String>)>,
    /// The cells a tab reported, and which one is selected.
    pub cells: HashMap<String, (Vec<protocol::Cell>, Option<String>)>,
    /// Cells the toolbar asked the host to run.
    pub run_cells: VecDeque<(String, String)>,
    /// Cells the toolbar asked the host to stop.
    pub stop_cells: VecDeque<(String, String)>,
    /// Answers to `claim`.
    pub claimed: VecDeque<(String, bool)>,
    /// Every tab report, each as its tabs' ids and files, while the host
    /// follows a quit. None when it is not following one.
    pub tab_log: Option<Vec<Vec<(String, Option<String>, String)>>>,
}

/// The sub-application's process, or the record that it never started.
pub struct Running {
    child: Option<Child>,
    events: Option<Receiver<PipeEvent>>,
    reader: Option<JoinHandle<()>>,
    /// Lines for the sub-application's standard input. A writer thread sends
    /// them, so a sub-application that stops reading never stops the frame.
    writer: Option<Sender<String>>,
    writer_thread: Option<JoinHandle<()>>,
    /// The window, once it exists, which a line from the sub-application wakes.
    wake: Arc<Mutex<Option<eframe::egui::Context>>>,
    /// Empty until `Accept::Ready`, then the accepted value.
    shown: Declaration,
    building: Declaration,
    /// True until Ready, Failed, a closed pipe before `end`, or an exit before `end`.
    pending: bool,
    exited: bool,
    pub declared: Declared,
    last_ping: Instant,
    last_pong: Instant,
}

impl Running {
    /// Spawns the sub-application's program, which `apps::discover` found
    /// from its manifest, with its own folder in Inpainter's data folder.
    /// With `restore`, it opens no document of its own and waits for `open`.
    /// A failed spawn is the same as [`Self::not_started`]: the window still opens.
    pub fn start(executable: &std::path::Path, data_dir: Option<PathBuf>, restore: bool) -> Self {
        let mut command = Command::new(executable);
        if let Some(dir) = data_dir
            && std::fs::create_dir_all(&dir).is_ok()
        {
            command.arg("--data-dir").arg(dir);
        }
        if restore {
            command.arg(protocol::RESTORE_FLAG);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => return Self::not_started(),
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Self::not_started();
        };
        let (tx, rx) = mpsc::channel();
        let wake: Arc<Mutex<Option<eframe::egui::Context>>> = Arc::new(Mutex::new(None));
        let reader_wake = Arc::clone(&wake);
        let reader = thread::spawn(move || {
            let notify = || {
                if let Ok(ctx) = reader_wake.lock()
                    && let Some(ctx) = ctx.as_ref()
                {
                    ctx.request_repaint();
                }
            };
            let lines = std::io::BufReader::new(stdout).lines();
            for line in lines {
                let event = match line {
                    Ok(line) => match protocol::parse_app_message(&line) {
                        Ok(parsed) => PipeEvent::Line(parsed),
                        Err(_) => PipeEvent::Invalid,
                    },
                    Err(_) => {
                        let _ = tx.send(PipeEvent::Closed);
                        notify();
                        return;
                    }
                };
                if tx.send(event).is_err() {
                    return;
                }
                notify();
            }
            let _ = tx.send(PipeEvent::Closed);
            notify();
        });
        let (writer, lines) = mpsc::channel::<String>();
        let writer_thread = child.stdin.take().map(|mut stdin| {
            thread::spawn(move || {
                for line in lines {
                    if stdin.write_all(line.as_bytes()).is_err() || stdin.flush().is_err() {
                        return;
                    }
                }
            })
        });
        let now = Instant::now();
        Self {
            child: Some(child),
            events: Some(rx),
            reader: Some(reader),
            writer: Some(writer),
            writer_thread,
            wake,
            shown: Declaration::empty(),
            building: Declaration::empty(),
            pending: true,
            exited: false,
            declared: Declared::default(),
            last_ping: now,
            last_pong: now,
        }
    }

    /// No process: no sub-application was found, or frame tests that do not
    /// spawn one.
    pub fn not_started() -> Self {
        let now = Instant::now();
        Self {
            child: None,
            events: None,
            reader: None,
            writer: None,
            writer_thread: None,
            wake: Arc::new(Mutex::new(None)),
            shown: Declaration::empty(),
            building: Declaration::empty(),
            pending: false,
            exited: true,
            declared: Declared::default(),
            last_ping: now,
            last_pong: now,
        }
    }

    /// No process, showing what these messages declare. Frame tests use this
    /// to draw a sub-application without spawning one.
    #[cfg(test)]
    pub fn declared(messages: Vec<AppMessage>) -> Self {
        let mut running = Self::not_started();
        running.pending = true;
        running.exited = false;
        for message in messages {
            running.apply(PipeEvent::Line(message));
        }
        running
    }

    /// A line from the sub-application now wakes this window.
    pub fn attach(&self, ctx: &eframe::egui::Context) {
        if let Ok(mut wake) = self.wake.lock() {
            *wake = Some(ctx.clone());
        }
    }

    pub fn declaration(&self) -> &Declaration {
        &self.shown
    }

    /// Drains parsed lines, then checks whether the child has exited.
    /// Lines already queued are applied before that exit is recorded.
    pub fn poll(&mut self) {
        let queued = self.drain_events();
        for event in queued {
            self.apply(event);
        }
        if self.exited {
            return;
        }
        if self.last_ping.elapsed() >= PING_EVERY && !self.pending {
            self.last_ping = Instant::now();
            self.send(&HostMessage::Ping);
        }
        let Some(child) = self.child.as_mut() else {
            return;
        };
        match child.try_wait() {
            Ok(None) => {}
            Ok(Some(_)) | Err(_) => self.note_exit(),
        }
    }

    /// Writes one message to the sub-application. A process that has
    /// exited, or never started, receives nothing.
    pub fn send(&mut self, message: &HostMessage) {
        if self.exited {
            return;
        }
        if let Some(writer) = &self.writer {
            let line = protocol::encode_host_message(message);
            if trace() {
                eprint!("inpainter → {line}");
            }
            let _ = writer.send(line);
        }
    }

    pub fn pending(&self) -> bool {
        self.pending
    }

    pub fn exited(&self) -> bool {
        self.exited
    }

    /// A program is running and can answer: not a stand-in a test made, and
    /// not one that has stopped.
    pub fn answers(&self) -> bool {
        self.child.is_some() && !self.exited
    }

    /// No `pong` for three seconds.
    pub fn not_responding(&self) -> bool {
        !self.exited && self.child.is_some() && self.last_pong.elapsed() >= NO_ANSWER
    }

    /// The tabs to draw: the latest `tabs`, or the ones the greeting reported,
    /// with the first one active.
    pub fn tabs(&self) -> (Vec<TabEntry>, Option<String>) {
        if let Some(tabs) = &self.declared.tabs {
            return (tabs.tabs.clone(), Some(tabs.active.clone()));
        }
        let tabs: Vec<TabEntry> = self
            .shown
            .tabs
            .iter()
            .map(|tab| TabEntry {
                id: tab.id.clone(),
                label: tab.label.clone(),
                dirty: false,
                close_waits: false,
                path: None,
                saves: false,
            })
            .collect();
        let active = tabs.first().map(|tab| tab.id.clone());
        (tabs, active)
    }

    /// Tells a still-running sub-application to exit and waits up to
    /// `SHUTDOWN_GRACE`. One that has not exited by then is stopped, so a stuck process
    /// cannot keep the window from closing.
    pub fn shutdown(mut self) {
        if !self.exited {
            if let Some(mut child) = self.child.take() {
                if let Some(writer) = self.writer.take() {
                    let _ = writer.send(protocol::encode_host_message(&HostMessage::Shutdown));
                }
                let deadline = Instant::now() + SHUTDOWN_GRACE;
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) if Instant::now() < deadline => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        _ => {
                            let _ = child.kill();
                            let _ = child.wait();
                            break;
                        }
                    }
                }
            }
            self.exited = true;
        }
        self.writer = None;
        if let Some(writer) = self.writer_thread.take() {
            let _ = writer.join();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }

    fn drain_events(&self) -> Vec<PipeEvent> {
        let Some(events) = &self.events else {
            return Vec::new();
        };
        let mut queued = Vec::new();
        while let Ok(event) = events.try_recv() {
            queued.push(event);
        }
        queued
    }

    fn apply(&mut self, event: PipeEvent) {
        let PipeEvent::Line(line) = event else {
            // A line that does not parse is not drawn. Before `end` it, or a
            // closed pipe, abandons the declaration.
            if self.pending {
                self.abandon();
            }
            return;
        };
        if self.pending {
            match self.building.accept(line.clone()) {
                Accept::More => {}
                Accept::Ready => {
                    self.shown = std::mem::replace(&mut self.building, Declaration::empty());
                    self.pending = false;
                    self.last_pong = Instant::now();
                    return;
                }
                Accept::Failed => {
                    self.abandon();
                    return;
                }
            }
        }
        self.update(line);
    }

    /// A message after the greeting replaces what it declares.
    fn update(&mut self, message: AppMessage) {
        if trace() {
            let line = protocol::encode_app_message(&message);
            let shown: String = line.chars().take(160).collect();
            eprintln!("inpainter ← {}", shown.trim_end());
        }
        let declared = &mut self.declared;
        match message {
            AppMessage::Document {
                tab,
                sketch,
                selection,
                active_canvas,
                camera,
            } => {
                let Some(snapshot) = draw::document::snapshot::sketch_from_value(&sketch) else {
                    return;
                };
                let mut shown = snapshot.sketch;
                let ids = |ids: &[u64]| -> Vec<NodeId> {
                    ids.iter()
                        .filter(|id| **id != 0)
                        .map(|id| NodeId::new(*id))
                        .collect()
                };
                shown.show_state(
                    &ids(&selection),
                    active_canvas.filter(|id| *id != 0).map(NodeId::new),
                );
                let previous = declared.documents.remove(&tab);
                let version = previous.as_ref().map_or(1, |previous| previous.version + 1);
                let camera = match (camera, &previous) {
                    (Some(camera), _) => Camera {
                        origin: camera.origin,
                        zoom: camera.zoom,
                    },
                    (None, Some(previous)) => previous.camera,
                    (None, None) => shown.camera(),
                };
                declared.documents.insert(
                    tab,
                    Display {
                        sketch: shown,
                        camera,
                        version,
                    },
                );
            }
            AppMessage::File {
                id,
                kind,
                name,
                folder,
                filter,
            } => declared.files.push_back(FileRequest {
                id,
                kind,
                name,
                folder,
                filter,
            }),
            AppMessage::Keys(keys) => declared.keys = keys,
            AppMessage::Toolbar(toolbar) => declared.toolbar = Some(toolbar),
            AppMessage::Menu { rows } => declared.menu = rows,
            AppMessage::Properties(properties) => declared.properties = Some(properties),
            AppMessage::Tabs(tabs) => {
                declared
                    .documents
                    .retain(|id, _| tabs.tabs.iter().any(|tab| tab.id == *id));
                declared
                    .rasters
                    .retain(|id, _| tabs.tabs.iter().any(|tab| tab.id == *id));
                if let Some(log) = &mut declared.tab_log {
                    log.push(
                        tabs.tabs
                            .iter()
                            .map(|tab| (tab.id.clone(), tab.path.clone(), tab.label.clone()))
                            .collect(),
                    );
                }
                declared.tabs = Some(tabs);
            }
            AppMessage::Canvas(canvas) => declared.canvas = Some(canvas),
            // The camera a view carries is taken only when a picture is
            // opened or made; otherwise the host's own camera stays.
            AppMessage::Raster(view) => {
                let previous = declared.rasters.remove(&view.tab);
                let version = previous.as_ref().map_or(1, |previous| previous.version + 1);
                let camera = match (&view.camera, previous) {
                    (Some(camera), _) => Some(Camera {
                        origin: camera.origin,
                        zoom: camera.zoom,
                    }),
                    (None, Some(previous)) => previous.camera,
                    (None, None) => None,
                };
                declared.rasters.insert(
                    view.tab.clone(),
                    RasterDisplay {
                        view,
                        camera,
                        version,
                    },
                );
            }
            AppMessage::Ask(ask) => declared.ask = Some(ask),
            AppMessage::QuitReady => declared.quit_ready = true,
            AppMessage::Pong => self.last_pong = Instant::now(),
            // A tab opened after the greeting, such as one a workspace reopens
            // under `--restore`, declares its contents and panel this way.
            AppMessage::Contents { tab, kind } => {
                self.shown.contents.retain(|contents| contents.tab != tab);
                self.shown.contents.push(protocol::ContentsReport { tab, kind });
            }
            AppMessage::Panel { tab, side } => {
                self.shown.panels.retain(|panel| panel.tab != tab);
                self.shown.panels.push(protocol::PanelReport { tab, side });
            }
            AppMessage::Hello { .. } | AppMessage::Tab { .. } | AppMessage::End => {}
            AppMessage::Skills { skills } => {
                declared.skills = skills;
                declared.skills_version += 1;
            }
            AppMessage::ToolResult(result) => declared.results.push_back(result),
            AppMessage::Kept { tab, path, error } => declared.kept.push_back((tab, path, error)),
            AppMessage::Cells { tab, cells, selected } => {
                declared.cells.insert(tab, (cells, selected));
            }
            AppMessage::RunCell { tab, cell } => declared.run_cells.push_back((tab, cell)),
            AppMessage::StopCell { tab, cell } => declared.stop_cells.push_back((tab, cell)),
            AppMessage::Claimed { id, yours } => declared.claimed.push_back((id, yours)),
        }
    }

    fn note_exit(&mut self) {
        self.exited = true;
        if self.pending {
            self.abandon();
        }
    }

    fn abandon(&mut self) {
        self.building = Declaration::empty();
        self.shown = Declaration::empty();
        self.pending = false;
    }
}

/// `INPAINTER_TRACE` set: every message to and from sub-applications is
/// written to standard error, so the two processes' parts can be read apart.
fn trace() -> bool {
    std::env::var_os("INPAINTER_TRACE").is_some()
}
