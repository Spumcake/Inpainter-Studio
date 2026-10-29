//! Session commands. The session does not depend on egui.

use draw::defaults::{self, FONT_MAX, FONT_MIN, SIZE_MAX, SIZE_MIN};
use draw::document::{
    AlignEdge, Allocator, BoundedSize, Cap, ColorId, ColorRole, Dash, Dynamics, ErasePath,
    BoxSide, InkPaint, LayerColor as LayerNodeColor, LayerNode, ListKind, LiveStroke, Node, NodeId, Point,
    Reorder, Sample, SceneMark, ShapeKind, ShapeTool, Sketch, SketchId, Step, Stroke, StrokeId,
    TextBox, TextFit, TextTool, ToolColor, brush_palette, map_samples,
    dragged_box, object_palette, palette_roles_match, preview_shape_paints, resolve_drag, shape_palette,
    text_border, text_palette,
};
use draw::ink;
use crate::record::Record;
use crate::registry::{self, CommandId, Enable, MenuAction, MenuActionDef, SizeTarget, Tool};
use crate::schema::{self, Inspect, PropId, PropValue};
use kurbo::Affine;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

mod assist;
mod declare;
pub(crate) use declare::{action_named, command_named, session_value, tool_named};

/// How long a dirty sketch waits between recovery copies.
pub(crate) const RECOVERY_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// What a close command did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseEffect {
    /// The only clean sketch stayed open.
    Unchanged,
    /// The sketch has changes or a renamed title. It stayed open.
    NeedsConfirm,
    /// The sketch closed. `replacement` is set only when it was the last one.
    Closed { replacement: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandEffect {
    None,
    CenterView,
    Closed { replacement: bool },
    ToggleMaximize,
}

/// A file dialog the window should open. Tests read this and pass a path in
/// instead of opening the dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FileIntent {
    Open,
    Save {
        id: SketchId,
        copy: bool,
        close_after: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SaveResult {
    Wrote,
    /// `path` exists and is not this sketch's file. Nothing was written.
    NeedsReplace(PathBuf),
    Failed,
}

/// A save or open that failed, for the window to tell the person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileError {
    pub saving: bool,
    /// The file's name without `.smx`.
    pub file: String,
    /// Why, as a sentence.
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecoveryOffer {
    pub id: SketchId,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandError {
    GestureAlreadyOpen,
    NoOpenGesture,
    SizeOutOfRange,
    InvalidSample,
    EmptyHistory,
    NotAStrokeTool,
    Unavailable,
}

/// One open tab, in tab order. Widgets read this and do not copy the sketch.
pub struct SketchTab<'a> {
    pub id: SketchId,
    pub name: &'a str,
    pub active: bool,
    /// The close control waits: this is the only sketch, and it is clean.
    pub close_waits: bool,
    /// Changes or a renamed title, so closing asks before the sketch goes away.
    pub needs_keep: bool,
}

pub struct SwatchView {
    pub id: ColorId,
    pub name: String,
    pub rgb: [u8; 3],
    pub active: bool,
    pub muted: bool,
}

pub struct SizeControl {
    pub value: u8,
    pub min: u8,
    pub max: u8,
    /// While the slider follows a locked palette color, it is drawn in that
    /// color. `None` draws it near-black.
    pub tint: Option<[u8; 3]>,
}

/// Who owns a palette color the panel is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColorOwner {
    Tool(Tool),
    Node(NodeId),
}

/// What the properties panel is showing. The current tool and this subject
/// are independent: a right-click opens a page without switching tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Subject {
    Tool(Tool),
    Color {
        owner: ColorOwner,
        color: ColorId,
    },
    /// The outliner, with the select tool.
    Layers,
    /// The selected objects' own settings, or the text being edited in place.
    Objects,
}

/// Tool settings, including every tool's palette, stored in the preferences file.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSnapshot {
    pub brush: Dynamics,
    pub eraser: Dynamics,
    pub eraser_size: u8,
    pub brush_palette: Vec<ToolColor>,
    pub brush_active: Option<ColorId>,
    pub rectangle: ShapeTool,
    pub ellipse: ShapeTool,
    pub line: ShapeTool,
    pub font_size: u8,
    pub text: TextTool,
}

impl ToolSnapshot {
    pub(crate) fn builtin() -> Self {
        let mut ids = Allocator::new();
        Self {
            brush: Dynamics::round(),
            eraser: Dynamics::round(),
            eraser_size: defaults::ERASER_SIZE,
            brush_palette: brush_palette(&mut ids),
            brush_active: None,
            rectangle: ShapeTool::builtin(ShapeKind::Rectangle, &mut ids),
            ellipse: ShapeTool::builtin(ShapeKind::Ellipse, &mut ids),
            line: ShapeTool::builtin(ShapeKind::Line, &mut ids),
            font_size: defaults::FONT_SIZE,
            text: TextTool::builtin(&mut ids),
        }
    }
}

fn subject_for(tool: Tool) -> Subject {
    if tool == Tool::Select {
        Subject::Layers
    } else {
        Subject::Tool(tool)
    }
}

enum GestureKind {
    /// `canvas` is the highlighted canvas, or `None` when the commit makes a
    /// new one for the stroke. `source` is the brush color. `layer` is the
    /// canvas layer that came from it, or `None` when the commit adds that
    /// layer. `size` is the stroke's own size, or `None` to follow the layer.
    Brush {
        canvas: Option<NodeId>,
        source: ColorId,
        layer: Option<ColorId>,
        size: Option<u8>,
        dynamics: Dynamics,
    },
    Erase {
        canvas: NodeId,
        size: u8,
        dynamics: Dynamics,
    },
    Shape {
        tool: Tool,
        origin: Point,
        pointer: Point,
        shift: bool,
    },
    /// A text-tool press that has not been released. `hit` is an existing text object.
    Text {
        origin: Point,
        pointer: Point,
        hit: Option<NodeId>,
    },
}

/// Text being typed. It is not history until [`Session::commit_text`].
#[derive(Clone)]
enum TextDraft {
    New {
        frame: TextBox,
        content: String,
    },
    Existing {
        id: NodeId,
        content: String,
        original: String,
    },
}

struct OpenGesture {
    kind: GestureKind,
    samples: Vec<Sample>,
}

enum PendingProp {
    Color {
        sketch: SketchId,
        node: NodeId,
        color: ColorId,
        id: PropId,
        before: PropValue,
    },
    Nodes {
        sketch: SketchId,
        id: PropId,
        before: Vec<(NodeId, PropValue)>,
    },
}

/// In-app copies of whole objects. Not the operating-system clipboard, and not
/// an undo step. It belongs to the window, so a tab change leaves it.
struct Clipboard {
    nodes: Vec<Node>,
    /// Pastes already made from `nodes`. The next one steps another (16, 16).
    pasted: u32,
}

impl Clipboard {
    fn empty() -> Self {
        Self {
            nodes: Vec::new(),
            pasted: 0,
        }
    }
}

/// Document units a paste moves from the source, then again on each repeat.
const PASTE_STEP: f64 = 16.0;

pub struct Session {
    sketches: Vec<Record>,
    active: usize,
    next_sketch_id: u64,
    ids: Allocator,
    clipboard: Clipboard,
    tool: Tool,
    brush: Dynamics,
    eraser: Dynamics,
    eraser_size: u8,
    rectangle: ShapeTool,
    ellipse: ShapeTool,
    line: ShapeTool,
    text: TextTool,
    /// The brush's colors. A new canvas copies them as its layers.
    brush_palette: Vec<ToolColor>,
    /// The brush color the next stroke is drawn in.
    brush_active: ColorId,
    /// Words typed before Escape or a click elsewhere.
    text_draft: Option<TextDraft>,
    /// Bumps when a new edit starts, so the frame can focus the caret once.
    text_generation: u64,
    open: Option<OpenGesture>,
    panel_open: bool,
    subject: Subject,
    /// The page a color page was opened from, for its back row.
    return_to: Option<Subject>,
    pending: Option<PendingProp>,
    /// Undo or redo interrupted a color slider. Previews from that drag are
    /// ignored until the pointer is released, so release cannot commit over
    /// the history change.
    color_drag_abandoned: bool,
    /// A tool slider moved and has not been released yet.
    tool_pending: bool,
    prefs_dirty: bool,
    /// User-data directory: preferences, recovery, and leftover sketch files.
    /// Tests pass a directory they created. `None` does not touch the
    /// platform user-data directory.
    data_dir: Option<PathBuf>,
    /// Last 10 sketch paths, most recent first.
    recent: Vec<PathBuf>,
    /// The project's folder, when a host runs this inside a project. New
    /// sketches are written there by themselves, and Open Recent and the
    /// file dialogs stay inside it.
    project: Option<PathBuf>,
    /// Whether a recent path held a readable sketch, with the modification
    /// time and size it had when it was read.
    recent_checked: HashMap<PathBuf, ((Option<SystemTime>, u64), bool)>,
    /// Last folder a file dialog in this window returned.
    dialog_folder: Option<PathBuf>,
    /// The last save or open that failed, until the window shows it.
    file_error: Option<FileError>,
    file_intent: Option<FileIntent>,
    /// When each dirty sketch is next due a recovery copy.
    recovery_due: HashMap<SketchId, Instant>,
    /// Recovery files offered at startup and not yet restored or discarded.
    unrestored: HashSet<SketchId>,
}

impl Session {
    pub fn new() -> Self {
        let mut ids = Allocator::new();
        let brush_palette = brush_palette(&mut ids);
        let brush_active = brush_palette[0].id();
        let rectangle = ShapeTool::builtin(ShapeKind::Rectangle, &mut ids);
        let ellipse = ShapeTool::builtin(ShapeKind::Ellipse, &mut ids);
        let line = ShapeTool::builtin(ShapeKind::Line, &mut ids);
        let mut text = TextTool::builtin(&mut ids);
        text.style.size = registry::default_font_size();
        let mut session = Self {
            sketches: Vec::new(),
            active: 0,
            next_sketch_id: 1,
            ids,
            clipboard: Clipboard::empty(),
            tool: Tool::Brush,
            brush: Dynamics::round(),
            eraser: Dynamics::round(),
            eraser_size: registry::default_eraser_size(),
            rectangle,
            ellipse,
            line,
            text,
            brush_palette,
            brush_active,
            text_draft: None,
            text_generation: 0,
            open: None,
            panel_open: false,
            subject: Subject::Tool(Tool::Brush),
            return_to: None,
            pending: None,
            color_drag_abandoned: false,
            tool_pending: false,
            prefs_dirty: false,
            data_dir: None,
            recent: Vec::new(),
            project: None,
            recent_checked: HashMap::new(),
            dialog_folder: None,
            file_error: None,
            file_intent: None,
            recovery_due: HashMap::new(),
            unrestored: HashSet::new(),
        };
        session.push_blank();
        session
    }

    pub(crate) fn active_sketch(&self) -> &Record {
        &self.sketches[self.active]
    }

    fn active_sketch_mut(&mut self) -> &mut Record {
        &mut self.sketches[self.active]
    }

    fn index_of(&self, id: SketchId) -> Option<usize> {
        self.sketches.iter().position(|sketch| sketch.id() == id)
    }

    fn allocate_sketch(&mut self) -> SketchId {
        let id = SketchId::new(self.next_sketch_id);
        self.next_sketch_id += 1;
        id
    }

    fn push_blank(&mut self) -> SketchId {
        let id = self.allocate_sketch();
        let name = untitled_name(&self.sketches);
        let palette = self.brush_palette.clone();
        self.sketches
            .push(Record::untitled(id, name, &mut self.ids, &palette));
        self.active = self.sketches.len() - 1;
        id
    }

    pub fn active_id(&self) -> SketchId {
        self.active_sketch().id()
    }

    pub fn active_name(&self) -> &str {
        self.active_sketch().name()
    }

    pub fn canvas_name(&self) -> Option<&str> {
        self.active_sketch().canvas_name()
    }

    pub fn sketch_count(&self) -> usize {
        self.sketches.len()
    }

    pub fn tabs(&self) -> Vec<SketchTab<'_>> {
        let only = self.sketches.len() == 1;
        self.sketches
            .iter()
            .enumerate()
            .map(|(index, sketch)| SketchTab {
                id: sketch.id(),
                name: sketch.name(),
                active: index == self.active,
                close_waits: only && sketch.is_clean(),
                needs_keep: sketch.needs_keep(),
            })
            .collect()
    }

    pub fn name_of(&self, id: SketchId) -> Option<&str> {
        self.index_of(id).map(|index| self.sketches[index].name())
    }

    /// Appends an empty sketch and shows it. Tool settings stay with the window.
    pub fn new_sketch(&mut self) -> SketchId {
        self.settle();
        self.push_blank()
    }

    pub fn activate(&mut self, id: SketchId) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        if index == self.active {
            return;
        }
        self.settle();
        if let Some(index) = self.index_of(id) {
            self.active = index;
        }
    }

    /// Sets a sketch's name. An empty name leaves the previous one. A completed
    /// rename dirties the sketch and is not an undo step.
    pub fn rename(&mut self, id: SketchId, name: &str) {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return;
        }
        let Some(index) = self.index_of(id) else {
            return;
        };
        if self.sketches[index].name() == trimmed {
            return;
        }
        self.sketches[index].set_name(trimmed.to_string());
    }

    /// Closes a clean sketch. A dirty sketch stays open and needs a confirmation.
    /// The only clean sketch stays open.
    pub fn request_close(&mut self, id: SketchId) -> CloseEffect {
        let Some(index) = self.index_of(id) else {
            return CloseEffect::Unchanged;
        };
        if index == self.active {
            self.settle();
        }
        let Some(index) = self.index_of(id) else {
            return CloseEffect::Unchanged;
        };
        self.close_at(index, false)
    }

    /// Drops the sketch, including unsaved strokes. Deletes that sketch's
    /// recovery file, so the discarded work does not come back. Creates a
    /// blank only when that sketch was the last one.
    pub fn discard(&mut self, id: SketchId) -> CloseEffect {
        self.delete_sketch_recovery(id);
        let Some(index) = self.index_of(id) else {
            return CloseEffect::Unchanged;
        };
        if index == self.active {
            self.open = None;
            self.pending = None;
            if self.tool_pending {
                self.prefs_dirty = true;
                self.tool_pending = false;
            }
        }
        self.close_at(index, true)
    }

    /// Recovery and leftover `sketch-<id>.json` files live in `dir`. Those
    /// leftover files are not opened and not deleted. Their ids, and recovery
    /// file ids, stay reserved before the first sketch exists, so a new blank
    /// does not reuse a recovery file that is still waiting.
    pub(crate) fn set_data_dir(&mut self, dir: impl Into<PathBuf>) {
        let dir = dir.into();
        let replace_blank = self.sketches.len() == 1
            && self.sketches[0].is_clean()
            && self.text_draft.is_none()
            && self.open.is_none();
        if replace_blank {
            self.sketches.clear();
            self.active = 0;
        }
        crate::store::reserve(&dir, &mut self.ids, &mut self.next_sketch_id);
        self.unrestored = crate::store::recovery_offers(&dir)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        self.data_dir = Some(dir);
        if replace_blank {
            self.push_blank();
        }
    }

    pub(crate) fn data_dir(&self) -> Option<&Path> {
        self.data_dir.as_deref()
    }

    pub(crate) fn set_recent(&mut self, recent: Vec<PathBuf>) {
        self.recent = recent.into_iter().take(10).collect();
    }

    /// Recent paths whose files can be read, most recent first. A missing or
    /// unreadable file is left out and does not stop the app. A file is read
    /// again only when its size or modification time changes, so an open menu
    /// does not parse every recent sketch each frame.
    pub(crate) fn recent_menu(&mut self) -> Vec<PathBuf> {
        let mut shown = Vec::new();
        let recent = self.recent.clone();
        for path in &recent {
            if self.project.is_some() && !self.in_project(path) {
                continue;
            }
            let Ok(meta) = std::fs::metadata(path) else {
                continue;
            };
            let stamp = (meta.modified().ok(), meta.len());
            let readable = match self.recent_checked.get(path) {
                Some((checked, readable)) if *checked == stamp => *readable,
                _ => {
                    let readable = crate::store::readable_smx(path);
                    self.recent_checked.insert(path.clone(), (stamp, readable));
                    readable
                }
            };
            if readable {
                shown.push(path.clone());
            }
        }
        shown
    }

    /// Empties the recent-file list and stores the empty list.
    fn clear_recent(&mut self) {
        self.recent.clear();
        self.recent_checked.clear();
        if let Some(dir) = self.data_dir.clone() {
            let _ = crate::prefs::save_recent(&dir, &self.recent);
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn file_intent(&self) -> Option<FileIntent> {
        self.file_intent.clone()
    }

    pub(crate) fn take_file_intent(&mut self) -> Option<FileIntent> {
        self.file_intent.take()
    }

    pub(crate) fn file_path(&self, id: SketchId) -> Option<PathBuf> {
        self.index_of(id)
            .and_then(|index| self.sketches[index].path().map(Path::to_path_buf))
    }

    /// The folder Save As and Save a Copy start in: the project's folder in
    /// a project, otherwise the last folder chosen in this window, otherwise
    /// the home directory.
    pub(crate) fn dialog_directory(&self) -> Option<PathBuf> {
        self.project
            .clone()
            .or_else(|| self.dialog_folder.clone())
            .or_else(crate::prefs::home_dir)
    }

    /// Works inside this project's folder from now on.
    pub(crate) fn set_project(&mut self, folder: &Path) {
        let folder = crate::store::normalize(folder);
        self.project = Some(folder.canonicalize().unwrap_or(folder));
    }

    pub(crate) fn project(&self) -> Option<&Path> {
        self.project.as_deref()
    }

    /// Whether `path` is inside the project's folder. Always true outside a
    /// project.
    pub(crate) fn in_project(&self, path: &Path) -> bool {
        let Some(project) = &self.project else {
            return true;
        };
        let path = crate::store::normalize(path);
        let path = path.canonicalize().unwrap_or(path);
        path.starts_with(project)
    }

    /// Sketches with work not yet in their file, in tab order. A sketch
    /// whose words are still being typed waits, unless `drafts` is set.
    pub(crate) fn unwritten(&self, drafts: bool) -> Vec<SketchId> {
        self.sketches
            .iter()
            .enumerate()
            .filter(|(index, sketch)| {
                let typing = *index == self.active && self.text_draft.is_some();
                if typing && !drafts {
                    return false;
                }
                sketch.needs_keep() || (typing && self.draft_is_unsaved())
            })
            .map(|(_, sketch)| sketch.id())
            .collect()
    }

    /// Writes the sketch into the project's folder: to its own file, or, for
    /// a sketch never written, to a new file named after it that replaces
    /// nothing, such as `Untitled 1.smx`, then `Untitled 2.smx`. Returns the
    /// file, or none for an untouched sketch that has nothing to write.
    pub(crate) fn write_into_project(&mut self, id: SketchId) -> Result<Option<PathBuf>, String> {
        let Some(folder) = self.project.clone() else {
            return Err("No project folder.".to_string());
        };
        let Some(index) = self.index_of(id) else {
            return Ok(None);
        };
        let typing = index == self.active && self.draft_is_unsaved();
        if !self.sketches[index].needs_keep() && !typing {
            return Ok(self.file_path(id));
        }
        if let Some(path) = self.file_path(id) {
            return match self.save_path(id, path.clone(), false, true) {
                SaveResult::Wrote => Ok(Some(path)),
                _ => Err(self.write_error()),
            };
        }
        let name = self.name_of(id).unwrap_or("Untitled").trim_end_matches(".smx").to_string();
        for candidate in free_names(&name) {
            let path = folder.join(format!("{candidate}.smx"));
            if path.exists() {
                continue;
            }
            match self.save_path(id, path.clone(), false, false) {
                SaveResult::Wrote => return Ok(Some(path)),
                SaveResult::NeedsReplace(_) => continue,
                SaveResult::Failed => return Err(self.write_error()),
            }
        }
        Err("No free file name.".to_string())
    }

    fn write_error(&mut self) -> String {
        self.file_error
            .as_ref()
            .map(|error| error.reason.clone())
            .unwrap_or_else(|| "The sketch could not be written.".to_string())
    }

    pub(crate) fn note_dialog_folder(&mut self, path: &Path) {
        let path = crate::store::normalize(path);
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            self.dialog_folder = Some(parent.to_path_buf());
        }
    }

    pub(crate) fn suggested_save_name(&self, id: SketchId) -> String {
        let name = self.name_of(id).unwrap_or("Untitled");
        if name.to_ascii_lowercase().ends_with(".smx") {
            name.to_string()
        } else {
            format!("{name}.smx")
        }
    }

    pub(crate) fn recovery_offers(&self) -> Vec<RecoveryOffer> {
        let Some(dir) = &self.data_dir else {
            return Vec::new();
        };
        crate::store::recovery_offers(dir)
            .into_iter()
            .map(|(id, name)| RecoveryOffer { id, name })
            .collect()
    }

    /// Writes one recovery copy for each dirty sketch, as the 10-minute
    /// interval elapsing would. A test calls this instead of waiting.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn recovery_tick(&mut self) {
        let now = Instant::now();
        self.sync_recovery_arms(now);
        let ids: Vec<SketchId> = self.recovery_due.keys().copied().collect();
        for id in ids {
            if let Some(id) = self.write_recovery_file(id) {
                self.recovery_due.insert(id, now + RECOVERY_INTERVAL);
            }
        }
    }

    /// Writes a recovery copy for each dirty sketch whose interval is up.
    /// A failed write leaves that sketch due, so the next poll tries again.
    pub(crate) fn poll_recovery(&mut self, now: Instant) {
        self.sync_recovery_arms(now);
        let due: Vec<SketchId> = self
            .recovery_due
            .iter()
            .filter(|(_, due)| **due <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            if let Some(id) = self.write_recovery_file(id) {
                self.recovery_due.insert(id, now + RECOVERY_INTERVAL);
            }
        }
    }

    /// The panic hook writes these bytes. Publishing does not touch the disk.
    pub(crate) fn publish_emergency(&mut self) {
        let Some(dir) = self.data_dir.clone() else {
            crate::store::publish_emergency(Vec::new());
            return;
        };
        let ids: Vec<SketchId> = self.sketches.iter().map(|sketch| sketch.id()).collect();
        for id in ids {
            if self.needs_recovery_id(id) {
                let _ = self.retarget_recovery_id(id);
            }
        }
        let mut files = Vec::new();
        let ids: Vec<SketchId> = self.sketches.iter().map(|sketch| sketch.id()).collect();
        for id in ids {
            if !self.needs_recovery_id(id) {
                continue;
            }
            if let Some(bytes) = self.bytes_including_draft(id) {
                files.push((crate::store::recovery_path(&dir, id), bytes));
            }
        }
        crate::store::publish_emergency(files);
    }

    /// Writes the panic hook's recovery copies without panicking the process.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn write_emergency_recovery(&mut self) {
        self.commit_text();
        self.publish_emergency();
        crate::store::flush_emergency();
        let ids: Vec<SketchId> = self
            .sketches
            .iter()
            .filter(|sketch| sketch.is_dirty())
            .map(|sketch| sketch.id())
            .collect();
        for id in ids {
            if let Some(index) = self.index_of(id) {
                self.sketches[index].set_recovery_file(id);
            }
        }
    }

    /// Opens `path` as the only sketch when the window is still the blank it
    /// started with. An unreadable file leaves that blank and returns false.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn open_initial(&mut self, path: &Path) -> bool {
        let path = crate::store::normalize(path);
        let Some(sketch) = self.read_or_report(&path) else {
            return false;
        };
        if self.sketches.len() == 1 && self.sketches[0].is_clean() {
            self.sketches.clear();
            self.active = 0;
        }
        self.adopt(sketch, Some(path), None);
        true
    }

    /// Focuses the tab when that path is already open. Otherwise adds a tab
    /// with no selection and no highlighted canvas. An unreadable file leaves
    /// the session as it was.
    pub(crate) fn open_path(&mut self, path: &Path) -> bool {
        let path = crate::store::normalize(path);
        if let Some(id) = self.find_open_path(&path) {
            self.activate(id);
            self.remember_path(&path);
            return true;
        }
        let Some(sketch) = self.read_or_report(&path) else {
            return false;
        };
        self.adopt(sketch, Some(path), None);
        true
    }

    /// Reads a sketch file. A failure is kept for the window to show.
    fn read_or_report(&mut self, path: &Path) -> Option<Record> {
        match crate::store::read_smx(path) {
            Ok(sketch) => Some(sketch),
            Err(reason) => {
                self.file_error = Some(FileError {
                    saving: false,
                    file: crate::store::file_title(path),
                    reason,
                });
                None
            }
        }
    }

    /// The last save or open that failed, once.
    pub(crate) fn take_file_error(&mut self) -> Option<FileError> {
        self.file_error.take()
    }

    /// Opens one recovery copy, still unsaved, and does not write its `.smx`.
    /// A path that is already open is replaced by the recovery, still one tab.
    pub(crate) fn restore_recovery(&mut self, file_id: SketchId) -> bool {
        let Some(dir) = self.data_dir.clone() else {
            return false;
        };
        let Some(loaded) = crate::store::read_recovery(&dir, file_id) else {
            return false;
        };
        self.unrestored.remove(&file_id);
        let path = loaded.path.map(|path| crate::store::normalize(&path));
        if let Some(path) = path.clone()
            && let Some(open_id) = self.find_open_path(&path)
        {
            self.replace_sketch(open_id, loaded.sketch, Some(path), Some(file_id));
            return true;
        }
        self.adopt(loaded.sketch, path, Some(file_id));
        true
    }

    /// Deletes that recovery file. Does not open it.
    pub(crate) fn discard_recovery_file(&mut self, file_id: SketchId) {
        self.unrestored.remove(&file_id);
        if let Some(dir) = &self.data_dir {
            crate::store::delete_recovery(dir, file_id);
        }
    }

    /// Commits a text draft on the active sketch, then writes `path`. The
    /// dirty flag clears only after a Save or Save As. Save a Copy leaves the
    /// sketch's file and dirty flag as they were. A failed write leaves the
    /// previous file and the dirty flag.
    pub(crate) fn save_path(
        &mut self,
        id: SketchId,
        path: impl Into<PathBuf>,
        copy: bool,
        replace: bool,
    ) -> SaveResult {
        let Some(index) = self.index_of(id) else {
            return SaveResult::Failed;
        };
        let path = crate::store::ensure_smx(crate::store::normalize(&path.into()));
        self.note_dialog_folder(&path);
        let own = self.sketches[index]
            .path()
            .is_some_and(|current| crate::store::same_file(current, &path));
        if path.is_file() && !own && !replace {
            return SaveResult::NeedsReplace(path);
        }
        if index == self.active {
            self.commit_text();
        }
        let Some(index) = self.index_of(id) else {
            return SaveResult::Failed;
        };
        let title = crate::store::file_title(&path);
        if let Err(reason) = crate::store::write_smx(&path, &self.sketches[index], &title) {
            self.file_error = Some(FileError {
                saving: true,
                file: title,
                reason,
            });
            return SaveResult::Failed;
        }
        self.remember_path(&path);
        if copy {
            return SaveResult::Wrote;
        }
        self.sketches[index].set_name(title);
        self.sketches[index].set_path(Some(path));
        self.sketches[index].mark_saved();
        self.delete_sketch_recovery(id);
        self.file_intent = None;
        SaveResult::Wrote
    }

    fn adopt(&mut self, mut sketch: Record, path: Option<PathBuf>, recovery: Option<SketchId>) {
        sketch.reserve_ids(&mut self.ids);
        self.next_sketch_id = self.next_sketch_id.max(sketch.id().get() + 1);
        let held = self.held_ids();
        sketch.separate_ids(&held, &mut self.ids, &mut self.next_sketch_id);
        sketch.reserve_ids(&mut self.ids);
        self.next_sketch_id = self.next_sketch_id.max(sketch.id().get() + 1);
        let title = path
            .as_deref()
            .map(crate::store::file_title)
            .unwrap_or_else(|| sketch.name().to_string());
        if let Some(file_id) = recovery {
            sketch.mark_restored(path.clone(), title, file_id);
        } else if let Some(path) = path.clone() {
            sketch.mark_opened(path, title);
        }
        self.settle();
        self.sketches.push(sketch);
        self.active = self.sketches.len() - 1;
        if recovery.is_none()
            && let Some(path) = path
        {
            self.remember_path(&path);
        }
    }

    fn held_ids(&self) -> draw::document::HeldIds {
        let mut held = draw::document::HeldIds::default();
        for sketch in &self.sketches {
            sketch.collect_held(&mut held);
        }
        for color in &self.brush_palette {
            held.colors.insert(color.id().get());
        }
        for tool in [&self.rectangle, &self.ellipse, &self.line] {
            for color in &tool.palette {
                held.colors.insert(color.id().get());
            }
        }
        for color in &self.text.palette {
            held.colors.insert(color.id().get());
        }
        held
    }

    fn find_open_path(&self, path: &Path) -> Option<SketchId> {
        self.sketches.iter().find_map(|sketch| {
            sketch
                .path()
                .filter(|open| crate::store::same_file(open, path))
                .map(|_| sketch.id())
        })
    }

    fn remember_path(&mut self, path: &Path) {
        let path = crate::store::normalize(path);
        self.recent
            .retain(|item| !crate::store::same_file(item, &path));
        self.recent.insert(0, path);
        self.recent.truncate(10);
        if let Some(dir) = self.data_dir.clone() {
            let _ = crate::prefs::save_recent(&dir, &self.recent);
        }
    }

    fn sync_recovery_arms(&mut self, now: Instant) {
        let mut dirty = Vec::new();
        for index in 0..self.sketches.len() {
            if self.needs_recovery(index) {
                dirty.push(self.sketches[index].id());
            }
        }
        self.recovery_due.retain(|id, _| dirty.contains(id));
        for id in dirty {
            self.recovery_due
                .entry(id)
                .or_insert(now + RECOVERY_INTERVAL);
        }
    }

    /// `Some` is the sketch id the file was named with. `None` means the copy
    /// was not written; the previous deadline stays where it is.
    fn write_recovery_file(&mut self, id: SketchId) -> Option<SketchId> {
        if self.index_of(id) == Some(self.active) {
            self.commit_text();
        }
        let id = self.retarget_recovery_id(id);
        let Some(dir) = self.data_dir.clone() else {
            return None;
        };
        let Some(index) = self.index_of(id) else {
            return None;
        };
        if !self.sketches[index].is_dirty() {
            return None;
        }
        let previous = self.sketches[index].recovery_file();
        if !crate::store::write_recovery(&dir, &self.sketches[index]) {
            return None;
        }
        let live = self.sketches[index].id();
        if let Some(previous) = previous
            && previous != live
            && !self.recovery_named_by_other(index, previous)
        {
            crate::store::delete_recovery(&dir, previous);
            self.unrestored.remove(&previous);
        }
        self.sketches[index].set_recovery_file(live);
        Some(live)
    }

    fn delete_sketch_recovery(&mut self, id: SketchId) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let owned = self.sketches[index].recovery_file();
        let live = self.sketches[index].id();
        let mut files = Vec::new();
        match owned {
            Some(file) => files.push(file),
            None => files.push(live),
        }
        if owned.is_some() && live != files[0] {
            files.push(live);
        }
        if let Some(dir) = self.data_dir.clone() {
            for file in files {
                if self.recovery_named_by_other(index, file) {
                    continue;
                }
                if owned != Some(file) && self.unrestored.contains(&file) {
                    continue;
                }
                crate::store::delete_recovery(&dir, file);
                self.unrestored.remove(&file);
            }
        }
        self.recovery_due.remove(&id);
    }

    fn needs_recovery(&self, index: usize) -> bool {
        self.sketches[index].is_dirty() || (index == self.active && self.draft_is_unsaved())
    }

    fn needs_recovery_id(&self, id: SketchId) -> bool {
        self.index_of(id)
            .is_some_and(|index| self.needs_recovery(index))
    }

    /// Words still being typed count as unsaved. An empty new box does not.
    fn draft_is_unsaved(&self) -> bool {
        match &self.text_draft {
            Some(TextDraft::New { content, .. }) => !content.is_empty(),
            Some(TextDraft::Existing {
                content, original, ..
            }) => content != original,
            None => false,
        }
    }

    /// Sketches the window should ask about before quitting, in tab order.
    /// An open draft counts even when the sketch is otherwise clean.
    pub(crate) fn sketches_to_keep(&self) -> Vec<SketchId> {
        self.sketches
            .iter()
            .enumerate()
            .filter(|(index, sketch)| {
                sketch.needs_keep() || (*index == self.active && self.draft_is_unsaved())
            })
            .map(|(_, sketch)| sketch.id())
            .collect()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn quit_needs_keep(&self) -> bool {
        !self.sketches_to_keep().is_empty()
    }

    /// The first real viewport, and a new blank. Centers a blank without
    /// marking it dirty. An opened or restored sketch keeps its camera.
    pub(crate) fn present_view(&mut self, viewport: Point) {
        if self.active_sketch().keeps_stored_view() {
            return;
        }
        self.active_sketch_mut().center_without_dirty(viewport);
    }

    /// Puts the recovery in the tab that already has `path`, and does not
    /// open a second tab for that file.
    fn replace_sketch(
        &mut self,
        open_id: SketchId,
        sketch: Record,
        path: Option<PathBuf>,
        recovery: Option<SketchId>,
    ) {
        let Some(index) = self.index_of(open_id) else {
            self.adopt(sketch, path, recovery);
            return;
        };
        self.recovery_due.remove(&open_id);
        self.sketches.remove(index);
        if self.sketches.is_empty() {
            self.active = 0;
        } else if self.active > index {
            self.active -= 1;
        } else if self.active == index {
            self.active = index.min(self.sketches.len() - 1);
        }
        self.adopt(sketch, path, recovery);
        let last = self.sketches.len() - 1;
        if last != index {
            let restored = self.sketches.remove(last);
            self.sketches.insert(index, restored);
            self.active = index;
        }
    }

    /// A recovery file this sketch does not own, and that another offer or
    /// open sketch still uses, is not a path this sketch may write.
    fn retarget_recovery_id(&mut self, id: SketchId) -> SketchId {
        let Some(index) = self.index_of(id) else {
            return id;
        };
        let current = self.sketches[index].id();
        if !self.recovery_path_blocked(index, current) {
            return current;
        }
        let fresh = self.allocate_sketch();
        self.sketches[index].set_id(fresh);
        if let Some(due) = self.recovery_due.remove(&current) {
            self.recovery_due.insert(fresh, due);
        }
        fresh
    }

    fn recovery_path_blocked(&self, index: usize, file_id: SketchId) -> bool {
        if self.sketches[index].recovery_file() == Some(file_id) {
            return false;
        }
        self.unrestored.contains(&file_id) || self.recovery_named_by_other(index, file_id)
    }

    fn recovery_named_by_other(&self, index: usize, file_id: SketchId) -> bool {
        self.sketches
            .iter()
            .enumerate()
            .any(|(other, sketch)| other != index && sketch.recovery_file() == Some(file_id))
    }

    /// Recovery bytes for `id`. An open draft is in the bytes and is put back,
    /// so publishing every frame does not end the edit.
    fn bytes_including_draft(&mut self, id: SketchId) -> Option<Vec<u8>> {
        let include = self.index_of(id) == Some(self.active) && self.draft_is_unsaved();
        if !include {
            let index = self.index_of(id)?;
            return crate::store::recovery_bytes(&self.sketches[index]);
        }
        let draft = self.text_draft.clone();
        let selection = self.selection();
        let dirty = self.active_sketch().is_dirty();
        let undo_before = self.undo_len();
        let ids = self.ids.clone();
        self.commit_text();
        let bytes = self
            .index_of(id)
            .and_then(|index| crate::store::recovery_bytes(&self.sketches[index]));
        while self.undo_len() > undo_before {
            let _ = self.active_sketch_mut().undo();
        }
        self.ids = ids;
        let index = self.active;
        self.sketches[index].clear_redo();
        self.sketches[index].set_dirty(dirty);
        self.sketches[index].set_selection(selection);
        self.text_draft = draft;
        bytes
    }

    fn save_active(&mut self) {
        let id = self.active_id();
        if let Some(path) = self.file_path(id) {
            self.file_intent = None;
            let _ = self.save_path(id, path, false, true);
            return;
        }
        self.ask_save(id, false, false);
    }

    pub(crate) fn request_save_dialog(&mut self, id: SketchId, close_after: bool) {
        self.ask_save(id, false, close_after);
    }

    fn ask_save(&mut self, id: SketchId, copy: bool, close_after: bool) {
        self.file_intent = Some(FileIntent::Save {
            id,
            copy,
            close_after,
        });
    }

    fn close_at(&mut self, index: usize, force: bool) -> CloseEffect {
        let pristine = self.sketches[index].is_clean();
        let only = self.sketches.len() == 1;
        if only && pristine {
            return CloseEffect::Unchanged;
        }
        if !force && self.sketches[index].needs_keep() {
            return CloseEffect::NeedsConfirm;
        }
        self.remove_at(index);
        let replacement = self.sketches.is_empty();
        if replacement {
            self.push_blank();
        }
        CloseEffect::Closed { replacement }
    }

    fn remove_at(&mut self, index: usize) {
        self.sketches.remove(index);
        if self.sketches.is_empty() {
            self.active = 0;
            return;
        }
        if self.active > index {
            self.active -= 1;
        } else if self.active == index && self.active >= self.sketches.len() {
            self.active = self.sketches.len() - 1;
        }
    }

    fn settle(&mut self) {
        self.finish_size_edit();
        self.commit_open_gesture();
    }

    fn commit_open_gesture(&mut self) {
        let placing_text = matches!(
            self.open.as_ref().map(|open| &open.kind),
            Some(GestureKind::Text { .. })
        );
        if placing_text {
            self.open = None;
        } else if self.open.is_some() {
            let _ = self.commit();
        }
        self.commit_text();
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    pub fn set_tool(&mut self, tool: Tool) {
        if self.tool == tool {
            return;
        }
        self.settle();
        self.tool = tool;
        if matches!(tool, Tool::Brush | Tool::Eraser) {
            self.active_sketch_mut().select_active_canvas();
        }
        self.subject = subject_for(self.tool);
    }

    pub(crate) fn panel_open(&self) -> bool {
        self.panel_open
    }

    pub(crate) fn show_panel(&mut self) {
        self.panel_open = true;
    }

    pub(crate) fn toggle_panel(&mut self) {
        self.finish_size_edit();
        self.panel_open = !self.panel_open;
    }

    /// Opens that tool's page and leaves the current tool as it is.
    pub(crate) fn open_tool_page(&mut self, tool: Tool) {
        self.finish_size_edit();
        self.subject = subject_for(tool);
        self.panel_open = true;
    }

    /// Opens a color of the current tool's palette, as a right-click on its
    /// swatch does. The brush's active color stays as it is.
    pub(crate) fn open_color_page(&mut self, color: ColorId) {
        if self.tool_color(self.tool, color).is_none() {
            return;
        }
        self.finish_size_edit();
        self.subject = Subject::Color {
            owner: ColorOwner::Tool(self.tool),
            color,
        };
        self.return_to = Some(Subject::Tool(self.tool));
        self.panel_open = true;
    }

    /// A click on a swatch: a brush color becomes active; a shape's or text's
    /// color opens its page, since all of those colors are used at once.
    pub fn swatch_click(&mut self, color: ColorId) {
        if self.tool == Tool::Brush {
            self.set_active_color(color);
        } else {
            self.open_color_page(color);
        }
    }

    fn resolved_subject(&self) -> Subject {
        match self.subject {
            Subject::Color { owner, color } if self.owns_color(owner, color) => self.subject,
            Subject::Color { .. } => subject_for(self.tool),
            // The text the text tool is working on is an object, so the
            // panel shows it, as the toolbar does.
            Subject::Tool(Tool::Text) if self.active_text().is_some() => Subject::Objects,
            Subject::Tool(tool) => Subject::Tool(tool),
            Subject::Layers => Subject::Layers,
            Subject::Objects if !self.selection_sections().is_empty() => Subject::Objects,
            Subject::Objects => subject_for(self.tool),
        }
    }

    /// The object page has something to show for `node`: a shape, text, or a
    /// canvas's layers. A group has no settings of its own.
    pub(crate) fn object_page_available(&self, node: NodeId) -> bool {
        object_sections(self.active_sketch(), node).is_some()
    }

    /// Opens the panel on an object's own settings, as Properties in its
    /// right-click menu does. An object outside the selection becomes the
    /// selection; one inside it keeps the selection as it is.
    pub(crate) fn open_object_page(&mut self, node: NodeId) {
        if !self.object_page_available(node) {
            return;
        }
        self.finish_size_edit();
        if !self.active_sketch().selection().contains(&node) {
            self.active_sketch_mut().set_selection([node]);
        }
        self.subject = Subject::Objects;
        self.panel_open = true;
    }

    /// A right-click on the canvas with the select tool. Anywhere inside the
    /// selection's box keeps the selection; otherwise the object under the
    /// pointer becomes the selection.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn pick_for_menu(&mut self, point: Point) -> Option<NodeId> {
        if let Some((min, max)) = self.active_sketch().selection_bounds()
            && (min[0]..=max[0]).contains(&point[0])
            && (min[1]..=max[1]).contains(&point[1])
        {
            return self.active_sketch().selection().first().copied();
        }
        let hit = self.active_sketch().hit_test(point)?;
        if !self.active_sketch().selection().contains(&hit) {
            self.active_sketch_mut().set_selection([hit]);
        }
        Some(hit)
    }

    /// Starts editing a text object in place, as a double-click on it with
    /// the select tool, or a click on it with the text tool, does.
    pub fn edit_text(&mut self, id: NodeId) -> bool {
        if !self.active_sketch().is_text(id) || !self.active_sketch().can_edit(id, PropId::Name) {
            return false;
        }
        self.commit_text();
        let content = self.active_sketch().text_content(id).unwrap_or_default();
        self.text_draft = Some(TextDraft::Existing {
            id,
            original: content.clone(),
            content,
        });
        self.active_sketch_mut().set_selection([id]);
        self.text_generation += 1;
        true
    }

    fn owns_color(&self, owner: ColorOwner, color: ColorId) -> bool {
        match owner {
            ColorOwner::Tool(tool) => self.tool_color(tool, color).is_some(),
            ColorOwner::Node(node) => self.active_sketch().has_color(node, color),
        }
    }

    pub(crate) fn shows_layers(&self) -> bool {
        matches!(self.resolved_subject(), Subject::Layers)
    }

    /// Section titles and their properties. A tool's preset section is titled
    /// with the tool's name, as in "Brush Preset". Rows the subject does not
    /// have are left out, and a section left with no rows is dropped.
    pub(crate) fn panel_sections(&self) -> Vec<(String, Vec<PropId>)> {
        let subject = self.resolved_subject();
        let mut rows = match subject {
            Subject::Tool(tool) => section_rows(registry::tool(tool).sections, Some(tool)),
            Subject::Color { .. } => section_rows(schema::COLOR_SECTIONS, None),
            // The Layers page is the outliner alone.
            Subject::Layers => Vec::new(),
            // Constrain and snap stay on the tool.
            Subject::Objects => self.selection_sections(),
        };
        rows.retain_mut(|(_, props)| {
            props.retain(|prop| self.panel_property(*prop).is_some());
            !props.is_empty()
        });
        rows
    }

    /// The dynamics the panel's tool subject draws with, for the preset preview.
    pub(crate) fn panel_dynamics(&self) -> Option<(Tool, &Dynamics)> {
        match self.resolved_subject() {
            Subject::Tool(Tool::Brush) => Some((Tool::Brush, &self.brush)),
            Subject::Tool(Tool::Eraser) => Some((Tool::Eraser, &self.eraser)),
            _ => None,
        }
    }

    pub(crate) fn panel_property(&self, id: PropId) -> Option<PropValue> {
        if id == PropId::Palette {
            return self.palette_owner().map(|_| PropValue::Idle);
        }
        match self.resolved_subject() {
            Subject::Tool(tool) => self.tool_property(tool, id),
            Subject::Color { owner, color } => match (id, owner) {
                (_, ColorOwner::Tool(tool)) => self.tool_color(tool, color)?.read(id),
                (PropId::LockSize, ColorOwner::Node(_)) => None,
                (_, ColorOwner::Node(node)) => self.active_sketch().color_prop(node, color, id),
            },
            Subject::Layers => None,
            Subject::Objects => self.shared_property(id),
        }
    }

    /// Whose palette the panel's Palette section lists: the tool on its page,
    /// or the one selected shape or text object under Layers.
    fn palette_owner(&self) -> Option<ColorOwner> {
        match self.resolved_subject() {
            Subject::Tool(tool) => self.tool_palette(tool).map(|_| ColorOwner::Tool(tool)),
            Subject::Layers => None,
            Subject::Objects => {
                let selection = self.active_sketch().selection();
                let editing = match &self.text_draft {
                    Some(TextDraft::Existing { id, .. }) => Some(*id),
                    _ => None,
                };
                let node = match (selection, editing) {
                    (_, Some(id)) => id,
                    ([only], None) => *only,
                    _ => return None,
                };
                let listed = self
                    .active_sketch()
                    .node_prop(node, PropId::Palette)
                    .is_some();
                listed.then_some(ColorOwner::Node(node))
            }
            Subject::Color { .. } => None,
        }
    }

    /// Opens a color from the panel's Palette section.
    pub(crate) fn open_palette_color(&mut self, color: ColorId) {
        let Some(owner) = self.palette_owner() else {
            return;
        };
        if !self.owns_color(owner, color) {
            return;
        }
        self.finish_size_edit();
        self.return_to = Some(self.resolved_subject());
        self.subject = Subject::Color { owner, color };
    }

    /// The page a color page goes back to, named for its back row.
    pub(crate) fn back_label(&self) -> Option<String> {
        if !matches!(self.resolved_subject(), Subject::Color { .. }) {
            return None;
        }
        Some(match self.return_to? {
            Subject::Tool(tool) => registry::tool(tool).label.to_string(),
            Subject::Layers => "Layers".to_string(),
            Subject::Objects => self
                .edit_targets()
                .first()
                .and_then(|id| self.active_sketch().node_prop(*id, PropId::Name))
                .and_then(|name| match name {
                    PropValue::Text(name) => Some(name),
                    _ => None,
                })
                .unwrap_or_else(|| "Object".to_string()),
            Subject::Color { .. } => return None,
        })
    }

    /// Leaves a color page for the page it was opened from.
    pub(crate) fn go_back(&mut self) {
        if let Some(subject) = self.return_to.take() {
            self.finish_size_edit();
            self.subject = subject;
        }
    }

    /// The colors of `owner` as rows for the shared palette list: a canvas's
    /// layers, or a shape's or text's fixed roles.
    pub(crate) fn palette_entries(&self, owner: NodeId) -> Vec<LayerNodeColor> {
        if let Some(node) = self
            .active_sketch()
            .layers()
            .into_iter()
            .find(|node| node.id == owner && !node.colors.is_empty())
        {
            return node.colors;
        }
        self.active_sketch()
            .node_palette(owner)
            .unwrap_or_default()
            .into_iter()
            .map(|color| LayerNodeColor {
                id: color.id,
                name: color.name,
                rgb: color.rgb,
                visible: color.visible,
                locked: false,
                fixed: color.role.fixed(),
            })
            .collect()
    }

    /// The object whose colors the panel's Palette section lists.
    pub(crate) fn palette_node(&self) -> Option<NodeId> {
        match self.palette_owner()? {
            ColorOwner::Node(node) => Some(node),
            ColorOwner::Tool(_) => None,
        }
    }

    /// Colors offered as quick choices in a color well: the brush's palette.
    pub(crate) fn quick_colors(&self) -> Vec<[u8; 3]> {
        self.brush_palette
            .iter()
            .map(|item| item.color.rgb)
            .collect()
    }

    fn selection_sections(&self) -> Vec<(String, Vec<PropId>)> {
        let editing = match &self.text_draft {
            Some(TextDraft::Existing { id, .. }) => Some(vec![*id]),
            _ => None,
        };
        let selected = editing.unwrap_or_else(|| self.active_sketch().selection().to_vec());
        let mut pages = Vec::new();
        for id in &selected {
            let Some(sections) = object_sections(self.active_sketch(), *id) else {
                return Vec::new();
            };
            pages.push(sections);
        }
        let Some(first) = pages.first() else {
            return Vec::new();
        };
        let mut rows = section_rows(first, None);
        for page in pages.iter().skip(1) {
            let next = section_rows(page, None);
            rows.retain_mut(|(title, props)| {
                let Some((_, shared)) = next.iter().find(|(other, _)| other == title) else {
                    return false;
                };
                props.retain(|prop| shared.contains(prop));
                !props.is_empty()
            });
        }
        rows
    }

    /// The objects a Layers edit applies to: the text being edited in place,
    /// or the selection.
    fn edit_targets(&self) -> Vec<NodeId> {
        match &self.text_draft {
            Some(TextDraft::Existing { id, .. }) => vec![*id],
            _ => self.active_sketch().selection().to_vec(),
        }
    }

    fn nodes_with_prop(&self, id: PropId) -> Vec<NodeId> {
        self.edit_targets()
            .into_iter()
            .filter(|node| self.active_sketch().node_prop(*node, id).is_some())
            .collect()
    }

    fn shared_property(&self, id: PropId) -> Option<PropValue> {
        let nodes = self.nodes_with_prop(id);
        if nodes.is_empty() {
            return None;
        }
        let mut values = nodes
            .iter()
            .map(|node| self.active_sketch().node_prop(*node, id));
        let first = values.next().flatten()?;
        if values.all(|value| value.as_ref() == Some(&first)) {
            Some(first)
        } else {
            Some(PropValue::Mixed)
        }
    }

    pub(crate) fn preset_labels(&self) -> &'static [&'static str] {
        match self.resolved_subject() {
            Subject::Tool(Tool::Eraser) => defaults::ERASER_PRESETS,
            _ => defaults::BRUSH_PRESETS,
        }
    }

    /// Applies a value while a slider is still down. History waits for [`finish_size_edit`].
    pub(crate) fn preview_panel_property(&mut self, id: PropId, value: PropValue) -> bool {
        match self.resolved_subject() {
            Subject::Tool(tool) => self.preview_tool(tool, id, value),
            Subject::Color {
                owner: ColorOwner::Tool(tool),
                color,
            } => self.preview_tool_color(tool, color, id, value),
            Subject::Color {
                owner: ColorOwner::Node(node),
                color,
            } => self.edit_color(node, color, id, value, false),
            Subject::Layers => false,
            Subject::Objects => self.edit_nodes(id, value, false),
        }
    }

    /// Applies a value and finishes it: one undo step for an object's color,
    /// or a preferences save for a tool's setting or color.
    pub(crate) fn set_panel_property(&mut self, id: PropId, value: PropValue) -> bool {
        let subject = self.resolved_subject();
        let ok = match subject {
            Subject::Tool(tool) => self.preview_tool(tool, id, value),
            Subject::Color {
                owner: ColorOwner::Tool(tool),
                color,
            } => self.preview_tool_color(tool, color, id, value),
            Subject::Color {
                owner: ColorOwner::Node(node),
                color,
            } => self.edit_color(node, color, id, value, true),
            Subject::Layers => false,
            Subject::Objects => self.edit_nodes(id, value, true),
        };
        if matches!(
            subject,
            Subject::Tool(_)
                | Subject::Color {
                    owner: ColorOwner::Tool(_),
                    ..
                }
        ) {
            self.finish_size_edit();
        }
        ok
    }

    /// A swatch's right-click menu: a brush color can be duplicated and
    /// deleted; a shape's or text's color is a fixed role.
    pub(crate) fn swatch_menu(&self, color: ColorId) -> &'static [MenuActionDef] {
        match self.tool_color(self.tool, color) {
            Some(item) if item.color.role.fixed() => registry::role_menu(),
            _ => registry::color_menu(),
        }
    }

    /// The brush keeps at least one color.
    pub(crate) fn swatch_action_enabled(&self, action: MenuAction) -> bool {
        action != MenuAction::Delete || self.brush_palette.len() > 1
    }

    /// Runs a swatch menu's action on a color of the current tool.
    pub(crate) fn swatch_action(&mut self, color: ColorId, action: MenuAction) -> bool {
        if !self.swatch_action_enabled(action) {
            return false;
        }
        match (action, self.tool) {
            (MenuAction::Properties, _) => {
                self.open_color_page(color);
                true
            }
            (MenuAction::Duplicate, Tool::Brush) => self.duplicate_brush_color(color),
            (MenuAction::Delete, Tool::Brush) => self.delete_brush_color(color),
            _ => false,
        }
    }

    /// Runs an object's menu action, from the canvas or an outliner row.
    pub(crate) fn object_action(&mut self, node: NodeId, action: MenuAction) {
        if action == MenuAction::Properties {
            self.open_object_page(node);
        }
    }

    /// Runs a tool button's menu action.
    pub(crate) fn tool_action(&mut self, tool: Tool, action: MenuAction) {
        if action == MenuAction::Properties {
            self.open_tool_page(tool);
        }
    }

    pub(crate) fn layer_count(&self, canvas: NodeId) -> usize {
        self.active_sketch().palette_len(canvas).unwrap_or(0)
    }

    /// Runs a layer row's menu action. Rename is the outliner's own.
    pub(crate) fn layer_action(
        &mut self,
        canvas: NodeId,
        color: ColorId,
        action: MenuAction,
    ) -> bool {
        match action {
            MenuAction::Duplicate => {
                self.finish_size_edit();
                let index = self.active;
                let step = self.sketches[index].duplicate_color(canvas, color, &mut self.ids);
                self.sketches[index].commit(step)
            }
            MenuAction::Delete => self.delete_layer(canvas, color),
            MenuAction::Properties => {
                self.select_color(canvas, color);
                self.panel_open = true;
                true
            }
            MenuAction::Rename | MenuAction::MakePaletteDefault => false,
        }
    }

    /// Removes a canvas layer and its strokes. The last layer is left in place.
    pub(crate) fn delete_layer(&mut self, canvas: NodeId, id: ColorId) -> bool {
        self.finish_size_edit();
        let Some(index) = self.active_sketch().color_index(canvas, id) else {
            return false;
        };
        let Some(len) = self.active_sketch().palette_len(canvas) else {
            return false;
        };
        if len <= 1 {
            return false;
        }
        let neighbor = if index + 1 < len {
            self.active_sketch().color_id_at(canvas, index + 1)
        } else {
            self.active_sketch().color_id_at(canvas, index - 1)
        };
        let step = self.active_sketch_mut().delete_color(canvas, id);
        if !self.active_sketch_mut().commit(step) {
            return false;
        }
        if let Subject::Color {
            owner: ColorOwner::Node(shown),
            color,
        } = self.subject
            && color == id
            && let Some(neighbor) = neighbor
        {
            self.subject = Subject::Color {
                owner: ColorOwner::Node(shown),
                color: neighbor,
            };
        }
        true
    }

    /// Adds a copy of a brush color directly below it. A tool setting.
    fn duplicate_brush_color(&mut self, color: ColorId) -> bool {
        let Some(index) = self
            .brush_palette
            .iter()
            .position(|item| item.id() == color)
        else {
            return false;
        };
        let mut copy = self.brush_palette[index].clone();
        copy.color.id = self.ids.color();
        self.brush_palette.insert(index + 1, copy);
        self.prefs_dirty = true;
        true
    }

    /// Removes a brush color. The brush keeps at least one. Layers copied from
    /// it stay on their canvases.
    fn delete_brush_color(&mut self, color: ColorId) -> bool {
        if self.brush_palette.len() <= 1 {
            return false;
        }
        let Some(index) = self
            .brush_palette
            .iter()
            .position(|item| item.id() == color)
        else {
            return false;
        };
        self.brush_palette.remove(index);
        let neighbor = self.brush_palette[index.min(self.brush_palette.len() - 1)].id();
        if self.brush_active == color {
            self.brush_active = neighbor;
        }
        if matches!(self.subject, Subject::Color { color: shown, .. } if shown == color) {
            self.subject = Subject::Color {
                owner: ColorOwner::Tool(Tool::Brush),
                color: neighbor,
            };
        }
        self.prefs_dirty = true;
        true
    }

    /// Moves a brush color, as dragging its swatch does. A tool setting.
    pub fn move_brush_color(&mut self, id: ColorId, to: usize) -> bool {
        self.finish_size_edit();
        let Some(from) = self.brush_palette.iter().position(|item| item.id() == id) else {
            return false;
        };
        if to >= self.brush_palette.len() || from == to {
            return false;
        }
        let color = self.brush_palette.remove(from);
        self.brush_palette.insert(to, color);
        self.prefs_dirty = true;
        true
    }

    fn tool_palette(&self, tool: Tool) -> Option<&[ToolColor]> {
        match tool {
            Tool::Brush => Some(&self.brush_palette),
            Tool::Rectangle => Some(&self.rectangle.palette),
            Tool::Ellipse => Some(&self.ellipse.palette),
            Tool::Line => Some(&self.line.palette),
            Tool::Text => Some(&self.text.palette),
            Tool::Eraser | Tool::Select => None,
        }
    }

    fn tool_palette_mut(&mut self, tool: Tool) -> Option<&mut Vec<ToolColor>> {
        match tool {
            Tool::Brush => Some(&mut self.brush_palette),
            Tool::Rectangle => Some(&mut self.rectangle.palette),
            Tool::Ellipse => Some(&mut self.ellipse.palette),
            Tool::Line => Some(&mut self.line.palette),
            Tool::Text => Some(&mut self.text.palette),
            Tool::Eraser | Tool::Select => None,
        }
    }

    fn tool_color(&self, tool: Tool, color: ColorId) -> Option<&ToolColor> {
        self.tool_palette(tool)?
            .iter()
            .find(|item| item.id() == color)
    }

    fn tool_color_mut(&mut self, tool: Tool, color: ColorId) -> Option<&mut ToolColor> {
        self.tool_palette_mut(tool)?
            .iter_mut()
            .find(|item| item.id() == color)
    }

    /// The tool color the Size slider sizes: the brush's active color, or a
    /// shape tool's Outline or Stroke.
    fn sized_tool_color(&self, tool: Tool) -> Option<&ToolColor> {
        match tool {
            Tool::Brush => self.tool_color(Tool::Brush, self.brush_active),
            Tool::Rectangle | Tool::Ellipse | Tool::Line => self
                .tool_palette(tool)?
                .iter()
                .find(|item| item.color.role.has_stroke()),
            _ => None,
        }
    }

    fn preview_tool_color(
        &mut self,
        tool: Tool,
        color: ColorId,
        id: PropId,
        value: PropValue,
    ) -> bool {
        self.finish_color_pending();
        let Some(item) = self.tool_color_mut(tool, color) else {
            return false;
        };
        let before = item.clone();
        if !item.write(id, &value) {
            return false;
        }
        if *item != before {
            self.tool_pending = true;
        }
        true
    }

    pub(crate) fn tool_snapshot(&self) -> ToolSnapshot {
        ToolSnapshot {
            brush: self.brush.clone(),
            eraser: self.eraser.clone(),
            eraser_size: self.eraser_size,
            brush_palette: self.brush_palette.clone(),
            brush_active: Some(self.brush_active),
            rectangle: self.rectangle.clone(),
            ellipse: self.ellipse.clone(),
            line: self.line.clone(),
            font_size: self.text.style.size,
            text: self.text.clone(),
        }
    }

    pub(crate) fn restore_tools(&mut self, tools: ToolSnapshot) {
        self.brush = tools.brush.sanitized();
        self.eraser = tools.eraser.sanitized();
        self.eraser_size = clamp_stored_size(tools.eraser_size, SIZE_MIN, SIZE_MAX);
        self.rectangle = sanitize_shape_tool(tools.rectangle, ShapeKind::Rectangle, &mut self.ids);
        self.ellipse = sanitize_shape_tool(tools.ellipse, ShapeKind::Ellipse, &mut self.ids);
        self.line = sanitize_shape_tool(tools.line, ShapeKind::Line, &mut self.ids);
        self.text = sanitize_text_tool(tools.text, &mut self.ids);
        self.text.style.size = clamp_stored_size(tools.font_size, FONT_MIN, FONT_MAX);
        self.brush_palette = if tools.brush_palette.is_empty() {
            brush_palette(&mut self.ids)
        } else {
            tools
                .brush_palette
                .into_iter()
                .map(|mut color| {
                    color.color.role = ColorRole::Layer;
                    color.sanitized()
                })
                .collect()
        };
        self.unique_tool_ids();
        self.brush_active = tools
            .brush_active
            .filter(|id| self.brush_palette.iter().any(|item| item.id() == *id))
            .unwrap_or_else(|| self.brush_palette[0].id());
        let palette = self.brush_palette.clone();
        if let Some(sketch) = self.sketches.first_mut() {
            sketch.adopt_palette(&palette, &mut self.ids);
        }
        self.tool_pending = false;
        self.prefs_dirty = false;
    }

    /// Tool colors keep their ids in preferences. Each id is reserved so the
    /// session never hands it out again, and a repeated id is replaced.
    fn unique_tool_ids(&mut self) {
        let mut seen: Vec<ColorId> = Vec::new();
        let mut ids = std::mem::replace(&mut self.ids, Allocator::new());
        for tool in [
            Tool::Brush,
            Tool::Rectangle,
            Tool::Ellipse,
            Tool::Line,
            Tool::Text,
        ] {
            let Some(palette) = self.tool_palette_mut(tool) else {
                continue;
            };
            for item in palette.iter() {
                ids.reserve_color(item.id());
            }
            for item in palette.iter_mut() {
                if seen.contains(&item.id()) {
                    item.color.id = ids.color();
                }
                seen.push(item.id());
            }
        }
        self.ids = ids;
    }

    pub(crate) fn prefs_dirty(&self) -> bool {
        self.prefs_dirty
    }

    pub(crate) fn clear_prefs_dirty(&mut self) {
        self.prefs_dirty = false;
    }

    /// The current tool's palette as swatches. The eraser and select have none.
    pub fn swatches(&self) -> Vec<SwatchView> {
        let Some(colors) = self.tool_palette(self.tool) else {
            return Vec::new();
        };
        let canvas = self.active_sketch().highlighted_id();
        colors
            .iter()
            .map(|item| {
                let muted = if self.tool == Tool::Brush {
                    canvas
                        .and_then(|canvas| {
                            self.active_sketch().source_layer_flags(canvas, item.id())
                        })
                        .is_some_and(|(visible, locked)| !visible || locked)
                } else {
                    !item.color.visible
                };
                SwatchView {
                    id: item.id(),
                    name: item.color.name.clone(),
                    rgb: item.color.rgb,
                    active: self.tool == Tool::Brush && item.id() == self.brush_active,
                    muted,
                }
            })
            .collect()
    }

    /// Makes a brush color the one the next stroke uses. A tool setting.
    pub fn set_active_color(&mut self, id: ColorId) {
        self.finish_size_edit();
        if self.brush_palette.iter().any(|item| item.id() == id) && self.brush_active != id {
            self.brush_active = id;
            self.prefs_dirty = true;
        }
    }

    pub fn highlighted_id(&self) -> Option<NodeId> {
        self.active_sketch().highlighted_id()
    }

    /// With the text tool, the text its toolbar changes: the text being
    /// edited, or the one selected text. New text being typed has none, as it
    /// is drawn with the tool's own settings, which the toolbar then changes.
    pub(crate) fn active_text(&self) -> Option<NodeId> {
        if self.tool != Tool::Text {
            return None;
        }
        match &self.text_draft {
            Some(TextDraft::Existing { id, .. }) => Some(*id),
            Some(TextDraft::New { .. }) => None,
            None => self.sole_text(),
        }
    }

    /// A toolbar control's value: the active text's, or the current tool's.
    pub(crate) fn bar_value(&self, prop: PropId) -> Option<PropValue> {
        self.active_text()
            .and_then(|id| self.active_sketch().node_prop(id, prop))
            .or_else(|| self.tool_property(self.tool, prop))
    }

    /// Sets a toolbar control. On the active text it is one undo step, a
    /// slider's whole drag included; otherwise it is the current tool's
    /// setting for its next mark. `finished` ends a slider drag.
    pub(crate) fn set_bar_value(&mut self, prop: PropId, value: PropValue, finished: bool) -> bool {
        if let Some(id) = self.active_text()
            && self.active_sketch().node_prop(id, prop).is_some()
        {
            return self.edit_nodes(prop, value, finished);
        }
        let ok = self.preview_tool(self.tool, prop, value);
        if finished {
            self.finish_size_edit();
        }
        ok
    }

    /// The list button's state: whether the list is on, and its kind. A text
    /// object's list off keeps no kind, so the button offers the tool's.
    pub(crate) fn text_list(&self) -> (bool, usize) {
        let kind = match self.text.style.list_kind {
            ListKind::Bullets => 0,
            ListKind::Numbers => 1,
            ListKind::Checks => 2,
        };
        match self.bar_value(PropId::List) {
            Some(PropValue::Index(index)) if index > 0 => (true, usize::from(index - 1)),
            _ => (false, kind),
        }
    }

    /// The list button: a click turns the list on with its kind, or off.
    pub(crate) fn toggle_text_list(&mut self) {
        let (on, kind) = self.text_list();
        let index = if on { 0 } else { kind as u8 + 1 };
        self.set_bar_value(PropId::List, PropValue::Index(index), true);
    }

    /// A kind chosen from the list button's menu turns the list on in it.
    pub(crate) fn choose_text_list(&mut self, kind: usize) {
        self.set_bar_value(PropId::List, PropValue::Index(kind as u8 + 1), true);
    }

    /// What the Size slider shows: the size of the next mark the current tool
    /// makes. A locked color's slider shows that color's size, in its color;
    /// a brush color's is its layer's size on the highlighted canvas.
    pub fn size_control(&self) -> Option<SizeControl> {
        let spec = registry::tool(self.tool).size?;
        let (value, tint) = match spec.target {
            SizeTarget::Color | SizeTarget::ShapeStroke => {
                let item = self.sized_tool_color(self.tool)?;
                if item.lock_size {
                    let layer_size = (self.tool == Tool::Brush)
                        .then(|| {
                            let canvas = self.active_sketch().highlighted_id()?;
                            let layer = self.active_sketch().layer_for_source(canvas, item.id())?;
                            self.active_sketch().color_size(canvas, layer)
                        })
                        .flatten();
                    (layer_size.unwrap_or(item.color.size), Some(item.color.rgb))
                } else {
                    (item.next_size, None)
                }
            }
            SizeTarget::Eraser => (self.eraser_size, None),
            SizeTarget::Font => match self.bar_value(PropId::FontSize) {
                Some(PropValue::Number(size)) => (size.round().clamp(0.0, 255.0) as u8, None),
                _ => (self.text.style.size, None),
            },
        };
        Some(SizeControl {
            value,
            min: spec.min,
            max: spec.max,
            tint,
        })
    }

    pub fn size(&self) -> u8 {
        self.size_control()
            .map(|control| control.value)
            .unwrap_or(draw::defaults::COLOR_SIZE)
    }

    pub fn set_size(&mut self, size: u8) -> Result<(), CommandError> {
        self.apply_size(size, true)
    }

    /// Applies a size while a slider is still down. The undo step is the whole drag.
    pub fn preview_size(&mut self, size: u8) -> Result<(), CommandError> {
        self.apply_size(size, false)
    }

    pub fn finish_size_edit(&mut self) {
        if self.color_drag_abandoned {
            self.color_drag_abandoned = false;
            if let Some(pending) = self.pending.take() {
                self.restore_pending(&pending);
            }
        } else {
            self.finish_color_pending();
        }
        if self.tool_pending {
            self.prefs_dirty = true;
            self.tool_pending = false;
        }
    }

    pub fn step_size(&mut self, delta: i32) {
        let Some(control) = self.size_control() else {
            return;
        };
        let next = (i32::from(control.value) + delta)
            .clamp(i32::from(control.min), i32::from(control.max));
        let _ = self.set_size(next as u8);
    }

    fn apply_size(&mut self, size: u8, finished: bool) -> Result<(), CommandError> {
        let Some(spec) = registry::tool(self.tool).size else {
            return Err(CommandError::Unavailable);
        };
        if !(spec.min..=spec.max).contains(&size) {
            return Err(CommandError::SizeOutOfRange);
        }
        match spec.target {
            SizeTarget::Color | SizeTarget::ShapeStroke => {
                self.set_next_size(size, finished);
                Ok(())
            }
            SizeTarget::Eraser => {
                self.set_stored_size(SizeSlot::Eraser, size, finished);
                Ok(())
            }
            SizeTarget::Font if self.active_text().is_some() => {
                self.set_bar_value(
                    PropId::FontSize,
                    PropValue::Number(f64::from(size)),
                    finished,
                );
                Ok(())
            }
            SizeTarget::Font => {
                self.set_stored_size(SizeSlot::Font, size, finished);
                Ok(())
            }
        }
    }

    /// The Size slider moved for a brush color or a shape's outline: that
    /// size is used for the next marks in that color, and its lock is off.
    /// A tool setting; nothing already drawn changes.
    fn set_next_size(&mut self, size: u8, finished: bool) {
        self.finish_color_pending();
        let shown = self.size_control().map(|control| control.value);
        let tool = self.tool;
        let Some(id) = self.sized_tool_color(tool).map(ToolColor::id) else {
            return;
        };
        let Some(item) = self.tool_color_mut(tool, id) else {
            return;
        };
        let size = size.clamp(SIZE_MIN, SIZE_MAX);
        // A press that leaves a locked slider where it was keeps the lock.
        let moved = shown != Some(size) || !item.lock_size;
        if moved && (item.lock_size || item.next_size != size) {
            item.lock_size = false;
            item.next_size = size;
            self.tool_pending = true;
        }
        if finished {
            self.finish_size_edit();
        }
    }

    /// Moves a layer on the highlighted canvas. Index 0 paints on top.
    pub fn move_color(&mut self, id: ColorId, to: usize) -> Result<(), CommandError> {
        self.finish_size_edit();
        let canvas = self
            .active_sketch()
            .highlighted_id()
            .ok_or(CommandError::Unavailable)?;
        let from = self
            .active_sketch()
            .color_index(canvas, id)
            .ok_or(CommandError::Unavailable)?;
        let len = self
            .active_sketch()
            .palette_len(canvas)
            .ok_or(CommandError::Unavailable)?;
        if to >= len {
            return Err(CommandError::Unavailable);
        }
        if from == to {
            return Ok(());
        }
        self.active_sketch_mut()
            .commit_step(Step::MoveColor { canvas, from, to });
        Ok(())
    }

    /// Rotate and flip are left out of the select tool's bar while the
    /// selection holds text, rather than drawn lighter.
    pub(crate) fn command_shown(&self, id: CommandId) -> bool {
        !matches!(
            id,
            CommandId::RotateClockwise
                | CommandId::RotateCounterclockwise
                | CommandId::RotateHalf
                | CommandId::FlipHorizontal
                | CommandId::FlipVertical
        ) || self.active_sketch().selection_turns()
    }

    /// The rotation handle shows, as rotate and flip do.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn selection_turns(&self) -> bool {
        self.active_sketch().selection_turns()
    }

    pub fn command_enabled(&self, id: CommandId) -> bool {
        let sketch = self.active_sketch();
        match id {
            CommandId::Delete => sketch.can_delete(),
            CommandId::NudgeLeft
            | CommandId::NudgeRight
            | CommandId::NudgeUp
            | CommandId::NudgeDown => sketch.can_nudge(),
            CommandId::ZoomToSelection => sketch.selection_bounds().is_some(),
            CommandId::NewCanvas => true,
            CommandId::Group => sketch.can_group(),
            CommandId::Ungroup => sketch.can_ungroup(),
            CommandId::BringForward => sketch.can_reorder(Reorder::Forward),
            CommandId::SendBackward => sketch.can_reorder(Reorder::Backward),
            CommandId::BringToFront => sketch.can_reorder(Reorder::ToFront),
            CommandId::SendToBack => sketch.can_reorder(Reorder::ToBack),
            CommandId::AlignLeft | CommandId::AlignCenter | CommandId::AlignRight => {
                sketch.can_align()
            }
            CommandId::RotateClockwise
            | CommandId::RotateCounterclockwise
            | CommandId::RotateHalf
            | CommandId::FlipHorizontal
            | CommandId::FlipVertical => sketch.can_rotate(),
            CommandId::Copy => !self.text_editing() && !self.active_sketch().selection().is_empty(),
            CommandId::Cut => !self.text_editing() && self.active_sketch().can_delete(),
            CommandId::Paste => !self.text_editing() && !self.clipboard.nodes.is_empty(),
            _ => match registry::command(id).enable {
                Enable::Always => true,
                Enable::WhenUndo => self.undo_len() > 0,
                Enable::WhenRedo => self.redo_len() > 0,
                Enable::WhenClosable => {
                    !(self.sketches.len() == 1 && self.active_sketch().is_clean())
                }
                Enable::WhenRecent => !self.recent.is_empty(),
                Enable::Waiting => false,
            },
        }
    }

    pub fn perform(&mut self, id: CommandId, viewport: Point) -> CommandEffect {
        self.perform_shifted(id, viewport, false)
    }

    pub fn perform_shifted(
        &mut self,
        id: CommandId,
        viewport: Point,
        shift: bool,
    ) -> CommandEffect {
        if !self.command_enabled(id) {
            return CommandEffect::None;
        }
        let nudge = if shift { 8.0 } else { 1.0 };
        let edited = match id {
            CommandId::NewSketch => {
                self.new_sketch();
                CommandEffect::CenterView
            }
            CommandId::Undo => {
                let _ = self.undo();
                CommandEffect::None
            }
            CommandId::Redo => {
                let _ = self.redo();
                CommandEffect::None
            }
            CommandId::ZoomIn => {
                self.zoom_at_center(true, viewport);
                CommandEffect::None
            }
            CommandId::ZoomOut => {
                self.zoom_at_center(false, viewport);
                CommandEffect::None
            }
            CommandId::Fit => {
                self.fit(viewport);
                CommandEffect::None
            }
            CommandId::Maximize => CommandEffect::ToggleMaximize,
            CommandId::TogglePanel => {
                self.toggle_panel();
                CommandEffect::None
            }
            CommandId::SizeDown => {
                self.step_size(-1);
                CommandEffect::None
            }
            CommandId::SizeUp => {
                self.step_size(1);
                CommandEffect::None
            }
            CommandId::Close => match self.request_close(self.active_id()) {
                CloseEffect::Closed { replacement } => CommandEffect::Closed { replacement },
                _ => CommandEffect::None,
            },
            CommandId::NewCanvas => {
                self.add_canvas();
                CommandEffect::None
            }
            CommandId::Delete => {
                self.finish_size_edit();
                let step = self.active_sketch_mut().delete_selected();
                self.active_sketch_mut().commit(step);
                CommandEffect::None
            }
            CommandId::Copy => {
                self.copy_selection();
                CommandEffect::None
            }
            CommandId::Cut => {
                self.finish_size_edit();
                self.cut_selection();
                CommandEffect::None
            }
            CommandId::Paste => {
                self.finish_size_edit();
                self.paste_clipboard();
                CommandEffect::None
            }
            CommandId::Group => {
                self.finish_size_edit();
                let index = self.active;
                let step = self.sketches[index].group_selected(&mut self.ids);
                self.sketches[index].commit(step);
                CommandEffect::None
            }
            CommandId::Ungroup => {
                self.finish_size_edit();
                let step = self.active_sketch_mut().ungroup_selected();
                self.active_sketch_mut().commit(step);
                CommandEffect::None
            }
            CommandId::BringForward => self.reorder(Reorder::Forward),
            CommandId::SendBackward => self.reorder(Reorder::Backward),
            CommandId::BringToFront => self.reorder(Reorder::ToFront),
            CommandId::SendToBack => self.reorder(Reorder::ToBack),
            CommandId::AlignLeft => self.align(AlignEdge::Left),
            CommandId::AlignCenter => self.align(AlignEdge::Center),
            CommandId::AlignRight => self.align(AlignEdge::Right),
            CommandId::RotateClockwise => self.turn(std::f64::consts::FRAC_PI_2),
            CommandId::RotateCounterclockwise => self.turn(-std::f64::consts::FRAC_PI_2),
            CommandId::RotateHalf => self.turn(std::f64::consts::PI),
            CommandId::FlipHorizontal => self.flip(true),
            CommandId::FlipVertical => self.flip(false),
            CommandId::ZoomToSelection => {
                self.active_sketch_mut().zoom_to_selection(viewport);
                CommandEffect::None
            }
            CommandId::NudgeLeft => self.nudge([-nudge, 0.0]),
            CommandId::NudgeRight => self.nudge([nudge, 0.0]),
            CommandId::NudgeUp => self.nudge([0.0, -nudge]),
            CommandId::NudgeDown => self.nudge([0.0, nudge]),
            CommandId::Save => {
                self.save_active();
                CommandEffect::None
            }
            CommandId::SaveAs => {
                self.ask_save(self.active_id(), false, false);
                CommandEffect::None
            }
            CommandId::SaveCopy => {
                self.ask_save(self.active_id(), true, false);
                CommandEffect::None
            }
            CommandId::Open => {
                self.file_intent = Some(FileIntent::Open);
                CommandEffect::None
            }
            CommandId::ClearRecent => {
                self.clear_recent();
                CommandEffect::None
            }
        };
        let _ = edited;
        edited
    }

    fn reorder(&mut self, how: Reorder) -> CommandEffect {
        self.finish_size_edit();
        let step = self.active_sketch_mut().reorder_selected(how);
        self.active_sketch_mut().commit(step);
        CommandEffect::None
    }

    fn align(&mut self, edge: AlignEdge) -> CommandEffect {
        self.finish_size_edit();
        let step = self.active_sketch_mut().align_selected(edge);
        self.active_sketch_mut().commit(step);
        CommandEffect::None
    }

    fn turn(&mut self, radians: f64) -> CommandEffect {
        self.finish_size_edit();
        let step = self.active_sketch_mut().rotate_selected(radians);
        self.active_sketch_mut().commit(step);
        CommandEffect::None
    }

    fn flip(&mut self, horizontal: bool) -> CommandEffect {
        self.finish_size_edit();
        let step = self.active_sketch_mut().flip_selected(horizontal);
        self.active_sketch_mut().commit(step);
        CommandEffect::None
    }

    fn nudge(&mut self, delta: Point) -> CommandEffect {
        self.finish_size_edit();
        let step = self.active_sketch_mut().nudge_selected(delta);
        self.active_sketch_mut().commit(step);
        CommandEffect::None
    }

    /// Copy leaves the sketch alone, including a locked object that is selected.
    /// Nothing selected leaves the clipboard as it is.
    fn copy_selection(&mut self) {
        let nodes = self.active_sketch().copied_subtrees(false);
        if nodes.is_empty() {
            return;
        }
        self.clipboard = Clipboard { nodes, pasted: 0 };
    }

    /// Copies what can be changed, then deletes it as one undo step. A selection
    /// that cannot be changed leaves both the sketch and the clipboard alone.
    fn cut_selection(&mut self) {
        let nodes = self.active_sketch().copied_subtrees(true);
        if nodes.is_empty() {
            return;
        }
        self.clipboard = Clipboard { nodes, pasted: 0 };
        let step = self.active_sketch_mut().delete_selected();
        self.active_sketch_mut().commit(step);
    }

    fn paste_clipboard(&mut self) {
        if self.clipboard.nodes.is_empty() {
            return;
        }
        let step = self.clipboard.pasted + 1;
        let delta = PASTE_STEP * f64::from(step);
        let sources = self.clipboard.nodes.clone();
        let mut nodes = Vec::with_capacity(sources.len());
        for node in &sources {
            nodes.push(node.duplicate_for_paste(&mut self.ids, [delta, delta]));
        }
        let ids: Vec<NodeId> = nodes.iter().map(|node| node.id()).collect();
        let edit = self.active_sketch_mut().paste_copies(nodes);
        if self.active_sketch_mut().commit(edit) {
            self.active_sketch_mut().set_selection(ids);
            self.clipboard.pasted = step;
        }
    }

    fn add_canvas(&mut self) {
        self.finish_size_edit();
        let palette = self.brush_palette.clone();
        let index = self.active;
        let (_, step) = self.sketches[index].add_canvas(&mut self.ids, &palette);
        self.sketches[index].commit(Some(step));
    }

    pub fn gesture_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn begin(&mut self) -> Result<(), CommandError> {
        if self.open.is_some() {
            return Err(CommandError::GestureAlreadyOpen);
        }
        match self.tool {
            Tool::Brush => {
                // With no canvas highlighted the stroke makes a new canvas, as a
                // shape tool makes a new shape. A highlighted canvas whose layer
                // for this color is hidden or locked draws nothing.
                let canvas = self.active_sketch().highlighted_id();
                let source = self.brush_active;
                if canvas.is_some() && !self.active_sketch().brush_ready(source) {
                    return Err(CommandError::Unavailable);
                }
                let item = self
                    .tool_color(Tool::Brush, source)
                    .ok_or(CommandError::Unavailable)?;
                let size = (!item.lock_size).then_some(item.next_size);
                let layer =
                    canvas.and_then(|canvas| self.active_sketch().layer_for_source(canvas, source));
                self.open = Some(OpenGesture {
                    kind: GestureKind::Brush {
                        canvas,
                        source,
                        layer,
                        size,
                        dynamics: self.brush.clone(),
                    },
                    samples: Vec::new(),
                });
                Ok(())
            }
            Tool::Eraser => {
                // With no canvas highlighted there is nothing to erase.
                let canvas = self
                    .active_sketch()
                    .highlighted_id()
                    .ok_or(CommandError::Unavailable)?;
                self.open = Some(OpenGesture {
                    kind: GestureKind::Erase {
                        canvas,
                        size: self.eraser_size,
                        dynamics: self.eraser.clone(),
                    },
                    samples: Vec::new(),
                });
                Ok(())
            }
            Tool::Select | Tool::Rectangle | Tool::Ellipse | Tool::Line | Tool::Text => {
                Err(CommandError::NotAStrokeTool)
            }
        }
    }

    pub fn add_sample(&mut self, sample: Sample) -> Result<(), CommandError> {
        if self.open.is_none() {
            return Err(CommandError::NoOpenGesture);
        }
        if !sample.x.is_finite() || !sample.y.is_finite() {
            return Err(CommandError::InvalidSample);
        }
        if let Some(pressure) = sample.pressure {
            if !pressure.is_finite() || !(0.0..=1.0).contains(&pressure) {
                return Err(CommandError::InvalidSample);
            }
        }
        let open = self.open.as_mut().unwrap();
        if let Some(last) = open.samples.last_mut() {
            if last.x == sample.x && last.y == sample.y {
                if last.pressure != sample.pressure {
                    last.pressure = sample.pressure;
                }
                return Ok(());
            }
        }
        open.samples.push(sample);
        Ok(())
    }

    pub fn begin_shape(&mut self, point: Point) -> Result<(), CommandError> {
        if self.open.is_some() {
            return Err(CommandError::GestureAlreadyOpen);
        }
        if !matches!(self.tool, Tool::Rectangle | Tool::Ellipse | Tool::Line) {
            return Err(CommandError::NotAStrokeTool);
        }
        if !point[0].is_finite() || !point[1].is_finite() {
            return Err(CommandError::InvalidSample);
        }
        self.open = Some(OpenGesture {
            kind: GestureKind::Shape {
                tool: self.tool,
                origin: point,
                pointer: point,
                shift: false,
            },
            samples: Vec::new(),
        });
        Ok(())
    }

    pub fn move_shape(&mut self, point: Point, shift: bool) -> Result<(), CommandError> {
        if !point[0].is_finite() || !point[1].is_finite() {
            return Err(CommandError::InvalidSample);
        }
        let Some(open) = self.open.as_mut() else {
            return Err(CommandError::NoOpenGesture);
        };
        let GestureKind::Shape {
            pointer,
            shift: held,
            ..
        } = &mut open.kind
        else {
            return Err(CommandError::NotAStrokeTool);
        };
        *pointer = point;
        *held = shift;
        Ok(())
    }

    /// Starts a text placement. A press on existing text is remembered so the
    /// release edits it. The selection and the current tool stay as they are.
    pub fn begin_text(&mut self, point: Point) -> Result<(), CommandError> {
        if self.open.is_some() {
            return Err(CommandError::GestureAlreadyOpen);
        }
        if self.tool != Tool::Text {
            return Err(CommandError::NotAStrokeTool);
        }
        if !point[0].is_finite() || !point[1].is_finite() {
            return Err(CommandError::InvalidSample);
        }
        self.commit_text();
        let hit = self
            .active_sketch()
            .hit_test(point)
            .filter(|id| self.active_sketch().is_text(*id));
        self.open = Some(OpenGesture {
            kind: GestureKind::Text {
                origin: point,
                pointer: point,
                hit,
            },
            samples: Vec::new(),
        });
        Ok(())
    }

    pub fn move_text(&mut self, point: Point) -> Result<(), CommandError> {
        if !point[0].is_finite() || !point[1].is_finite() {
            return Err(CommandError::InvalidSample);
        }
        let Some(open) = self.open.as_mut() else {
            return Err(CommandError::NoOpenGesture);
        };
        let GestureKind::Text { pointer, .. } = &mut open.kind else {
            return Err(CommandError::NotAStrokeTool);
        };
        *pointer = point;
        Ok(())
    }

    /// Ends the placement. A drag stores the horizontal distance as the box
    /// width and turns auto-size off. A click stores no width. Either one
    /// starts editing; the object is inserted only when the edit is committed.
    pub fn end_text(&mut self, dragged: bool) -> Result<(), CommandError> {
        let Some(open) = self.open.take() else {
            return Err(CommandError::NoOpenGesture);
        };
        let GestureKind::Text {
            origin,
            pointer,
            hit,
        } = open.kind
        else {
            self.open = Some(open);
            return Err(CommandError::NotAStrokeTool);
        };
        if let Some(id) = hit.filter(|id| self.active_sketch().is_text(*id)) {
            self.edit_text(id);
            return Ok(());
        }
        let span = (pointer[0] - origin[0]).abs();
        // A drag sets the box's width. A click places auto-sized text, or,
        // with the text-box button on, a box of the starting width that wraps.
        let box_width = if dragged && span >= 1.0 {
            Some(span)
        } else if !self.text.auto_size {
            Some(defaults::TEXT_BOX_WIDTH)
        } else {
            None
        };
        self.text_draft = Some(TextDraft::New {
            frame: TextBox {
                origin,
                width: box_width,
                height: None,
            },
            content: String::new(),
        });
        self.text_generation += 1;
        Ok(())
    }

    pub fn text_editing(&self) -> bool {
        self.text_draft.is_some()
    }

    pub fn text_generation(&self) -> u64 {
        self.text_generation
    }

    pub fn set_text_draft(&mut self, content: String) -> bool {
        let Some(draft) = self.text_draft.as_mut() else {
            return false;
        };
        match draft {
            TextDraft::New { content: slot, .. } | TextDraft::Existing { content: slot, .. } => {
                *slot = content;
            }
        }
        true
    }

    /// Commits the open edit. A new object is one insert. An existing object's
    /// new words are one content step. An empty new object was never inserted,
    /// so it is dropped with no history. Emptying an existing object removes
    /// it in one undo step. Spaces count as characters.
    pub fn set_text_metrics(&mut self, id: NodeId, width: f64, height: f64, natural: f64) {
        let index = self.active;
        self.sketches[index].set_text_metrics(id, width, height, natural);
    }

    pub fn commit_text(&mut self) {
        let Some(draft) = self.text_draft.take() else {
            return;
        };
        match draft {
            TextDraft::New { frame, content } => {
                if content.is_empty() {
                    return;
                }
                let palette = object_palette(&self.text.palette, &mut self.ids);
                let index = self.active;
                let made = self.sketches[index].add_text(
                    &mut self.ids,
                    frame,
                    content,
                    self.text.style.clone(),
                    palette,
                );
                self.sketches[index].commit(made.map(|(_, step)| step));
            }
            TextDraft::Existing {
                id,
                content,
                original,
            } => {
                if content.is_empty() {
                    let index = self.active;
                    let step = self.sketches[index].remove_text(id);
                    self.sketches[index].commit(step);
                    return;
                }
                if content == original {
                    return;
                }
                let index = self.active;
                self.sketches[index].commit_step(Step::SetText {
                    id,
                    before: original,
                    after: content,
                });
            }
        }
    }

    pub fn commit(&mut self) -> Result<(), CommandError> {
        let Some(open) = self.open.take() else {
            return Err(CommandError::NoOpenGesture);
        };
        if matches!(open.kind, GestureKind::Text { .. }) {
            self.open = Some(open);
            return Err(CommandError::NotAStrokeTool);
        }
        if let GestureKind::Shape {
            tool,
            origin,
            pointer,
            shift,
        } = open.kind
        {
            self.commit_shape(tool, origin, pointer, shift);
            return Ok(());
        }
        if open.samples.len() < 2 {
            return Ok(());
        }
        match open.kind {
            GestureKind::Brush {
                canvas,
                source,
                size,
                dynamics,
                ..
            } => {
                let transform = canvas
                    .and_then(|canvas| self.active_sketch().world_transform(canvas))
                    .unwrap_or(kurbo::Affine::IDENTITY);
                let stroke = Stroke::new(
                    self.ids.stroke(),
                    map_samples(transform, &open.samples),
                    dynamics,
                    size,
                );
                let Some(item) = self.tool_color(Tool::Brush, source).cloned() else {
                    return Err(CommandError::Unavailable);
                };
                let index = self.active;
                match canvas {
                    Some(canvas) => {
                        let step = self.sketches[index].commit_brush_stroke(
                            &mut self.ids,
                            canvas,
                            &item,
                            stroke,
                        );
                        self.sketches[index].commit(step);
                    }
                    None => {
                        let palette = self.brush_palette.clone();
                        let made = self.sketches[index].commit_stroke_on_new_canvas(
                            &mut self.ids,
                            &palette,
                            item.id(),
                            stroke,
                        );
                        self.sketches[index].commit(made.map(|(_, step)| step));
                    }
                }
                Ok(())
            }
            GestureKind::Erase {
                canvas,
                size,
                dynamics,
            } => self.commit_erase(canvas, size, dynamics, open.samples),
            GestureKind::Shape { .. } | GestureKind::Text { .. } => Ok(()),
        }
    }

    pub fn cancel(&mut self) -> Result<(), CommandError> {
        if self.open.take().is_none() {
            return Err(CommandError::NoOpenGesture);
        }
        Ok(())
    }

    pub fn undo(&mut self) -> Result<(), CommandError> {
        self.abandon_color_drag();
        if self.active_sketch_mut().undo() {
            Ok(())
        } else {
            Err(CommandError::EmptyHistory)
        }
    }

    pub fn redo(&mut self) -> Result<(), CommandError> {
        self.abandon_color_drag();
        if self.active_sketch_mut().redo() {
            Ok(())
        } else {
            Err(CommandError::EmptyHistory)
        }
    }

    pub fn stroke_count(&self) -> usize {
        self.active_sketch().stroke_count()
    }

    pub fn stroke_ids(&self) -> Vec<StrokeId> {
        self.active_sketch().stroke_ids()
    }

    pub fn stroke_samples(&self, id: StrokeId) -> Option<&[Sample]> {
        self.active_sketch().stroke(id).map(Stroke::samples)
    }

    pub fn stroke_dynamics(&self, id: StrokeId) -> Option<&Dynamics> {
        self.active_sketch().stroke(id).map(Stroke::dynamics)
    }

    pub fn erase_paths(&self, id: StrokeId) -> Option<&[ErasePath]> {
        self.active_sketch().stroke(id).map(Stroke::erase_paths)
    }

    pub fn undo_len(&self) -> usize {
        self.active_sketch().undo_len()
    }

    pub fn redo_len(&self) -> usize {
        self.active_sketch().redo_len()
    }

    pub fn zoom(&self) -> f64 {
        self.active_sketch().camera().zoom
    }

    pub fn camera_origin(&self) -> Point {
        self.active_sketch().camera().origin
    }

    pub fn pan_screen(&mut self, delta: Point) {
        self.active_sketch_mut()
            .move_view(|camera, _| draw::camera::pan_screen(camera, delta));
    }

    pub fn zoom_toward(&mut self, factor: f64, pointer: Point) {
        self.active_sketch_mut()
            .move_view(|camera, _| draw::camera::zoom_toward(camera, factor, pointer));
    }

    pub fn zoom_at_center(&mut self, zoom_in: bool, viewport: Point) {
        self.active_sketch_mut()
            .move_view(|camera, _| draw::camera::zoom_at_center(camera, zoom_in, viewport));
    }

    pub fn fit(&mut self, viewport: Point) {
        self.active_sketch_mut()
            .move_view(|camera, sketch| draw::camera::fit(camera, sketch, viewport));
    }

    pub fn translate_node(&mut self, id: NodeId, delta: Point) -> bool {
        let step = self.active_sketch().translate_node(id, delta);
        self.active_sketch_mut().preview(step)
    }

    pub fn ink_outline(&self, id: StrokeId) -> Option<Vec<Point>> {
        self.active_sketch().ink_outline(id)
    }

    pub fn erase_outline(&self, id: StrokeId, index: usize) -> Option<Vec<Point>> {
        self.active_sketch().erase_outline(id, index)
    }

    pub fn visible_polygons(&self, id: StrokeId) -> Option<ink::Shapes> {
        self.active_sketch().visible_polygons(id)
    }

    pub fn visible_at(&self, id: StrokeId, point: Point) -> bool {
        self.active_sketch().visible_at(id, point)
    }

    pub fn hit_test(&self, point: Point) -> Option<NodeId> {
        self.active_sketch().hit_test(point)
    }

    pub fn selection(&self) -> Vec<NodeId> {
        self.active_sketch().selection().to_vec()
    }

    pub fn select_click(&mut self, id: NodeId, shift: bool) {
        if shift {
            self.active_sketch_mut().toggle_selection(id);
        } else {
            self.active_sketch_mut().set_selection([id]);
        }
    }

    pub fn clear_selection(&mut self) {
        self.active_sketch_mut().set_selection([]);
    }

    pub fn replace_selection(&mut self, ids: Vec<NodeId>) {
        self.active_sketch_mut().set_selection(ids);
    }

    /// Shift adds the hits to `base`. A plain marquee replaces the selection.
    pub fn marquee_select(&mut self, min: Point, max: Point, shift: bool, base: &[NodeId]) {
        let hits = self.active_sketch().marquee_hits(min, max);
        if shift {
            let mut ids = base.to_vec();
            for id in hits {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            self.active_sketch_mut().set_selection(ids);
        } else {
            self.active_sketch_mut().set_selection(hits);
        }
    }

    pub fn selection_bounds(&self) -> Option<(Point, Point)> {
        self.active_sketch().selection_bounds()
    }

    /// The one selected object, when it is a text object that can be changed.
    pub(crate) fn sole_text(&self) -> Option<NodeId> {
        let [id] = self.active_sketch().selection() else {
            return None;
        };
        (self.active_sketch().is_text(*id) && self.active_sketch().transform_target(*id))
            .then_some(*id)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn text_box(&self, id: NodeId) -> Option<TextFit> {
        self.active_sketch().text_box(id)
    }

    /// A handle's drag on text: a side handle sets the box's width, so the
    /// words re-wrap, and a top or bottom handle its height, which the lines
    /// sit in by the vertical alignment. The opposite side stays put.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn preview_text_box(
        &mut self,
        id: NodeId,
        before: TextFit,
        from: Point,
        to: Point,
        side: BoxSide,
    ) {
        let sketch = self.active_sketch();
        let (Some(from), Some(to)) = (sketch.local_point(id, from), sketch.local_point(id, to))
        else {
            return;
        };
        let em = sketch.text_size(id).map_or(8.0, f64::from);
        let after = dragged_box(before, [to[0] - from[0], to[1] - from[1]], em, side);
        let step = self.active_sketch().place_text_box(id, after);
        self.active_sketch_mut().preview(step);
    }

    pub(crate) fn text_box_handles(&self) -> bool {
        self.tool == Tool::Text
    }

    /// Sets the new text's box. It is not history until the text is.
    pub(crate) fn set_draft_box(&mut self, to: TextBox) {
        if let Some(TextDraft::New { frame, .. }) = self.text_draft.as_mut() {
            *frame = to;
        }
    }

    pub(crate) fn restore_text_box(&mut self, id: NodeId, before: TextBox) {
        let step = self.active_sketch().place_text_box(id, before);
        self.active_sketch_mut().preview(step);
    }

    pub(crate) fn commit_text_box(&mut self, id: NodeId, before: TextBox) -> bool {
        let step = self.active_sketch_mut().record_text_box(id, before);
        self.active_sketch_mut().record(step)
    }

    pub fn changeable_locals(&self) -> Vec<(NodeId, Affine)> {
        self.active_sketch()
            .selection()
            .iter()
            .filter(|id| self.active_sketch().transform_target(**id))
            .filter_map(|id| {
                self.active_sketch()
                    .local_transform(*id)
                    .map(|transform| (*id, transform))
            })
            .collect()
    }

    pub fn preview_world(&mut self, before: &[(NodeId, Affine)], map: impl Fn(Affine) -> Affine) {
        let mut locals = Vec::new();
        for (id, local) in before {
            let Some(parent) = self.active_sketch().parent_of_world(*id) else {
                continue;
            };
            locals.push((*id, parent.inverse() * map(parent * *local)));
        }
        let step = self.active_sketch().set_locals(&locals);
        self.active_sketch_mut().preview(step);
    }

    pub fn commit_preview(&mut self, before: &[(NodeId, Affine)]) -> bool {
        let step = self.active_sketch_mut().record_transform_changes(before);
        self.active_sketch_mut().record(step)
    }

    pub fn restore_locals(&mut self, before: &[(NodeId, Affine)]) {
        let step = self.active_sketch().set_locals(before);
        self.active_sketch_mut().preview(step);
    }

    pub(crate) fn layers(&self) -> Vec<LayerNode> {
        self.active_sketch().layers()
    }

    /// A click on a layer row: highlights its canvas and opens that layer's
    /// color page.
    pub fn select_color(&mut self, canvas: NodeId, color: ColorId) {
        self.finish_size_edit();
        self.active_sketch_mut().set_selection([canvas]);
        if self.active_sketch().has_color(canvas, color) {
            self.subject = Subject::Color {
                owner: ColorOwner::Node(canvas),
                color,
            };
            self.return_to = Some(Subject::Layers);
        }
    }

    pub fn rename_node(&mut self, id: NodeId, name: &str) -> bool {
        if !self.active_sketch().can_edit(id, PropId::Name) {
            return false;
        }
        let Some(before) = self.active_sketch().node_prop(id, PropId::Name) else {
            return false;
        };
        let value = PropValue::Text(name.to_string());
        let step = self.active_sketch().assign_node_prop(id, PropId::Name, &value);
        if !self.active_sketch_mut().preview(step) {
            return false;
        }
        let Some(after) = self.active_sketch().node_prop(id, PropId::Name) else {
            return false;
        };
        let step = self
            .active_sketch_mut()
            .record_node_props(vec![(id, PropId::Name, before, after)]);
        self.active_sketch_mut().record(step)
    }

    pub fn toggle_node(&mut self, id: NodeId, prop: PropId) -> bool {
        if !matches!(prop, PropId::Visible | PropId::Locked) {
            return false;
        }
        if !self.active_sketch().can_edit(id, prop) {
            return false;
        }
        let Some(PropValue::Bool(on)) = self.active_sketch().node_prop(id, prop) else {
            return false;
        };
        let after = PropValue::Bool(!on);
        let step = self.active_sketch().assign_node_prop(id, prop, &after);
        if !self.active_sketch_mut().preview(step) {
            return false;
        }
        let step = self
            .active_sketch_mut()
            .record_node_props(vec![(id, prop, PropValue::Bool(on), after)]);
        self.active_sketch_mut().record(step)
    }

    pub fn toggle_color_flag(&mut self, canvas: NodeId, color: ColorId, prop: PropId) -> bool {
        if !matches!(prop, PropId::Visible | PropId::Locked) {
            return false;
        }
        if !self.active_sketch().can_edit(canvas, PropId::Visible) {
            return false;
        }
        let Some(PropValue::Bool(on)) = self.active_sketch().color_prop(canvas, color, prop) else {
            return false;
        };
        self.edit_color(canvas, color, prop, PropValue::Bool(!on), true)
    }

    pub fn reorder_node(&mut self, id: NodeId, to: usize) -> bool {
        self.finish_size_edit();
        let step = self.active_sketch_mut().reorder_sibling(id, to);
        self.active_sketch_mut().commit(step)
    }

    pub(crate) fn scene(&self) -> Vec<SceneMark> {
        let open = self.open.as_ref().filter(|open| !open.samples.is_empty());
        let live = open.and_then(|open| self.live_stroke(open));
        let mut marks = self.active_sketch().scene(live.as_ref());
        // A stroke that will make a new canvas paints above everything, as
        // that canvas will be the top of the sketch.
        if let Some(live) = live.as_ref().filter(|live| live.canvas.is_none())
            && let Ok(mesh) = ink::mesh_from_shapes(live.shapes.clone())
        {
            marks.push(SceneMark::Ink(InkPaint {
                rgb: live.look.0,
                opacity: live.look.1,
                eraser_preview: false,
                triangles: mesh.triangles,
            }));
        }
        if let Some(trail) = open.and_then(|open| self.eraser_trail(open)) {
            marks.push(SceneMark::Ink(trail));
        }
        for paint in self.shape_preview() {
            marks.push(SceneMark::Ink(paint));
        }
        marks
    }

    pub fn paints(&self) -> Vec<InkPaint> {
        self.scene()
            .into_iter()
            .filter_map(|mark| match mark {
                SceneMark::Ink(paint) => Some(paint),
                SceneMark::Text(_) => None,
            })
            .collect()
    }

    fn shape_preview(&self) -> Vec<InkPaint> {
        let Some(OpenGesture {
            kind:
                GestureKind::Shape {
                    tool,
                    origin,
                    pointer,
                    shift,
                },
            ..
        }) = self.open.as_ref()
        else {
            return Vec::new();
        };
        let Some(settings) = self.shape_tool(*tool) else {
            return Vec::new();
        };
        let Some(kind) = shape_kind(*tool) else {
            return Vec::new();
        };
        let Some(placed) = resolve_drag(
            kind,
            *origin,
            *pointer,
            settings.constrain ^ shift,
            settings.snap,
        ) else {
            return Vec::new();
        };
        let palette = settings
            .palette
            .iter()
            .map(|item| {
                let mut color = item.color.clone();
                if color.role.has_stroke() {
                    color.size = item.mark_size();
                }
                color
            })
            .collect();
        preview_shape_paints(kind, placed, palette, settings.corner_radius)
    }

    fn commit_shape(&mut self, tool: Tool, origin: Point, pointer: Point, shift: bool) {
        let Some(settings) = self.shape_tool(tool).cloned() else {
            return;
        };
        let Some(kind) = shape_kind(tool) else {
            return;
        };
        let Some(placed) = resolve_drag(
            kind,
            origin,
            pointer,
            settings.constrain ^ shift,
            settings.snap,
        ) else {
            return;
        };
        let palette = object_palette(&settings.palette, &mut self.ids);
        let index = self.active;
        let made = self.sketches[index].add_shape(
            &mut self.ids,
            kind,
            placed,
            palette,
            settings.corner_radius,
        );
        self.sketches[index].commit(made.map(|(_, step)| step));
    }

    /// The open brush stroke, in its canvas's space and outlined as it will be
    /// committed, so it paints in its color layer while it is drawn.
    fn live_stroke(&self, open: &OpenGesture) -> Option<LiveStroke> {
        let GestureKind::Brush {
            canvas,
            source,
            layer,
            size,
            dynamics,
        } = &open.kind
        else {
            return None;
        };
        let sketch = self.active_sketch();
        let (style, layer) = match layer
            .zip(*canvas)
            .and_then(|(layer, canvas)| Some((sketch.color_style(canvas, layer)?, layer)))
        {
            Some((style, layer)) => (
                (
                    style.size,
                    style.cap,
                    style.dash,
                    (style.rgb, style.opacity),
                ),
                Some(layer),
            ),
            None => {
                let item = self.tool_color(Tool::Brush, *source)?;
                let color = &item.color;
                (
                    (
                        color.size,
                        color.cap,
                        color.dash,
                        (color.rgb, color.opacity),
                    ),
                    None,
                )
            }
        };
        let (layer_size, cap, dash, look) = style;
        let transform = canvas
            .and_then(|canvas| sketch.world_transform(canvas))
            .unwrap_or(kurbo::Affine::IDENTITY);
        let samples = map_samples(transform, &open.samples);
        let shapes = ink::stroke_visible(
            &samples,
            dynamics,
            &[],
            f64::from(size.unwrap_or(layer_size)),
            cap,
            dash,
        )?;
        Some(LiveStroke {
            canvas: *canvas,
            color: layer,
            look,
            shapes,
        })
    }

    /// The eraser's path while it is dragged, a translucent trail over the ink.
    fn eraser_trail(&self, open: &OpenGesture) -> Option<InkPaint> {
        let GestureKind::Erase { size, dynamics, .. } = &open.kind else {
            return None;
        };
        let shapes = ink::stroke_visible(
            &open.samples,
            dynamics,
            &[],
            *size as f64,
            Cap::Round,
            Dash::Solid,
        )?;
        let mesh = ink::mesh_from_shapes(shapes).ok()?;
        if mesh.triangles.is_empty() {
            return None;
        }
        Some(InkPaint {
            rgb: [90, 90, 90],
            opacity: 1.0,
            eraser_preview: true,
            triangles: mesh.triangles,
        })
    }

    fn commit_erase(
        &mut self,
        canvas: NodeId,
        size: u8,
        dynamics: Dynamics,
        samples: Vec<Sample>,
    ) -> Result<(), CommandError> {
        let transform = self
            .active_sketch()
            .world_transform(canvas)
            .unwrap_or(kurbo::Affine::IDENTITY);
        let path = ErasePath::new(map_samples(transform, &samples), size, dynamics);
        let outline = ink::eraser_outline(&path);
        if outline.len() < 3 {
            return Ok(());
        }
        let hits = self.active_sketch().strokes_hit_by_eraser(canvas, &outline);
        if hits.is_empty() {
            return Ok(());
        }
        let added = hits.into_iter().map(|id| (id, path.clone())).collect();
        self.active_sketch_mut()
            .commit_step(Step::AddErases { added });
        self.active_sketch_mut().set_selection([canvas]);
        Ok(())
    }

    /// Drops an in-progress color preview and puts the value back. The drag
    /// was not a history step. Later samples from that same drag do not
    /// become one either.
    fn abandon_color_drag(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        self.color_drag_abandoned = true;
        self.restore_pending(&pending);
    }

    fn restore_pending(&mut self, pending: &PendingProp) {
        match pending {
            PendingProp::Color {
                sketch,
                node,
                color,
                id,
                before,
            } => {
                if let Some(sketch) = self.sketches.iter_mut().find(|item| item.id() == *sketch) {
                    let step = sketch.set_color_prop(*node, *color, *id, before);
                    sketch.preview(step);
                }
            }
            PendingProp::Nodes { sketch, id, before } => {
                if let Some(sketch) = self.sketches.iter_mut().find(|item| item.id() == *sketch) {
                    for (node, value) in before {
                        let step = sketch.assign_node_prop(*node, *id, value);
                        sketch.preview(step);
                    }
                }
            }
        }
    }

    fn finish_color_pending(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        match pending {
            PendingProp::Color {
                sketch,
                node,
                color,
                id,
                before,
            } => {
                let Some(after) = self
                    .sketches
                    .iter()
                    .find(|item| item.id() == sketch)
                    .and_then(|item| item.color_prop(node, color, id))
                else {
                    return;
                };
                if after == before {
                    return;
                }
                let Some(sketch) = self.sketches.iter_mut().find(|item| item.id() == sketch) else {
                    return;
                };
                sketch.commit_step(Step::SetProps {
                    node,
                    color,
                    before: vec![(id, before)],
                    after: vec![(id, after)],
                });
            }
            PendingProp::Nodes { sketch, id, before } => {
                let Some(current) = self.sketches.iter().find(|item| item.id() == sketch) else {
                    return;
                };
                let edits: Vec<_> = before
                    .iter()
                    .filter_map(|(node, previous)| {
                        let after = current.node_prop(*node, id)?;
                        Some((*node, id, previous.clone(), after))
                    })
                    .collect();
                if let Some(sketch) = self.sketches.iter_mut().find(|item| item.id() == sketch) {
                    let step = sketch.record_node_props(edits);
                    sketch.record(step);
                }
            }
        }
    }

    fn edit_color(
        &mut self,
        node: NodeId,
        color: ColorId,
        id: PropId,
        value: PropValue,
        finished: bool,
    ) -> bool {
        if self.color_drag_abandoned && !finished {
            return false;
        }
        if self.tool_pending {
            self.prefs_dirty = true;
            self.tool_pending = false;
        }
        let sketch_id = self.active_id();
        let different = self.pending.as_ref().is_some_and(|pending| {
            !matches!(
                pending,
                PendingProp::Color {
                    sketch,
                    node: pending_node,
                    color: pending_color,
                    id: pending_id,
                    ..
                } if *sketch == sketch_id
                    && *pending_node == node
                    && *pending_color == color
                    && *pending_id == id
            )
        });
        if different {
            self.finish_color_pending();
        }
        if self.pending.is_none() {
            if !self.active_sketch().can_edit(node, PropId::Visible) {
                return false;
            }
            let Some(before) = self.active_sketch().color_prop(node, color, id) else {
                return false;
            };
            self.pending = Some(PendingProp::Color {
                sketch: sketch_id,
                node,
                color,
                id,
                before,
            });
        }
        let step = self.active_sketch().set_color_prop(node, color, id, &value);
        let ok = self.active_sketch_mut().preview(step);
        if finished {
            self.finish_color_pending();
        }
        ok
    }

    fn edit_nodes(&mut self, id: PropId, value: PropValue, finished: bool) -> bool {
        if self.color_drag_abandoned && !finished {
            return false;
        }
        let sketch_id = self.active_id();
        let targets: Vec<NodeId> = self
            .nodes_with_prop(id)
            .into_iter()
            .filter(|node| self.active_sketch().can_edit(*node, id))
            .collect();
        if targets.is_empty() {
            return false;
        }
        let different = self.pending.as_ref().is_some_and(|pending| {
            !matches!(
                pending,
                PendingProp::Nodes {
                    sketch,
                    id: pending_id,
                    ..
                } if *sketch == sketch_id && *pending_id == id
            )
        });
        if different {
            self.finish_color_pending();
        }
        if self.pending.is_none() {
            let before: Vec<_> = targets
                .iter()
                .filter_map(|node| {
                    self.active_sketch()
                        .node_prop(*node, id)
                        .map(|value| (*node, value))
                })
                .collect();
            if before.is_empty() {
                return false;
            }
            self.pending = Some(PendingProp::Nodes {
                sketch: sketch_id,
                id,
                before,
            });
        }
        let mut ok = false;
        for node in &targets {
            let step = self.active_sketch().assign_node_prop(*node, id, &value);
            ok |= self.active_sketch_mut().preview(step);
        }
        if finished {
            self.finish_color_pending();
        }
        ok
    }

    /// Copies a canvas's layers into the brush's palette, so canvases made
    /// later start with them, and links that canvas's layers to the new brush
    /// colors. A preference, not an undo step.
    pub fn make_palette_default(&mut self, canvas: NodeId) -> bool {
        self.finish_size_edit();
        let Some(colors) = self.active_sketch().layer_colors(canvas) else {
            return false;
        };
        if colors.is_empty() {
            return false;
        }
        let active_layer = self
            .active_sketch()
            .layer_for_source(canvas, self.brush_active);
        let palette: Vec<ToolColor> = colors
            .iter()
            .map(|color| ToolColor::new(color.copied(&mut self.ids)))
            .collect();
        let active = colors
            .iter()
            .position(|color| Some(color.id) == active_layer)
            .unwrap_or(0);
        let sources: Vec<ColorId> = palette.iter().map(ToolColor::id).collect();
        self.brush_active = sources[active];
        self.brush_palette = palette;
        self.active_sketch_mut().link_layers(canvas, &sources);
        self.prefs_dirty = true;
        true
    }

    /// Renames a palette color as one undo step. A blank name is refused.
    pub fn rename_color(&mut self, canvas: NodeId, color: ColorId, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        self.edit_color(
            canvas,
            color,
            PropId::Name,
            PropValue::Text(name.to_string()),
            true,
        )
    }

    fn tool_property(&self, tool: Tool, id: PropId) -> Option<PropValue> {
        match id {
            PropId::Preset if matches!(tool, Tool::Brush | Tool::Eraser) => {
                Some(PropValue::Index(self.preset_index(tool)))
            }
            PropId::EraserSize if tool == Tool::Eraser => {
                Some(PropValue::Number(f64::from(self.eraser_size)))
            }
            _ => match tool {
                Tool::Brush => self.brush.read(id),
                Tool::Eraser => self.eraser.read(id),
                Tool::Rectangle | Tool::Ellipse | Tool::Line => {
                    self.shape_tool(tool).and_then(|shape| shape.read(id))
                }
                Tool::Text => self.text.read(id),
                Tool::Select => None,
            },
        }
    }

    fn preset_index(&self, tool: Tool) -> u8 {
        match tool {
            Tool::Brush if self.brush == Dynamics::round() => 0,
            Tool::Brush if self.brush == Dynamics::monoline() => 1,
            Tool::Eraser
                if self.eraser == Dynamics::round()
                    && self.eraser_size == defaults::ERASER_SIZE =>
            {
                0
            }
            _ => schema::CUSTOM_PRESET,
        }
    }

    fn preview_tool(&mut self, tool: Tool, id: PropId, value: PropValue) -> bool {
        self.finish_color_pending();
        match self.write_tool(tool, id, &value) {
            Write::Rejected => false,
            Write::Unchanged => true,
            Write::Changed => {
                self.tool_pending = true;
                true
            }
        }
    }

    fn write_tool(&mut self, tool: Tool, id: PropId, value: &PropValue) -> Write {
        if matches!(tool, Tool::Rectangle | Tool::Ellipse | Tool::Line) {
            return self.write_shape_tool(tool, id, value);
        }
        if tool == Tool::Text {
            return self.write_text_tool(id, value);
        }
        if !matches!(tool, Tool::Brush | Tool::Eraser) {
            return Write::Rejected;
        }
        match id {
            PropId::Preset => self.write_preset(tool, value),
            PropId::EraserSize if tool == Tool::Eraser => {
                let mut size = BoundedSize::new(self.eraser_size, SIZE_MIN, SIZE_MAX);
                if !size.write(id, value) {
                    return Write::Rejected;
                }
                if size.value == self.eraser_size {
                    Write::Unchanged
                } else {
                    self.eraser_size = size.value;
                    Write::Changed
                }
            }
            PropId::EraserSize => Write::Rejected,
            _ => {
                let dynamics = match tool {
                    Tool::Brush => &mut self.brush,
                    Tool::Eraser => &mut self.eraser,
                    _ => return Write::Rejected,
                };
                let before = dynamics.clone();
                if !dynamics.write(id, value) {
                    return Write::Rejected;
                }
                if *dynamics == before {
                    Write::Unchanged
                } else {
                    Write::Changed
                }
            }
        }
    }

    fn write_preset(&mut self, tool: Tool, value: &PropValue) -> Write {
        let PropValue::Index(index) = value else {
            return Write::Rejected;
        };
        match tool {
            Tool::Brush => {
                let next = match index {
                    0 => Dynamics::round(),
                    1 => Dynamics::monoline(),
                    _ => return Write::Rejected,
                };
                if self.brush == next {
                    Write::Unchanged
                } else {
                    self.brush = next;
                    Write::Changed
                }
            }
            Tool::Eraser => {
                if *index != 0 {
                    return Write::Rejected;
                }
                let next = Dynamics::round();
                if self.eraser == next && self.eraser_size == defaults::ERASER_SIZE {
                    Write::Unchanged
                } else {
                    self.eraser = next;
                    self.eraser_size = defaults::ERASER_SIZE;
                    Write::Changed
                }
            }
            _ => Write::Rejected,
        }
    }

    fn write_text_tool(&mut self, id: PropId, value: &PropValue) -> Write {
        let before = self.text.clone();
        if !self.text.write(id, value) {
            return Write::Rejected;
        }
        if self.text == before {
            Write::Unchanged
        } else {
            Write::Changed
        }
    }

    fn write_shape_tool(&mut self, tool: Tool, id: PropId, value: &PropValue) -> Write {
        let Some(shape) = self.shape_tool_mut(tool) else {
            return Write::Rejected;
        };
        let before = shape.clone();
        if !shape.write(id, value) {
            return Write::Rejected;
        }
        if *shape == before {
            Write::Unchanged
        } else {
            Write::Changed
        }
    }

    fn shape_tool(&self, tool: Tool) -> Option<&ShapeTool> {
        match tool {
            Tool::Rectangle => Some(&self.rectangle),
            Tool::Ellipse => Some(&self.ellipse),
            Tool::Line => Some(&self.line),
            _ => None,
        }
    }

    fn shape_tool_mut(&mut self, tool: Tool) -> Option<&mut ShapeTool> {
        match tool {
            Tool::Rectangle => Some(&mut self.rectangle),
            Tool::Ellipse => Some(&mut self.ellipse),
            Tool::Line => Some(&mut self.line),
            _ => None,
        }
    }

    fn set_stored_size(&mut self, slot: SizeSlot, size: u8, finished: bool) {
        self.finish_color_pending();
        let (current, min, max) = match slot {
            SizeSlot::Eraser => (self.eraser_size, SIZE_MIN, SIZE_MAX),
            SizeSlot::Font => (self.text.style.size, FONT_MIN, FONT_MAX),
        };
        let mut bounded = BoundedSize::new(current, min, max);
        if !bounded.write(PropId::EraserSize, &PropValue::Number(f64::from(size))) {
            return;
        }
        if bounded.value != current {
            match slot {
                SizeSlot::Eraser => self.eraser_size = bounded.value,
                SizeSlot::Font => self.text.style.size = bounded.value,
            }
            self.tool_pending = true;
        }
        if finished {
            self.finish_size_edit();
        }
    }
}

enum Write {
    Rejected,
    Unchanged,
    Changed,
}

enum SizeSlot {
    Eraser,
    Font,
}

fn clamp_stored_size(size: u8, min: u8, max: u8) -> u8 {
    let mut bounded = BoundedSize::new(min, min, max);
    let _ = bounded.write(PropId::EraserSize, &PropValue::Number(f64::from(size)));
    bounded.value
}

fn section_rows(ids: &[schema::SectionId], tool: Option<Tool>) -> Vec<(String, Vec<PropId>)> {
    ids.iter()
        .map(|id| {
            let section = schema::section(*id);
            let title = match (tool, id) {
                (Some(tool), schema::SectionId::Preset) => {
                    format!("{} {}", registry::tool(tool).label, section.title)
                }
                _ => section.title.to_string(),
            };
            (title, section.props.to_vec())
        })
        .collect()
}

fn object_sections(sketch: &Sketch, id: NodeId) -> Option<&'static [schema::SectionId]> {
    if sketch.is_text(id) {
        return Some(schema::TEXT_OBJECT_SECTIONS);
    }
    if sketch.is_canvas(id) {
        return Some(schema::CANVAS_OBJECT_SECTIONS);
    }
    match sketch.shape_kind(id)? {
        ShapeKind::Rectangle => Some(schema::RECTANGLE_OBJECT_SECTIONS),
        ShapeKind::Ellipse => Some(schema::ELLIPSE_OBJECT_SECTIONS),
        ShapeKind::Line => Some(schema::LINE_OBJECT_SECTIONS),
    }
}

fn shape_kind(tool: Tool) -> Option<ShapeKind> {
    match tool {
        Tool::Rectangle => Some(ShapeKind::Rectangle),
        Tool::Ellipse => Some(ShapeKind::Ellipse),
        Tool::Line => Some(ShapeKind::Line),
        _ => None,
    }
}

fn sanitize_text_tool(mut tool: TextTool, ids: &mut Allocator) -> TextTool {
    tool.style.size = clamp_stored_size(tool.style.size, FONT_MIN, FONT_MAX);
    let _ = tool.style.write(
        PropId::LineHeight,
        &PropValue::Number(tool.style.line_height),
    );
    let _ = tool.style.write(
        PropId::LetterSpacing,
        &PropValue::Number(tool.style.letter_spacing),
    );
    // A palette saved before text had a Border keeps its two colors.
    if palette_roles_match(&tool.palette, &[ColorRole::Text, ColorRole::Background]) {
        tool.palette.push(text_border(ids));
    }
    if !palette_roles_match(
        &tool.palette,
        &[ColorRole::Text, ColorRole::Background, ColorRole::Border],
    ) {
        tool.palette = text_palette(ids);
    }
    tool.palette = tool.palette.into_iter().map(ToolColor::sanitized).collect();
    tool
}

fn sanitize_shape_tool(mut tool: ShapeTool, kind: ShapeKind, ids: &mut Allocator) -> ShapeTool {
    let roles: &[ColorRole] = match kind {
        ShapeKind::Line => &[ColorRole::Stroke],
        ShapeKind::Rectangle | ShapeKind::Ellipse => &[ColorRole::Outline, ColorRole::Fill],
    };
    if !palette_roles_match(&tool.palette, roles) {
        tool.palette = shape_palette(kind, ids);
    }
    tool.palette = tool.palette.into_iter().map(ToolColor::sanitized).collect();
    let _ = tool.write(PropId::CornerRadius, &PropValue::Number(tool.corner_radius));
    tool
}

fn untitled_name(sketches: &[Record]) -> String {
    let mut number = 1u32;
    loop {
        let candidate = format!("Untitled {number}");
        if sketches.iter().all(|sketch| sketch.name() != candidate) {
            return candidate;
        }
        number += 1;
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

/// `name`, then the names after it: "Untitled 1" gives "Untitled 1",
/// "Untitled 2", and on; "Lighthouse" gives "Lighthouse", "Lighthouse 2".
fn free_names(name: &str) -> impl Iterator<Item = String> {
    let (stem, first) = match name.rsplit_once(' ') {
        Some((stem, number)) if !stem.is_empty() => match number.parse::<u32>() {
            Ok(number) => (stem.to_string(), Some(number)),
            Err(_) => (name.to_string(), None),
        },
        _ => (name.to_string(), None),
    };
    let start = first.unwrap_or(1);
    (start..start + 10_000).map(move |number| match first {
        None if number == 1 => stem.clone(),
        _ => format!("{stem} {number}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use draw::document::TextLook;
    use draw::ink::{point_in_ring, point_in_shapes};

    fn line(from: f64, to: f64, y: f64, step: f64, pressure: Option<f64>) -> Vec<Sample> {
        let mut samples = Vec::new();
        let mut x = from;
        while x < to {
            samples.push(Sample { x, y, pressure });
            x += step;
        }
        samples.push(Sample { x: to, y, pressure });
        samples
    }

    fn tab_names(session: &Session) -> Vec<String> {
        session
            .tabs()
            .iter()
            .map(|tab| tab.name.to_string())
            .collect()
    }

    fn commit_samples(session: &mut Session, samples: &[Sample]) {
        session.begin().unwrap();
        for sample in samples {
            session.add_sample(sample.clone()).unwrap();
        }
        session.commit().unwrap();
    }

    /// The highlighted canvas's layer at `index`.
    fn layer_at(session: &Session, index: usize) -> ColorId {
        let canvas = session.highlighted_id().unwrap();
        session.active_sketch().color_id_at(canvas, index).unwrap()
    }

    /// Opens a layer's color page, as clicking its row in the outliner does.
    fn open_layer(session: &mut Session, index: usize) -> ColorId {
        let canvas = session.highlighted_id().unwrap();
        let layer = layer_at(session, index);
        session.select_color(canvas, layer);
        layer
    }

    fn horizontal_diameter(outline: &[Point], x0: f64, x1: f64) -> f64 {
        let mut min_y = f64::INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        let mut count = 0;
        for point in outline {
            if point[0] >= x0 && point[0] <= x1 {
                min_y = min_y.min(point[1]);
                max_y = max_y.max(point[1]);
                count += 1;
            }
        }
        assert!(
            count >= 2,
            "outline had {count} points in the measured span"
        );
        max_y - min_y
    }

    fn dist_to_segment(point: Point, start: Point, end: Point) -> f64 {
        let side = [end[0] - start[0], end[1] - start[1]];
        let offset = [point[0] - start[0], point[1] - start[1]];
        let length_sq = side[0] * side[0] + side[1] * side[1];
        let t = if length_sq == 0.0 {
            0.0
        } else {
            ((offset[0] * side[0] + offset[1] * side[1]) / length_sq).clamp(0.0, 1.0)
        };
        let closest = [start[0] + side[0] * t, start[1] + side[1] * t];
        let dx = point[0] - closest[0];
        let dy = point[1] - closest[1];
        (dx * dx + dy * dy).sqrt()
    }

    fn in_centerline_capsule(point: Point, samples: &[Sample], radius: f64) -> bool {
        if samples.len() < 2 {
            return false;
        }
        samples.windows(2).any(|pair| {
            dist_to_segment(point, [pair[0].x, pair[0].y], [pair[1].x, pair[1].y]) <= radius
        })
    }

    /// One long stroke that scribbles back and forth inside a 200-unit box and
    /// crosses itself many times. Seeded, so every run draws the same stroke.
    fn self_overlapping_scribble(seed: u64) -> Vec<Sample> {
        let mut state = seed;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % 1_000_000) as f64 / 1_000_000.0
        };
        let mut samples = Vec::new();
        let (mut x, mut y) = (100.0, 100.0);
        for _ in 0..40 {
            let (to_x, to_y) = (next() * 200.0, next() * 200.0);
            let steps = 4 + (next() * 25.0) as usize;
            for step in 1..=steps {
                let t = step as f64 / steps as f64;
                samples.push(Sample {
                    x: x + (to_x - x) * t,
                    y: y + (to_y - y) * t,
                    pressure: None,
                });
            }
            (x, y) = (to_x, to_y);
        }
        samples
    }

    fn in_triangle(point: Point, triangle: &[Point; 3]) -> bool {
        let side = |a: Point, b: Point| {
            (b[0] - a[0]) * (point[1] - a[1]) - (b[1] - a[1]) * (point[0] - a[0])
        };
        let sides = [
            side(triangle[0], triangle[1]),
            side(triangle[1], triangle[2]),
            side(triangle[2], triangle[0]),
        ];
        sides.iter().all(|s| *s >= 0.0) || sides.iter().all(|s| *s <= 0.0)
    }

    #[test]
    fn self_overlapping_stroke_paints_only_near_its_path() {
        const REACH: f64 = 12.0;
        const MIN: f64 = -20.0;
        const CELLS: usize = 240;
        let cell = |value: f64| ((value - MIN).floor().max(0.0) as usize).min(CELLS - 1);
        let center = |index: usize| MIN + index as f64 + 0.5;

        for seed in [
            0x9E37_79B9_7F4A_7C15,
            0x2545_F491_4F6C_DD1D,
            0xD1B5_4A32_D192_ED03,
        ] {
            let samples = self_overlapping_scribble(seed);
            let mut session = Session::new();
            commit_samples(&mut session, &samples);

            let mut near = vec![false; CELLS * CELLS];
            for pair in samples.windows(2) {
                let (a, b) = ([pair[0].x, pair[0].y], [pair[1].x, pair[1].y]);
                for row in cell(a[1].min(b[1]) - REACH)..=cell(a[1].max(b[1]) + REACH) {
                    for column in cell(a[0].min(b[0]) - REACH)..=cell(a[0].max(b[0]) + REACH) {
                        if dist_to_segment([center(column), center(row)], a, b) <= REACH {
                            near[row * CELLS + column] = true;
                        }
                    }
                }
            }

            let paints = session.paints();
            assert_eq!(
                paints.len(),
                1,
                "seed {seed:#x}: the stroke painted nothing"
            );
            let mut stray = 0;
            for triangle in &paints[0].triangles {
                let low = triangle
                    .iter()
                    .fold([f64::MAX; 2], |m, p| [m[0].min(p[0]), m[1].min(p[1])]);
                let high = triangle
                    .iter()
                    .fold([f64::MIN; 2], |m, p| [m[0].max(p[0]), m[1].max(p[1])]);
                assert!(
                    low[0] >= MIN
                        && low[1] >= MIN
                        && high[0] < MIN + CELLS as f64
                        && high[1] < MIN + CELLS as f64,
                    "seed {seed:#x}: a triangle reaches outside the scribble box"
                );
                for row in cell(low[1])..=cell(high[1]) {
                    for column in cell(low[0])..=cell(high[0]) {
                        if !near[row * CELLS + column]
                            && in_triangle([center(column), center(row)], triangle)
                        {
                            stray += 1;
                        }
                    }
                }
            }
            assert_eq!(
                stray, 0,
                "seed {seed:#x}: {stray} painted cells lie more than {REACH} units from the stroke"
            );
        }
    }

    #[test]
    fn brush_diameter_follows_the_linear_pressure_curve() {
        let mut session = Session::new();
        session.set_size(16).unwrap();
        let full = line(0.0, 360.0, 0.0, 8.0, Some(1.0));
        let light = line(0.0, 360.0, 80.0, 8.0, Some(0.25));
        let missing = line(0.0, 360.0, 160.0, 8.0, None);
        commit_samples(&mut session, &full);
        commit_samples(&mut session, &light);
        commit_samples(&mut session, &missing);

        let ids = session.stroke_ids();
        let full_width = horizontal_diameter(&session.ink_outline(ids[0]).unwrap(), 120.0, 240.0);
        let light_width = horizontal_diameter(&session.ink_outline(ids[1]).unwrap(), 120.0, 240.0);
        let missing_width =
            horizontal_diameter(&session.ink_outline(ids[2]).unwrap(), 120.0, 240.0);

        assert!(
            (full_width - 16.0).abs() < 0.75,
            "full pressure diameter {full_width}"
        );
        assert!(
            (missing_width - 16.0).abs() < 0.75,
            "missing pressure diameter {missing_width}"
        );
        assert!(
            (light_width - 4.0).abs() < 0.75,
            "light pressure diameter {light_width}"
        );
        assert!(light_width < full_width * 0.6);

        session.set_tool(Tool::Eraser);
        session.set_size(16).unwrap();
        commit_samples(&mut session, &line(0.0, 360.0, 0.0, 8.0, Some(0.25)));
        let eraser_width =
            horizontal_diameter(&session.erase_outline(ids[0], 0).unwrap(), 120.0, 240.0);
        assert!(
            (eraser_width - 4.0).abs() < 0.75,
            "eraser diameter {eraser_width} ignored the linear curve"
        );
    }

    #[test]
    fn outline_leaves_samples_and_id_unchanged() {
        let mut session = Session::new();
        let samples = line(10.0, 80.0, 12.0, 5.0, Some(0.4));
        commit_samples(&mut session, &samples);
        let id = session.stroke_ids()[0];
        let _ = session.ink_outline(id).unwrap();
        let _ = session.visible_polygons(id).unwrap();
        assert_eq!(session.stroke_samples(id), Some(samples.as_slice()));
        let dynamics = session.stroke_dynamics(id).unwrap();
        assert!(dynamics.pressure_size);
        assert_eq!(dynamics.smoothing, 50);
        assert_eq!(dynamics.stabilization, 0);
        assert_eq!(dynamics.streamline, 50);
        assert_eq!(dynamics.curve.len(), 2);
        assert_eq!(session.stroke_ids()[0], id);
    }

    #[test]
    fn erase_hides_ink_inside_the_outline_and_undo_restores_it() {
        let mut session = Session::new();
        session.set_size(32).unwrap();
        commit_samples(&mut session, &line(0.0, 400.0, 0.0, 8.0, Some(1.0)));
        let id = session.stroke_ids()[0];
        let samples_before = session.stroke_samples(id).unwrap().to_vec();

        session.set_tool(Tool::Eraser);
        session.set_size(8).unwrap();
        let mut eraser = Vec::new();
        let mut x = 60.0;
        while x <= 110.0 {
            eraser.push(Sample {
                x,
                y: 0.0,
                pressure: Some(1.0),
            });
            x += 10.0;
        }
        for y in [0.0, 5.0, 10.0, 5.0, 0.0] {
            eraser.push(Sample {
                x: 120.0,
                y,
                pressure: Some(1.0),
            });
        }
        x = 130.0;
        while x <= 180.0 {
            eraser.push(Sample {
                x,
                y: 0.0,
                pressure: Some(1.0),
            });
            x += 10.0;
        }
        commit_samples(&mut session, &eraser);

        // A stroke and an erase. The Size slider is a tool setting.
        assert_eq!(session.undo_len(), 2);
        assert_eq!(session.stroke_samples(id), Some(samples_before.as_slice()));
        assert_eq!(session.stroke_ids()[0], id);
        assert_eq!(session.erase_paths(id).unwrap().len(), 1);

        let ink = session.ink_outline(id).unwrap();
        let erase_ring = session.erase_outline(id, 0).unwrap();
        let polygons = session.visible_polygons(id).unwrap();
        let path = &session.erase_paths(id).unwrap()[0];
        let radius = path.size() as f64 / 2.0;
        let mut hidden = None;
        let mut kept = None;
        let mut hidden_count = 0;
        let mut kept_count = 0;
        let mut x = 80.0;
        while x <= 160.0 {
            let mut y = -14.0;
            while y <= 14.0 {
                let point = [x, y];
                if point_in_ring(point, &ink) {
                    let in_outline = point_in_ring(point, &erase_ring);
                    let in_capsule = in_centerline_capsule(point, path.samples(), radius);
                    let visible = point_in_shapes(point, &polygons);
                    if in_outline && !visible {
                        hidden_count += 1;
                        hidden.get_or_insert(point);
                    }
                    if in_capsule && !in_outline && visible {
                        kept_count += 1;
                        kept.get_or_insert(point);
                    }
                }
                y += 1.0;
            }
            x += 1.0;
        }
        assert!(
            hidden.is_some() && kept.is_some(),
            "hidden {hidden_count} at {hidden:?}, centerline-only {kept_count} at {kept:?}"
        );
        let hidden = hidden.unwrap();
        let kept = kept.unwrap();
        assert!(!session.visible_at(id, hidden));
        assert!(session.visible_at(id, kept));

        session.set_tool(Tool::Brush);
        commit_samples(&mut session, &line(90.0, 150.0, 40.0, 5.0, Some(1.0)));
        let later = session
            .stroke_ids()
            .into_iter()
            .find(|stroke| *stroke != id)
            .unwrap();
        assert!(session.erase_paths(later).unwrap().is_empty());
        assert!(session.visible_at(later, [120.0, 40.0]));
        assert!(!session.visible_at(id, hidden));

        session.undo().unwrap();
        assert!(session.stroke_samples(later).is_none());
        session.undo().unwrap();
        assert!(session.visible_at(id, hidden));
        assert!(session.erase_paths(id).unwrap().is_empty());
        session.redo().unwrap();
        assert!(!session.visible_at(id, hidden));

        let canvas = session.highlighted_id().unwrap();
        assert!(session.translate_node(canvas, [30.0, -12.0]));
        assert_eq!(session.undo_len(), 2);
        assert_eq!(session.stroke_samples(id), Some(samples_before.as_slice()));
        let moved_hidden = [hidden[0] + 30.0, hidden[1] - 12.0];
        let moved_kept = [kept[0] + 30.0, kept[1] - 12.0];
        assert!(!session.visible_at(id, moved_hidden));
        assert!(session.visible_at(id, moved_kept));

        session.undo().unwrap();
        assert!(session.visible_at(id, moved_hidden));
        assert!(session.erase_paths(id).unwrap().is_empty());
    }

    #[test]
    fn fully_hidden_stroke_remains_and_fit_matches_empty() {
        let viewport = [800.0, 600.0];
        let mut session = Session::new();
        session.set_size(6).unwrap();
        commit_samples(&mut session, &line(0.0, 200.0, 0.0, 4.0, Some(1.0)));
        let id = session.stroke_ids()[0];
        let canvas = session.highlighted_id().unwrap();
        session.set_tool(Tool::Eraser);
        session.set_size(32).unwrap();
        commit_samples(&mut session, &line(-40.0, 240.0, 0.0, 4.0, Some(1.0)));

        assert_eq!(session.stroke_count(), 1);
        assert_eq!(session.stroke_ids()[0], id);
        let mut x = 0.0;
        while x <= 200.0 {
            assert!(!session.visible_at(id, [x, 0.0]), "ink remained at {x}");
            assert!(session.hit_test([x, 0.0]).is_none());
            x += 20.0;
        }

        let undo_len = session.undo_len();
        let redo_len = session.redo_len();
        session.fit(viewport);
        assert_eq!(session.undo_len(), undo_len);
        assert_eq!(session.redo_len(), redo_len);
        assert_eq!(session.stroke_count(), 1);

        let mut empty = Session::new();
        empty.fit(viewport);
        assert_eq!(session.zoom(), empty.zoom());
        assert_eq!(session.zoom(), 1.0);
        let origin = session.camera_origin();
        assert!((origin[0] - empty.camera_origin()[0]).abs() < 1e-9);
        assert!((origin[1] - empty.camera_origin()[1]).abs() < 1e-9);
        assert!((origin[0] - (-400.0)).abs() < 1e-9);
        assert!((origin[1] - (-300.0)).abs() < 1e-9);

        session.undo().unwrap();
        assert!(session.visible_at(id, [100.0, 0.0]));
        assert_eq!(session.hit_test([100.0, 0.0]), Some(canvas));
    }

    #[test]
    fn camera_does_not_bake_samples_and_zoom_clamps() {
        let viewport = [800.0, 600.0];
        let mut empty = Session::new();
        empty.fit(viewport);
        assert_eq!(empty.zoom(), 1.0);
        assert!((empty.camera_origin()[0] - (-400.0)).abs() < 1e-9);
        assert!((empty.camera_origin()[1] - (-300.0)).abs() < 1e-9);

        let mut session = Session::new();
        session.set_size(8).unwrap();
        let samples = line(0.0, 120.0, 4.0, 10.0, Some(1.0));
        commit_samples(&mut session, &samples);
        let id = session.stroke_ids()[0];

        session.pan_screen([40.0, -25.0]);
        assert_ne!(session.camera_origin(), [0.0, 0.0]);
        session.zoom_at_center(true, viewport);
        assert!((session.zoom() - 1.25).abs() < 1e-12);
        session.zoom_at_center(false, viewport);
        session.fit(viewport);

        assert_eq!(session.stroke_samples(id), Some(samples.as_slice()));
        assert_eq!(session.size(), 8);
        assert_eq!(session.undo_len(), 1);
        assert_eq!(session.redo_len(), 0);

        for _ in 0..30 {
            session.zoom_at_center(true, viewport);
        }
        assert_eq!(session.zoom(), 8.0);
        for _ in 0..40 {
            session.zoom_at_center(false, viewport);
        }
        assert_eq!(session.zoom(), 0.125);
        assert_eq!(session.stroke_samples(id), Some(samples.as_slice()));
        assert_eq!(session.size(), 8);
        assert_eq!(session.undo_len(), 1);
        assert_eq!(session.redo_len(), 0);
    }

    #[test]
    fn size_rejects_out_of_range_and_keeps_limits() {
        let mut session = Session::new();
        assert_eq!(session.size(), 3);
        assert_eq!(session.set_size(0), Err(CommandError::SizeOutOfRange));
        assert_eq!(session.size(), 3);
        assert_eq!(session.set_size(33), Err(CommandError::SizeOutOfRange));
        assert_eq!(session.size(), 3);

        session.set_size(1).unwrap();
        assert_eq!(session.size(), 1);
        assert_eq!(session.undo_len(), 0, "the Size slider is a tool setting");
        commit_samples(&mut session, &line(0.0, 20.0, 0.0, 10.0, None));
        assert_eq!(session.stroke_count(), 1);

        assert_eq!(session.set_size(33), Err(CommandError::SizeOutOfRange));
        assert_eq!(session.size(), 1);
        session.set_size(32).unwrap();
        assert_eq!(session.size(), 32);
        commit_samples(&mut session, &line(0.0, 40.0, 30.0, 10.0, None));
        assert_eq!(session.stroke_count(), 2);
        assert_eq!(session.undo_len(), 2);
    }

    #[test]
    fn short_pen_and_miss_erase_leave_history_unchanged() {
        let mut session = Session::new();
        session.begin().unwrap();
        session
            .add_sample(Sample {
                x: 1.0,
                y: 1.0,
                pressure: Some(1.0),
            })
            .unwrap();
        session.commit().unwrap();
        assert_eq!(session.stroke_count(), 0);
        assert_eq!(session.undo_len(), 0);
        assert_eq!(session.redo_len(), 0);
        assert!(!session.gesture_open());

        commit_samples(&mut session, &line(0.0, 40.0, 0.0, 10.0, None));
        assert_eq!(session.undo_len(), 1);
        let id = session.stroke_ids()[0];

        session.set_tool(Tool::Eraser);
        commit_samples(&mut session, &line(500.0, 540.0, 500.0, 10.0, Some(1.0)));
        assert_eq!(session.undo_len(), 1);
        assert_eq!(session.redo_len(), 0);
        assert!(session.erase_paths(id).unwrap().is_empty());
        assert_eq!(session.stroke_count(), 1);

        session.begin().unwrap();
        session
            .add_sample(Sample {
                x: 0.0,
                y: 0.0,
                pressure: Some(0.2),
            })
            .unwrap();
        session.commit().unwrap();
        assert_eq!(session.undo_len(), 1);
        assert!(session.erase_paths(id).unwrap().is_empty());
    }

    #[test]
    fn new_committed_gesture_clears_redo() {
        let mut session = Session::new();
        commit_samples(&mut session, &line(0.0, 30.0, 0.0, 10.0, Some(1.0)));
        let first = session
            .stroke_samples(session.stroke_ids()[0])
            .unwrap()
            .to_vec();
        session.undo().unwrap();
        assert_eq!(session.stroke_count(), 0);
        assert_eq!(session.redo_len(), 1);
        session.redo().unwrap();
        assert_eq!(
            session.stroke_samples(session.stroke_ids()[0]),
            Some(first.as_slice())
        );

        session.undo().unwrap();
        commit_samples(&mut session, &line(0.0, 30.0, 20.0, 10.0, Some(1.0)));
        assert_eq!(session.redo_len(), 0);
        assert!(session.redo().is_err());
        assert_eq!(session.stroke_count(), 1);
        assert_eq!(
            session.stroke_samples(session.stroke_ids()[0]).unwrap()[0].y,
            20.0
        );
        session.undo().unwrap();
        assert_eq!(session.stroke_count(), 0);
        assert_eq!(session.redo_len(), 1);
    }

    #[test]
    fn sketches_keep_separate_strokes_camera_and_history() {
        let mut session = Session::new();
        let first = session.active_id();
        commit_samples(&mut session, &line(0.0, 40.0, 0.0, 10.0, None));
        session.pan_screen([30.0, -10.0]);
        session.zoom_at_center(true, [800.0, 600.0]);
        let origin = session.camera_origin();
        let zoom = session.zoom();
        let samples = session
            .stroke_samples(session.stroke_ids()[0])
            .unwrap()
            .to_vec();
        assert_eq!(session.undo_len(), 1);
        assert_eq!(session.redo_len(), 0);

        let second = session.new_sketch();
        assert_ne!(second, first);
        assert_eq!(session.stroke_count(), 0);
        assert_eq!(session.undo_len(), 0);
        assert_eq!(session.redo_len(), 0);
        assert_eq!(session.zoom(), 1.0);
        assert_eq!(session.camera_origin(), [0.0, 0.0]);

        commit_samples(&mut session, &line(0.0, 40.0, 20.0, 10.0, None));
        session.undo().unwrap();
        assert_eq!(session.stroke_count(), 0);
        assert_eq!(session.redo_len(), 1);

        session.activate(first);
        assert_eq!(session.stroke_count(), 1);
        assert_eq!(
            session.stroke_samples(session.stroke_ids()[0]),
            Some(samples.as_slice())
        );
        assert_eq!(session.undo_len(), 1);
        assert_eq!(session.redo_len(), 0);
        assert_eq!(session.zoom(), zoom);
        assert_eq!(session.camera_origin(), origin);

        session.activate(second);
        assert_eq!(session.stroke_count(), 0);
        assert_eq!(session.redo_len(), 1);
        session.redo().unwrap();
        assert_eq!(
            session.stroke_samples(session.stroke_ids()[0]).unwrap()[0].y,
            20.0
        );

        session.activate(first);
        assert_eq!(
            session.stroke_samples(session.stroke_ids()[0]),
            Some(samples.as_slice())
        );
        assert_eq!(session.undo_len(), 1);
        assert_eq!(session.redo_len(), 0);
    }

    #[test]
    fn tool_settings_and_the_active_color_stay_across_tabs() {
        let mut session = Session::new();
        let first = session.active_id();
        session.set_tool(Tool::Eraser);
        session.set_size(12).unwrap();

        let second = session.new_sketch();
        assert_eq!(session.tool(), Tool::Eraser);
        assert_eq!(session.size(), 12);

        session.activate(first);
        assert_eq!(session.tool(), Tool::Eraser);
        assert_eq!(session.size(), 12);

        session.set_tool(Tool::Brush);
        let red = session.swatches()[1].id;
        session.set_active_color(red);
        session.set_size(20).unwrap();
        assert_eq!(session.size(), 20);

        // The brush's active color and its size are window settings.
        session.activate(second);
        assert_eq!(session.tool(), Tool::Brush);
        assert!(session.swatches()[1].active);
        assert_eq!(session.size(), 20);
        session.set_tool(Tool::Eraser);
        assert_eq!(session.size(), 12);

        session.activate(first);
        assert_eq!(session.tool(), Tool::Eraser);
        session.set_tool(Tool::Brush);
        assert_eq!(session.size(), 20);
        assert!(session.swatches()[1].active);
    }

    #[test]
    fn choosing_another_color_keeps_a_new_sketch_clean() {
        let mut session = Session::new();
        let first = session.active_id();
        let red = session.swatches()[1].id;
        session.set_active_color(red);
        assert!(session.swatches()[1].active);
        assert_eq!(session.undo_len(), 0);
        assert_eq!(session.request_close(first), CloseEffect::Unchanged);
        assert_eq!(session.sketch_count(), 1);
        assert_eq!(session.active_id(), first);
    }

    #[test]
    fn closing_sole_clean_sketch_leaves_it() {
        let mut session = Session::new();
        let first = session.active_id();
        assert_eq!(session.sketch_count(), 1);
        assert_eq!(session.canvas_name(), Some("Canvas 1"));
        assert_eq!(session.swatches().len(), 9);
        assert!(session.swatches()[0].active);
        assert_eq!(session.swatches()[0].rgb, [20, 20, 20]);
        assert_eq!(session.size(), 3);
        assert_eq!(session.stroke_count(), 0);
        assert_eq!(session.request_close(first), CloseEffect::Unchanged);
        assert_eq!(session.discard(first), CloseEffect::Unchanged);
        assert_eq!(session.sketch_count(), 1);
        assert_eq!(session.active_id(), first);
        assert_eq!(session.active_name(), "Untitled 1");

        // The window failure was an empty Untitled 2 left as the only tab.
        let second = session.new_sketch();
        assert_eq!(
            session.request_close(first),
            CloseEffect::Closed { replacement: false }
        );
        assert_eq!(session.sketch_count(), 1);
        assert_eq!(session.active_id(), second);
        assert_eq!(session.active_name(), "Untitled 2");
        assert_eq!(session.stroke_count(), 0);
        assert!(session.swatches()[0].active);
        assert_eq!(session.request_close(second), CloseEffect::Unchanged);
        assert_eq!(session.discard(second), CloseEffect::Unchanged);
        assert_eq!(session.sketch_count(), 1);
        assert_eq!(session.active_id(), second);
        assert_eq!(tab_names(&session), vec!["Untitled 2".to_string()]);
    }

    #[test]
    fn discarding_last_dirty_sketch_leaves_one_blank() {
        let mut session = Session::new();
        let id = session.active_id();
        commit_samples(&mut session, &line(0.0, 30.0, 0.0, 10.0, None));
        assert_eq!(session.request_close(id), CloseEffect::NeedsConfirm);
        assert_eq!(session.sketch_count(), 1);
        assert_eq!(session.stroke_count(), 1);
        assert_eq!(session.active_id(), id);

        assert_eq!(
            session.discard(id),
            CloseEffect::Closed { replacement: true }
        );
        assert_eq!(session.sketch_count(), 1);
        assert_ne!(session.active_id(), id);
        assert_eq!(session.stroke_count(), 0);
        assert_eq!(session.undo_len(), 0);
        assert_eq!(session.redo_len(), 0);
        assert_eq!(session.active_name(), "Untitled 1");
        assert_eq!(session.zoom(), 1.0);
        assert_eq!(session.camera_origin(), [0.0, 0.0]);
        assert!(session.swatches()[0].active);
        assert_eq!(session.size(), 3);
    }

    fn paint_bounds(paint: &InkPaint) -> (Point, Point) {
        paint.triangles.iter().flatten().fold(
            ([f64::MAX; 2], [f64::MIN; 2]),
            |(min, max), point| {
                (
                    [min[0].min(point[0]), min[1].min(point[1])],
                    [max[0].max(point[0]), max[1].max(point[1])],
                )
            },
        )
    }

    #[test]
    fn an_open_stroke_paints_in_its_color_layer() {
        let mut session = Session::new();
        let black = session.swatches()[0].rgb;
        let red_id = session.swatches()[1].id;
        let red = session.swatches()[1].rgb;
        commit_samples(&mut session, &line(0.0, 80.0, 0.0, 8.0, Some(1.0)));
        let canvas = session.highlighted_id().unwrap();
        assert!(session.translate_node(canvas, [40.0, 25.0]));

        session.set_active_color(red_id);
        session.begin().unwrap();
        for sample in line(0.0, 80.0, 0.0, 8.0, Some(1.0)) {
            session.add_sample(sample).unwrap();
        }
        let open = session.paints();
        let black_at = open.iter().position(|paint| paint.rgb == black).unwrap();
        let red_at = open.iter().position(|paint| paint.rgb == red).unwrap();
        assert!(
            red_at < black_at,
            "the open red stroke painted above the black layer"
        );
        let open_bounds = paint_bounds(&open[red_at]);

        session.commit().unwrap();
        let committed = session.paints();
        let red_at = committed.iter().position(|paint| paint.rgb == red).unwrap();
        let committed_bounds = paint_bounds(&committed[red_at]);
        for axis in 0..2 {
            assert!((open_bounds.0[axis] - committed_bounds.0[axis]).abs() < 1e-6);
            assert!((open_bounds.1[axis] - committed_bounds.1[axis]).abs() < 1e-6);
        }
    }

    #[test]
    fn earlier_palette_color_paints_above_a_later_one() {
        let mut session = Session::new();
        let black = session.swatches()[0].rgb;
        let red_id = session.swatches()[1].id;
        let red = session.swatches()[1].rgb;
        commit_samples(&mut session, &line(0.0, 80.0, 0.0, 8.0, Some(1.0)));
        let black_stroke = session.stroke_ids()[0];
        let black_samples = session.stroke_samples(black_stroke).unwrap().to_vec();
        session.set_active_color(red_id);
        commit_samples(&mut session, &line(0.0, 80.0, 0.0, 8.0, Some(1.0)));
        let red_stroke = session
            .stroke_ids()
            .into_iter()
            .find(|id| *id != black_stroke)
            .unwrap();
        let red_samples = session.stroke_samples(red_stroke).unwrap().to_vec();

        let paints = session.paints();
        let black_at = paints.iter().position(|paint| paint.rgb == black).unwrap();
        let red_at = paints.iter().position(|paint| paint.rgb == red).unwrap();
        assert!(black_at > red_at);

        let undo_len = session.undo_len();
        let red_layer = layer_at(&session, 1);
        session.move_color(red_layer, 0).unwrap();
        assert_eq!(session.undo_len(), undo_len + 1);
        assert_eq!(layer_at(&session, 0), red_layer);
        let paints = session.paints();
        let black_at = paints.iter().position(|paint| paint.rgb == black).unwrap();
        let red_at = paints.iter().position(|paint| paint.rgb == red).unwrap();
        assert!(red_at > black_at);
        assert_eq!(
            session.stroke_samples(black_stroke),
            Some(black_samples.as_slice())
        );
        assert_eq!(
            session.stroke_samples(red_stroke),
            Some(red_samples.as_slice())
        );

        session.undo().unwrap();
        let paints = session.paints();
        let black_at = paints.iter().position(|paint| paint.rgb == black).unwrap();
        let red_at = paints.iter().position(|paint| paint.rgb == red).unwrap();
        assert!(black_at > red_at);
    }

    #[test]
    fn editing_a_color_size_changes_the_outline_and_keeps_samples() {
        let mut session = Session::new();
        assert_eq!(session.size(), 3);
        let samples = line(0.0, 240.0, 0.0, 8.0, Some(1.0));
        commit_samples(&mut session, &samples);
        let id = session.stroke_ids()[0];
        let width = horizontal_diameter(&session.ink_outline(id).unwrap(), 80.0, 160.0);
        assert!((width - 3.0).abs() < 0.75, "size 3 diameter {width}");

        // The layer's size is edited on its page, as one step for the drag.
        open_layer(&mut session, 0);
        let undo_len = session.undo_len();
        for size in [8.0, 12.0, 16.0] {
            assert!(session.preview_panel_property(PropId::ColorSize, PropValue::Number(size)));
        }
        assert_eq!(session.undo_len(), undo_len);
        session.finish_size_edit();
        assert_eq!(session.undo_len(), undo_len + 1);
        assert_eq!(session.redo_len(), 0);
        assert_eq!(session.stroke_samples(id), Some(samples.as_slice()));
        assert_eq!(session.stroke_ids()[0], id);
        let width = horizontal_diameter(&session.ink_outline(id).unwrap(), 80.0, 160.0);
        assert!((width - 16.0).abs() < 0.75, "size 16 diameter {width}");

        session.undo().unwrap();
        assert_eq!(session.size(), 3);
        assert_eq!(session.stroke_samples(id), Some(samples.as_slice()));
        let width = horizontal_diameter(&session.ink_outline(id).unwrap(), 80.0, 160.0);
        assert!((width - 3.0).abs() < 0.75, "restored diameter {width}");

        session.redo().unwrap();
        assert_eq!(
            session.size(),
            16,
            "a locked brush color's slider shows its layer's size"
        );
        assert_eq!(session.stroke_samples(id), Some(samples.as_slice()));
    }

    #[test]
    fn a_color_edit_undoes_and_leaves_samples_unchanged() {
        let mut session = Session::new();
        session.set_size(10).unwrap();
        let samples = line(0.0, 200.0, 0.0, 4.0, Some(1.0));
        commit_samples(&mut session, &samples);
        let id = session.stroke_ids()[0];
        assert_eq!(session.visible_polygons(id).unwrap().len(), 1);

        open_layer(&mut session, 0);
        let undo_len = session.undo_len();
        assert!(session.set_panel_property(PropId::Dash, PropValue::Index(1)));
        assert_eq!(session.undo_len(), undo_len + 1);
        assert_eq!(session.stroke_samples(id), Some(samples.as_slice()));
        assert!(
            session.visible_polygons(id).unwrap().len() > 1,
            "a dashed stroke still drew as one piece"
        );

        session.undo().unwrap();
        assert_eq!(session.visible_polygons(id).unwrap().len(), 1);
        assert_eq!(session.stroke_samples(id), Some(samples.as_slice()));
        assert_eq!(session.stroke_ids()[0], id);

        session.redo().unwrap();
        assert!(session.visible_polygons(id).unwrap().len() > 1);
        assert_eq!(session.stroke_samples(id), Some(samples.as_slice()));
    }

    #[test]
    fn brush_and_eraser_settings_are_not_undo_steps() {
        let mut session = Session::new();
        commit_samples(&mut session, &line(0.0, 40.0, 0.0, 10.0, Some(1.0)));
        session.undo().unwrap();
        assert_eq!(session.redo_len(), 1);

        assert!(session.set_panel_property(PropId::Smoothing, PropValue::Number(20.0)));
        assert_eq!(session.undo_len(), 0);
        assert_eq!(session.redo_len(), 1);
        commit_samples(&mut session, &line(0.0, 80.0, 0.0, 8.0, Some(1.0)));
        let id = session.stroke_ids()[0];
        assert_eq!(session.stroke_dynamics(id).unwrap().smoothing, 20);
        let undo_len = session.undo_len();

        assert!(session.set_panel_property(PropId::Smoothing, PropValue::Number(80.0)));
        assert_eq!(session.undo_len(), undo_len);
        assert_eq!(session.redo_len(), 0);
        assert_eq!(session.stroke_dynamics(id).unwrap().smoothing, 20);

        session.undo().unwrap();
        assert!(session.stroke_samples(id).is_none());
        assert_eq!(
            session.panel_property(PropId::Smoothing),
            Some(PropValue::Number(80.0))
        );

        session.set_tool(Tool::Eraser);
        let undo_len = session.undo_len();
        let redo_len = session.redo_len();
        assert!(session.set_panel_property(PropId::EraserSize, PropValue::Number(14.0)));
        assert_eq!(session.undo_len(), undo_len);
        assert_eq!(session.redo_len(), redo_len);
        assert_eq!(session.size(), 14);
        session.redo().unwrap();
        assert_eq!(
            session
                .stroke_dynamics(session.stroke_ids()[0])
                .unwrap()
                .smoothing,
            20
        );
        assert_eq!(session.size(), 14);
    }

    #[test]
    fn an_out_of_range_value_is_clamped_and_the_rest_stays() {
        let mut session = Session::new();
        assert!(session.set_panel_property(PropId::Smoothing, PropValue::Number(240.0)));
        assert_eq!(
            session.panel_property(PropId::Smoothing),
            Some(PropValue::Number(100.0))
        );
        assert_eq!(
            session.panel_property(PropId::Streamline),
            Some(PropValue::Number(50.0))
        );
        assert_eq!(
            session.panel_property(PropId::PressureSize),
            Some(PropValue::Bool(true))
        );
        assert_eq!(session.undo_len(), 0);
        assert!(!session.set_panel_property(PropId::Smoothing, PropValue::Text("no".to_string())));
        assert_eq!(
            session.panel_property(PropId::Smoothing),
            Some(PropValue::Number(100.0))
        );
        assert!(!session.set_panel_property(PropId::Curve, PropValue::Curve(vec![(0.0, 0.0)])));
        match session.panel_property(PropId::Curve) {
            Some(PropValue::Curve(points)) => assert_eq!(points.len(), 2),
            other => panic!("curve was {other:?}"),
        }

        open_layer(&mut session, 0);
        let undo_len = session.undo_len();
        assert!(session.set_panel_property(PropId::Opacity, PropValue::Number(-0.4)));
        assert_eq!(
            session.panel_property(PropId::Opacity),
            Some(PropValue::Number(0.0))
        );
        assert_eq!(
            session.panel_property(PropId::ColorSize),
            Some(PropValue::Number(3.0))
        );
        assert_eq!(
            session.panel_property(PropId::Name),
            Some(PropValue::Text("Black".to_string()))
        );
        assert_eq!(session.undo_len(), undo_len + 1);
        assert!(!session.set_panel_property(PropId::Opacity, PropValue::Number(f64::NAN)));
        assert_eq!(
            session.panel_property(PropId::Opacity),
            Some(PropValue::Number(0.0))
        );
        assert_eq!(session.undo_len(), undo_len + 1);
        assert_eq!(
            session.panel_property(PropId::ColorSize),
            Some(PropValue::Number(3.0))
        );

        assert!(session.set_panel_property(PropId::ColorSize, PropValue::Number(100.0)));
        assert_eq!(
            session.panel_property(PropId::ColorSize),
            Some(PropValue::Number(32.0))
        );
        assert_eq!(
            session.panel_property(PropId::Opacity),
            Some(PropValue::Number(0.0))
        );
        assert_eq!(
            session.panel_property(PropId::Name),
            Some(PropValue::Text("Black".to_string()))
        );
    }

    #[test]
    fn deleting_the_last_color_does_nothing() {
        let mut session = Session::new();
        commit_samples(&mut session, &line(0.0, 30.0, 0.0, 10.0, None));
        let stroke = session.stroke_ids()[0];
        let samples = session.stroke_samples(stroke).unwrap().to_vec();
        let canvas = session.highlighted_id().unwrap();
        let layers = |session: &Session| session.active_sketch().palette_len(canvas).unwrap();
        while layers(&session) > 1 {
            let last = layers(&session) - 1;
            let id = session.active_sketch().color_id_at(canvas, last).unwrap();
            assert!(session.delete_layer(canvas, id));
        }
        let undo_len = session.undo_len();
        let only = session.active_sketch().color_id_at(canvas, 0).unwrap();
        assert!(!session.delete_layer(canvas, only));
        assert_eq!(layers(&session), 1);
        assert_eq!(session.undo_len(), undo_len);
        assert_eq!(session.redo_len(), 0);
        assert_eq!(session.stroke_count(), 1);
        assert_eq!(session.stroke_samples(stroke), Some(samples.as_slice()));
    }

    #[test]
    fn a_color_drag_does_not_commit_over_undo_or_redo() {
        let mut session = Session::new();
        commit_samples(&mut session, &line(0.0, 40.0, 0.0, 10.0, Some(1.0)));
        open_layer(&mut session, 0);
        assert!(session.preview_panel_property(PropId::Opacity, PropValue::Number(0.25)));
        assert_eq!(session.undo_len(), 1);

        session.undo().unwrap();
        assert_eq!(session.stroke_count(), 0);
        assert_eq!(session.redo_len(), 1);
        assert_eq!(session.undo_len(), 0);
        assert_eq!(
            session.panel_property(PropId::Opacity),
            Some(PropValue::Number(1.0))
        );
        assert!(!session.preview_panel_property(PropId::Opacity, PropValue::Number(0.1)));
        session.finish_size_edit();
        assert_eq!(session.redo_len(), 1);
        assert_eq!(session.undo_len(), 0);
        assert_eq!(
            session.panel_property(PropId::Opacity),
            Some(PropValue::Number(1.0))
        );

        // The Size slider is a tool setting: undo during its drag leaves it,
        // and its release records nothing.
        session.redo().unwrap();
        assert_eq!(session.stroke_count(), 1);
        assert!(session.preview_size(12).is_ok());
        assert_eq!(session.size(), 12);
        assert_eq!(session.undo_len(), 1);
        session.undo().unwrap();
        assert_eq!(session.stroke_count(), 0);
        assert_eq!(session.size(), 12);
        session.finish_size_edit();
        assert_eq!(session.size(), 12);
        assert_eq!(session.redo_len(), 1);
        assert_eq!(session.undo_len(), 0);
    }

    #[test]
    fn a_hidden_or_locked_layer_draws_nothing_and_keeps_the_brush() {
        let mut session = Session::new();
        open_layer(&mut session, 0);
        assert!(session.set_panel_property(PropId::Visible, PropValue::Bool(false)));
        assert!(session.begin().is_err(), "the hidden layer is not drawn on");
        assert_eq!(session.tool(), Tool::Brush);
        assert!(session.set_panel_property(PropId::Visible, PropValue::Bool(true)));
        assert!(session.set_panel_property(PropId::Locked, PropValue::Bool(true)));
        assert!(session.begin().is_err(), "nor is a locked one");
        assert!(session.set_panel_property(PropId::Locked, PropValue::Bool(false)));
        commit_samples(&mut session, &line(0.0, 40.0, 0.0, 10.0, None));
        assert_eq!(session.stroke_count(), 1);
    }

    fn translation(transform: Affine) -> [f64; 2] {
        let coeffs = transform.as_coeffs();
        [coeffs[4], coeffs[5]]
    }

    fn swatch_rgbs(session: &Session) -> Vec<[u8; 3]> {
        session.swatches().iter().map(|swatch| swatch.rgb).collect()
    }

    #[test]
    fn moving_a_canvas_changes_its_transform_and_undo_restores_it() {
        let mut session = Session::new();
        let samples = line(0.0, 80.0, 0.0, 4.0, Some(1.0));
        commit_samples(&mut session, &samples);
        let canvas = session.selection()[0];
        let stroke = session.stroke_ids()[0];
        let undo_len = session.undo_len();
        session.clear_selection();
        session.select_click(canvas, false);
        assert_eq!(session.undo_len(), undo_len, "selecting recorded history");
        assert_eq!(session.selection(), vec![canvas]);

        let before = session.changeable_locals();
        session.preview_world(&before, |world| {
            Affine::translate(kurbo::Vec2::new(40.0, -15.0)) * world
        });
        assert_eq!(
            session.undo_len(),
            undo_len,
            "a drag preview recorded history"
        );
        assert!(session.commit_preview(&before));
        assert_eq!(session.undo_len(), undo_len + 1);
        assert_eq!(session.redo_len(), 0);
        assert_eq!(session.stroke_samples(stroke), Some(samples.as_slice()));
        let moved = translation(session.changeable_locals()[0].1);
        assert!((moved[0] - 40.0).abs() < 1.0e-6 && (moved[1] + 15.0).abs() < 1.0e-6);
        assert_eq!(session.hit_test([12.0, 0.0]), None);
        assert_eq!(session.hit_test([52.0, -15.0]), Some(canvas));

        session.undo().unwrap();
        assert_eq!(translation(session.changeable_locals()[0].1), [0.0, 0.0]);
        assert_eq!(session.stroke_samples(stroke), Some(samples.as_slice()));
        assert_eq!(session.hit_test([12.0, 0.0]), Some(canvas));
        assert_eq!(session.redo_len(), 1);

        session.redo().unwrap();
        assert_eq!(session.hit_test([52.0, -15.0]), Some(canvas));
        assert_eq!(session.stroke_samples(stroke), Some(samples.as_slice()));
    }

    #[test]
    fn grouping_then_ungrouping_keeps_world_positions() {
        let mut session = Session::new();
        let first = session.selection()[0];
        session.perform(CommandId::NewCanvas, [1.0, 1.0]);
        let second = session.selection()[0];
        session.select_click(first, true);
        assert_eq!(session.selection().len(), 2);
        session.perform(CommandId::Group, [1.0, 1.0]);
        assert_eq!(session.selection().len(), 1);
        let before = session.changeable_locals();
        session.preview_world(&before, |world| {
            Affine::translate(kurbo::Vec2::new(30.0, -12.0)) * world
        });
        assert!(session.commit_preview(&before));
        session.perform(CommandId::Ungroup, [1.0, 1.0]);
        let locals = session.changeable_locals();
        assert_eq!(locals.len(), 2);
        assert!(locals.iter().any(|(id, _)| *id == first));
        assert!(locals.iter().any(|(id, _)| *id == second));
        for (_, transform) in locals {
            let moved = translation(transform);
            assert!((moved[0] - 30.0).abs() < 1.0e-6 && (moved[1] + 12.0).abs() < 1.0e-6);
        }
    }

    #[test]
    fn a_hidden_or_locked_node_is_not_picked() {
        let mut session = Session::new();
        commit_samples(&mut session, &line(0.0, 80.0, 0.0, 4.0, Some(1.0)));
        let canvas = session.selection()[0];
        let on_ink = [12.0, 0.0];
        assert_eq!(session.hit_test(on_ink), Some(canvas));

        assert!(session.toggle_node(canvas, PropId::Visible));
        assert_eq!(session.hit_test(on_ink), None);
        assert!(session.toggle_node(canvas, PropId::Visible));

        assert!(session.toggle_node(canvas, PropId::Locked));
        assert_eq!(session.hit_test(on_ink), None);
        let undo_len = session.undo_len();
        session.perform(CommandId::NudgeRight, [1.0, 1.0]);
        assert_eq!(session.undo_len(), undo_len);
        assert!(session.toggle_node(canvas, PropId::Locked));
        assert_eq!(translation(session.changeable_locals()[0].1), [0.0, 0.0]);

        session.perform(CommandId::NewCanvas, [1.0, 1.0]);
        session.select_click(canvas, true);
        session.perform(CommandId::Group, [1.0, 1.0]);
        let group = session.selection()[0];
        assert!(session.toggle_node(group, PropId::Visible));
        assert_eq!(session.hit_test(on_ink), None);
        assert!(session.toggle_node(group, PropId::Visible));
        assert!(session.toggle_node(group, PropId::Locked));
        assert_eq!(session.hit_test(on_ink), None);
    }

    #[test]
    fn with_no_canvas_the_brush_makes_one_and_the_eraser_erases_nothing() {
        let mut session = Session::new();
        session.perform(CommandId::Delete, [1.0, 1.0]);
        assert!(session.selection().is_empty());
        assert_eq!(session.tool(), Tool::Brush, "the brush stays chosen");
        assert_eq!(session.active_sketch().root_len(), 0);

        let undo_len = session.undo_len();
        commit_samples(&mut session, &line(0.0, 40.0, 0.0, 10.0, None));
        assert_eq!(session.active_sketch().root_len(), 1);
        let canvas = session
            .highlighted_id()
            .expect("the new canvas is highlighted");
        assert_eq!(
            session.active_sketch().node_prop(canvas, PropId::Name),
            Some(PropValue::Text("Canvas 1".into()))
        );
        assert_eq!(session.stroke_count(), 1);
        assert_eq!(
            session.undo_len(),
            undo_len + 1,
            "the canvas and stroke are one step"
        );
        session.undo().unwrap();
        assert_eq!(session.active_sketch().root_len(), 0);

        session.set_tool(Tool::Eraser);
        assert_eq!(session.tool(), Tool::Eraser);
        assert!(session.begin().is_err(), "nothing to erase");
        assert_eq!(session.undo_len(), undo_len);
    }

    #[test]
    fn a_new_canvas_copies_the_brush_palette_and_keeps_its_own() {
        let mut session = Session::new();
        let original = swatch_rgbs(&session);
        // Editing a layer leaves the brush's palette, and later canvases, alone.
        open_layer(&mut session, 0);
        assert!(session.set_panel_property(PropId::Rgb, PropValue::Rgb([10, 20, 30])));
        assert_eq!(swatch_rgbs(&session), original);
        session.perform(CommandId::NewCanvas, [1.0, 1.0]);
        let second = session.highlighted_id().unwrap();
        let rgb = session
            .active_sketch()
            .color_prop(second, layer_at(&session, 0), PropId::Rgb);
        assert_eq!(rgb, Some(PropValue::Rgb(original[0])));

        // Editing a brush color changes canvases made later, not this one.
        let black = session.swatches()[0].id;
        session.open_color_page(black);
        let undo_len = session.undo_len();
        assert!(session.set_panel_property(PropId::Rgb, PropValue::Rgb([1, 2, 3])));
        assert_eq!(session.undo_len(), undo_len, "a tool color is not history");
        let kept = session
            .active_sketch()
            .color_prop(second, layer_at(&session, 0), PropId::Rgb);
        assert_eq!(kept, Some(PropValue::Rgb(original[0])));
        session.perform(CommandId::NewCanvas, [1.0, 1.0]);
        let third = session.highlighted_id().unwrap();
        let rgb = session
            .active_sketch()
            .color_prop(third, layer_at(&session, 0), PropId::Rgb);
        assert_eq!(rgb, Some(PropValue::Rgb([1, 2, 3])));
    }

    #[test]
    fn renaming_a_color_is_one_undo_step_and_refuses_a_blank() {
        let mut session = Session::new();
        let canvas = session.selection()[0];
        let red = layer_at(&session, 1);
        let name = |session: &Session| {
            session
                .active_sketch()
                .color_prop(canvas, red, PropId::Name)
        };
        let undo_len = session.undo_len();
        assert!(!session.rename_color(canvas, red, "   "));
        assert_eq!(session.undo_len(), undo_len);
        assert!(session.rename_color(canvas, red, "  Signal red "));
        assert_eq!(name(&session), Some(PropValue::Text("Signal red".into())));
        assert_eq!(session.undo_len(), undo_len + 1);
        session.undo().unwrap();
        assert_eq!(name(&session), Some(PropValue::Text("Red".into())));
    }

    #[test]
    fn bring_to_front_and_send_to_back_are_one_step_each() {
        let mut session = Session::new();
        session.perform(CommandId::NewCanvas, [800.0, 600.0]);
        session.perform(CommandId::NewCanvas, [800.0, 600.0]);
        let order = |session: &Session| -> Vec<NodeId> {
            session.layers().iter().map(|node| node.id).collect()
        };
        let start = order(&session);
        assert_eq!(start.len(), 3);
        let bottom = start[2];
        session.select_click(bottom, false);
        assert!(!session.command_enabled(CommandId::SendToBack));
        assert!(session.command_enabled(CommandId::BringToFront));

        let undo_len = session.undo_len();
        session.perform(CommandId::BringToFront, [800.0, 600.0]);
        assert_eq!(order(&session), vec![bottom, start[0], start[1]]);
        assert_eq!(session.undo_len(), undo_len + 1);
        assert!(!session.command_enabled(CommandId::BringToFront));

        session.perform(CommandId::SendToBack, [800.0, 600.0]);
        assert_eq!(order(&session), start);
        session.undo().unwrap();
        session.undo().unwrap();
        assert_eq!(order(&session), start);
    }

    #[test]
    fn a_rotated_canvas_box_hugs_its_ink() {
        let mut session = Session::new();
        let circle: Vec<Sample> = (0..=72)
            .map(|step| {
                let angle = f64::from(step) / 72.0 * std::f64::consts::TAU;
                Sample {
                    x: 100.0 * angle.cos(),
                    y: 100.0 * angle.sin(),
                    pressure: None,
                }
            })
            .collect();
        commit_samples(&mut session, &circle);
        let (min, max) = session.selection_bounds().unwrap();
        let width = max[0] - min[0];

        let before = session.changeable_locals();
        let center = kurbo::Point::new((min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0);
        let turn = kurbo::Affine::rotate_about(std::f64::consts::FRAC_PI_4, center);
        session.preview_world(&before, |world| turn * world);
        assert!(session.commit_preview(&before));
        let (min, max) = session.selection_bounds().unwrap();
        let turned = max[0] - min[0];
        // A box around the rotated box would be about 1.4 times as wide.
        assert!(
            (turned - width).abs() < width * 0.03,
            "the box went from {width} to {turned} wide"
        );
    }

    #[test]
    fn saving_a_palette_as_default_changes_the_next_canvas() {
        let mut session = Session::new();
        let first = session.selection()[0];
        open_layer(&mut session, 0);
        assert!(session.set_panel_property(PropId::Rgb, PropValue::Rgb([10, 20, 30])));
        session.perform(CommandId::NewCanvas, [1.0, 1.0]);
        let second = session.selection()[0];
        assert_ne!(first, second);
        // From the first canvas's row menu, with the second still selected.
        assert!(session.make_palette_default(first));
        assert_eq!(session.selection(), vec![second]);
        assert_eq!(session.swatches()[0].rgb, [10, 20, 30]);
        session.perform(CommandId::NewCanvas, [1.0, 1.0]);
        let third = session.highlighted_id().unwrap();
        let rgb = session
            .active_sketch()
            .color_prop(third, layer_at(&session, 0), PropId::Rgb);
        assert_eq!(rgb, Some(PropValue::Rgb([10, 20, 30])));

        // The first canvas's layers now answer to the new brush colors, so a
        // stroke there joins its own first layer instead of adding one.
        session.select_click(first, false);
        let layers = session.active_sketch().palette_len(first);
        commit_samples(&mut session, &line(0.0, 40.0, 0.0, 10.0, None));
        assert_eq!(session.active_sketch().palette_len(first), layers);
        session.perform(CommandId::NewSketch, [1.0, 1.0]);
        assert_eq!(session.swatches()[0].rgb, [10, 20, 30]);
    }

    #[test]
    fn a_marquee_selects_a_group_rather_than_a_child() {
        let mut session = Session::new();
        commit_samples(&mut session, &line(0.0, 80.0, 0.0, 4.0, Some(1.0)));
        let child = session.selection()[0];
        session.perform(CommandId::NewCanvas, [1.0, 1.0]);
        session.select_click(child, true);
        session.perform(CommandId::Group, [1.0, 1.0]);
        let group = session.selection()[0];
        session.clear_selection();
        session.marquee_select([-5.0, -5.0], [40.0, 5.0], false, &[]);
        assert_eq!(session.selection(), vec![group]);
        assert!(!session.selection().contains(&child));
    }

    #[test]
    fn nudging_a_selected_group_leaves_its_selected_child() {
        let mut session = Session::new();
        let child = session.selection()[0];
        session.perform(CommandId::NewCanvas, [1.0, 1.0]);
        session.select_click(child, true);
        session.perform(CommandId::Group, [1.0, 1.0]);
        let group = session.selection()[0];
        session.select_click(child, true);

        let locals = session.changeable_locals();
        assert_eq!(locals.len(), 1);
        assert_eq!(locals[0].0, group);

        session.perform(CommandId::NudgeRight, [1.0, 1.0]);
        session.select_click(child, false);
        assert_eq!(translation(session.changeable_locals()[0].1), [0.0, 0.0]);
        session.select_click(group, false);
        assert_eq!(translation(session.changeable_locals()[0].1), [1.0, 0.0]);
    }

    #[test]
    fn a_locked_canvas_does_not_toggle_its_colors() {
        let mut session = Session::new();
        let canvas = session.selection()[0];
        let color = layer_at(&session, 0);
        assert!(session.toggle_node(canvas, PropId::Locked));
        let undo_len = session.undo_len();
        assert!(!session.toggle_color_flag(canvas, color, PropId::Visible));
        assert!(!session.toggle_color_flag(canvas, color, PropId::Locked));
        assert_eq!(session.undo_len(), undo_len);
        assert!(session.toggle_node(canvas, PropId::Locked));
        assert_eq!(session.active_sketch().color_id_at(canvas, 0), Some(color));
        // Locking the canvas switched to select, which has no swatches.
        session.set_tool(Tool::Brush);
        assert!(!session.swatches()[0].muted);
    }

    fn place_shape(session: &mut Session, from: Point, to: Point, shift: bool) {
        session.begin_shape(from).unwrap();
        session.move_shape(to, shift).unwrap();
        session.commit().unwrap();
    }

    #[test]
    fn text_alone_or_in_a_group_has_no_rotate_or_flip() {
        let mut session = Session::new();
        session.set_tool(Tool::Rectangle);
        place_shape(&mut session, [0.0, 0.0], [40.0, 30.0], false);
        let shape = session.active_sketch().root_at(0).unwrap();
        session.set_tool(Tool::Text);
        place_text(&mut session, [100.0, 0.0], [100.0, 0.0], false, "Hi");
        let text = session.active_sketch().root_at(0).unwrap();
        session.set_tool(Tool::Select);
        let turns = [
            CommandId::RotateClockwise,
            CommandId::RotateCounterclockwise,
            CommandId::RotateHalf,
            CommandId::FlipHorizontal,
            CommandId::FlipVertical,
        ];

        session.replace_selection(vec![shape]);
        assert!(session.selection_turns());
        for id in turns {
            assert!(session.command_shown(id) && session.command_enabled(id));
        }

        for ids in [vec![text], vec![shape, text]] {
            session.replace_selection(ids);
            assert!(!session.selection_turns());
            for id in turns {
                assert!(!session.command_shown(id) && !session.command_enabled(id));
            }
            let undo_len = session.undo_len();
            session.perform(CommandId::RotateClockwise, [1.0, 1.0]);
            session.perform(CommandId::FlipHorizontal, [1.0, 1.0]);
            assert_eq!(session.undo_len(), undo_len, "nothing turned");
        }

        // A group holding text does not turn either.
        session.perform(CommandId::Group, [1.0, 1.0]);
        assert_eq!(session.selection().len(), 1);
        assert!(!session.selection_turns());
        assert!(session.command_shown(CommandId::Group));
    }

    #[test]
    fn a_shape_keeps_the_stroke_it_was_drawn_with() {
        let mut session = Session::new();
        session.set_tool(Tool::Rectangle);
        session.set_size(12).unwrap();
        let outline = session.swatches()[0].id;
        session.swatch_click(outline);
        assert!(session.set_panel_property(PropId::Dash, PropValue::Index(1)));
        assert!(session.set_panel_property(PropId::Rgb, PropValue::Rgb([1, 2, 3])));
        place_shape(&mut session, [0.0, 0.0], [40.0, 20.0], false);

        let id = session.active_sketch().root_at(0).unwrap();
        let stroke = session.active_sketch().shape_outline(id).unwrap();
        assert_eq!(stroke.size, 12);
        assert_eq!(stroke.dash, Dash::Dashed);
        assert_eq!(stroke.rgb, [1, 2, 3]);
        assert_eq!(session.selection(), vec![id], "the new shape is selected");
        assert_eq!(session.tool(), Tool::Rectangle);

        session.set_size(4).unwrap();
        assert!(session.set_panel_property(PropId::Dash, PropValue::Index(0)));
        assert!(session.set_panel_property(PropId::Rgb, PropValue::Rgb([9, 9, 9])));
        let stroke = session.active_sketch().shape_outline(id).unwrap();
        assert_eq!(stroke.size, 12);
        assert_eq!(stroke.dash, Dash::Dashed);
        assert_eq!(stroke.rgb, [1, 2, 3]);
        assert_eq!(session.undo_len(), 1);
    }

    #[test]
    fn constrain_makes_a_square_from_an_unequal_drag() {
        let mut session = Session::new();
        session.set_tool(Tool::Rectangle);
        assert!(session.set_panel_property(PropId::Constrain, PropValue::Bool(true)));
        place_shape(&mut session, [0.0, 0.0], [10.0, 40.0], false);
        let id = session.active_sketch().root_at(0).unwrap();
        assert_eq!(
            session.active_sketch().shape_box(id),
            Some(([0.0, 0.0], [40.0, 40.0]))
        );
    }

    #[test]
    fn snap_lands_corners_on_the_grid() {
        let mut session = Session::new();
        session.set_tool(Tool::Rectangle);
        assert!(session.set_panel_property(PropId::Snap, PropValue::Bool(true)));
        place_shape(&mut session, [10.0, 10.0], [50.0, 30.0], false);
        let id = session.active_sketch().root_at(0).unwrap();
        assert_eq!(
            session.active_sketch().shape_box(id),
            Some(([0.0, 0.0], [48.0, 24.0]))
        );
    }

    #[test]
    fn a_drag_that_does_not_move_adds_nothing() {
        let mut session = Session::new();
        session.set_tool(Tool::Ellipse);
        let roots = session.active_sketch().root_len();
        place_shape(&mut session, [12.0, 18.0], [12.0, 18.0], false);
        assert_eq!(session.active_sketch().root_len(), roots);
        assert_eq!(session.undo_len(), 0);
        assert_eq!(session.redo_len(), 0);
    }

    #[test]
    fn the_eraser_does_not_change_a_shape() {
        let mut session = Session::new();
        session.set_tool(Tool::Rectangle);
        place_shape(&mut session, [0.0, 0.0], [100.0, 80.0], false);
        let id = session.active_sketch().root_at(0).unwrap();
        let before = session.active_sketch().shape_box(id);
        let stroke = session.active_sketch().shape_outline(id).unwrap().clone();
        session.set_tool(Tool::Eraser);
        commit_samples(&mut session, &line(-20.0, 140.0, 40.0, 4.0, Some(1.0)));
        assert_eq!(session.active_sketch().shape_box(id), before);
        assert_eq!(session.active_sketch().shape_outline(id), Some(&stroke));
        assert_eq!(session.undo_len(), 1);
        assert_eq!(session.stroke_count(), 0);
    }

    #[test]
    fn undo_removes_a_new_shape() {
        let mut session = Session::new();
        session.set_tool(Tool::Line);
        place_shape(&mut session, [0.0, 0.0], [80.0, 10.0], false);
        assert_eq!(session.active_sketch().root_len(), 2);
        assert_eq!(session.undo_len(), 1);
        session.undo().unwrap();
        assert_eq!(session.active_sketch().root_len(), 1);
        assert_eq!(session.undo_len(), 0);
        assert_eq!(session.redo_len(), 1);
    }

    fn place_text(session: &mut Session, from: Point, to: Point, dragged: bool, content: &str) {
        session.begin_text(from).unwrap();
        session.move_text(to).unwrap();
        session.end_text(dragged).unwrap();
        session.set_text_draft(content.to_string());
        session.commit_text();
    }

    #[test]
    fn an_empty_text_commit_leaves_no_object_and_no_history() {
        let mut session = Session::new();
        session.set_tool(Tool::Text);
        let roots = session.active_sketch().root_len();
        place_text(&mut session, [10.0, 20.0], [10.0, 20.0], false, "");
        assert_eq!(session.active_sketch().root_len(), roots);
        assert_eq!(session.undo_len(), 0);
        assert_eq!(session.redo_len(), 0);
    }

    #[test]
    fn a_committed_text_object_is_one_undo_step() {
        let mut session = Session::new();
        session.set_tool(Tool::Text);
        let roots = session.active_sketch().root_len();
        place_text(&mut session, [10.0, 20.0], [10.0, 20.0], false, "Hi");
        assert_eq!(session.undo_len(), 1);
        assert_eq!(session.active_sketch().root_len(), roots + 1);
        let id = session.active_sketch().root_at(0).unwrap();
        let record = session.active_sketch().text_record(id).unwrap();
        assert_eq!(record.content, "Hi");
        assert_eq!(record.box_width, None);
        session.undo().unwrap();
        assert_eq!(session.active_sketch().root_len(), roots);
        assert_eq!(session.undo_len(), 0);
    }

    #[test]
    fn a_text_drag_stores_a_width_and_turns_auto_size_off() {
        let mut session = Session::new();
        session.set_tool(Tool::Text);
        place_text(&mut session, [10.0, 20.0], [90.0, 25.0], true, "Wide");
        let id = session.active_sketch().root_at(0).unwrap();
        let record = session.active_sketch().text_record(id).unwrap();
        assert_eq!(record.box_width, Some(80.0));
        assert!(record.wrap);
    }

    #[test]
    fn the_eraser_does_not_change_text() {
        let mut session = Session::new();
        session.set_tool(Tool::Text);
        place_text(&mut session, [10.0, 20.0], [10.0, 20.0], false, "Letters");
        let id = session.active_sketch().root_at(0).unwrap();
        let before = session.active_sketch().text_record(id).unwrap().content;
        let undo_len = session.undo_len();
        session.set_tool(Tool::Eraser);
        commit_samples(&mut session, &line(-20.0, 140.0, 30.0, 4.0, Some(1.0)));
        assert_eq!(
            session.active_sketch().text_record(id).unwrap().content,
            before
        );
        assert_eq!(session.undo_len(), undo_len);
    }

    #[test]
    fn a_later_tool_edit_does_not_change_committed_text() {
        let mut session = Session::new();
        session.set_tool(Tool::Text);
        place_text(&mut session, [4.0, 8.0], [4.0, 8.0], false, "Stay");
        let id = session.active_sketch().root_at(0).unwrap();
        // With nothing selected, the toolbar and page set up the next text.
        session.clear_selection();
        session.set_size(40).unwrap();
        assert!(session.set_panel_property(PropId::Bold, PropValue::Bool(true)));
        let record = session.active_sketch().text_record(id).unwrap();
        assert_eq!(record.size, 24);
        assert!(!record.bold);
        assert_eq!(session.undo_len(), 1);
    }

    #[test]
    fn emptying_existing_text_is_one_undo_step() {
        let mut session = Session::new();
        session.set_tool(Tool::Text);
        place_text(&mut session, [10.0, 20.0], [10.0, 20.0], false, "Hi");
        let id = session.active_sketch().root_at(0).unwrap();
        session.set_tool(Tool::Rectangle);
        place_shape(&mut session, [0.0, 0.0], [30.0, 10.0], false);
        session.undo().unwrap();
        assert_eq!(session.redo_len(), 1);

        session.set_tool(Tool::Text);
        session.begin_text([10.0, 20.0]).unwrap();
        session.end_text(false).unwrap();
        session.set_text_draft(String::new());
        session.commit_text();

        assert!(session.active_sketch().text_record(id).is_none());
        assert_eq!(session.undo_len(), 2);
        assert_eq!(session.redo_len(), 0);
        session.undo().unwrap();
        assert_eq!(
            session.active_sketch().text_record(id).unwrap().content,
            "Hi"
        );
    }

    #[test]
    fn the_size_slider_sets_the_next_stroke_and_never_resizes_old_ones() {
        let mut session = Session::new();
        let samples = line(0.0, 240.0, 0.0, 8.0, None);
        commit_samples(&mut session, &samples);
        let locked = session.stroke_ids()[0];
        let black = session.swatches()[0].rgb;
        assert_eq!(session.size_control().unwrap().tint, Some(black));

        let undo_len = session.undo_len();
        session.set_size(12).unwrap();
        assert_eq!(session.undo_len(), undo_len, "the slider is a tool setting");
        assert_eq!(
            session.size_control().unwrap().tint,
            None,
            "moving it releases the lock"
        );
        let width = horizontal_diameter(&session.ink_outline(locked).unwrap(), 80.0, 160.0);
        assert!(
            (width - 3.0).abs() < 0.75,
            "the old stroke kept size 3: {width}"
        );

        commit_samples(&mut session, &line(0.0, 240.0, 60.0, 8.0, None));
        let sized = *session
            .stroke_ids()
            .iter()
            .find(|id| **id != locked)
            .unwrap();
        assert_eq!(
            session.active_sketch().stroke(sized).unwrap().own_size(),
            Some(12)
        );
        assert_eq!(
            session.active_sketch().stroke(locked).unwrap().own_size(),
            None
        );

        // Thickening the layer moves only the stroke that follows it.
        open_layer(&mut session, 0);
        assert!(session.set_panel_property(PropId::ColorSize, PropValue::Number(20.0)));
        let followed = horizontal_diameter(&session.ink_outline(locked).unwrap(), 80.0, 160.0);
        let kept = horizontal_diameter(&session.ink_outline(sized).unwrap(), 80.0, 160.0);
        assert!(
            (followed - 20.0).abs() < 0.75,
            "the locked stroke follows: {followed}"
        );
        assert!(
            (kept - 12.0).abs() < 0.75,
            "the slider's stroke keeps 12: {kept}"
        );

        // Lock size on returns the slider to the layer's size, in its color.
        let brush = session.swatches()[0].id;
        session.open_color_page(brush);
        assert!(session.set_panel_property(PropId::LockSize, PropValue::Bool(true)));
        let control = session.size_control().unwrap();
        assert_eq!(control.value, 20);
        assert_eq!(control.tint, Some(black));
    }

    #[test]
    fn a_brush_color_added_later_adds_its_layer_with_the_stroke() {
        let mut session = Session::new();
        let canvas = session.highlighted_id().unwrap();
        let layers = session.active_sketch().palette_len(canvas).unwrap();
        let black = session.swatches()[0].id;
        assert!(session.swatch_action(black, MenuAction::Duplicate));
        assert_eq!(session.undo_len(), 0, "a brush color is a tool setting");
        let copy = session.swatches()[1].id;
        session.set_active_color(copy);
        assert_eq!(session.active_sketch().palette_len(canvas), Some(layers));

        commit_samples(&mut session, &line(0.0, 40.0, 0.0, 10.0, None));
        assert_eq!(
            session.active_sketch().palette_len(canvas),
            Some(layers + 1)
        );
        let added = layer_at(&session, layers);
        assert_eq!(
            session.active_sketch().layer_for_source(canvas, copy),
            Some(added)
        );
        assert_eq!(
            session.undo_len(),
            1,
            "the layer and the stroke are one step"
        );

        // A second stroke joins that layer.
        commit_samples(&mut session, &line(0.0, 40.0, 20.0, 10.0, None));
        assert_eq!(
            session.active_sketch().palette_len(canvas),
            Some(layers + 1)
        );

        session.undo().unwrap();
        session.undo().unwrap();
        assert_eq!(session.active_sketch().palette_len(canvas), Some(layers));
        assert_eq!(session.stroke_count(), 0);
    }

    #[test]
    fn each_tool_shows_its_own_palette_and_objects_copy_it() {
        let mut session = Session::new();
        assert_eq!(session.swatches().len(), 9);
        session.set_tool(Tool::Eraser);
        assert!(session.swatches().is_empty());
        session.set_tool(Tool::Select);
        assert!(session.swatches().is_empty());

        session.set_tool(Tool::Rectangle);
        let names: Vec<String> = session
            .swatches()
            .into_iter()
            .map(|swatch| swatch.name)
            .collect();
        assert_eq!(names, ["Outline", "Fill"]);
        assert!(session.swatches()[1].muted, "fill starts off");
        let fill = session.swatches()[1].id;
        session.swatch_click(fill);
        assert!(session.panel_open());
        assert!(session.set_panel_property(PropId::Visible, PropValue::Bool(true)));
        assert!(session.set_panel_property(PropId::Rgb, PropValue::Rgb([5, 6, 7])));
        assert_eq!(session.undo_len(), 0);
        place_shape(&mut session, [0.0, 0.0], [40.0, 30.0], false);
        let shape = session.active_sketch().root_at(0).unwrap();
        let colors = session.active_sketch().node_palette(shape).unwrap();
        assert_eq!(colors[1].rgb, [5, 6, 7]);
        assert!(colors[1].visible);
        assert_ne!(colors[1].id, fill, "the object's colors are its own");

        // A later tool edit leaves the shape alone; an object edit is undoable.
        assert!(session.set_panel_property(PropId::Rgb, PropValue::Rgb([9, 9, 9])));
        let colors = session.active_sketch().node_palette(shape).unwrap();
        assert_eq!(colors[1].rgb, [5, 6, 7]);

        session.set_tool(Tool::Line);
        let names: Vec<String> = session
            .swatches()
            .into_iter()
            .map(|swatch| swatch.name)
            .collect();
        assert_eq!(names, ["Stroke"]);
        session.set_tool(Tool::Text);
        let names: Vec<String> = session
            .swatches()
            .into_iter()
            .map(|swatch| swatch.name)
            .collect();
        assert_eq!(names, ["Text", "Background", "Border"]);
    }

    #[test]
    fn texts_border_is_a_line_with_a_size_and_style_and_old_palettes_gain_it() {
        let mut ids = Allocator::new();
        let mut tool = TextTool::builtin(&mut ids);
        let border = tool.palette[2].clone();
        assert_eq!(border.color.role, ColorRole::Border);
        assert!(!border.color.visible, "the border starts off");
        assert!(border.read(PropId::ColorSize).is_some());
        assert!(border.read(PropId::Dash).is_some());
        assert_eq!(border.read(PropId::Cap), None);
        assert_eq!(
            border.read(PropId::LockSize),
            None,
            "the text's Size slider is the font's"
        );

        // A palette saved before the border keeps its colors and gains one.
        tool.palette.truncate(2);
        tool.palette[1].color.rgb = [216, 52, 50];
        tool.palette[1].color.visible = true;
        let tool = sanitize_text_tool(tool, &mut ids);
        let roles: Vec<ColorRole> = tool.palette.iter().map(|item| item.color.role).collect();
        assert_eq!(
            roles,
            [ColorRole::Text, ColorRole::Background, ColorRole::Border]
        );
        assert_eq!(tool.palette[1].color.rgb, [216, 52, 50]);
        assert!(tool.palette[1].color.visible);

        // Turned on, the border is part of the text's look.
        let mut colors: Vec<_> = tool.palette.iter().map(|item| item.color.clone()).collect();
        assert_eq!(TextLook::of(&colors).border, None);
        colors[2].visible = true;
        colors[2].size = 4;
        let look = TextLook::of(&colors);
        assert_eq!(look.border.map(|border| border.size), Some(4.0));
    }

    #[test]
    fn a_selected_shapes_palette_opens_its_colors_as_undo_steps() {
        let mut session = Session::new();
        session.set_tool(Tool::Rectangle);
        place_shape(&mut session, [0.0, 0.0], [40.0, 30.0], false);
        let shape = session.active_sketch().root_at(0).unwrap();
        session.set_tool(Tool::Select);
        session.select_click(shape, false);
        assert!(
            session.panel_sections().is_empty(),
            "Layers is the outliner alone"
        );
        session.object_action(shape, MenuAction::Properties);
        assert!(session.panel_open());
        assert!(
            session
                .panel_sections()
                .iter()
                .any(|(title, _)| title == "Palette")
        );
        let rows = session.palette_entries(session.palette_node().unwrap());
        assert_eq!(rows.len(), 2);
        session.open_palette_color(rows[0].id);
        let undo_len = session.undo_len();
        assert!(session.set_panel_property(PropId::ColorSize, PropValue::Number(9.0)));
        assert_eq!(session.undo_len(), undo_len + 1);
        assert_eq!(
            session.active_sketch().shape_outline(shape).unwrap().size,
            9
        );
        assert_eq!(session.panel_property(PropId::LockSize), None);
        session.set_tool(Tool::Rectangle);
        assert!(!session.shows_layers());
    }

    #[test]
    fn editing_text_in_place_shows_and_edits_that_object() {
        let mut session = Session::new();
        session.set_tool(Tool::Text);
        place_text(&mut session, [10.0, 20.0], [10.0, 20.0], false, "Hi");
        let id = session.active_sketch().root_at(0).unwrap();

        // Clicking the text with the text tool edits it, and the panel shows it.
        session.begin_text([12.0, 25.0]).unwrap();
        session.end_text(false).unwrap();
        assert!(
            session
                .panel_sections()
                .iter()
                .any(|(title, _)| title == "Text")
        );
        let undo_len = session.undo_len();
        assert!(session.set_panel_property(PropId::FontSize, PropValue::Number(60.0)));
        assert_eq!(session.active_sketch().text_record(id).unwrap().size, 60);
        assert_eq!(session.undo_len(), undo_len + 1);
        session.commit_text();

        // The text stays selected, so the toolbar's Size is still its size.
        assert_eq!(session.size(), 60);
        session.set_size(40).unwrap();
        assert_eq!(session.active_sketch().text_record(id).unwrap().size, 40);
        // With nothing selected, it is the next text's size and leaves this one.
        session.clear_selection();
        assert_eq!(session.size(), 24);
        session.set_size(30).unwrap();
        assert_eq!(session.active_sketch().text_record(id).unwrap().size, 40);
    }

    #[test]
    fn properties_opens_a_page_and_every_color_has_visible() {
        let mut session = Session::new();
        assert!(!session.panel_open(), "the panel starts closed");

        // A tool's page has no Palette: its colors are its swatches.
        session.tool_action(Tool::Rectangle, MenuAction::Properties);
        assert!(session.panel_open());
        assert_eq!(
            session.tool(),
            Tool::Brush,
            "Properties does not switch tools"
        );
        let titles: Vec<String> = session
            .panel_sections()
            .into_iter()
            .map(|(title, _)| title)
            .collect();
        assert!(!titles.contains(&"Palette".to_string()), "{titles:?}");

        // A shape's Fill is turned on from its page's Visible switch.
        session.set_tool(Tool::Rectangle);
        assert_eq!(session.swatch_menu(session.swatches()[1].id).len(), 1);
        let fill = session.swatches()[1].id;
        assert!(session.swatch_action(fill, MenuAction::Properties));
        assert_eq!(
            session.panel_property(PropId::Visible),
            Some(PropValue::Bool(false))
        );
        assert!(session.set_panel_property(PropId::Visible, PropValue::Bool(true)));
        assert!(!session.swatches()[1].muted);

        // A brush color's menu duplicates and deletes, keeping one color.
        session.set_tool(Tool::Brush);
        let black = session.swatches()[0].id;
        assert_eq!(session.swatch_menu(black).len(), 3);
        assert!(session.swatch_action(black, MenuAction::Delete));
        assert_eq!(session.swatches().len(), 8);
        assert_eq!(session.undo_len(), 0);
    }

    #[test]
    fn properties_opens_an_objects_page_and_a_canvas_lists_its_layers() {
        let mut session = Session::new();
        session.set_tool(Tool::Rectangle);
        place_shape(&mut session, [0.0, 0.0], [40.0, 30.0], false);
        session.set_tool(Tool::Text);
        place_text(&mut session, [200.0, 20.0], [200.0, 20.0], false, "Hi");
        session.set_tool(Tool::Select);
        let text = session.active_sketch().root_at(0).unwrap();
        let shape = session.active_sketch().root_at(1).unwrap();
        let canvas = session.active_sketch().root_at(2).unwrap();

        // A right-click picks what is under the pointer.
        assert_eq!(session.pick_for_menu([20.0, 0.0]), Some(shape));
        assert_eq!(session.selection(), vec![shape]);
        session.object_action(shape, MenuAction::Properties);
        assert!(!session.shows_layers());

        // A canvas's page lists its layers; each opens that layer's page.
        session.object_action(canvas, MenuAction::Properties);
        assert_eq!(session.selection(), vec![canvas]);
        let rows = session.palette_entries(session.palette_node().unwrap());
        assert_eq!(rows.len(), 9);
        session.open_palette_color(rows[1].id);
        assert_eq!(
            session.panel_property(PropId::Name),
            Some(PropValue::Text("Red".into()))
        );

        // A group has no page of its own.
        session.select_click(shape, false);
        session.select_click(text, true);
        session.perform(CommandId::Group, [800.0, 600.0]);
        let group = session.selection()[0];
        assert!(!session.object_page_available(group));

        // A double-click on text with the select tool edits it in place.
        session.perform(CommandId::Ungroup, [800.0, 600.0]);
        assert!(session.edit_text(text));
        assert!(session.text_editing());
        assert_eq!(session.tool(), Tool::Select);
        session.set_text_draft("Hello".into());
        session.commit_text();
        assert_eq!(
            session.active_sketch().text_record(text).unwrap().content,
            "Hello"
        );
    }

    #[test]
    fn a_color_page_goes_back_where_it_came_from_and_the_box_takes_right_clicks() {
        let mut session = Session::new();
        session.set_tool(Tool::Rectangle);
        place_shape(&mut session, [0.0, 0.0], [100.0, 80.0], false);
        session.set_tool(Tool::Select);
        let shape = session.active_sketch().root_at(0).unwrap();
        let canvas = session.active_sketch().root_at(1).unwrap();

        // Inside the selection's box, away from the outline, keeps it.
        session.select_click(shape, false);
        assert_eq!(session.pick_for_menu([50.0, 40.0]), Some(shape));
        assert_eq!(session.selection(), vec![shape]);

        // From the object's palette, back returns to the object.
        session.object_action(shape, MenuAction::Properties);
        let fill = session.palette_entries(shape)[1].id;
        session.open_palette_color(fill);
        assert_eq!(session.back_label().as_deref(), Some("Rectangle 1"));
        session.go_back();
        assert!(session.back_label().is_none());
        assert!(
            session
                .panel_sections()
                .iter()
                .any(|(title, _)| title == "Palette")
        );

        // From the outliner, back returns to Layers.
        let red = session.palette_entries(canvas)[1].id;
        session.select_color(canvas, red);
        assert_eq!(session.back_label().as_deref(), Some("Layers"));
        session.go_back();
        assert!(session.shows_layers());

        // From a swatch, back returns to the tool's page.
        session.set_tool(Tool::Brush);
        let black = session.swatches()[0].id;
        session.swatch_action(black, MenuAction::Properties);
        assert_eq!(session.back_label().as_deref(), Some("Brush"));
        session.go_back();
        assert!(
            session
                .panel_sections()
                .iter()
                .any(|(title, _)| title == "Brush Preset")
        );
    }

    #[test]
    fn the_selection_follows_what_was_made_last_and_the_brush_keeps_its_canvas() {
        let mut session = Session::new();
        let canvas = session.highlighted_id().unwrap();
        commit_samples(&mut session, &line(0.0, 80.0, 0.0, 8.0, None));
        assert_eq!(session.selection(), vec![canvas]);

        // A shape drawn next is what the select tool picks up.
        session.set_tool(Tool::Rectangle);
        place_shape(&mut session, [0.0, 40.0], [60.0, 90.0], false);
        let shape = session.active_sketch().root_at(0).unwrap();
        session.set_tool(Tool::Select);
        assert_eq!(session.selection(), vec![shape]);

        // Text drawn next is selected the same way.
        session.set_tool(Tool::Text);
        place_text(&mut session, [200.0, 0.0], [200.0, 0.0], false, "Hi");
        let text = session.active_sketch().root_at(0).unwrap();
        session.set_tool(Tool::Select);
        assert_eq!(session.selection(), vec![text]);

        // The brush goes back to the canvas it was drawing on, not a new one.
        session.set_tool(Tool::Brush);
        assert_eq!(session.selection(), vec![canvas]);
        let roots = session.active_sketch().root_len();
        commit_samples(&mut session, &line(0.0, 80.0, 20.0, 8.0, None));
        assert_eq!(session.active_sketch().root_len(), roots);
        assert_eq!(session.stroke_count(), 2);

        // Selecting a shape and drawing keeps the stroke on that canvas, and
        // selects the canvas again.
        session.select_click(shape, false);
        commit_samples(&mut session, &line(0.0, 80.0, 30.0, 8.0, None));
        assert_eq!(session.active_sketch().root_len(), roots);
        assert_eq!(session.selection(), vec![canvas]);
    }

    #[test]
    fn the_text_tools_bar_changes_the_text_it_is_working_on() {
        let mut session = Session::new();
        session.set_tool(Tool::Text);
        place_text(&mut session, [0.0, 0.0], [0.0, 0.0], false, "Hello");
        let id = session.active_sketch().root_at(0).unwrap();
        assert_eq!(session.active_text(), Some(id), "a new text stays selected");
        let tool_style = session.text.style.clone();
        let undo_len = session.undo_len();

        assert!(session.set_bar_value(PropId::Align, PropValue::Index(1), true));
        assert_eq!(
            session.active_sketch().node_prop(id, PropId::Align),
            Some(PropValue::Index(1))
        );
        assert_eq!(session.bar_value(PropId::Align), Some(PropValue::Index(1)));
        assert_eq!(session.undo_len(), undo_len + 1);

        // A Size drag is one step, and the slider shows the text's size.
        session.preview_size(30).unwrap();
        session.preview_size(40).unwrap();
        session.set_size(40).unwrap();
        assert_eq!(session.size(), 40);
        assert_eq!(
            session.active_sketch().node_prop(id, PropId::FontSize),
            Some(PropValue::Number(40.0))
        );
        assert_eq!(session.undo_len(), undo_len + 2);

        session.choose_text_list(1);
        assert_eq!(session.text_list(), (true, 1));
        session.toggle_text_list();
        assert_eq!(session.text_list().0, false);
        assert_eq!(session.text.style, tool_style, "the next text is unchanged");
        assert!(matches!(session.resolved_subject(), Subject::Objects));

        // New text being typed takes the tool's settings, so they change.
        session.begin_text([0.0, 300.0]).unwrap();
        session.end_text(false).unwrap();
        assert_eq!(session.active_text(), None);
        assert!(session.set_bar_value(PropId::Align, PropValue::Index(2), true));
        assert_eq!(session.text.style.align, draw::document::TextAlign::Right);
        assert_eq!(
            session.active_sketch().node_prop(id, PropId::Align),
            Some(PropValue::Index(1))
        );
    }

    #[test]
    fn the_text_tools_bar_sets_up_the_next_text() {
        let mut session = Session::new();
        session.set_tool(Tool::Text);
        let undo_len = session.undo_len();
        assert!(session.set_bar_value(PropId::Align, PropValue::Index(3), true));
        assert!(session.set_bar_value(PropId::TextPadding, PropValue::Number(80.0), true));
        assert_eq!(
            session.bar_value(PropId::TextPadding),
            Some(PropValue::Number(48.0))
        );
        assert!(session.set_bar_value(PropId::TextRadius, PropValue::Number(6.0), true));
        assert_eq!(session.text_list(), (false, 0));
        session.toggle_text_list();
        assert_eq!(session.text_list(), (true, 0));
        session.choose_text_list(1);
        assert_eq!(session.text_list(), (true, 1));
        session.toggle_text_list();
        assert_eq!(session.text_list(), (false, 1), "off keeps the kind");
        session.toggle_text_list();
        assert_eq!(
            session.undo_len(),
            undo_len,
            "tool settings are not history"
        );

        // With Auto-size off on the text page, a click places a box that wraps.
        assert!(session.set_bar_value(PropId::AutoSize, PropValue::Bool(false), true));
        place_text(&mut session, [0.0, 0.0], [0.0, 0.0], false, "Hello");
        let id = session.active_sketch().root_at(0).unwrap();
        let record = session.active_sketch().text_record(id).unwrap();
        assert_eq!(record.box_width, Some(defaults::TEXT_BOX_WIDTH));
        let width = session.text_box(id).unwrap().frame.width;
        assert_eq!(width, Some(defaults::TEXT_BOX_WIDTH));
        for (prop, value) in [
            (PropId::Align, PropValue::Index(3)),
            (PropId::List, PropValue::Index(2)),
            (PropId::TextPadding, PropValue::Number(48.0)),
            (PropId::TextRadius, PropValue::Number(6.0)),
        ] {
            assert_eq!(session.active_sketch().node_prop(id, prop), Some(value));
        }
    }

    #[test]
    fn a_side_handle_sets_texts_width_and_an_end_its_height_in_one_step() {
        let mut session = Session::new();
        session.set_tool(Tool::Text);
        place_text(&mut session, [10.0, 0.0], [10.0, 0.0], false, "Hello there");
        let id = session.active_sketch().root_at(0).unwrap();
        session.set_tool(Tool::Select);
        assert_eq!(session.sole_text(), Some(id));
        let fit = session.text_box(id).unwrap();
        assert_eq!(fit.frame.width, None);
        assert_eq!(fit.frame.height, None);
        let undo_len = session.undo_len();

        // The right side widens the box; it is no longer auto-sized.
        session.preview_text_box(id, fit, [100.0, 0.0], [140.0, 0.0], BoxSide::Right);
        let after = session.text_box(id).unwrap().frame;
        assert_eq!(after.origin, fit.frame.origin);
        assert!((after.width.unwrap() - (fit.width + 40.0)).abs() < 1e-9);
        assert!(session.commit_text_box(id, fit.frame));
        assert_eq!(session.undo_len(), undo_len + 1);

        // The left side keeps the right edge where it is.
        let fit = session.text_box(id).unwrap();
        let width = fit.frame.width.unwrap();
        session.preview_text_box(id, fit, [10.0, 0.0], [30.0, 0.0], BoxSide::Left);
        let moved = session.text_box(id).unwrap().frame;
        let right = moved.origin[0] + moved.width.unwrap();
        assert!((right - (fit.frame.origin[0] + width)).abs() < 1e-9);
        session.restore_text_box(id, fit.frame);

        // The bottom makes the box taller than its lines; the top keeps the
        // bottom edge where it is, and neither goes shorter than the lines.
        let fit = session.text_box(id).unwrap();
        let natural = fit.natural;
        assert!(natural > 0.0);
        session.preview_text_box(id, fit, [0.0, 0.0], [0.0, 60.0], BoxSide::Bottom);
        let tall = session.text_box(id).unwrap().frame;
        assert!((tall.height.unwrap() - (fit.height + 60.0)).abs() < 1e-9);
        assert_eq!(tall.width, fit.frame.width, "the width stays");
        session.preview_text_box(id, fit, [0.0, 0.0], [0.0, -500.0], BoxSide::Bottom);
        let short = session.text_box(id).unwrap().frame;
        assert!((short.height.unwrap() - natural).abs() < 1e-9);
        session.preview_text_box(id, fit, [0.0, 0.0], [0.0, -30.0], BoxSide::Top);
        let raised = session.text_box(id).unwrap().frame;
        let bottom = raised.origin[1] + raised.height.unwrap();
        assert!((bottom - (fit.frame.origin[1] + fit.height)).abs() < 1e-9);
        assert!(session.commit_text_box(id, fit.frame));
        assert_eq!(session.undo_len(), undo_len + 2);

        // Auto-size on lets the box fit its lines again, both ways.
        session.open_object_page(id);
        assert!(session.set_panel_property(PropId::AutoSize, PropValue::Bool(true)));
        let record = session.active_sketch().text_record(id).unwrap();
        assert_eq!((record.box_width, record.box_height), (None, None));
        session.undo().unwrap();
        session.undo().unwrap();
        session.undo().unwrap();
        let record = session.active_sketch().text_record(id).unwrap();
        assert_eq!(
            (record.box_width, record.box_height),
            (None, None),
            "undo makes it auto-sized again"
        );
    }

    fn translation_of(session: &Session, id: NodeId) -> [f64; 2] {
        translation(session.active_sketch().local_transform(id).unwrap())
    }

    #[test]
    fn a_second_paste_is_thirty_two_units_from_the_source() {
        let mut session = Session::new();
        let source = session.selection()[0];
        assert_eq!(translation_of(&session, source), [0.0, 0.0]);
        let undo_len = session.undo_len();
        session.perform(CommandId::Copy, [1.0, 1.0]);
        assert_eq!(session.undo_len(), undo_len, "copy is not an undo step");

        session.perform(CommandId::Paste, [1.0, 1.0]);
        let first = session.selection()[0];
        assert_ne!(first, source);
        assert_eq!(session.active_sketch().root_at(0), Some(first));
        assert_eq!(translation_of(&session, first), [16.0, 16.0]);

        session.perform(CommandId::Paste, [1.0, 1.0]);
        let second = session.selection()[0];
        assert_ne!(second, first);
        assert_eq!(session.active_sketch().root_at(0), Some(second));
        assert_eq!(translation_of(&session, second), [32.0, 32.0]);
        assert_eq!(translation_of(&session, first), [16.0, 16.0]);
        assert_eq!(translation_of(&session, source), [0.0, 0.0]);
        assert_eq!(session.active_sketch().root_len(), 3);
    }

    #[test]
    fn a_pasted_canvas_keeps_samples_and_erasures_under_new_ids() {
        let mut session = Session::new();
        session.set_size(32).unwrap();
        commit_samples(&mut session, &line(0.0, 80.0, 0.0, 8.0, Some(1.0)));
        let canvas = session.selection()[0];
        let stroke = session.stroke_ids()[0];
        session.set_tool(Tool::Eraser);
        session.set_size(8).unwrap();
        commit_samples(&mut session, &line(20.0, 60.0, 0.0, 8.0, Some(1.0)));
        let samples = session.stroke_samples(stroke).unwrap().to_vec();
        let erases = session.erase_paths(stroke).unwrap().to_vec();
        assert_eq!(erases.len(), 1);
        let source_colors: Vec<ColorId> = session
            .layers()
            .into_iter()
            .find(|row| row.id == canvas)
            .unwrap()
            .colors
            .into_iter()
            .map(|color| color.id)
            .collect();

        session.perform(CommandId::Copy, [1.0, 1.0]);
        session.perform(CommandId::Paste, [1.0, 1.0]);
        let pasted = session.selection()[0];
        assert_ne!(pasted, canvas);
        assert!(session.active_sketch().is_canvas(pasted));

        let pasted_strokes: Vec<StrokeId> = session
            .stroke_ids()
            .into_iter()
            .filter(|id| *id != stroke)
            .collect();
        assert_eq!(pasted_strokes.len(), 1);
        assert_ne!(pasted_strokes[0], stroke);
        assert_eq!(
            session.stroke_samples(pasted_strokes[0]),
            Some(samples.as_slice())
        );
        assert_eq!(
            session.erase_paths(pasted_strokes[0]),
            Some(erases.as_slice())
        );
        assert_eq!(session.stroke_samples(stroke), Some(samples.as_slice()));

        let pasted_colors: Vec<ColorId> = session
            .layers()
            .into_iter()
            .find(|row| row.id == pasted)
            .unwrap()
            .colors
            .into_iter()
            .map(|color| color.id)
            .collect();
        assert_eq!(pasted_colors.len(), source_colors.len());
        assert!(pasted_colors.iter().all(|id| !source_colors.contains(id)));
    }

    #[test]
    fn a_cut_is_one_undo_step_and_undo_leaves_the_clipboard() {
        let mut session = Session::new();
        let source = session.selection()[0];
        let undo_len = session.undo_len();
        session.perform(CommandId::Cut, [1.0, 1.0]);
        assert_eq!(session.undo_len(), undo_len + 1);
        assert_eq!(session.active_sketch().root_len(), 0);

        session.undo().unwrap();
        assert_eq!(session.active_sketch().root_at(0), Some(source));
        assert_eq!(session.active_sketch().root_len(), 1);

        session.perform(CommandId::Paste, [1.0, 1.0]);
        assert_eq!(session.active_sketch().root_len(), 2);
        let pasted = session.selection()[0];
        assert_ne!(pasted, source);
        assert_eq!(session.active_sketch().root_at(0), Some(pasted));
    }

    #[test]
    fn paste_with_an_empty_clipboard_adds_nothing() {
        let mut session = Session::new();
        let roots = session.active_sketch().root_len();
        let undo_len = session.undo_len();
        let selection = session.selection();
        assert!(!session.command_enabled(CommandId::Paste));
        session.perform(CommandId::Paste, [1.0, 1.0]);
        assert_eq!(session.active_sketch().root_len(), roots);
        assert_eq!(session.undo_len(), undo_len);
        assert_eq!(session.selection(), selection);
    }

    #[test]
    fn a_selected_group_copied_with_a_selected_child_pastes_the_child_once() {
        let mut session = Session::new();
        let first = session.selection()[0];
        session.perform(CommandId::NewCanvas, [1.0, 1.0]);
        session.select_click(first, true);
        session.perform(CommandId::Group, [1.0, 1.0]);
        let group = session.selection()[0];
        session.select_click(first, true);
        assert!(session.selection().contains(&group));
        assert!(session.selection().contains(&first));

        session.perform(CommandId::Copy, [1.0, 1.0]);
        session.perform(CommandId::Paste, [1.0, 1.0]);
        assert_eq!(session.selection().len(), 1);
        let pasted = session.selection()[0];
        assert_ne!(pasted, group);
        assert_eq!(session.active_sketch().root_len(), 2);
        let children = session
            .layers()
            .into_iter()
            .filter(|row| row.parent == Some(pasted))
            .count();
        assert_eq!(children, 2);
    }

    #[test]
    fn a_locked_object_is_not_deleted_by_cut() {
        let mut session = Session::new();
        let canvas = session.selection()[0];
        assert!(session.toggle_node(canvas, PropId::Locked));
        let undo_len = session.undo_len();
        assert!(!session.command_enabled(CommandId::Cut));
        session.perform(CommandId::Cut, [1.0, 1.0]);
        assert_eq!(session.active_sketch().root_len(), 1);
        assert_eq!(session.active_sketch().root_at(0), Some(canvas));
        assert_eq!(
            session.active_sketch().node_prop(canvas, PropId::Locked),
            Some(PropValue::Bool(true))
        );
        assert_eq!(session.undo_len(), undo_len);
        session.perform(CommandId::Paste, [1.0, 1.0]);
        assert_eq!(session.active_sketch().root_len(), 1);
    }

    fn temp_store(label: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("simple-sketch-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn drawings(dir: &std::path::Path) -> std::path::PathBuf {
        let drawings = dir.join("drawings");
        std::fs::create_dir_all(&drawings).unwrap();
        drawings
    }

    fn forbid_stored_keys(value: &serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for key in ["undo", "redo", "selection", "clipboard", "history", "path"] {
                    assert!(!map.contains_key(key), "{key} is in the snapshot");
                }
                for child in map.values() {
                    forbid_stored_keys(child);
                }
            }
            serde_json::Value::Array(items) => {
                for child in items {
                    forbid_stored_keys(child);
                }
            }
            serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_) => {}
        }
    }

    #[test]
    fn a_sketch_round_trips_through_smx() {
        let dir = temp_store("round");
        let mut session = Session::new();
        session.set_data_dir(&dir);
        session.set_size(7).unwrap();
        commit_samples(&mut session, &line(0.0, 80.0, 0.0, 8.0, Some(1.0)));
        let stroke = session.stroke_ids()[0];
        assert_eq!(
            session.active_sketch().stroke(stroke).unwrap().own_size(),
            Some(7)
        );
        let source = session
            .swatches()
            .iter()
            .find(|swatch| swatch.active)
            .unwrap()
            .id
            .get();
        session.set_tool(Tool::Eraser);
        commit_samples(&mut session, &line(-4.0, 84.0, 0.0, 4.0, Some(1.0)));
        let samples = session.stroke_samples(stroke).unwrap().to_vec();
        let erased = session.erase_paths(stroke).unwrap()[0].samples().to_vec();
        assert!(!erased.is_empty());

        session.set_tool(Tool::Rectangle);
        place_shape(&mut session, [200.0, 40.0], [280.0, 90.0], false);
        let shape = session.active_sketch().root_at(0).unwrap();
        let shape_box = session.active_sketch().shape_box(shape).unwrap();
        session.set_tool(Tool::Text);
        place_text(&mut session, [120.0, 40.0], [120.0, 40.0], false, "Hello");
        let text = session.active_sketch().root_at(0).unwrap();
        session.set_tool(Tool::Select);
        session.replace_selection(vec![shape, text]);
        session.perform(CommandId::Group, [1.0, 1.0]);
        let group = session.selection()[0];
        session.zoom_toward(2.0, [40.0, 30.0]);
        let zoom = session.zoom();
        let origin = session.camera_origin();
        session.perform(CommandId::Copy, [1.0, 1.0]);
        assert!(session.command_enabled(CommandId::Paste));
        let undo_len = session.undo_len();
        assert!(undo_len > 0);
        let sketch_id = session.active_id();

        let path = drawings(&dir).join("Round.smx");
        assert_eq!(
            session.save_path(sketch_id, &path, false, true),
            SaveResult::Wrote
        );
        assert!(!session.tabs()[0].needs_keep);
        assert_eq!(session.active_name(), "Round");
        assert_eq!(session.undo_len(), undo_len, "save leaves history");
        assert!(session.command_enabled(CommandId::Paste));

        let text_file = std::fs::read_to_string(&path).unwrap();
        let stored: serde_json::Value = serde_json::from_str(&text_file).unwrap();
        assert_eq!(stored["format_version"], 1);
        assert_eq!(stored["id"], sketch_id.get());
        assert_eq!(stored["name"], "Round");
        forbid_stored_keys(&stored);
        assert!(
            text_file.contains(&format!("\"source\": {source}")),
            "the layer's source id was not stored"
        );

        let mut opened = Session::new();
        opened.set_data_dir(&dir);
        assert!(opened.open_initial(&path));
        assert_eq!(
            opened.sketch_count(),
            1,
            "a command-line open has no Untitled beside it"
        );
        assert_eq!(opened.active_id(), sketch_id);
        assert_eq!(opened.active_name(), "Round");
        assert!(opened.selection().is_empty());
        assert!(opened.highlighted_id().is_none());
        assert_eq!(opened.undo_len(), 0);
        assert_eq!(opened.redo_len(), 0);
        assert!(!opened.command_enabled(CommandId::Paste));
        assert!((opened.zoom() - zoom).abs() < 1e-9);
        let opened_origin = opened.camera_origin();
        assert!((opened_origin[0] - origin[0]).abs() < 1e-9);
        assert!((opened_origin[1] - origin[1]).abs() < 1e-9);
        assert_eq!(opened.stroke_samples(stroke), Some(samples.as_slice()));
        assert_eq!(
            opened.erase_paths(stroke).unwrap()[0].samples(),
            erased.as_slice()
        );
        assert_eq!(
            opened.active_sketch().stroke(stroke).unwrap().own_size(),
            Some(7)
        );
        assert_eq!(
            opened.active_sketch().text_content(text).as_deref(),
            Some("Hello")
        );
        assert_eq!(opened.active_sketch().shape_box(shape), Some(shape_box));
        let layers = opened.layers();
        assert!(
            layers
                .iter()
                .any(|row| row.id == group && row.kind == draw::document::LayerKind::Group)
        );
        assert_eq!(
            layers
                .iter()
                .filter(|row| row.parent == Some(group))
                .count(),
            2
        );

        let max_node = layers.iter().map(|row| row.id.get()).max().unwrap();
        let roots = opened.active_sketch().root_len();
        opened.set_tool(Tool::Brush);
        commit_samples(&mut opened, &line(0.0, 40.0, 200.0, 10.0, None));
        assert_eq!(opened.active_sketch().root_len(), roots + 1);
        let made = opened.highlighted_id().expect("the new stroke's canvas");
        assert!(made.get() > max_node);
        assert!(opened.stroke_ids().iter().any(|id| id.get() > stroke.get()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_overwrites_its_file_and_a_failed_write_keeps_it() {
        let dir = temp_store("overwrite");
        let mut session = Session::new();
        session.set_data_dir(&dir);
        commit_samples(&mut session, &line(0.0, 30.0, 0.0, 10.0, None));
        let id = session.active_id();
        let path = drawings(&dir).join("notes");
        assert_eq!(session.save_path(id, &path, false, true), SaveResult::Wrote);
        let path = drawings(&dir).join("notes.smx");
        assert!(path.is_file(), "a name without .smx gained the suffix");
        assert_eq!(session.active_name(), "notes");
        let first = std::fs::read(&path).unwrap();
        commit_samples(&mut session, &line(0.0, 30.0, 40.0, 10.0, None));
        assert!(session.tabs()[0].needs_keep);
        assert_eq!(
            session.save_path(id, &path, false, false),
            SaveResult::Wrote
        );
        assert!(!session.tabs()[0].needs_keep);
        assert_eq!(
            session.file_path(id).as_deref(),
            Some(crate::store::normalize(&path).as_path())
        );
        let second = std::fs::read(&path).unwrap();
        assert_ne!(first, second);

        let recovery = dir.join("recovery").join(format!("{}.json", id.get()));
        assert!(!recovery.exists(), "a clean sketch wrote a recovery file");
        commit_samples(&mut session, &line(0.0, 20.0, 80.0, 8.0, None));
        session.recovery_tick();
        assert!(recovery.is_file());
        let saved = std::fs::read(&path).unwrap();
        let blocker = dir.join("not-a-directory");
        std::fs::write(&blocker, b"x").unwrap();
        assert_eq!(
            session.save_path(id, blocker.join("notes.smx"), false, true),
            SaveResult::Failed
        );
        assert!(session.tabs()[0].needs_keep);
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        assert_eq!(session.file_path(id).as_deref(), Some(path.as_path()));
        assert!(
            recovery.is_file(),
            "a failed save removed the recovery file"
        );
        assert_eq!(session.stroke_count(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_a_copy_leaves_the_path_and_an_open_path_focuses() {
        let dir = temp_store("copy");
        let drawings = drawings(&dir);
        let mut session = Session::new();
        session.set_data_dir(&dir);
        let home = session.dialog_directory();
        commit_samples(&mut session, &line(0.0, 20.0, 0.0, 10.0, None));
        let first = session.active_id();
        let original = drawings.join("one").join("Shared.smx");
        assert_eq!(
            session.save_path(first, &original, false, true),
            SaveResult::Wrote
        );
        assert_eq!(
            session.dialog_directory().as_deref(),
            Some(original.parent().unwrap())
        );
        assert_ne!(session.dialog_directory(), home);
        commit_samples(&mut session, &line(0.0, 20.0, 30.0, 10.0, None));
        session.recovery_tick();
        let recovery = dir.join("recovery").join(format!("{}.json", first.get()));
        assert!(recovery.is_file());
        let kept = std::fs::read(&original).unwrap();
        let copy = drawings.join("two").join("Shared.smx");
        assert_eq!(
            session.save_path(first, &copy, true, true),
            SaveResult::Wrote
        );
        assert_eq!(
            session.file_path(first).as_deref(),
            Some(crate::store::normalize(&original).as_path())
        );
        assert!(session.tabs()[0].needs_keep);
        assert!(recovery.is_file(), "Save a Copy removed the recovery file");
        assert_eq!(std::fs::read(&original).unwrap(), kept);
        assert!(copy.is_file());
        assert_eq!(session.active_name(), "Shared");

        session.new_sketch();
        session.set_tool(Tool::Rectangle);
        place_shape(&mut session, [0.0, 0.0], [30.0, 20.0], false);
        let second = session.active_id();
        let other = drawings.join("two").join("Other.smx");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        let shared_again = drawings.join("three").join("Shared.smx");
        assert_eq!(
            session.save_path(second, &shared_again, false, true),
            SaveResult::Wrote
        );
        assert_eq!(session.active_name(), "Shared");
        assert_ne!(session.file_path(first), session.file_path(second));

        let mut opened = Session::new();
        opened.set_data_dir(&dir);
        assert!(opened.open_path(&original));
        assert_eq!(opened.sketch_count(), 2, "Open leaves the blank Untitled");
        assert_eq!(opened.tabs()[0].name, "Untitled 1");
        let stroke_tab = opened.active_id();
        assert!(opened.open_path(&shared_again));
        assert_eq!(opened.sketch_count(), 3);
        assert_eq!(opened.stroke_count(), 0);
        assert_eq!(opened.active_name(), "Shared");
        let tabs = opened.sketch_count();
        assert!(opened.open_path(&original));
        assert_eq!(opened.sketch_count(), tabs);
        assert_eq!(opened.active_id(), stroke_tab);
        assert_eq!(opened.stroke_count(), 1);
        assert!(opened.selection().is_empty());
        assert!(opened.highlighted_id().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replacing_another_file_asks_and_the_sketch_file_does_not() {
        let dir = temp_store("replace");
        let drawings = drawings(&dir);
        let mut session = Session::new();
        session.set_data_dir(&dir);
        commit_samples(&mut session, &line(0.0, 16.0, 0.0, 8.0, None));
        let id = session.active_id();
        let own = drawings.join("own.smx");
        assert_eq!(session.save_path(id, &own, false, true), SaveResult::Wrote);
        commit_samples(&mut session, &line(0.0, 16.0, 20.0, 8.0, None));
        let own_bytes = std::fs::read(&own).unwrap();
        let other = drawings.join("other.smx");
        std::fs::write(&other, b"{\"format_version\":1}").unwrap();
        match session.save_path(id, &other, false, false) {
            SaveResult::NeedsReplace(path) => {
                assert_eq!(path, crate::store::normalize(&other));
            }
            other => panic!("expected a replace question, got {other:?}"),
        }
        assert_eq!(std::fs::read(&other).unwrap(), b"{\"format_version\":1}");
        assert_eq!(session.file_path(id).as_deref(), Some(own.as_path()));
        assert_eq!(std::fs::read(&own).unwrap(), own_bytes);
        assert!(session.tabs()[0].needs_keep);
        assert_eq!(session.save_path(id, &own, false, false), SaveResult::Wrote);
        assert!(!session.tabs()[0].needs_keep);
        commit_samples(&mut session, &line(0.0, 12.0, 40.0, 6.0, None));
        assert_eq!(
            session.save_path(id, &other, false, true),
            SaveResult::Wrote
        );
        assert_eq!(
            session.file_path(id).as_deref(),
            Some(crate::store::normalize(&other).as_path())
        );
        assert_eq!(session.active_name(), "other");
        assert_ne!(std::fs::read(&other).unwrap(), b"{\"format_version\":1}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recovery_does_not_change_the_smx_until_save_or_discard() {
        let dir = temp_store("recovery");
        let mut session = Session::new();
        session.set_data_dir(&dir);
        commit_samples(&mut session, &line(0.0, 24.0, 0.0, 8.0, None));
        let id = session.active_id();
        let path = drawings(&dir).join("kept.smx");
        assert_eq!(session.save_path(id, &path, false, true), SaveResult::Wrote);
        commit_samples(&mut session, &line(0.0, 24.0, 30.0, 8.0, None));
        let smx = std::fs::read(&path).unwrap();
        let start = std::time::Instant::now();
        session.poll_recovery(start);
        let recovery = dir.join("recovery").join(format!("{}.json", id.get()));
        assert!(!recovery.exists(), "recovery wrote before 10 minutes");
        session.poll_recovery(start + RECOVERY_INTERVAL - std::time::Duration::from_secs(1));
        assert!(!recovery.exists());
        session.poll_recovery(start + RECOVERY_INTERVAL);
        assert!(recovery.is_file());
        assert_eq!(std::fs::read(&path).unwrap(), smx);
        let stored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&recovery).unwrap()).unwrap();
        assert_eq!(
            stored["path"],
            session.file_path(id).unwrap().to_string_lossy().as_ref()
        );
        let first_recovery = std::fs::read(&recovery).unwrap();
        commit_samples(&mut session, &line(0.0, 12.0, 60.0, 6.0, None));
        session.recovery_tick();
        assert_ne!(std::fs::read(&recovery).unwrap(), first_recovery);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            smx,
            "a recovery tick wrote the .smx"
        );

        assert_eq!(session.save_path(id, &path, false, true), SaveResult::Wrote);
        assert!(!recovery.exists(), "Save left the recovery file");
        assert_ne!(std::fs::read(&path).unwrap(), smx);

        commit_samples(&mut session, &line(0.0, 12.0, 90.0, 6.0, None));
        session.recovery_tick();
        assert!(recovery.is_file());
        let after_save = std::fs::read(&path).unwrap();
        assert!(matches!(session.discard(id), CloseEffect::Closed { .. }));
        assert!(!recovery.exists(), "Discard left the recovery file");
        assert_eq!(std::fs::read(&path).unwrap(), after_save);

        let mut again = Session::new();
        again.set_data_dir(&dir);
        commit_samples(&mut again, &line(0.0, 20.0, 0.0, 8.0, None));
        let again_id = again.active_id();
        let again_path = drawings(&dir).join("later.smx");
        assert_eq!(
            again.save_path(again_id, &again_path, false, true),
            SaveResult::Wrote
        );
        commit_samples(&mut again, &line(0.0, 20.0, 24.0, 8.0, None));
        again.recovery_tick();
        let untouched = std::fs::read(&again_path).unwrap();
        drop(again);

        let mut restored = Session::new();
        restored.set_data_dir(&dir);
        let offers = restored.recovery_offers();
        assert_eq!(offers.len(), 1);
        assert!(restored.restore_recovery(offers[0].id));
        assert!(restored.tabs().iter().any(|tab| tab.needs_keep));
        assert_eq!(
            restored.file_path(restored.active_id()).as_deref(),
            Some(crate::store::normalize(&again_path).as_path())
        );
        assert!(restored.selection().is_empty());
        assert!(restored.highlighted_id().is_none());
        assert_eq!(restored.stroke_count(), 2);
        assert_eq!(std::fs::read(&again_path).unwrap(), untouched);
        assert_eq!(
            restored.save_path(restored.active_id(), &again_path, false, true),
            SaveResult::Wrote
        );
        assert_ne!(std::fs::read(&again_path).unwrap(), untouched);
        assert!(
            !dir.join("recovery")
                .join(format!("{}.json", offers[0].id.get()))
                .exists()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn several_recoveries_are_taken_one_at_a_time() {
        let dir = temp_store("offers");
        let mut session = Session::new();
        session.set_data_dir(&dir);
        commit_samples(&mut session, &line(0.0, 16.0, 0.0, 8.0, None));
        let first = session.active_id();
        session.new_sketch();
        commit_samples(&mut session, &line(0.0, 16.0, 10.0, 8.0, None));
        let second = session.active_id();
        session.recovery_tick();
        assert!(
            dir.join("recovery")
                .join(format!("{}.json", first.get()))
                .is_file()
        );
        assert!(
            dir.join("recovery")
                .join(format!("{}.json", second.get()))
                .is_file()
        );
        drop(session);

        let mut launched = Session::new();
        launched.set_data_dir(&dir);
        let offers = launched.recovery_offers();
        assert_eq!(offers.len(), 2);
        let before = launched.sketch_count();
        launched.discard_recovery_file(offers[0].id);
        assert_eq!(
            launched.sketch_count(),
            before,
            "Discard opened the recovery"
        );
        assert!(
            !dir.join("recovery")
                .join(format!("{}.json", offers[0].id.get()))
                .exists()
        );
        assert_eq!(launched.recovery_offers().len(), 1);
        assert_eq!(launched.recovery_offers()[0].id, offers[1].id);

        let mut next = Session::new();
        next.set_data_dir(&dir);
        assert_eq!(
            next.recovery_offers().len(),
            1,
            "a discarded recovery was offered again"
        );
        assert!(next.restore_recovery(offers[1].id));
        assert_eq!(next.sketch_count(), 2, "restore opened every recovery");
        assert!(next.tabs().iter().any(|tab| tab.needs_keep));
        assert!(next.selection().is_empty());
        assert!(next.highlighted_id().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_emergency_write_covers_each_dirty_sketch() {
        let dir = temp_store("panic");
        let mut session = Session::new();
        session.set_data_dir(&dir);
        commit_samples(&mut session, &line(0.0, 12.0, 0.0, 6.0, None));
        let dirty = session.active_id();
        let path = drawings(&dir).join("safe.smx");
        assert_eq!(
            session.save_path(dirty, &path, false, true),
            SaveResult::Wrote
        );
        commit_samples(&mut session, &line(0.0, 12.0, 20.0, 6.0, None));
        session.new_sketch();
        let clean = session.active_id();
        let smx = std::fs::read(&path).unwrap();
        session.write_emergency_recovery();
        assert!(
            dir.join("recovery")
                .join(format!("{}.json", dirty.get()))
                .is_file()
        );
        assert!(
            !dir.join("recovery")
                .join(format!("{}.json", clean.get()))
                .exists()
        );
        assert_eq!(std::fs::read(&path).unwrap(), smx);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreadable_files_are_skipped_and_leftover_snapshots_stay() {
        let dir = temp_store("skip");
        let junk = dir.join("sketch-4.json");
        std::fs::write(&junk, b"{").unwrap();
        std::fs::create_dir_all(dir.join("recovery")).unwrap();
        std::fs::write(dir.join("recovery").join("9.json"), b"{").unwrap();
        let bad = drawings(&dir).join("bad.smx");
        std::fs::write(&bad, b"{").unwrap();
        let mut session = Session::new();
        session.set_data_dir(&dir);
        assert!(session.recovery_offers().is_empty());
        assert!(session.command_enabled(CommandId::Open));
        assert!(session.command_enabled(CommandId::Save));
        assert!(session.command_enabled(CommandId::SaveAs));
        assert!(session.command_enabled(CommandId::SaveCopy));
        session.perform(CommandId::Open, [0.0, 0.0]);
        assert_eq!(session.file_intent(), Some(FileIntent::Open));
        assert!(!session.open_path(&bad));
        assert_eq!(session.sketch_count(), 1);
        let mut ids = vec![session.active_id().get()];
        for _ in 0..6 {
            ids.push(session.new_sketch().get());
        }
        assert!(!ids.contains(&4), "a leftover snapshot id was reused");
        assert!(!ids.contains(&9), "an unreadable recovery id was reused");
        commit_samples(&mut session, &line(0.0, 16.0, 0.0, 8.0, None));
        let id = session.active_id();
        assert_eq!(
            session.save_path(id, drawings(&dir).join("kept.smx"), false, true),
            SaveResult::Wrote
        );
        assert_eq!(std::fs::read(&junk).unwrap(), b"{");
        assert_eq!(std::fs::read(&bad).unwrap(), b"{");
        assert_eq!(
            std::fs::read(dir.join("recovery").join("9.json")).unwrap(),
            b"{"
        );
        std::fs::write(
            dir.join("recovery").join("3.json"),
            std::fs::read(drawings(&dir).join("kept.smx")).unwrap(),
        )
        .unwrap();
        let mut later = Session::new();
        later.set_data_dir(&dir);
        assert!(
            later
                .recovery_offers()
                .iter()
                .all(|offer| offer.id.get() != 3),
            "a recovery whose id disagrees with its name was offered"
        );
        assert_eq!(std::fs::read(&junk).unwrap(), b"{");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_file_does_not_alias_live_ids() {
        let dir = temp_store("alias");
        let mut session = Session::new();
        session.set_data_dir(&dir);
        commit_samples(&mut session, &line(0.0, 20.0, 0.0, 8.0, None));
        let source = session
            .swatches()
            .iter()
            .find(|swatch| swatch.active)
            .unwrap()
            .id;
        let original = session.active_id();
        let stroke = session.stroke_ids()[0];
        let canvas = session
            .layers()
            .iter()
            .find(|row| row.kind == draw::document::LayerKind::Canvas)
            .unwrap()
            .id;
        let layer = session
            .active_sketch()
            .layer_for_source(canvas, source)
            .unwrap();
        let path = drawings(&dir).join("original.smx");
        assert_eq!(
            session.save_path(original, &path, false, true),
            SaveResult::Wrote
        );
        let copy = drawings(&dir).join("copy.smx");
        std::fs::copy(&path, &copy).unwrap();
        assert!(session.open_path(&copy));
        assert_eq!(session.sketch_count(), 2);
        assert_ne!(session.active_id(), original);
        assert!(session.selection().is_empty());
        assert!(session.highlighted_id().is_none());
        let copied_canvas = session
            .layers()
            .iter()
            .find(|row| row.kind == draw::document::LayerKind::Canvas)
            .unwrap()
            .id;
        assert_ne!(copied_canvas, canvas);
        let copied_layer = session
            .active_sketch()
            .layer_for_source(copied_canvas, source)
            .expect("the layer source changed");
        assert_ne!(copied_layer, layer);
        assert_ne!(session.stroke_ids()[0], stroke);
        session.activate(original);
        assert_eq!(session.stroke_ids()[0], stroke);
        assert_eq!(
            session.active_sketch().layer_for_source(canvas, source),
            Some(layer)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recent_paths_keep_the_last_ten_and_skip_missing() {
        let dir = temp_store("recent");
        let drawings = drawings(&dir);
        let mut session = Session::new();
        session.set_data_dir(&dir);
        let mut paths = Vec::new();
        for index in 0..11 {
            if index > 0 {
                session.new_sketch();
            }
            commit_samples(&mut session, &line(0.0, 8.0, index as f64, 4.0, None));
            let path = drawings.join(format!("n{index}.smx"));
            assert_eq!(
                session.save_path(session.active_id(), &path, false, true),
                SaveResult::Wrote
            );
            paths.push(crate::store::normalize(&path));
        }
        let shown = session.recent_menu();
        assert_eq!(shown.len(), 10);
        assert_eq!(shown[0], paths[10]);
        assert!(!shown.contains(&paths[0]));
        assert_eq!(
            session.save_path(session.active_id(), &paths[5], true, true),
            SaveResult::Wrote
        );
        assert_eq!(session.recent_menu()[0], paths[5]);
        std::fs::write(&paths[5], b"{").unwrap();
        assert!(!session.recent_menu().contains(&paths[5]));
        let broken = drawings.join("broken.smx");
        std::fs::write(&broken, b"{").unwrap();
        let open = session.sketch_count();
        assert!(!session.open_path(&broken));
        assert_eq!(session.sketch_count(), open);
        let _ = crate::prefs::save_tools(&dir, &session.tool_snapshot());
        let (tools, stored) = crate::prefs::load_dir(&dir);
        assert_eq!(tools.eraser_size, session.tool_snapshot().eraser_size);
        assert!(stored.contains(&paths[10]));
        assert!(stored.contains(&paths[5]));
        assert!(!session.recent_menu().iter().any(|path| path == &paths[5]));
        let mut reloaded = Session::new();
        reloaded.set_data_dir(&dir);
        reloaded.set_recent(stored);
        assert_eq!(reloaded.recent_menu()[0], paths[10]);
        assert!(!reloaded.recent_menu().contains(&paths[5]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_stores_words_still_being_typed() {
        let dir = temp_store("draft");
        let mut session = Session::new();
        session.set_data_dir(&dir);
        session.set_tool(Tool::Text);
        session.begin_text([10.0, 20.0]).unwrap();
        session.end_text(false).unwrap();
        session.set_text_draft("Hello".into());
        assert!(session.text_editing());
        let id = session.active_id();
        let path = drawings(&dir).join("words.smx");
        assert_eq!(session.save_path(id, &path, false, true), SaveResult::Wrote);
        assert!(!session.text_editing());
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            raw.contains("\"Hello\""),
            "words still being typed are missing from the file"
        );
        let mut opened = Session::new();
        opened.set_data_dir(&dir);
        assert!(opened.open_initial(&path));
        let text = opened.active_sketch().root_at(0).unwrap();
        assert_eq!(
            opened.active_sketch().text_content(text).as_deref(),
            Some("Hello")
        );
        assert!(opened.selection().is_empty());
        assert!(opened.highlighted_id().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn opening_a_sketch_keeps_its_camera_through_the_first_view() {
        let dir = temp_store("camera");
        let mut session = Session::new();
        session.set_data_dir(&dir);
        session.pan_screen([80.0, -30.0]);
        let origin = session.camera_origin();
        let zoom = session.zoom();
        let path = drawings(&dir).join("view.smx");
        assert_eq!(
            session.save_path(session.active_id(), &path, false, true),
            SaveResult::Wrote
        );
        let mut opened = Session::new();
        opened.set_data_dir(&dir);
        assert!(opened.open_initial(&path));
        opened.present_view([1000.0, 800.0]);
        assert_eq!(opened.camera_origin(), origin);
        assert_eq!(opened.zoom(), zoom);
        assert!(
            !opened.tabs()[0].needs_keep,
            "the first view marked an opened sketch dirty"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_blank_does_not_replace_a_recovery_offer() {
        let dir = temp_store("offer-id");
        let mut session = Session::new();
        commit_samples(&mut session, &line(0.0, 16.0, 0.0, 6.0, None));
        session.set_data_dir(&dir);
        assert_eq!(session.active_id().get(), 1);
        let path = drawings(&dir).join("kept.smx");
        assert_eq!(
            session.save_path(session.active_id(), &path, false, true),
            SaveResult::Wrote
        );
        commit_samples(&mut session, &line(0.0, 16.0, 20.0, 6.0, None));
        session.recovery_tick();
        let offer = dir.join("recovery").join("1.json");
        let original = std::fs::read(&offer).unwrap();
        drop(session);

        let mut blank = Session::new();
        blank.set_data_dir(&dir);
        assert_ne!(
            blank.active_id().get(),
            1,
            "the blank reused a recovery offer's id"
        );
        commit_samples(&mut blank, &line(0.0, 10.0, 4.0, 4.0, None));
        blank.recovery_tick();
        assert_eq!(
            std::fs::read(&offer).unwrap(),
            original,
            "the blank replaced a recovery offer"
        );

        let mut opened = Session::new();
        opened.set_data_dir(&dir);
        assert!(opened.open_initial(&path));
        assert_eq!(opened.active_id().get(), 1);
        commit_samples(&mut opened, &line(0.0, 8.0, 40.0, 4.0, None));
        opened.recovery_tick();
        assert_eq!(
            std::fs::read(&offer).unwrap(),
            original,
            "an opened sketch replaced a recovery offer"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restoring_an_open_path_replaces_that_tab() {
        let dir = temp_store("replace-tab");
        let mut session = Session::new();
        session.set_data_dir(&dir);
        commit_samples(&mut session, &line(0.0, 18.0, 0.0, 6.0, None));
        let path = drawings(&dir).join("notes.smx");
        assert_eq!(
            session.save_path(session.active_id(), &path, false, true),
            SaveResult::Wrote
        );
        commit_samples(&mut session, &line(0.0, 18.0, 24.0, 6.0, None));
        session.recovery_tick();
        let saved = std::fs::read(&path).unwrap();
        drop(session);

        let mut opened = Session::new();
        opened.set_data_dir(&dir);
        assert!(opened.open_path(&path));
        assert_eq!(opened.sketch_count(), 2);
        let offers = opened.recovery_offers();
        assert_eq!(offers.len(), 1);
        let recovery = dir
            .join("recovery")
            .join(format!("{}.json", offers[0].id.get()));
        assert!(opened.restore_recovery(offers[0].id));
        assert_eq!(
            opened.sketch_count(),
            2,
            "restore added a second tab for the open file"
        );
        let aimed = opened
            .tabs()
            .iter()
            .filter(|tab| {
                opened.file_path(tab.id).as_deref()
                    == Some(crate::store::normalize(&path).as_path())
            })
            .count();
        assert_eq!(aimed, 1);
        assert_eq!(opened.stroke_count(), 2);
        assert!(opened.tabs().iter().any(|tab| tab.needs_keep));
        assert!(recovery.is_file(), "restore deleted the recovery file");
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn words_still_being_typed_are_recovered_and_block_quit() {
        let dir = temp_store("draft-recovery");
        let mut session = Session::new();
        session.set_data_dir(&dir);
        commit_samples(&mut session, &line(0.0, 12.0, 0.0, 4.0, None));
        let id = session.active_id();
        let path = drawings(&dir).join("words.smx");
        assert_eq!(session.save_path(id, &path, false, true), SaveResult::Wrote);
        let saved = std::fs::read(&path).unwrap();
        session.set_tool(Tool::Text);
        session.begin_text([10.0, 20.0]).unwrap();
        session.end_text(false).unwrap();
        session.set_text_draft("Hello".into());
        assert!(session.text_editing());
        assert!(
            !session.tabs()[0].needs_keep,
            "the saved sketch was already dirty"
        );
        assert!(
            session.quit_needs_keep(),
            "words still being typed allowed a clean quit"
        );
        let start = std::time::Instant::now();
        session.poll_recovery(start);
        session.poll_recovery(start + RECOVERY_INTERVAL);
        let recovery = dir.join("recovery").join(format!("{}.json", id.get()));
        let raw = std::fs::read_to_string(&recovery).unwrap();
        assert!(
            raw.contains("\"Hello\""),
            "a recovery copy omitted words still being typed"
        );
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_recovery_write_does_not_skip_the_interval() {
        let dir = temp_store("retry");
        let mut session = Session::new();
        session.set_data_dir(&dir);
        commit_samples(&mut session, &line(0.0, 14.0, 0.0, 5.0, None));
        let id = session.active_id();
        let start = std::time::Instant::now();
        session.poll_recovery(start);
        let recovery = dir.join("recovery").join(format!("{}.json", id.get()));
        assert!(!recovery.exists());
        std::fs::write(dir.join("recovery"), b"x").unwrap();
        session.poll_recovery(start + RECOVERY_INTERVAL);
        assert!(
            !recovery.exists(),
            "a blocked recovery directory still wrote a file"
        );
        std::fs::remove_file(dir.join("recovery")).unwrap();
        session.poll_recovery(start + RECOVERY_INTERVAL);
        assert!(
            recovery.is_file(),
            "a failed recovery write skipped the interval"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
