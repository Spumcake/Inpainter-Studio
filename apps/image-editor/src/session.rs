//! The Image Editor's tabs, each a gallery of layered pictures, and
//! everything the host sends them: commands, gestures, panel edits, the
//! filmstrip's clicks, files, and the project.
//!
//! Inside a project, which the host names with `project`, every image is
//! written into the project's folder by itself, a second after it changes,
//! and before its tab closes, a `keep` is answered, or the host quits, as
//! Mission Control's notebooks are. Save and Save As are offered only
//! outside a project.

use crate::declare;
use crate::doc::{Doc, Setting};
use crate::files;
use crate::gallery::{Frame, Gallery};
use crate::paint::{Brush, Combine, Mask};
use crate::picture::{Blend, Picture, Pixels};
use protocol::{
    AppMessage, Ask, AskButton, ContentsKind, DocumentCamera, FileFilter, FileKind, HostMessage, PanelSide,
    PropValue, RasterGesture, RasterTool, RasterView, Thumbnail,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How many files of rows sent while a stroke is drawn are kept for each
/// picture, for a host that falls behind; one that falls further waits for
/// the stroke's end.
const KEPT_PATCHES: usize = 32;

/// How long a change waits before it is written, so a burst of edits is
/// written once.
const WRITE_AFTER: Duration = Duration::from_secs(1);
/// How long a failed write waits before it is tried again by itself.
const RETRY_AFTER: Duration = Duration::from_secs(5);
/// A new picture's name, as the owner set on 2026-10-30: it is an asset.
pub const UNTITLED: &str = "Untitled Asset";
/// A new image's size, as the owner set on 2026-10-30.
pub const NEW_SIZE: [u32; 2] = [1920, 1080];
/// The largest a layer's thumbnail is, in pixels.
const THUMB_SIZE: [u32; 2] = [56, 40];
/// The largest a filmstrip thumbnail is, in pixels: 44 tall, as the gallery
/// views draw them, and no wider than four times that.
const FILM_SIZE: [u32; 2] = [176, 44];
/// The space kept around a picture when it is fitted to the window.
const FIT_MARGIN: f64 = 40.0;

/// The tools of the toolbar, in its order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Move,
    Marquee,
    Lasso,
    Crop,
    Eyedropper,
    Brush,
    Eraser,
    Hand,
    Zoom,
}

impl ToolKind {
    pub const ALL: [ToolKind; 9] = [
        ToolKind::Move,
        ToolKind::Marquee,
        ToolKind::Lasso,
        ToolKind::Crop,
        ToolKind::Eyedropper,
        ToolKind::Brush,
        ToolKind::Eraser,
        ToolKind::Hand,
        ToolKind::Zoom,
    ];

    pub fn key(self) -> &'static str {
        match self {
            ToolKind::Move => "move",
            ToolKind::Marquee => "marquee",
            ToolKind::Lasso => "lasso",
            ToolKind::Crop => "crop",
            ToolKind::Eyedropper => "eyedropper",
            ToolKind::Brush => "brush",
            ToolKind::Eraser => "eraser",
            ToolKind::Hand => "hand",
            ToolKind::Zoom => "zoom",
        }
    }

    pub fn named(key: &str) -> Option<ToolKind> {
        ToolKind::ALL.into_iter().find(|tool| tool.key() == key)
    }
}

/// A brush's or the eraser's settings, as the settings row shows them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushSettings {
    /// In picture pixels.
    pub size: u32,
    /// Percents.
    pub hardness: u32,
    pub opacity: u32,
    pub pressure: bool,
}

/// What the toolbar and its settings row hold, for every tab.
#[derive(Debug, Clone)]
pub struct Tools {
    pub tool: ToolKind,
    pub rgb: [u8; 3],
    pub brush: BrushSettings,
    pub eraser: BrushSettings,
    pub combine: Combine,
    pub feather: f64,
    /// An index into `declare::RATIOS`.
    pub ratio: usize,
}

impl Default for Tools {
    /// As brush.png shows: black, 24 px, 80 % hard, 100 % opaque, pressure on.
    fn default() -> Self {
        let brush = BrushSettings { size: 24, hardness: 80, opacity: 100, pressure: true };
        Self {
            tool: ToolKind::Brush,
            rgb: [0, 0, 0],
            brush,
            eraser: brush,
            combine: Combine::New,
            feather: 0.0,
            ratio: 0,
        }
    }
}

/// One tab: a gallery of pictures, kept in one file.
pub struct Image {
    pub id: String,
    pub label: String,
    pub path: Option<PathBuf>,
    pub gallery: Gallery,
    pub dirty: bool,
    /// The filmstrip, not the picture, has the keyboard: Undo and Redo act
    /// on the strip's history until the picture is used again.
    pub strip_focus: bool,
    /// Inside a project: when its change was first seen unwritten, and its
    /// last failed write, why and when.
    unwritten_since: Option<Instant>,
    write_failed: Option<(String, Instant)>,
    /// Being written by the writer's thread: what the image was, and where.
    saving: Option<(Signature, PathBuf)>,
    /// A camera to send with the next picture: the picture is new, opened,
    /// or newly shown.
    send_camera: bool,
}

impl Image {
    /// The picture shown.
    pub fn frame(&self) -> &Frame {
        self.gallery.frame()
    }

    pub fn frame_mut(&mut self) -> &mut Frame {
        self.gallery.frame_mut()
    }

    /// The shown picture's document.
    pub fn doc(&self) -> &Doc {
        &self.gallery.frame().doc
    }

    pub fn doc_mut(&mut self) -> &mut Doc {
        &mut self.gallery.frame_mut().doc
    }

    /// Whether Undo, as the keyboard has it now, has something to undo.
    pub fn can_undo(&self) -> bool {
        if self.strip_focus { self.gallery.can_undo() } else { self.doc().can_undo() }
    }

    pub fn can_redo(&self) -> bool {
        if self.strip_focus { self.gallery.can_redo() } else { self.doc().can_redo() }
    }
}

/// What a file dialog's answer is for.
#[derive(Debug, Clone)]
enum Purpose {
    Open,
    OpenLayer,
    /// The filmstrip's "Open PNG or JPEG…": a new picture in this tab.
    OpenPicture { tab: String },
    Export { jpeg: bool },
    Save { tab: String, then_quit: bool },
}

pub struct Session {
    pub images: Vec<Image>,
    pub active: usize,
    next_tab: u64,
    pub tools: Tools,
    pub panel_open: bool,
    pub project: Option<PathBuf>,
    /// Where pictures and thumbnails for the host are written.
    cache: PathBuf,
    viewport: [f64; 2],
    file: Option<(String, Purpose)>,
    next_file: u64,
    /// Every picture file and thumbnail written gets the next revision. It
    /// starts from the time the program started, so a picture of an earlier
    /// run is never taken for a current one.
    next_revision: u64,
    /// Points of a stroke were painted and the picture is not sent yet.
    live_due: bool,
    /// The thread that writes images due inside a project, started with the
    /// first.
    writer: Option<Writer>,
    /// Tab ids still to ask about while quitting, after the one asked.
    quit_rest: Vec<String>,
    quitting: bool,
    /// A close of one image is asking, rather than the window's quit.
    closing: bool,
}

impl Session {
    /// Launched: one new image, already declared.
    pub fn launch() -> Self {
        let mut session = Self::empty();
        session.push_image(new_gallery(), None, None);
        session
    }

    /// Started with `--restore`: no image until the host sends `open`.
    pub fn restored() -> Self {
        Self::empty()
    }

    fn empty() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64 * 1000)
            .unwrap_or(0);
        Self {
            images: Vec::new(),
            active: 0,
            next_tab: 1,
            tools: Tools::default(),
            panel_open: true,
            project: None,
            cache: scratch_folder(),
            viewport: [1200.0, 800.0],
            file: None,
            next_file: 1,
            next_revision: seed,
            live_due: false,
            writer: None,
            quit_rest: Vec::new(),
            quitting: false,
            closing: false,
        }
    }

    pub fn greeting(&mut self) -> Vec<AppMessage> {
        let mut messages = vec![AppMessage::Hello { protocol: protocol::PROTOCOL_VERSION }];
        if let Some(image) = self.images.first() {
            messages.push(AppMessage::Tab { id: image.id.clone(), label: image.label.clone() });
            messages.push(AppMessage::Contents { tab: image.id.clone(), kind: ContentsKind::Raster });
            messages.push(AppMessage::Panel { tab: image.id.clone(), side: PanelSide::Right });
        }
        messages.push(AppMessage::Skills { skills: crate::assist::skills() });
        messages.push(AppMessage::End);
        if !self.images.is_empty() {
            messages.extend(self.emit());
        }
        messages
    }

    /// One message from the host, and every reply.
    pub fn handle(&mut self, message: HostMessage) -> Vec<AppMessage> {
        let mut out = Vec::new();
        // A stroke being drawn lasts while its points come. A message that
        // could change the picture takes it off first; the host's ping each
        // second, the camera, and the viewport leave it be, so a stroke held
        // longer than a second stays.
        let drawing = matches!(
            &message,
            HostMessage::RasterGesture { gesture: RasterGesture::StrokePart { .. } | RasterGesture::Stroke { .. }, .. }
                | HostMessage::Camera { .. }
                | HostMessage::Viewport { .. }
                | HostMessage::Ping
                | HostMessage::Keep { .. }
                | HostMessage::Claim { .. }
        );
        if !drawing {
            for image in &mut self.images {
                for frame in &mut image.gallery.frames {
                    frame.doc.end_live();
                }
            }
        }
        match message {
            HostMessage::Shutdown => return Vec::new(),
            HostMessage::Ping => return vec![AppMessage::Pong],
            HostMessage::Quit => {
                if self.project.is_some() {
                    // Everything is written first, so the quit asks nothing.
                    for index in 0..self.images.len() {
                        let _ = self.write_into_project(index);
                    }
                }
                self.note_quit(&mut out);
            }
            HostMessage::Project { folder } => {
                let folder = PathBuf::from(folder);
                let folder = folder.canonicalize().unwrap_or(folder);
                if self.project.is_none() {
                    // The scratch pictures written before the project was
                    // known are not needed any more.
                    let _ = std::fs::remove_dir_all(&self.cache);
                }
                self.cache = folder.join(".inpainter").join("cache").join("image-editor");
                self.project = Some(folder);
                // Rows a stroke left there in an earlier run are not needed.
                for entry in std::fs::read_dir(&self.cache).into_iter().flatten().flatten() {
                    if entry.path().extension().is_some_and(|extension| extension == "patch") {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
                // Every picture is written again where the host now looks.
                for image in &mut self.images {
                    for frame in &mut image.gallery.frames {
                        frame.written = None;
                        frame.thumbs.clear();
                        frame.film = None;
                    }
                }
                out.extend(self.emit());
            }
            // An image file from outside the project opens nothing.
            HostMessage::Open { path: Some(path) }
                if files::is_ipx(Path::new(&path)) && !self.in_project(Path::new(&path)) => {}
            HostMessage::Open { path } => {
                self.open_path(path.map(PathBuf::from), &mut out);
            }
            HostMessage::Command { id, tab, arg, viewport, .. } => {
                if viewport[0] > 0.0 && viewport[1] > 0.0 {
                    self.viewport = viewport;
                }
                if let Some(tab) = &tab {
                    self.activate(tab);
                }
                if !keeps_focus(&id) {
                    self.picture_focus();
                }
                self.command(&id, tab.as_deref(), arg.unwrap_or(Value::Null), &mut out);
            }
            // The newest points of a stroke: painted at once, and the
            // picture sent when the lines waiting have all been read.
            HostMessage::RasterGesture { tab, gesture: RasterGesture::StrokePart { from, points } } => {
                self.activate(&tab);
                self.picture_focus();
                if let Some(brush) = self.brush()
                    && let Some(image) = self.images.get_mut(self.active)
                {
                    image.doc_mut().paint_live(from, &points, &brush);
                    self.live_due = true;
                }
            }
            HostMessage::RasterGesture { tab, gesture } => {
                self.activate(&tab);
                self.picture_focus();
                self.gesture(gesture);
                out.extend(self.emit());
            }
            HostMessage::Camera { tab, origin, zoom } => {
                if let Some(image) = self.images.iter_mut().find(|image| image.id == tab) {
                    let frame = image.frame_mut();
                    frame.camera = Some(DocumentCamera { origin, zoom });
                    frame.host_camera = true;
                    // The zoom menu shows the new percent.
                    out.push(AppMessage::Toolbar(declare::toolbar(self)));
                }
            }
            HostMessage::Viewport { viewport } => {
                if viewport[0] > 0.0 && viewport[1] > 0.0 && viewport != self.viewport {
                    self.viewport = viewport;
                    // A picture fitted before the window's size was known is
                    // fitted again, unless the host has moved its camera.
                    for image in &mut self.images {
                        if !image.frame().host_camera {
                            image.send_camera = true;
                        }
                    }
                    out.extend(self.emit());
                }
            }
            HostMessage::Answer { id, button } => {
                if id == "keep-image" {
                    self.answer(&button, &mut out);
                }
            }
            HostMessage::Property { prop, value, preview } => {
                self.picture_focus();
                self.property(&prop, &value, preview);
                out.extend(self.emit());
            }
            HostMessage::FileResult { id, path } => self.file_result(&id, path, &mut out),
            HostMessage::Invoke { call, tab, tool, input, group } => {
                out.extend(self.invoke(call, &tab, &tool, input, &group));
            }
            HostMessage::Keep { tab } => {
                let kept = match self.images.iter().position(|image| image.id == tab) {
                    Some(index) if self.project.is_some() => self.write_into_project(index),
                    Some(index) => Ok(self.images[index].path.clone()),
                    None => Ok(None),
                };
                out.extend(self.declared());
                out.push(match kept {
                    Ok(path) => AppMessage::Kept { tab, path: path.map(|path| path.display().to_string()), error: None },
                    Err(error) => AppMessage::Kept { tab, path: None, error: Some(error) },
                });
            }
            HostMessage::Claim { id, path } => {
                let path = Path::new(&path);
                out.push(AppMessage::Claimed { id, yours: files::is_ipx(path) && self.in_project(path) });
            }
            _ => {}
        }
        if self.finish_writes(false) {
            out.extend(self.declared());
        }
        out
    }

    /// The loop's wake with no message: writes what is due inside a project,
    /// and sends what that changed, such as a tab's new name.
    pub fn tick(&mut self) -> Vec<AppMessage> {
        let mut out = self.catch_up();
        if self.project.is_some() {
            self.write_due(Instant::now());
        }
        if self.finish_writes(false) {
            out.extend(self.declared());
        }
        out
    }

    /// Writes whole every picture file a stroke left behind, while nothing
    /// is being drawn, and tells the host the picture shown is whole again.
    fn catch_up(&mut self) -> Vec<AppMessage> {
        if self.images.iter().any(|image| image.gallery.frames.iter().any(|frame| frame.doc.drawing())) {
            return Vec::new();
        }
        let cache = self.cache.clone();
        let mut shown = false;
        for (index, image) in self.images.iter_mut().enumerate() {
            let shown_uid = image.frame().uid;
            for frame in &mut image.gallery.frames {
                if !frame.behind {
                    continue;
                }
                let picture = cache.join(format!("{}-{}.rgba", image.id, frame.uid));
                if files::write_atomic(&picture, frame.doc.composite()).is_ok() {
                    caught_up(frame);
                    shown |= index == self.active && frame.uid == shown_uid;
                }
            }
        }
        if !shown {
            return Vec::new();
        }
        self.raster_view(self.active, false).map(AppMessage::Raster).into_iter().collect()
    }

    /// Inside a project, writes every image not yet written, as when the
    /// host has gone. Outside one, the scratch pictures are removed.
    pub fn write_all(&mut self) {
        if self.project.is_some() {
            for index in 0..self.images.len() {
                let _ = self.write_into_project(index);
            }
        } else {
            let _ = std::fs::remove_dir_all(&self.cache);
        }
    }

    fn in_project(&self, path: &Path) -> bool {
        let Some(project) = &self.project else {
            return true;
        };
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        path.starts_with(project)
    }

    pub fn image_index(&self, tab: &str) -> Option<usize> {
        self.images.iter().position(|image| image.id == tab)
    }

    fn activate(&mut self, tab: &str) {
        if let Some(index) = self.image_index(tab) {
            self.active = index;
        }
    }

    pub fn active_image(&self) -> Option<&Image> {
        self.images.get(self.active)
    }

    /// The picture, not the filmstrip, has the keyboard again.
    fn picture_focus(&mut self) {
        if let Some(image) = self.images.get_mut(self.active) {
            image.strip_focus = false;
        }
    }

    fn push_image(&mut self, mut gallery: Gallery, path: Option<PathBuf>, label: Option<String>) -> usize {
        let id = self.next_tab.to_string();
        self.next_tab += 1;
        let label = label
            .or_else(|| path.as_deref().map(file_title))
            .unwrap_or_else(|| String::from(UNTITLED));
        let frame = gallery.frame_mut();
        frame.camera = Some(fit(frame.doc.picture.width, frame.doc.picture.height, self.viewport));
        self.images.push(Image {
            id,
            label,
            path,
            gallery,
            dirty: false,
            strip_focus: false,
            unwritten_since: None,
            write_failed: None,
            saving: None,
            send_camera: true,
        });
        self.active = self.images.len() - 1;
        if self.tools.tool == ToolKind::Crop {
            self.start_crop();
        }
        self.active
    }

    /// A tab made after the greeting declares its contents and panel, then
    /// everything else.
    fn declare_new(&mut self, out: &mut Vec<AppMessage>) {
        let tab = self.images[self.active].id.clone();
        out.push(AppMessage::Contents { tab: tab.clone(), kind: ContentsKind::Raster });
        out.push(AppMessage::Panel { tab, side: PanelSide::Right });
        out.extend(self.emit());
    }

    /// Runs `change` on the shown picture's document, and marks the image
    /// changed when it was.
    fn change<T>(&mut self, change: impl FnOnce(&mut Doc) -> T) -> Option<T> {
        let image = self.images.get_mut(self.active)?;
        let before = image.doc().edits;
        let result = change(image.doc_mut());
        if image.doc().edits != before {
            image.dirty = true;
        }
        Some(result)
    }

    /// Runs `change` on the active tab's gallery: the strip shows what it
    /// changed, and its new picture gets its camera.
    fn film<T>(&mut self, change: impl FnOnce(&mut Gallery) -> T) -> Option<T> {
        let index = self.active;
        let result = self.film_of(index, change);
        if self.tools.tool == ToolKind::Crop && self.images.get(index).is_some_and(|image| image.frame().crop.is_none()) {
            self.start_crop();
        }
        result
    }

    fn film_of<T>(&mut self, index: usize, change: impl FnOnce(&mut Gallery) -> T) -> Option<T> {
        let in_project = self.project.is_some();
        let image = self.images.get_mut(index)?;
        // A held slider on the picture shown becomes its step first.
        image.doc_mut().finish_edit();
        let (edits, shown) = (image.gallery.edits, image.frame().uid);
        let result = change(&mut image.gallery);
        // Which picture is shown is kept too, but only a project's file is
        // written for it by itself: outside one, it asks nothing on close.
        if image.gallery.edits != edits || (in_project && image.frame().uid != shown) {
            image.dirty = true;
        }
        if image.frame().uid != shown {
            image.send_camera = true;
        }
        Some(result)
    }

    fn command(&mut self, id: &str, tab: Option<&str>, arg: Value, out: &mut Vec<AppMessage>) {
        let number = |key: &str| arg.get(key).and_then(Value::as_f64);
        let node = arg.get("node").and_then(Value::as_u64);
        match id {
            "new_image" => {
                self.push_image(new_gallery(), None, None);
                self.declare_new(out);
                return;
            }
            "open" => {
                self.ask_file(FileKind::Open, None, Purpose::Open, image_filter(true), out);
                return;
            }
            "open_layer" => {
                self.ask_file(FileKind::Open, None, Purpose::OpenLayer, image_filter(false), out);
                return;
            }
            "export_png" | "export_jpeg" => {
                let jpeg = id == "export_jpeg";
                let Some(image) = self.active_image() else {
                    return;
                };
                // The picture shown is exported, under its own name.
                let name = format!("{}.{}", image.frame().name, if jpeg { "jpg" } else { "png" });
                let filter = if jpeg {
                    FileFilter { name: "JPEG".into(), extensions: vec!["jpg".into(), "jpeg".into()] }
                } else {
                    FileFilter { name: "PNG".into(), extensions: vec!["png".into()] }
                };
                self.ask_file(FileKind::Save, Some(name), Purpose::Export { jpeg }, filter, out);
                return;
            }
            "save" | "save_as" if self.project.is_some() => {
                let _ = self.write_into_project(self.active);
            }
            "save" | "save_as" => {
                let Some(image) = self.active_image() else {
                    return;
                };
                match (&image.path, id) {
                    (Some(path), "save") => {
                        let path = path.clone();
                        self.write_at(self.active, &path, false, out);
                    }
                    _ => {
                        let purpose = Purpose::Save { tab: image.id.clone(), then_quit: false };
                        let name = format!("{}.ipx", image.label);
                        self.ask_file(FileKind::Save, Some(name), purpose, ipx_filter(), out);
                    }
                }
                return;
            }
            // While the strip has the keyboard, Undo and Redo act on its
            // own history; otherwise on the picture shown, and only it.
            "undo" | "redo" if self.active_image().is_some_and(|image| image.strip_focus) => {
                self.film(if id == "undo" { Gallery::undo } else { Gallery::redo });
            }
            "undo" | "redo" => {
                let size = self.active_image().map(|image| (image.doc().picture.width, image.doc().picture.height));
                self.change(if id == "undo" { Doc::undo } else { Doc::redo });
                // A crop undone or redone: the box starts again from the
                // whole picture.
                let now = self.active_image().map(|image| (image.doc().picture.width, image.doc().picture.height));
                if self.tools.tool == ToolKind::Crop && size != now {
                    self.start_crop();
                }
            }
            // A press on the filmstrip: it has the keyboard.
            "film_focus" => {
                if let Some(image) = self.images.get_mut(self.active) {
                    image.strip_focus = true;
                }
            }
            // Choosing a picture shows it with the keyboard: Undo is its
            // own, as the person who clicks a thumbnail and presses Undo
            // expects. The +, a thumbnail's menu, and a drag leave the
            // keyboard with the strip.
            "film_select" => {
                if let Some(index) = number("value") {
                    self.film(|gallery| gallery.select(index.max(0.0) as usize));
                }
                self.picture_focus();
            }
            "film_reorder" => {
                if let (Some(from), Some(to)) = (number("from"), number("to")) {
                    self.film(|gallery| gallery.move_to(from.max(0.0) as usize, to.max(0.0) as usize));
                }
            }
            "film_new" => {
                self.film(|gallery| gallery.add(UNTITLED, new_doc(), None));
            }
            "film_open" => {
                let Some(image) = self.active_image() else {
                    return;
                };
                let purpose = Purpose::OpenPicture { tab: image.id.clone() };
                self.ask_file(FileKind::Open, None, purpose, image_filter(false), out);
                return;
            }
            "film_duplicate" => {
                if let Some(index) = number("picture") {
                    self.film(|gallery| gallery.duplicate(index.max(0.0) as usize));
                }
            }
            "film_delete" => {
                if let Some(index) = number("picture") {
                    self.film(|gallery| gallery.remove(index.max(0.0) as usize));
                }
            }
            // The thumbnail's Rename row: the name typed in place comes as
            // `value`. A host that cannot take a name sends none; the person
            // is told how a picture is renamed instead.
            "film_rename" => {
                let Some(index) = number("picture") else {
                    return;
                };
                match arg.get("value").and_then(Value::as_str) {
                    Some(name) => {
                        let name = name.to_string();
                        if let Some(Err(reason)) = self.film(|gallery| gallery.rename(index.max(0.0) as usize, &name)) {
                            out.push(notice("The picture could not be renamed", &reason));
                        }
                    }
                    None => {
                        out.push(notice("Rename a picture", "Ask the assistant to rename this picture, and give it the new name."));
                        return;
                    }
                }
            }
            "close_tab" => {
                self.close_tab(tab, out);
                return;
            }
            "select_tab" => {}
            "tool" => {
                if let Some(tool) = arg.get("tool").and_then(Value::as_str).and_then(ToolKind::named) {
                    self.choose_tool(tool);
                }
            }
            "set_brush" => {
                let eraser = arg.get("tool").and_then(Value::as_str) == Some("eraser");
                let settings = if eraser { &mut self.tools.eraser } else { &mut self.tools.brush };
                if let (Some(setting), Some(value)) = (arg.get("setting").and_then(Value::as_str), number("value")) {
                    let value = value.round().max(0.0) as u32;
                    match setting {
                        "size" => settings.size = value.clamp(1, 1000),
                        "hardness" => settings.hardness = value.min(100),
                        "opacity" => settings.opacity = value.clamp(1, 100),
                        _ => {}
                    }
                }
                // A held slider sends its value many times; the toolbar is
                // declared again when it is let go.
                if arg.get("live").and_then(Value::as_bool) == Some(true) {
                    out.extend(self.raster_only());
                    return;
                }
            }
            // A brush slider let go: the toolbar shows its value again.
            "finish_brush" => {}
            "toggle_pressure" => {
                let eraser = arg.get("tool").and_then(Value::as_str) == Some("eraser");
                let settings = if eraser { &mut self.tools.eraser } else { &mut self.tools.brush };
                settings.pressure = !settings.pressure;
            }
            "size_down" | "size_up" => {
                let settings = if self.tools.tool == ToolKind::Eraser { &mut self.tools.eraser } else { &mut self.tools.brush };
                let step = (settings.size / 10).max(1);
                settings.size = if id == "size_up" { (settings.size + step).min(1000) } else { settings.size.saturating_sub(step).max(1) };
            }
            "set_color" => {
                if let Some(rgb) = arg.get("value").and_then(rgb_of) {
                    self.tools.rgb = rgb;
                }
            }
            "select_mode" => {
                if let Some(index) = number("value") {
                    self.tools.combine = Combine::ALL[(index.max(0.0) as usize).min(3)];
                }
            }
            "feather" => {
                if let Some(value) = number("value") {
                    self.tools.feather = value.clamp(0.0, 1000.0);
                }
            }
            "deselect" => {
                self.change(Doc::deselect);
            }
            "invert" => {
                self.change(Doc::invert);
            }
            "crop_ratio" => {
                if let Some(index) = number("value") {
                    self.tools.ratio = (index.max(0.0) as usize).min(declare::RATIOS.len() - 1);
                    if let Some(rect) = self.active_image().and_then(|image| image.frame().crop) {
                        self.set_crop(rect);
                    }
                }
            }
            // Enter and Escape mean these only while cropping.
            "crop_apply" | "crop_cancel" if self.tools.tool != ToolKind::Crop => return,
            "crop_apply" => self.apply_crop(),
            "crop_cancel" => {
                // Cancel drops the box; the crop tool shows the whole
                // picture's again, ready to drag.
                self.start_crop();
            }
            "layer_click" => {
                if let Some(node) = node {
                    self.change(|doc| doc.activate(node));
                }
            }
            "layer_rename" => {
                if let (Some(node), Some(name)) = (node, arg.get("value").and_then(Value::as_str)) {
                    let name = name.to_string();
                    self.change(|doc| doc.rename(node, &name, None));
                }
            }
            "layer_visible" => {
                if let Some(node) = node {
                    self.change(|doc| {
                        let visible = doc.picture.find(node).is_some_and(|found| found.visible);
                        doc.set_visible(node, !visible, None)
                    });
                }
            }
            "layer_lock" => {
                if let Some(node) = node {
                    self.change(|doc| doc.toggle_lock(node, 3));
                }
            }
            "reorder" => {
                if let (Some(node), Some(to)) = (node, number("value")) {
                    let to = to.max(0.0) as usize;
                    // With `parent`, null for the top level, the layer goes
                    // into that group or out of its own; without it, it
                    // stays among its siblings.
                    match arg.get("parent") {
                        Some(parent) => {
                            if parent.is_null() || parent.is_u64() {
                                let parent = parent.as_u64();
                                self.change(|doc| doc.move_node(node, parent, to, None));
                            }
                        }
                        None => {
                            self.change(|doc| doc.reorder(node, to, None));
                        }
                    }
                }
            }
            "add_layer" => {
                self.change(|doc| doc.add_layer("", None, None));
            }
            "add_group" => {
                self.change(Doc::group_active);
            }
            "delete_layer" => {
                let target = node.or_else(|| self.active_image().and_then(|image| image.doc().active));
                if let Some(target) = target {
                    self.change(|doc| doc.delete(target, None));
                }
            }
            "ungroup" => {
                if let Some(node) = node {
                    self.change(|doc| doc.ungroup(node));
                }
            }
            "finish_edit" => {
                self.change(Doc::finish_edit);
            }
            "toggle_panel" => self.panel_open = !self.panel_open,
            _ => return,
        }
        out.extend(self.emit());
    }

    pub fn choose_tool(&mut self, tool: ToolKind) {
        if self.tools.tool == tool {
            return;
        }
        self.tools.tool = tool;
        if tool == ToolKind::Crop {
            self.start_crop();
        } else if let Some(image) = self.images.get_mut(self.active) {
            image.frame_mut().crop = None;
        }
    }

    /// The crop tool starts with the whole picture's box, ready to drag.
    fn start_crop(&mut self) {
        if let Some(image) = self.images.get_mut(self.active) {
            let frame = image.frame_mut();
            frame.crop = Some([0.0, 0.0, f64::from(frame.doc.picture.width), f64::from(frame.doc.picture.height)]);
        }
    }

    /// Sets the crop box, kept inside the picture and to the chosen ratio.
    fn set_crop(&mut self, rect: [f64; 4]) {
        let ratio = declare::RATIOS[self.tools.ratio].1;
        let Some(image) = self.images.get_mut(self.active) else {
            return;
        };
        let image = image.frame_mut();
        if rect.iter().any(|value| !value.is_finite()) {
            return;
        }
        let width = f64::from(image.doc.picture.width);
        let height = f64::from(image.doc.picture.height);
        let x = rect[0].clamp(0.0, width - 1.0).round();
        let y = rect[1].clamp(0.0, height - 1.0).round();
        let mut w = rect[2].abs().min(width - x).max(1.0);
        let mut h = rect[3].abs().min(height - y).max(1.0);
        if let Some(ratio) = ratio {
            // The dimension the drag changed leads: the height when only it
            // changed, as when the top or bottom edge is dragged, and the
            // width otherwise. When the other would not fit, it leads.
            let height_leads = image
                .crop
                .is_some_and(|now| (rect[2].abs() - now[2]).abs() < 0.5 && (rect[3].abs() - now[3]).abs() >= 0.5);
            if height_leads {
                w = h * ratio;
                if w > width - x {
                    w = width - x;
                    h = w / ratio;
                }
            } else {
                h = w / ratio;
                if h > height - y {
                    h = height - y;
                    w = h * ratio;
                }
            }
        }
        image.crop = Some([x, y, w.round().max(1.0), h.round().max(1.0)]);
    }

    fn apply_crop(&mut self) {
        let Some(rect) = self.active_image().and_then(|image| image.frame().crop) else {
            return;
        };
        self.change(|doc| doc.crop(rect));
        self.start_crop();
    }

    /// The brush or eraser chosen, as the toolbar sets it.
    fn brush(&self) -> Option<Brush> {
        let tools = &self.tools;
        let (settings, erase) = match tools.tool {
            ToolKind::Brush => (tools.brush, false),
            ToolKind::Eraser => (tools.eraser, true),
            _ => return None,
        };
        Some(Brush {
            rgb: tools.rgb,
            size: f64::from(settings.size),
            hardness: f64::from(settings.hardness) / 100.0,
            opacity: f64::from(settings.opacity) / 100.0,
            pressure: settings.pressure,
            erase,
        })
    }

    /// The picture with the stroke being drawn, once the points waiting
    /// have all been painted: only the picture, as nothing else changed.
    pub fn take_live(&mut self) -> Vec<AppMessage> {
        if !std::mem::take(&mut self.live_due) {
            return Vec::new();
        }
        self.raster_view(self.active, true).map(AppMessage::Raster).into_iter().collect()
    }

    fn gesture(&mut self, gesture: RasterGesture) {
        let tools = self.tools.clone();
        match gesture {
            RasterGesture::Stroke { points } => {
                let Some(brush) = self.brush() else {
                    return;
                };
                // A stroke on a locked or hidden layer changes nothing.
                self.change(|doc| doc.stroke(&points, &brush));
            }
            // Taken off already, as every message but a stroke's does.
            RasterGesture::StrokeCancel | RasterGesture::StrokePart { .. } => {}
            RasterGesture::Rect { from, to } => {
                self.change(|doc| {
                    let shape = Mask::rect(doc.picture.width, doc.picture.height, from, to);
                    doc.select(shape, tools.combine, tools.feather);
                });
            }
            RasterGesture::Lasso { points } => {
                self.change(|doc| {
                    let shape = Mask::lasso(doc.picture.width, doc.picture.height, &points);
                    doc.select(shape, tools.combine, tools.feather);
                });
            }
            RasterGesture::Drag { from, to } => {
                let dx = (to[0] - from[0]).round();
                let dy = (to[1] - from[1]).round();
                if dx.is_finite() && dy.is_finite() {
                    self.change(|doc| doc.move_active(dx as i32, dy as i32));
                }
            }
            RasterGesture::Pick { at } => {
                if let Some(rgba) = self.images.get_mut(self.active).and_then(|image| image.doc_mut().pick(at)) {
                    self.tools.rgb = [rgba[0], rgba[1], rgba[2]];
                }
            }
            RasterGesture::Crop { rect } => self.set_crop(rect),
        }
    }

    /// A Layers page edit: blend mode, opacity, fill, or a lock.
    fn property(&mut self, prop: &Value, value: &Value, preview: bool) {
        let Some(name) = prop.as_str() else {
            return;
        };
        let value = serde_json::from_value::<PropValue>(value.clone()).ok();
        let setting = match (name, value) {
            ("blend", Some(PropValue::Index(index))) => Blend::ALL.get(usize::from(index)).copied().map(Setting::Blend),
            ("opacity", Some(PropValue::Number(number))) => Some(Setting::Opacity(number)),
            ("fill", Some(PropValue::Number(number))) => Some(Setting::Fill(number)),
            ("locks", Some(PropValue::Index(index))) => {
                self.change(|doc| match doc.active {
                    Some(active) => doc.toggle_lock(active, usize::from(index).min(3)),
                    None => Ok(()),
                });
                return;
            }
            _ => None,
        };
        if let Some(setting) = setting {
            self.change(|doc| doc.set_setting(setting, preview));
        }
    }

    fn ask_file(&mut self, kind: FileKind, name: Option<String>, purpose: Purpose, filter: FileFilter, out: &mut Vec<AppMessage>) {
        let id = format!("file-{}", self.next_file);
        self.next_file += 1;
        self.file = Some((id.clone(), purpose));
        out.push(AppMessage::File {
            id,
            kind,
            name,
            folder: self.project.as_ref().map(|folder| folder.display().to_string()),
            filter: Some(filter),
        });
    }

    fn file_result(&mut self, id: &str, path: Option<String>, out: &mut Vec<AppMessage>) {
        let Some((asked, purpose)) = self.file.take() else {
            return;
        };
        if asked != id {
            self.file = Some((asked, purpose));
            return;
        }
        let Some(path) = path.map(PathBuf::from) else {
            if matches!(purpose, Purpose::Save { then_quit: true, .. }) || self.quitting {
                self.cancel_quit(out);
            }
            return;
        };
        match purpose {
            Purpose::Open => {
                if files::is_ipx(&path) && !self.in_project(&path) {
                    out.push(notice("This image is not in the project", "Only images inside the project's folder open here."));
                    return;
                }
                self.open_path(Some(path), out);
            }
            Purpose::OpenLayer => match files::import(&path) {
                Ok(pixels) => {
                    let name = file_title(&path);
                    let result = self.change(|doc| doc.add_image(&name, pixels));
                    if let Some(Err(reason)) = result {
                        out.push(notice("The picture could not be added", &reason));
                    }
                    out.extend(self.emit());
                }
                Err(reason) => out.push(notice("This picture could not be opened", &reason)),
            },
            Purpose::OpenPicture { tab } => match files::import(&path) {
                Ok(pixels) => {
                    let Some(index) = self.image_index(&tab) else {
                        return;
                    };
                    self.active = index;
                    let title = file_title(&path);
                    let doc = Doc::new(Picture::from_pixels(&title, pixels));
                    self.film(|gallery| gallery.add(&title, doc, None));
                    out.extend(self.emit());
                }
                Err(reason) => out.push(notice("This picture could not be opened", &reason)),
            },
            Purpose::Export { jpeg } => {
                let Some(image) = self.images.get_mut(self.active) else {
                    return;
                };
                let doc = image.doc_mut();
                let (width, height) = (doc.picture.width, doc.picture.height);
                let pixels = doc.composite();
                let encoded = if jpeg {
                    files::encode_jpeg(width, height, pixels)
                } else {
                    files::encode_png(width, height, pixels)
                };
                if let Err(reason) = encoded.and_then(|bytes| files::write_atomic(&path, &bytes)) {
                    out.push(notice("The picture could not be exported", &reason));
                }
            }
            Purpose::Save { tab, then_quit } => {
                if let Some(index) = self.image_index(&tab) {
                    self.write_at(index, &path, then_quit, out);
                }
            }
        }
    }

    /// Opens a file in a new tab: an image file as itself, or a PNG or JPEG
    /// as a new image of one layer. No path makes a new image.
    fn open_path(&mut self, path: Option<PathBuf>, out: &mut Vec<AppMessage>) {
        let Some(path) = path else {
            self.push_image(new_gallery(), None, None);
            self.declare_new(out);
            return;
        };
        if let Some(index) = self.images.iter().position(|image| image.path.as_deref() == Some(path.as_path())) {
            self.active = index;
            out.extend(self.emit());
            return;
        }
        if files::is_ipx(&path) {
            match files::read_ipx(&path) {
                Ok(read) => {
                    let pictures = read
                        .pictures
                        .into_iter()
                        .map(|read| {
                            let mut doc = Doc::new(read.picture);
                            if read.active.is_some() {
                                doc.active = read.active;
                            }
                            (read.name, doc)
                        })
                        .collect();
                    self.push_image(Gallery::new(pictures, read.shown), Some(path), None);
                    self.declare_new(out);
                }
                Err(reason) => out.push(notice("This image could not be opened", &reason)),
            }
            return;
        }
        match files::import(&path) {
            Ok(pixels) => {
                let title = file_title(&path);
                let gallery = Gallery::one(&title, Doc::new(Picture::from_pixels(&title, pixels)));
                let index = self.push_image(gallery, None, Some(title));
                // It is new work, so it is kept in the project as an image.
                self.images[index].dirty = true;
                self.declare_new(out);
            }
            Err(reason) => out.push(notice("This picture could not be opened", &reason)),
        }
    }

    /// Starts writing every image whose change is a second old, and tries a
    /// failed one again after a while. The writer's thread encodes and
    /// writes, so painting never waits for it.
    fn write_due(&mut self, now: Instant) {
        for index in 0..self.images.len() {
            let image = &mut self.images[index];
            if !image.dirty {
                image.unwritten_since = None;
                continue;
            }
            let since = *image.unwritten_since.get_or_insert(now);
            if now.duration_since(since) < WRITE_AFTER {
                continue;
            }
            if image.write_failed.as_ref().is_some_and(|(_, failed)| now.duration_since(*failed) < RETRY_AFTER) {
                continue;
            }
            self.start_write(index);
        }
    }

    /// Hands one image to the writer's thread: its pictures as they are now,
    /// and the file it goes to. Not while it is being written already, or
    /// while a stroke is drawn on it.
    fn start_write(&mut self, index: usize) {
        let Some(folder) = self.project.clone() else {
            return;
        };
        // Nothing is copied for a write while a stroke is drawn anywhere.
        if self.images.iter().any(|image| image.gallery.frames.iter().any(|frame| frame.doc.drawing())) {
            return;
        }
        let taken: Vec<PathBuf> = self
            .images
            .iter()
            .flat_map(|image| image.path.iter().chain(image.saving.as_ref().map(|(_, path)| path)))
            .cloned()
            .collect();
        let Some(image) = self.images.get_mut(index) else {
            return;
        };
        if image.saving.is_some() {
            return;
        }
        let path = match &image.path {
            Some(path) => path.clone(),
            None => match free_names(&image.label)
                .map(|name| folder.join(format!("{name}.ipx")))
                .find(|path| !path.exists() && !taken.contains(path))
            {
                Some(path) => path,
                None => return,
            },
        };
        let (pictures, shown) = snapshot(&mut image.gallery);
        let job = Job { tab: image.id.clone(), signature: signature(image), path: path.clone(), pictures, shown };
        image.saving = Some((job.signature, path));
        let writer = self.writer.get_or_insert_with(Writer::start);
        if writer.jobs.send(job).is_ok() {
            writer.waiting += 1;
        } else if let Some(image) = self.images.get_mut(index) {
            image.saving = None;
        }
    }

    /// Takes the writer's finished writes, waiting for every one with
    /// `block`, as a write that must come after them does. Returns whether
    /// any finished.
    fn finish_writes(&mut self, block: bool) -> bool {
        let mut any = false;
        loop {
            let Some(writer) = self.writer.as_mut() else {
                return any;
            };
            if writer.waiting == 0 {
                return any;
            }
            let done = if block {
                writer.done.recv().map_err(|_| true)
            } else {
                writer.done.try_recv().map_err(|error| error == std::sync::mpsc::TryRecvError::Disconnected)
            };
            let done = match done {
                Ok(done) => done,
                Err(false) => return any,
                // The writer stopped: what it held is unwritten, and is tried
                // again with a new one after a while.
                Err(true) => {
                    self.writer = None;
                    for image in &mut self.images {
                        if image.saving.take().is_some() {
                            image.write_failed = Some(("The image could not be written.".into(), Instant::now()));
                        }
                    }
                    return true;
                }
            };
            writer.waiting -= 1;
            any = true;
            let Some(image) = self.images.iter_mut().find(|image| image.id == done.tab) else {
                continue;
            };
            image.saving = None;
            match done.result {
                Ok(encoded) => {
                    keep(&mut image.gallery, encoded);
                    image.label = file_title(&done.path);
                    image.path = Some(done.path);
                    image.write_failed = None;
                    // Changed while it was written: written again soon.
                    if signature(image) == done.signature {
                        image.dirty = false;
                        image.unwritten_since = None;
                    }
                }
                Err(reason) => image.write_failed = Some((reason, Instant::now())),
            }
        }
    }

    /// Writes one image into the project's folder: to its own file, or, for
    /// one never written, to a new file named after it that replaces
    /// nothing. Returns the file, or none for an untouched image.
    fn write_into_project(&mut self, index: usize) -> Result<Option<PathBuf>, String> {
        // The writer's earlier writes land first, so none lands after this.
        self.finish_writes(true);
        let Some(folder) = self.project.clone() else {
            return Err("No project folder.".into());
        };
        let Some(image) = self.images.get_mut(index) else {
            return Ok(None);
        };
        if !image.dirty {
            return Ok(image.path.clone());
        }
        let path = match &image.path {
            Some(path) => path.clone(),
            None => match free_names(&image.label).map(|name| folder.join(format!("{name}.ipx"))).find(|path| !path.exists()) {
                Some(path) => path,
                None => return Err("No free file name.".into()),
            },
        };
        match write_gallery(&path, &mut image.gallery) {
            Ok(()) => {
                image.label = file_title(&path);
                image.path = Some(path.clone());
                image.dirty = false;
                image.unwritten_since = None;
                image.write_failed = None;
                Ok(Some(path))
            }
            Err(reason) => {
                image.write_failed = Some((reason.clone(), Instant::now()));
                Err(reason)
            }
        }
    }

    /// Outside a project: Save, or Save As, to `path`.
    fn write_at(&mut self, index: usize, path: &Path, then_quit: bool, out: &mut Vec<AppMessage>) {
        self.finish_writes(true);
        let Some(image) = self.images.get_mut(index) else {
            return;
        };
        if let Err(reason) = write_gallery(path, &mut image.gallery) {
            out.push(notice("This image could not be saved", &reason));
            if then_quit || self.quitting {
                self.cancel_quit(out);
            }
            return;
        }
        image.path = Some(path.to_path_buf());
        image.label = file_title(path);
        image.dirty = false;
        out.extend(self.declared());
        if self.closing {
            self.closing = false;
            self.remove_image(index, out);
        } else if then_quit {
            self.advance_quit(out);
        }
    }

    fn close_tab(&mut self, tab: Option<&str>, out: &mut Vec<AppMessage>) {
        let Some(index) = tab.and_then(|tab| self.image_index(tab)).or((!self.images.is_empty()).then_some(self.active)) else {
            return;
        };
        if self.project.is_some() {
            // Written first, so closing asks nothing. An image that could
            // not be written still asks.
            let _ = self.write_into_project(index);
        }
        if self.images[index].dirty {
            self.active = index;
            self.closing = true;
            self.quitting = false;
            self.quit_rest.clear();
            out.push(AppMessage::Ask(keep_ask()));
            return;
        }
        self.remove_image(index, out);
    }

    /// Removes a tab and its pictures for the host, then sends what the
    /// host now shows. The last tab closed leaves a new image, declared as
    /// any new tab is.
    fn remove_image(&mut self, index: usize, out: &mut Vec<AppMessage>) {
        let image = self.images.remove(index);
        // Its picture, and every thumbnail of its pictures and their layers.
        let own = format!("{}-", image.id);
        for entry in std::fs::read_dir(&self.cache).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&own) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        if self.images.is_empty() {
            self.push_image(new_gallery(), None, None);
            self.declare_new(out);
            return;
        }
        if self.active >= self.images.len() {
            self.active = self.images.len() - 1;
        }
        out.extend(self.emit());
    }

    fn note_quit(&mut self, out: &mut Vec<AppMessage>) {
        if self.quitting {
            return;
        }
        let dirty: Vec<String> = self.images.iter().filter(|image| image.dirty).map(|image| image.id.clone()).collect();
        let Some(first) = dirty.first() else {
            out.push(AppMessage::QuitReady);
            return;
        };
        self.quitting = true;
        self.activate(&first.clone());
        self.quit_rest = dirty[1..].to_vec();
        out.push(AppMessage::Ask(keep_ask()));
    }

    fn answer(&mut self, button: &str, out: &mut Vec<AppMessage>) {
        match button {
            "save" => {
                let Some(image) = self.active_image() else {
                    return;
                };
                match image.path.clone() {
                    Some(path) => self.write_at(self.active, &path, self.quitting, out),
                    None => {
                        let purpose = Purpose::Save { tab: image.id.clone(), then_quit: self.quitting };
                        let name = format!("{}.ipx", image.label);
                        self.ask_file(FileKind::Save, Some(name), purpose, ipx_filter(), out);
                    }
                }
            }
            "discard" => {
                if self.closing {
                    self.closing = false;
                    self.remove_image(self.active, out);
                } else if self.quitting {
                    self.advance_quit(out);
                }
            }
            _ => self.cancel_quit(out),
        }
    }

    fn advance_quit(&mut self, out: &mut Vec<AppMessage>) {
        if !self.quitting {
            return;
        }
        if self.quit_rest.is_empty() {
            self.quitting = false;
            out.push(AppMessage::QuitReady);
            return;
        }
        let next = self.quit_rest.remove(0);
        self.activate(&next);
        out.push(AppMessage::Ask(keep_ask()));
    }

    fn cancel_quit(&mut self, out: &mut Vec<AppMessage>) {
        self.quitting = false;
        self.closing = false;
        self.quit_rest.clear();
        out.extend(self.declared());
    }

    // What the host draws.

    /// The active image's picture and every declaration.
    pub fn emit(&mut self) -> Vec<AppMessage> {
        self.live_due = false;
        let mut out = self.raster_only();
        out.extend(self.declared());
        out
    }

    /// The active image's picture, written for the host when it changed.
    fn raster_only(&mut self) -> Vec<AppMessage> {
        let index = self.active;
        if index >= self.images.len() {
            return Vec::new();
        }
        self.write_film(index);
        let view = self.raster_view(index, false);
        self.write_thumbnails(index);
        view.map(AppMessage::Raster).into_iter().collect()
    }

    /// The view of the picture shown. While a stroke is drawn (`live`), only
    /// the rows it changed are written, to a file of their own; the picture
    /// file is written whole at the next view that is not.
    fn raster_view(&mut self, index: usize, live: bool) -> Option<RasterView> {
        let tool = self.raster_tool();
        let move_tool = self.tools.tool == ToolKind::Move;
        let crop_tool = self.tools.tool == ToolKind::Crop;
        let cache = self.cache.clone();
        let viewport = self.viewport;
        let mut next = self.next_revision;
        let image = self.images.get_mut(index)?;
        let film = declare::film(image);
        let send_camera = std::mem::take(&mut image.send_camera);
        let image_id = image.id.clone();
        let frame = image.frame_mut();
        // Each picture has its own file, written again only when it
        // changed, so the host keeps every picture it has shown and a
        // picture shown again needs no reading. A change says which part
        // of the picture it touched, so the host reads only that.
        let picture = cache.join(format!("{image_id}-{}.rgba", frame.uid));
        frame.doc.refresh();
        let changed = frame.doc.take_changed();
        let mut patch = None;
        let revision = match frame.written {
            Some((made, told)) if made == frame.doc.revision => {
                // Until the picture file catches up, when the program is
                // idle, the host is told the picture is still being made.
                if frame.behind {
                    patch = frame.last_patch.clone();
                }
                told
            }
            earlier => {
                next += 1;
                let rect = changed
                    .map(|rect| [rect.x.max(0) as u32, rect.y.max(0) as u32, rect.w.max(0) as u32, rect.h.max(0) as u32])
                    .filter(|rect| rect[2] > 0 && rect[3] > 0);
                let rows = match (earlier, rect) {
                    (Some((_, since)), Some(rect)) if live => {
                        let path = cache.join(format!("{image_id}-{}-{next}.patch", frame.uid));
                        let width = frame.doc.picture.width as usize;
                        let composite = frame.doc.composite();
                        let mut bytes = Vec::with_capacity(rect[2] as usize * rect[3] as usize * 4);
                        for y in rect[1] as usize..(rect[1] + rect[3]) as usize {
                            let start = (y * width + rect[0] as usize) * 4;
                            bytes.extend_from_slice(&composite[start..start + rect[2] as usize * 4]);
                        }
                        files::write_atomic(&path, &bytes).ok().map(|()| (since, rect, path))
                    }
                    _ => None,
                };
                match rows {
                    Some((since, rect, path)) => {
                        patch = Some(protocol::RasterPatch { since, rect, file: Some(path.display().to_string()) });
                        frame.last_patch = patch.clone();
                        frame.behind = true;
                        // The host reads each in turn; a few are kept for a
                        // host that falls behind.
                        frame.patches.push_back(path);
                        while frame.patches.len() > KEPT_PATCHES {
                            if let Some(old) = frame.patches.pop_front() {
                                let _ = std::fs::remove_file(old);
                            }
                        }
                    }
                    None => {
                        let _ = files::write_atomic(&picture, frame.doc.composite());
                        caught_up(frame);
                        if let (Some((_, since)), Some(rect)) = (earlier, rect) {
                            patch = Some(protocol::RasterPatch { since, rect, file: None });
                        }
                    }
                }
                frame.written = Some((frame.doc.revision, next));
                next
            }
        };
        self.next_revision = next;
        let (width, height) = (frame.doc.picture.width, frame.doc.picture.height);
        // A new picture is fitted; one shown again goes back where the host
        // last had it.
        let camera = if send_camera {
            let camera = match (&frame.camera, frame.host_camera) {
                (Some(camera), true) => camera.clone(),
                _ => fit(width, height, viewport),
            };
            frame.camera = Some(camera.clone());
            Some(camera)
        } else {
            None
        };
        if crop_tool {
            // A tab or picture chosen, or a crop undone, since the box was
            // set: the box starts again from the whole picture.
            let (width, height) = (f64::from(width), f64::from(height));
            let fits = frame.crop.is_some_and(|rect| rect[0] + rect[2] <= width && rect[1] + rect[3] <= height);
            if !fits {
                frame.crop = Some([0.0, 0.0, width, height]);
            }
        }
        let bounds = if move_tool {
            frame.doc.active.and_then(|active| frame.doc.picture.node_rect(active))
        } else {
            None
        };
        Some(RasterView {
            tab: image_id,
            width,
            height,
            picture: picture.display().to_string(),
            revision,
            patch,
            tool,
            selection: frame.doc.outlines(),
            crop: if crop_tool { frame.crop } else { None },
            bounds: bounds.map(|rect| [f64::from(rect.x), f64::from(rect.y), f64::from(rect.w), f64::from(rect.h)]),
            camera,
            film: Some(film),
        })
    }

    pub fn raster_tool(&self) -> RasterTool {
        let tools = &self.tools;
        match tools.tool {
            ToolKind::Move => RasterTool::Move,
            ToolKind::Marquee => RasterTool::Marquee,
            ToolKind::Lasso => RasterTool::Lasso,
            ToolKind::Crop => RasterTool::Crop,
            ToolKind::Eyedropper => RasterTool::Eyedropper,
            ToolKind::Brush => RasterTool::Brush {
                rgb: tools.rgb,
                size: f64::from(tools.brush.size),
                hardness: f64::from(tools.brush.hardness) / 100.0,
                opacity: f64::from(tools.brush.opacity) / 100.0,
                pressure: tools.brush.pressure,
            },
            ToolKind::Eraser => RasterTool::Erase {
                size: f64::from(tools.eraser.size),
                hardness: f64::from(tools.eraser.hardness) / 100.0,
                opacity: f64::from(tools.eraser.opacity) / 100.0,
                pressure: tools.eraser.pressure,
            },
            ToolKind::Hand => RasterTool::Hand,
            ToolKind::Zoom => RasterTool::Zoom,
        }
    }

    /// Each layer's small picture, in the picture's frame, written when it
    /// changed.
    fn write_thumbnails(&mut self, index: usize) {
        let cache = self.cache.clone();
        let mut next = self.next_revision;
        let Some(image) = self.images.get_mut(index) else {
            return;
        };
        let tab = image.id.clone();
        let frame = image.frame_mut();
        let picture = &frame.doc.picture;
        let (width, height) = thumb_size(picture.width, picture.height);
        let layers = picture.all_layers();
        frame.thumbs.retain(|id, _| layers.contains(id));
        for id in layers {
            let (Some(node), Some(pixels)) = (picture.find(id), picture.pixels.get(&id)) else {
                continue;
            };
            let bytes = thumbnail(picture.width, picture.height, node.offset, pixels, width, height);
            if frame.thumbs.get(&id).is_some_and(|(kept, _)| *kept == bytes) {
                continue;
            }
            // Layer ids repeat between pictures, so the picture is named too.
            let path = cache.join(format!("{tab}-{}-{id}.rgba", frame.uid));
            if files::write_atomic(&path, &bytes).is_err() {
                continue;
            }
            next += 1;
            let thumb = Thumbnail { path: path.display().to_string(), width, height, revision: next };
            frame.thumbs.insert(id, (bytes, thumb));
        }
        self.next_revision = next;
    }

    /// Each picture's filmstrip thumbnail, its composite drawn small,
    /// written when that picture changed.
    fn write_film(&mut self, index: usize) {
        let cache = self.cache.clone();
        let mut next = self.next_revision;
        let Some(image) = self.images.get_mut(index) else {
            return;
        };
        for frame in &mut image.gallery.frames {
            frame.doc.refresh();
            let revision = frame.doc.revision;
            if frame.film.as_ref().is_some_and(|(made, _, _)| *made == revision) {
                continue;
            }
            let (width, height) = (frame.doc.picture.width, frame.doc.picture.height);
            let (to_w, to_h) = film_size(width, height);
            let bytes = files::resample(width, height, frame.doc.composite(), to_w, to_h);
            if let Some((made, kept, _)) = &mut frame.film
                && *kept == bytes
            {
                *made = revision;
                continue;
            }
            let path = cache.join(format!("{}-film-{}.rgba", image.id, frame.uid));
            // A thumbnail that could not be written is tried again with the
            // next change; the strip keeps its place meanwhile.
            let written = files::write_atomic(&path, &bytes).is_ok();
            next += 1;
            let thumb = Thumbnail { path: path.display().to_string(), width: to_w, height: to_h, revision: next };
            frame.film = Some(if written { (revision, bytes, thumb) } else { (u64::MAX, Vec::new(), thumb) });
        }
        self.next_revision = next;
    }

    /// The toolbar, menu, keys, panel, and tab list.
    pub fn declared(&self) -> Vec<AppMessage> {
        let mut messages = vec![
            AppMessage::Toolbar(declare::toolbar(self)),
            AppMessage::Menu { rows: declare::menu(self) },
            AppMessage::Keys(declare::keys()),
        ];
        if let Some(properties) = declare::properties(self) {
            messages.push(AppMessage::Properties(properties));
        }
        if let Some(tabs) = declare::tabs(self) {
            messages.push(AppMessage::Tabs(tabs));
        }
        messages
    }

    /// An assistant's call, carried out on its tab, then its result.
    fn invoke(&mut self, call: String, tab: &str, tool: &str, input: Value, group: &str) -> Vec<AppMessage> {
        let Some(index) = self.image_index(tab) else {
            return vec![crate::assist::result(call, Err(format!("There is no image in tab {tab}.")))];
        };
        let name = protocol::split_tool_key(tool).map_or(tool, |(_, _, name)| name);
        let in_project = self.project.is_some();
        let image = &mut self.images[index];
        let (edits, shown, doc_edits) = (image.gallery.edits, image.frame().uid, image.doc().edits);
        let (outcome, look) = crate::assist::run(&mut image.gallery, name, &input, group);
        let shown_changed = image.frame().uid != shown;
        let changed = image.gallery.edits != edits || (!shown_changed && image.doc().edits != doc_edits);
        if changed || (in_project && shown_changed) {
            image.dirty = true;
        }
        if shown_changed {
            image.send_camera = true;
        }
        let mut out = Vec::new();
        if changed || shown_changed {
            // The tab the call changed shows its picture, whichever tab is
            // selected; the declarations stay the selected tab's.
            let active = self.active;
            self.active = index;
            out.extend(self.raster_only());
            self.active = active;
            out.extend(self.declared());
        }
        out.push(crate::assist::result_with(call, outcome, look));
        out
    }
}

impl Drop for Session {
    /// The scratch pictures go with the session; a project's cache stays.
    fn drop(&mut self) {
        if self.project.is_none() {
            let _ = std::fs::remove_dir_all(&self.cache);
        }
    }
}

/// A scratch folder of this session's own for the host's pictures while
/// there is no project, so two sessions never write one file.
fn scratch_folder() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("inpainter-image-editor-{}-{number}", std::process::id()))
}

/// Where the camera goes for a new or opened picture: the whole picture
/// with a margin, and no larger than its own pixels.
pub fn fit(width: u32, height: u32, viewport: [f64; 2]) -> DocumentCamera {
    let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let zoom = ((viewport[0] - 2.0 * FIT_MARGIN) / w)
        .min((viewport[1] - 2.0 * FIT_MARGIN) / h)
        .clamp(0.01, 1.0);
    DocumentCamera {
        origin: [w / 2.0 - viewport[0] / (2.0 * zoom), h / 2.0 - viewport[1] / (2.0 * zoom)],
        zoom,
    }
}

/// A thumbnail's size: the picture's shape fitted into 56 × 40.
pub fn thumb_size(width: u32, height: u32) -> (u32, u32) {
    fit_into(width, height, THUMB_SIZE)
}

/// A filmstrip thumbnail's size: 44 tall at the picture's shape, unless it
/// is more than four times as wide as tall.
pub fn film_size(width: u32, height: u32) -> (u32, u32) {
    fit_into(width, height, FILM_SIZE)
}

fn fit_into(width: u32, height: u32, most: [u32; 2]) -> (u32, u32) {
    let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let scale = (f64::from(most[0]) / w).min(f64::from(most[1]) / h);
    (((w * scale).round() as u32).max(1), ((h * scale).round() as u32).max(1))
}

/// A new picture: white, 1920 × 1080.
pub fn new_doc() -> Doc {
    Doc::new(Picture::blank(NEW_SIZE[0], NEW_SIZE[1]))
}

/// A new tab's gallery: one new picture.
fn new_gallery() -> Gallery {
    Gallery::one(UNTITLED, new_doc())
}

/// The picture file holds the revision the host was told: the files of
/// rows sent while a stroke was drawn are not needed any more. The host
/// either holds that revision or reads the picture file whole.
fn caught_up(frame: &mut Frame) {
    frame.behind = false;
    frame.last_patch = None;
    for path in frame.patches.drain(..) {
        let _ = std::fs::remove_file(path);
    }
}

/// Writes a tab's gallery to its file, each held slider finished first.
fn write_gallery(path: &Path, gallery: &mut Gallery) -> Result<(), String> {
    let (pictures, shown) = snapshot(gallery);
    let encoded = files::write_ipx(path, pictures, shown)?;
    keep(gallery, encoded);
    Ok(())
}

/// A gallery's pictures as they are now, each held slider finished first,
/// and which is shown.
fn snapshot(gallery: &mut Gallery) -> (Vec<files::Snapshot>, usize) {
    let pictures = gallery
        .frames
        .iter_mut()
        .map(|frame| {
            frame.doc.finish_edit();
            files::Snapshot::take(frame.uid, &frame.name, &frame.doc.picture, frame.doc.active, frame.doc.edits, &frame.pngs)
        })
        .collect();
    (pictures, gallery.shown)
}

/// Keeps what a write encoded, in each picture still in the gallery.
fn keep(gallery: &mut Gallery, encoded: Vec<files::Encoded>) {
    for encoded in encoded {
        if let Some(frame) = gallery.frames.iter_mut().find(|frame| frame.uid == encoded.uid) {
            frame.pngs.keep(encoded);
        }
    }
}

/// What an image is, as far as its file goes: its gallery's changes, its
/// pictures' changes, and the picture shown. A write made of an image with
/// the same signature holds everything in it.
type Signature = (u64, u64, u64);

fn signature(image: &Image) -> Signature {
    let edits = image.gallery.frames.iter().map(|frame| frame.doc.edits).sum();
    (image.gallery.edits, edits, image.frame().uid)
}

/// The thread that writes images inside a project, one at a time and in
/// order, so encoding and writing never hold up painting.
struct Writer {
    jobs: std::sync::mpsc::Sender<Job>,
    done: std::sync::mpsc::Receiver<Done>,
    /// Writes handed over and not yet taken back.
    waiting: usize,
}

struct Job {
    tab: String,
    signature: Signature,
    path: PathBuf,
    pictures: Vec<files::Snapshot>,
    shown: usize,
}

struct Done {
    tab: String,
    signature: Signature,
    path: PathBuf,
    result: Result<Vec<files::Encoded>, String>,
}

impl Writer {
    fn start() -> Self {
        let (jobs, incoming) = std::sync::mpsc::channel::<Job>();
        let (finished, done) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for job in incoming {
                let result = files::write_ipx(&job.path, job.pictures, job.shown);
                if finished.send(Done { tab: job.tab, signature: job.signature, path: job.path, result }).is_err() {
                    break;
                }
            }
        });
        Self { jobs, done, waiting: 0 }
    }
}

/// The commands that leave the keyboard with the filmstrip once it has it:
/// its own, Undo and Redo, and those about the tab rather than its picture.
/// Every other command, as every gesture and panel edit, is the picture's.
fn keeps_focus(id: &str) -> bool {
    id.starts_with("film_")
        || id.starts_with("canvas.")
        || matches!(
            id,
            "undo" | "redo" | "select_tab" | "close_tab" | "new_image" | "open" | "save" | "save_as" | "export_png"
                | "export_jpeg" | "toggle_panel"
        )
}

/// One layer drawn small in the picture's frame, clear where it does not
/// reach.
fn thumbnail(width: u32, height: u32, offset: [i32; 2], pixels: &Pixels, to_w: u32, to_h: u32) -> Vec<u8> {
    let mut out = vec![0u8; to_w as usize * to_h as usize * 4];
    let samples = 3;
    for ty in 0..to_h {
        for tx in 0..to_w {
            let mut sum = [0u32; 4];
            for sy in 0..samples {
                for sx in 0..samples {
                    let px = ((f64::from(tx) + (f64::from(sx) + 0.5) / f64::from(samples)) * f64::from(width) / f64::from(to_w)) as i32;
                    let py = ((f64::from(ty) + (f64::from(sy) + 0.5) / f64::from(samples)) * f64::from(height) / f64::from(to_h)) as i32;
                    let lx = px - offset[0];
                    let ly = py - offset[1];
                    if lx < 0 || ly < 0 || lx >= pixels.width as i32 || ly >= pixels.height as i32 {
                        continue;
                    }
                    let at = (ly as usize * pixels.width as usize + lx as usize) * 4;
                    let alpha = u32::from(pixels.data[at + 3]);
                    for channel in 0..3 {
                        sum[channel] += u32::from(pixels.data[at + channel]) * alpha;
                    }
                    sum[3] += alpha;
                }
            }
            if sum[3] > 0 {
                let at = (ty as usize * to_w as usize + tx as usize) * 4;
                for channel in 0..3 {
                    out[at + channel] = (sum[channel] / sum[3]) as u8;
                }
                out[at + 3] = (sum[3] / (samples * samples)) as u8;
            }
        }
    }
    out
}

fn rgb_of(value: &Value) -> Option<[u8; 3]> {
    let list = value.as_array()?;
    if list.len() != 3 {
        return None;
    }
    let mut rgb = [0u8; 3];
    for (slot, item) in rgb.iter_mut().zip(list) {
        *slot = u8::try_from(item.as_u64()?).ok()?;
    }
    Some(rgb)
}

fn image_filter(with_ipx: bool) -> FileFilter {
    let mut extensions = vec!["png".to_string(), "jpg".to_string(), "jpeg".to_string()];
    if with_ipx {
        extensions.push("ipx".to_string());
    }
    FileFilter { name: "Images".into(), extensions }
}

fn ipx_filter() -> FileFilter {
    FileFilter { name: "Image".into(), extensions: vec!["ipx".into()] }
}

fn notice(title: &str, line: &str) -> AppMessage {
    AppMessage::Ask(Ask {
        id: "image-notice".into(),
        title: title.into(),
        lines: vec![line.to_string()],
        buttons: vec![AskButton { id: "ok".into(), label: "OK".into() }],
        dismiss: Some("ok".into()),
    })
}

fn keep_ask() -> Ask {
    Ask {
        id: "keep-image".into(),
        title: "Keep this image?".into(),
        lines: vec!["The image has changes that are not saved.".into()],
        buttons: vec![
            AskButton { id: "save".into(), label: "Save".into() },
            AskButton { id: "discard".into(), label: "Discard".into() },
            AskButton { id: "cancel".into(), label: "Cancel".into() },
        ],
        dismiss: Some("cancel".into()),
    }
}

pub fn file_title(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| String::from(UNTITLED))
}

/// `name`, then the names after it: "Untitled Asset" gives "Untitled Asset",
/// "Untitled Asset 2", and on; "Beach 3" gives "Beach 3", "Beach 4".
pub fn free_names(name: &str) -> impl Iterator<Item = String> {
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

/// One line of the protocol, and the lines to write back.
pub fn respond(session: &mut Session, line: &str) -> Vec<String> {
    let Ok(message) = protocol::parse_host_message(line) else {
        return Vec::new();
    };
    session.handle(message).iter().map(protocol::encode_app_message).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::ToolItem;
    use serde_json::json;

    fn command(session: &mut Session, id: &str, arg: Value) -> Vec<AppMessage> {
        session.handle(HostMessage::Command {
            id: id.into(),
            tab: Some(session.images[session.active].id.clone()),
            arg: Some(arg),
            node: None,
            viewport: [1200.0, 800.0],
        })
    }

    fn gesture(session: &mut Session, gesture: RasterGesture) -> Vec<AppMessage> {
        let tab = session.images[session.active].id.clone();
        session.handle(HostMessage::RasterGesture { tab, gesture })
    }

    fn last_raster(messages: &[AppMessage]) -> RasterView {
        messages
            .iter()
            .rev()
            .find_map(|message| match message {
                AppMessage::Raster(view) => Some(view.clone()),
                _ => None,
            })
            .expect("no raster message")
    }

    fn at(session: &mut Session, x: usize, y: usize) -> [u8; 4] {
        let image = &mut session.images[session.active];
        let width = image.doc().picture.width as usize;
        let composite = image.doc_mut().composite();
        let at = (y * width + x) * 4;
        [composite[at], composite[at + 1], composite[at + 2], composite[at + 3]]
    }

    fn stroke(session: &mut Session, from: [f64; 2], to: [f64; 2]) -> Vec<AppMessage> {
        gesture(session, RasterGesture::Stroke { points: vec![[from[0], from[1], 1.0], [to[0], to[1], 1.0]] })
    }

    /// Catches the tools reached through the protocol doing nothing: a
    /// lasso limits the brush, the eraser removes, the eyedropper takes the
    /// colour for the brush, a drag moves the layer, and the crop box
    /// waits for Apply, which crops.
    #[test]
    fn the_tools_work_through_the_protocol() {
        let mut session = Session::launch();
        let _ = session.greeting();
        command(&mut session, "set_color", json!({ "value": [200, 30, 40] }));
        command(&mut session, "tool", json!({ "tool": "lasso" }));
        let view = last_raster(&gesture(
            &mut session,
            RasterGesture::Lasso { points: vec![[100.0, 100.0], [300.0, 100.0], [300.0, 300.0], [100.0, 300.0]] },
        ));
        assert!(matches!(view.tool, RasterTool::Lasso));
        assert_eq!(view.selection.len(), 1, "the selection has no outline");
        command(&mut session, "tool", json!({ "tool": "brush" }));
        stroke(&mut session, [50.0, 200.0], [350.0, 200.0]);
        assert_eq!(at(&mut session, 200, 200), [200, 30, 40, 255]);
        assert_eq!(at(&mut session, 60, 200), [255, 255, 255, 255], "painted outside the lasso");
        command(&mut session, "deselect", json!({}));
        assert!(session.images[0].doc().selection.is_none());

        command(&mut session, "tool", json!({ "tool": "eraser" }));
        stroke(&mut session, [500.0, 500.0], [520.0, 500.0]);
        assert_eq!(at(&mut session, 510, 500)[3], 0, "the eraser left the pixel");

        command(&mut session, "tool", json!({ "tool": "eyedropper" }));
        command(&mut session, "set_color", json!({ "value": [1, 2, 3] }));
        gesture(&mut session, RasterGesture::Pick { at: [200.5, 200.5] });
        assert_eq!(session.tools.rgb, [200, 30, 40]);

        command(&mut session, "tool", json!({ "tool": "move" }));
        let view = last_raster(&gesture(&mut session, RasterGesture::Drag { from: [10.0, 10.0], to: [30.0, 5.0] }));
        assert_eq!(view.bounds, Some([20.0, -5.0, 1920.0, 1080.0]));

        command(&mut session, "tool", json!({ "tool": "crop" }));
        command(&mut session, "crop_ratio", json!({ "value": 4 }));
        let view = last_raster(&gesture(&mut session, RasterGesture::Crop { rect: [100.0, 50.0, 800.0, 100.0] }));
        // 16:9 from an 800 wide box makes it 450 tall.
        assert_eq!(view.crop, Some([100.0, 50.0, 800.0, 450.0]));
        assert_eq!((view.width, view.height), (1920, 1080), "the box cropped before Apply");
        let view = last_raster(&command(&mut session, "crop_apply", json!({})));
        assert_eq!((view.width, view.height), (800, 450));
        assert_eq!(std::fs::metadata(&view.picture).unwrap().len(), 800 * 450 * 4);
        let view = last_raster(&command(&mut session, "undo", json!({})));
        assert_eq!((view.width, view.height), (1920, 1080));
        assert_eq!(view.crop, Some([0.0, 0.0, 1920.0, 1080.0]));
    }

    /// Catches the Layers page's controls not reaching the layers: blend,
    /// a held opacity slider as one step, the locks row, and the list's
    /// add, group, rename, hide, reorder, and delete.
    #[test]
    fn the_layers_page_works_through_the_protocol() {
        let mut session = Session::launch();
        let _ = session.greeting();
        let background = session.images[0].doc().active.unwrap();
        let property = |session: &mut Session, prop: &str, value: PropValue, preview: bool| {
            session.handle(HostMessage::Property { prop: json!(prop), value: serde_json::to_value(value).unwrap(), preview })
        };
        property(&mut session, "blend", PropValue::Index(1), false);
        assert_eq!(session.images[0].doc().picture.nodes[0].blend, Blend::Multiply);
        for value in [0.8, 0.6, 0.4] {
            property(&mut session, "opacity", PropValue::Number(value), true);
        }
        command(&mut session, "finish_edit", json!({}));
        assert_eq!(session.images[0].doc().picture.nodes[0].opacity, 0.4);
        command(&mut session, "undo", json!({}));
        assert_eq!(session.images[0].doc().picture.nodes[0].opacity, 1.0, "the held slider was more than one step");
        assert_eq!(session.images[0].doc().picture.nodes[0].blend, Blend::Multiply);
        property(&mut session, "locks", PropValue::Index(1), false);
        assert!(session.images[0].doc().picture.nodes[0].locks.pixels);
        command(&mut session, "tool", json!({ "tool": "brush" }));
        stroke(&mut session, [10.0, 10.0], [40.0, 10.0]);
        assert_eq!(at(&mut session, 20, 10), [255, 255, 255, 255], "a pixel-locked layer took paint");

        command(&mut session, "add_layer", json!({}));
        let added = session.images[0].doc().active.unwrap();
        command(&mut session, "layer_rename", json!({ "node": added, "value": "Ink" }));
        command(&mut session, "layer_visible", json!({ "node": added }));
        command(&mut session, "reorder", json!({ "node": added, "value": 1 }));
        let messages = command(&mut session, "add_group", json!({}));
        let rows = messages
            .iter()
            .rev()
            .find_map(|message| match message {
                AppMessage::Properties(properties) => properties.layers.clone(),
                _ => None,
            })
            .unwrap()
            .rows;
        let names: Vec<(String, u32, bool)> = rows.iter().map(|row| (row.name.clone(), row.depth, row.visible)).collect();
        assert_eq!(
            names,
            [
                ("Background".to_string(), 0, true),
                ("Group 1".to_string(), 0, true),
                ("Ink".to_string(), 1, false),
            ]
        );
        assert!(rows[0].thumbnail.is_some() && rows[2].thumbnail.is_some());
        command(&mut session, "delete_layer", json!({}));
        assert!(session.images[0].doc().picture.find(added).is_none());
        assert!(session.images[0].doc().picture.find(background).is_some());
        command(&mut session, "undo", json!({}));
        assert!(session.images[0].doc().picture.find(added).is_some());
    }

    /// Catches the assistant's call answered without its picture or before
    /// the change it made is shown.
    #[test]
    fn an_invoke_shows_its_change_then_answers() {
        let mut session = Session::launch();
        let _ = session.greeting();
        let messages = session.handle(HostMessage::Invoke {
            call: "c1".into(),
            tab: "1".into(),
            tool: protocol::tool_key("image-editor", "image", "add_layer"),
            input: json!({ "name": "Notes" }),
            group: "r1".into(),
        });
        let AppMessage::ToolResult(result) = messages.last().unwrap() else { panic!("no result last") };
        assert!(result.ok, "{:?}", result.output);
        assert!(messages.iter().any(|message| matches!(message, AppMessage::Properties(_))));
        assert!(session.images[0].dirty);
        let messages = session.handle(HostMessage::Invoke {
            call: "c2".into(),
            tab: "1".into(),
            tool: protocol::tool_key("image-editor", "image", "look"),
            input: json!({}),
            group: "r1".into(),
        });
        let AppMessage::ToolResult(result) = messages.last().unwrap() else { panic!("no result last") };
        assert_eq!(result.images.len(), 1);
    }

    /// Catches a `keep` that never writes, Save offered in a project, a
    /// file outside the project claimed, and closing the last tab leaving
    /// the host with no declared tab.
    #[test]
    fn a_project_image_keeps_and_closes() {
        let folder = std::env::temp_dir().join(format!("image-editor-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let folder = folder.canonicalize().unwrap();
        let mut session = Session::launch();
        let _ = session.greeting();
        session.handle(HostMessage::Project { folder: folder.display().to_string() });
        let labels: Vec<String> = declare::menu(&session).into_iter().map(|row| row.label).collect();
        assert!(!labels.iter().any(|label| label.starts_with("Save")), "{labels:?}");
        command(&mut session, "add_layer", json!({}));
        let replies = session.handle(HostMessage::Keep { tab: "1".into() });
        let expected = folder.join("Untitled Asset.ipx").display().to_string();
        assert!(replies.iter().any(|reply| matches!(reply, AppMessage::Kept { path: Some(path), error: None, .. } if *path == expected)));
        assert_eq!(files::read_ipx(Path::new(&expected)).unwrap().pictures[0].picture.nodes.len(), 2);

        let outside = std::env::temp_dir().join(format!("image-editor-outside-{}.ipx", std::process::id()));
        std::fs::copy(&expected, &outside).unwrap();
        let replies = session.handle(HostMessage::Claim { id: "c".into(), path: outside.display().to_string() });
        assert!(replies.iter().any(|reply| matches!(reply, AppMessage::Claimed { yours: false, .. })));
        let _ = std::fs::remove_file(outside);

        let replies = command(&mut session, "close_tab", json!({}));
        assert!(replies.iter().any(|reply| matches!(reply, AppMessage::Contents { kind: ContentsKind::Raster, .. })));
        assert!(replies.iter().any(|reply| matches!(reply, AppMessage::Panel { .. })));
        assert_eq!(session.images.len(), 1);
        assert_ne!(session.images[0].id, "1");
        let replies = session.handle(HostMessage::Quit);
        assert!(replies.iter().any(|reply| matches!(reply, AppMessage::QuitReady)));
        let _ = std::fs::remove_dir_all(folder);
    }
    /// The settings row in the last toolbar sent.
    fn options(messages: &[AppMessage]) -> Vec<ToolItem> {
        messages
            .iter()
            .rev()
            .find_map(|message| match message {
                AppMessage::Toolbar(toolbar) => Some(toolbar.options.clone()),
                _ => None,
            })
            .expect("no toolbar was sent")
    }

    fn strong_labels(items: &[ToolItem]) -> Vec<String> {
        items
            .iter()
            .filter_map(|item| match item {
                ToolItem::Label { text, strong: true } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// Catches a refused stroke or move that says nothing: the settings row
    /// says why after the tool's settings, and the stroke is still refused.
    #[test]
    fn the_settings_row_says_why_a_tool_cannot_change_the_layer() {
        let mut session = Session::launch();
        let _ = session.greeting();
        let messages = command(&mut session, "tool", json!({ "tool": "brush" }));
        assert!(strong_labels(&options(&messages)).is_empty(), "a reason shown for a layer that takes paint");
        let background = session.images[0].doc().active.unwrap();
        let messages = command(&mut session, "layer_lock", json!({ "node": background }));
        let row = options(&messages);
        assert_eq!(strong_labels(&row), ["Background is locked."]);
        assert!(matches!(row.last(), Some(ToolItem::Label { strong: true, .. })), "the reason is not after the settings");
        stroke(&mut session, [10.0, 10.0], [40.0, 10.0]);
        assert_eq!(at(&mut session, 20, 10), [255, 255, 255, 255], "a locked layer took paint");
        let messages = command(&mut session, "tool", json!({ "tool": "move" }));
        assert_eq!(strong_labels(&options(&messages)), ["Background is locked."]);
        command(&mut session, "add_group", json!({}));
        let group = session.images[0].doc().picture.nodes[0].id;
        let messages = command(&mut session, "layer_click", json!({ "node": group }));
        assert_eq!(strong_labels(&options(&messages)), ["Background is locked."], "moving a group with a locked layer gave no reason");
        let messages = command(&mut session, "tool", json!({ "tool": "eraser" }));
        assert_eq!(strong_labels(&options(&messages)), ["Choose a layer to paint on; a group holds layers."]);
    }

    /// Catches the Layers list's drop into or out of a group not reaching
    /// the picture, or a drop without `parent` leaving its siblings.
    #[test]
    fn a_reorder_with_a_parent_moves_a_layer_into_and_out_of_a_group() {
        let mut session = Session::launch();
        let _ = session.greeting();
        let background = session.images[0].doc().active.unwrap();
        command(&mut session, "add_layer", json!({}));
        let top = session.images[0].doc().active.unwrap();
        command(&mut session, "add_group", json!({}));
        let group = session.images[0].doc().picture.place(top).unwrap().parent.unwrap();
        command(&mut session, "reorder", json!({ "node": background, "value": 0, "parent": group }));
        assert_eq!(session.images[0].doc().picture.place(background).unwrap().parent, Some(group));
        command(&mut session, "reorder", json!({ "node": background, "value": 1, "parent": null }));
        let place = session.images[0].doc().picture.place(background).unwrap();
        assert_eq!((place.parent, place.index), (None, 0), "the layer did not come out under the group");
        command(&mut session, "reorder", json!({ "node": background, "value": 0 }));
        let place = session.images[0].doc().picture.place(background).unwrap();
        assert_eq!((place.parent, place.index), (None, 1), "a reorder without a parent left the siblings");
    }

    /// Catches a ratio that only the width can lead: with a ratio chosen,
    /// dragging the bottom edge changes the height, and the width follows.
    #[test]
    fn a_crop_ratio_follows_whichever_edge_is_dragged() {
        let mut session = Session::launch();
        let _ = session.greeting();
        command(&mut session, "tool", json!({ "tool": "crop" }));
        command(&mut session, "crop_ratio", json!({ "value": 1 }));
        let view = last_raster(&gesture(&mut session, RasterGesture::Crop { rect: [100.0, 50.0, 400.0, 400.0] }));
        assert_eq!(view.crop, Some([100.0, 50.0, 400.0, 400.0]));
        // The bottom edge dragged down: only the height changed.
        let view = last_raster(&gesture(&mut session, RasterGesture::Crop { rect: [100.0, 50.0, 400.0, 600.0] }));
        assert_eq!(view.crop, Some([100.0, 50.0, 600.0, 600.0]), "dragging the bottom edge did nothing");
        // The top edge dragged up: the height leads again.
        let view = last_raster(&gesture(&mut session, RasterGesture::Crop { rect: [100.0, 0.0, 600.0, 650.0] }));
        assert_eq!(view.crop, Some([100.0, 0.0, 650.0, 650.0]));
        // The right edge: the width leads, as before.
        let view = last_raster(&gesture(&mut session, RasterGesture::Crop { rect: [100.0, 0.0, 500.0, 650.0] }));
        assert_eq!(view.crop, Some([100.0, 0.0, 500.0, 500.0]));
    }

    /// The action a declared control carries, sent as the host sends it.
    fn run(session: &mut Session, action: &protocol::Action) -> Vec<AppMessage> {
        command(session, &action.id, action.arg.clone().unwrap_or(Value::Null))
    }

    fn film_of(messages: &[AppMessage]) -> protocol::Film {
        last_raster(messages).film.expect("the raster view has no filmstrip")
    }

    fn layer_count(messages: &[AppMessage]) -> usize {
        messages
            .iter()
            .rev()
            .find_map(|message| match message {
                AppMessage::Properties(properties) => properties.layers.as_ref().map(|layers| layers.rows.len()),
                _ => None,
            })
            .expect("no Layers page was sent")
    }

    fn undo_enabled(messages: &[AppMessage]) -> bool {
        messages
            .iter()
            .rev()
            .find_map(|message| match message {
                AppMessage::Menu { rows } => rows.iter().find(|row| row.label == "Undo").map(|row| row.enabled),
                _ => None,
            })
            .expect("no menu was sent")
    }

    /// A click on a thumbnail, as the host sends it: `focus`, then `select`.
    fn click(session: &mut Session, film: &protocol::Film, index: usize) -> Vec<AppMessage> {
        run(session, &film.focus);
        run(session, &film.select.with_value(json!(index)))
    }

    /// Catches a filmstrip that drifts from the views or the pictures: every
    /// picture as an item with its name, the shown one, a thumbnail file of
    /// width × height × 4 bytes made from that picture's own composite, the
    /// menu's three rows with Delete off for the last picture, and the +
    /// tile's two rows.
    #[test]
    fn the_raster_view_carries_the_filmstrip() {
        let mut session = Session::launch();
        let film = film_of(&session.greeting());
        assert_eq!(film.items.len(), 1);
        assert_eq!(film.items[0].name, "Untitled Asset");
        let menu: Vec<(&str, bool)> = film.items[0].menu.iter().map(|row| (row.label.as_str(), row.enabled)).collect();
        assert_eq!(menu, [("Duplicate", true), ("Rename", true), ("Delete", false)], "the last picture can be deleted");
        assert!(film.items[0].menu[1].rename, "Rename does not rename in place");
        let add: Vec<&str> = film.add.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(add, ["New picture", "Open PNG or JPEG…"]);

        // Paint the first picture red, then add two more.
        command(&mut session, "set_color", json!({ "value": [220, 0, 0] }));
        command(&mut session, "tool", json!({ "tool": "brush" }));
        command(&mut session, "set_brush", json!({ "tool": "brush", "setting": "size", "value": 1000 }));
        for y in (0..1080).step_by(200) {
            stroke(&mut session, [0.0, f64::from(y)], [1920.0, f64::from(y)]);
        }
        run(&mut session, &film.add[0].action.clone().unwrap());
        let film = film_of(&run(&mut session, &film.add[0].action.clone().unwrap()));
        let names: Vec<&str> = film.items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, ["Untitled Asset", "Untitled Asset 2", "Untitled Asset 3"]);
        assert_eq!(film.shown, 2, "the added picture is not shown");
        assert!(film.items.iter().all(|item| item.menu[2].enabled), "Delete is off with three pictures");
        let mut paths = std::collections::HashSet::new();
        for item in &film.items {
            let thumb = &item.thumbnail;
            assert_eq!((thumb.width, thumb.height), (78, 44));
            let bytes = std::fs::read(&thumb.path).expect("a thumbnail that is not there");
            assert_eq!(bytes.len(), 78 * 44 * 4);
            assert!(paths.insert(thumb.path.clone()), "two pictures share a thumbnail");
        }
        let red = std::fs::read(&film.items[0].thumbnail.path).unwrap();
        assert_eq!(&red[..4], [220, 0, 0, 255], "the first thumbnail is not its picture");
        let white = std::fs::read(&film.items[1].thumbnail.path).unwrap();
        assert!(white.chunks(4).all(|pixel| pixel == [255, 255, 255, 255]));
    }

    /// Catches a stroke that writes the whole picture for every part, and
    /// a picture file or row files left behind: while drawn, each view's
    /// rows are a file of their own, holding the composite's rows, and the
    /// picture file is untouched, even when the stroke ends; once idle, the
    /// picture file is written whole under the revision the host already
    /// shows, and the row files go.
    #[test]
    fn a_stroke_drawn_sends_only_its_rows_and_the_picture_catches_up() {
        let mut session = Session::launch();
        let first = last_raster(&session.greeting());
        command(&mut session, "tool", json!({ "tool": "brush" }));
        let tab = session.images[0].id.clone();
        let before = std::fs::read(&first.picture).unwrap();
        let points = [[100.0, 100.0, 1.0], [300.0, 140.0, 1.0], [500.0, 100.0, 1.0]];
        let mut views: Vec<RasterView> = Vec::new();
        for (from, point) in points.iter().enumerate() {
            session.handle(HostMessage::RasterGesture { tab: tab.clone(), gesture: RasterGesture::StrokePart { from, points: vec![*point] } });
            let view = last_raster(&session.take_live());
            let patch = view.patch.clone().expect("a part came without its rows");
            assert_eq!(patch.since, views.last().map_or(first.revision, |view| view.revision), "a part does not follow the one before");
            let file = patch.file.as_ref().expect("a part's rows are not a file of their own");
            let [x, y, w, h] = patch.rect.map(|value| value as usize);
            let rows = std::fs::read(file).unwrap();
            let composite = session.images[0].doc_mut().composite();
            let mut want = Vec::new();
            for row in y..y + h {
                want.extend_from_slice(&composite[(row * 1920 + x) * 4..(row * 1920 + x + w) * 4]);
            }
            assert!(rows == want, "part {from}'s rows are not the picture's");
            views.push(view);
        }
        assert_eq!(std::fs::read(&first.picture).unwrap(), before, "the whole picture was written while the stroke was drawn");
        let last = views.last().unwrap().clone();
        let end = last_raster(&gesture(&mut session, RasterGesture::Stroke { points: points.to_vec() }));
        assert_eq!(end.revision, last.revision, "the stroke's end was sent as a new picture");
        assert_eq!(std::fs::read(&first.picture).unwrap(), before, "the whole picture was written as the stroke ended");
        let caught = last_raster(&session.tick());
        assert_eq!((caught.revision, caught.patch.is_none()), (last.revision, true), "the idle program did not say the picture is whole");
        let image = &mut session.images[0];
        assert_eq!(std::fs::read(&caught.picture).unwrap(), image.doc_mut().composite(), "the picture file did not catch up");
        let cache = Path::new(&caught.picture).parent().unwrap();
        let rows = std::fs::read_dir(cache).unwrap().flatten().filter(|entry| entry.path().extension().is_some_and(|extension| extension == "patch")).count();
        assert_eq!(rows, 0, "files of rows were left behind");
    }

    /// Catches a write in the background that starts while the person
    /// works, loses a change, or lands after a later one: a due write waits
    /// for the program to be idle; a picture changed while its write is made
    /// stays unwritten; and the next write holds both changes.
    #[test]
    fn a_change_made_while_written_is_written_too() {
        let folder = scratch_folder();
        std::fs::create_dir_all(&folder).unwrap();
        let folder = folder.canonicalize().unwrap();
        let mut session = Session::launch();
        let _ = session.greeting();
        session.handle(HostMessage::Project { folder: folder.display().to_string() });
        command(&mut session, "tool", json!({ "tool": "brush" }));
        stroke(&mut session, [100.0, 100.0], [300.0, 100.0]);
        // Due, a write waits for the program to be idle, never a message.
        session.images[0].unwritten_since = Some(Instant::now() - WRITE_AFTER * 2);
        command(&mut session, "add_layer", json!({}));
        command(&mut session, "undo", json!({}));
        assert!(session.images[0].saving.is_none(), "a write was started while the person works");
        session.tick();
        assert!(session.images[0].saving.is_some(), "the write did not go to the writer");
        stroke(&mut session, [100.0, 300.0], [300.0, 300.0]);
        session.finish_writes(true);
        assert!(session.images[0].dirty, "a change made while writing was taken as written");
        let path = session.images[0].path.clone().expect("the first write named no file");
        let read = files::read_ipx(&path).unwrap();
        let pixel = |picture: &Picture, y: usize| picture.pixels.values().next().unwrap().data[(y * 1920 + 200) * 4];
        assert_eq!(pixel(&read.pictures[0].picture, 100), 0);
        session.write_into_project(0).unwrap();
        assert!(!session.images[0].dirty);
        let read = files::read_ipx(&path).unwrap();
        assert_eq!((pixel(&read.pictures[0].picture, 100), pixel(&read.pictures[0].picture, 300)), (0, 0), "the second write lost a stroke");
        let _ = std::fs::remove_dir_all(folder);
    }

    /// Catches a stroke taken off the picture while it is still drawn: the
    /// host's ping each second, a camera move, and a new viewport between
    /// its parts leave every part on the picture.
    #[test]
    fn a_stroke_held_past_a_ping_stays_on_the_picture() {
        let mut session = Session::launch();
        let _ = session.greeting();
        command(&mut session, "tool", json!({ "tool": "brush" }));
        let tab = session.images[0].id.clone();
        let part = |session: &mut Session, from: usize, points: Vec<[f64; 3]>| {
            session.handle(HostMessage::RasterGesture { tab: tab.clone(), gesture: RasterGesture::StrokePart { from, points } });
        };
        part(&mut session, 0, vec![[100.0, 100.0, 1.0], [200.0, 100.0, 1.0]]);
        session.handle(HostMessage::Ping);
        session.handle(HostMessage::Camera { tab: tab.clone(), origin: [0.0, 0.0], zoom: 0.5 });
        session.handle(HostMessage::Viewport { viewport: [900.0, 600.0] });
        part(&mut session, 2, vec![[300.0, 100.0, 1.0], [400.0, 100.0, 1.0]]);
        assert_eq!(at(&mut session, 150, 100)[..3], [0, 0, 0], "the stroke's first part was taken off");
        assert_eq!(at(&mut session, 350, 100)[..3], [0, 0, 0], "the stroke's later part was not painted");
    }

    /// Catches a thumbnail click that changes nothing, or changes the
    /// picture without its layers: the shown picture's file, size, and
    /// Layers page follow the click.
    #[test]
    fn a_click_on_a_thumbnail_shows_that_picture_and_its_layers() {
        let mut session = Session::launch();
        let film = film_of(&session.greeting());
        command(&mut session, "add_layer", json!({}));
        command(&mut session, "add_layer", json!({}));
        let messages = run(&mut session, &film.add[0].action.clone().unwrap());
        assert_eq!(layer_count(&messages), 1);
        let second = last_raster(&messages);
        let messages = click(&mut session, &film_of(&messages), 0);
        let first = last_raster(&messages);
        assert_eq!(first.film.as_ref().unwrap().shown, 0);
        assert_eq!(layer_count(&messages), 3, "the Layers page is still the other picture's");
        assert_ne!(first.picture, second.picture, "two pictures share one file, so the host reads each again");
        assert!(first.camera.is_some(), "the picture shown came without its camera");
        // A picture shown again is the file and revision the host already
        // holds, so switching reads nothing.
        let messages = click(&mut session, &film_of(&messages), 1);
        assert_eq!(layer_count(&messages), 1);
        let again = last_raster(&messages);
        assert_eq!((again.picture.as_str(), again.revision), (second.picture.as_str(), second.revision), "a picture shown again was sent again");
        assert_eq!(session.images[0].gallery.shown, 1);
    }

    /// Catches one picture's Undo reaching another: each picture keeps its
    /// own history, and Undo with the picture's keyboard acts only on the
    /// picture shown.
    #[test]
    fn each_picture_undoes_on_its_own() {
        let mut session = Session::launch();
        let film = film_of(&session.greeting());
        command(&mut session, "tool", json!({ "tool": "brush" }));
        stroke(&mut session, [10.0, 10.0], [100.0, 10.0]);
        let messages = run(&mut session, &film.add[0].action.clone().unwrap());
        assert!(!undo_enabled(&messages), "a new picture offers another's undo");
        stroke(&mut session, [10.0, 10.0], [100.0, 10.0]);
        stroke(&mut session, [10.0, 50.0], [100.0, 50.0]);
        // A click on a thumbnail, as the host sends it (a press, then the
        // choice), gives Undo to the picture it shows, never the strip.
        let film = film_of(&messages);
        click(&mut session, &film, 0);
        let messages = command(&mut session, "undo", json!({}));
        assert_eq!(session.images[0].gallery.frames.len(), 2, "Undo after a thumbnail click undid the strip");
        assert_eq!(at(&mut session, 50, 10), [255, 255, 255, 255], "the first picture's stroke was not undone");
        assert!(!undo_enabled(&messages));
        click(&mut session, &film, 1);
        command(&mut session, "undo", json!({}));
        assert_eq!(session.images[0].gallery.frames.len(), 2, "Undo after a thumbnail click undid the strip");
        assert_eq!(at(&mut session, 50, 50), [255, 255, 255, 255], "the second picture's last stroke was not undone");
        click(&mut session, &film, 0);
        let second = &mut session.images[0].gallery.frames[1].doc;
        let painted = |doc: &mut Doc, y: usize| doc.composite()[(y * 1920 + 50) * 4];
        assert_eq!(painted(second, 10), 0, "an undo on the first picture reached the second");
        assert_eq!(painted(second, 50), 255);
        assert_eq!(session.images[0].gallery.frames.len(), 2, "an undo on the picture undid the strip");
    }

    /// Catches the strip's Undo missing a change, acting with the picture's
    /// keyboard, or keeping the keyboard after the picture is used: after a
    /// press on the strip, Undo reverses add, duplicate, delete, rename, and
    /// reorder, and the picture shown follows; a gesture gives Undo back to
    /// the picture.
    #[test]
    fn the_strips_undo_reverses_its_changes() {
        let mut session = Session::launch();
        let film = film_of(&session.greeting());
        let names = |session: &Session| -> Vec<String> {
            session.images[0].gallery.frames.iter().map(|frame| frame.name.clone()).collect()
        };
        run(&mut session, &film.focus);
        let messages = run(&mut session, &film.add[0].action.clone().unwrap());
        let film = film_of(&messages);
        run(&mut session, &film.focus);
        let film = film_of(&run(&mut session, &film.items[1].menu[0].action.clone().unwrap()));
        let rename = film.items[0].menu[1].action.clone().unwrap().with_value(json!("Beach"));
        let film = film_of(&run(&mut session, &rename));
        // A drag of the first thumbnail to the end, as the host sends it.
        let film = film_of(&command(&mut session, &film.reorder.id, json!({ "from": 0, "to": 2 })));
        assert_eq!(names(&session), ["Untitled Asset 2", "Untitled Asset 2 copy", "Beach"]);
        let messages = run(&mut session, &film.items[1].menu[2].action.clone().unwrap());
        assert_eq!(names(&session), ["Untitled Asset 2", "Beach"]);
        assert!(undo_enabled(&messages));
        assert!(session.images[0].dirty);

        // The strip's Undo, step by step, with the shown picture following.
        let mut seen = Vec::new();
        for _ in 0..5 {
            command(&mut session, "undo", json!({}));
            let gallery = &session.images[0].gallery;
            seen.push((names(&session).join(", "), gallery.frame().name.clone()));
        }
        let want = [
            ("Untitled Asset 2, Untitled Asset 2 copy, Beach", "Untitled Asset 2 copy"),
            ("Beach, Untitled Asset 2, Untitled Asset 2 copy", "Beach"),
            ("Untitled Asset, Untitled Asset 2, Untitled Asset 2 copy", "Untitled Asset"),
            ("Untitled Asset, Untitled Asset 2", "Untitled Asset 2"),
            ("Untitled Asset", "Untitled Asset"),
        ];
        let want: Vec<(String, String)> = want.iter().map(|(all, shown)| (all.to_string(), shown.to_string())).collect();
        assert_eq!(seen, want);
        let messages = command(&mut session, "undo", json!({}));
        assert!(!undo_enabled(&messages), "the strip's Undo offers the picture's history");
        command(&mut session, "redo", json!({}));
        assert_eq!(session.images[0].gallery.frames.len(), 2);

        // A stroke gives the keyboard back: Undo takes the stroke, not the
        // strip's step. The brush is already chosen, so only the stroke
        // reaches the picture.
        assert_eq!(session.tools.tool, ToolKind::Brush);
        stroke(&mut session, [10.0, 10.0], [100.0, 10.0]);
        command(&mut session, "undo", json!({}));
        assert_eq!(session.images[0].gallery.frames.len(), 2, "Undo after a stroke undid the strip");
        assert_eq!(at(&mut session, 50, 10), [255, 255, 255, 255]);
        // So do a Layers page command and a panel edit.
        command(&mut session, "film_focus", json!({}));
        command(&mut session, "add_layer", json!({}));
        command(&mut session, "undo", json!({}));
        assert_eq!(session.images[0].gallery.frames.len(), 2, "Undo after a layer command undid the strip");
        assert_eq!(session.images[0].doc().picture.nodes.len(), 1);
        command(&mut session, "film_focus", json!({}));
        session.handle(HostMessage::Property { prop: json!("opacity"), value: serde_json::to_value(PropValue::Number(0.5)).unwrap(), preview: false });
        command(&mut session, "undo", json!({}));
        assert_eq!(session.images[0].gallery.frames.len(), 2, "Undo after a panel edit undid the strip");
        assert_eq!(session.images[0].doc().picture.nodes[0].opacity, 1.0);
    }

    /// Catches the last picture deleted through its menu or a stale click.
    #[test]
    fn the_last_picture_is_not_deleted() {
        let mut session = Session::launch();
        let _ = session.greeting();
        command(&mut session, "film_focus", json!({}));
        let messages = command(&mut session, "film_delete", json!({ "picture": 0 }));
        assert_eq!(session.images[0].gallery.frames.len(), 1);
        assert!(!undo_enabled(&messages), "a refused delete became a step");
        assert!(!film_of(&messages).items[0].menu[2].enabled);
    }

    /// Catches a gallery that is not kept whole in its one file: three
    /// pictures, each with its own layers and name, and the one shown, are
    /// written into the project and open again as they were.
    #[test]
    fn a_gallery_is_kept_in_its_file() {
        let folder = std::env::temp_dir().join(format!("image-editor-gallery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let folder = folder.canonicalize().unwrap();
        let mut session = Session::launch();
        let _ = session.greeting();
        session.handle(HostMessage::Project { folder: folder.display().to_string() });
        command(&mut session, "film_new", json!({}));
        command(&mut session, "add_layer", json!({}));
        command(&mut session, "film_duplicate", json!({ "picture": 1 }));
        command(&mut session, "add_layer", json!({}));
        command(&mut session, "film_rename", json!({ "picture": 0, "value": "Sky" }));
        command(&mut session, "film_select", json!({ "value": 1 }));
        let replies = session.handle(HostMessage::Keep { tab: "1".into() });
        let path = folder.join("Untitled Asset.ipx");
        assert!(replies.iter().any(|reply| matches!(reply, AppMessage::Kept { path: Some(_), error: None, .. })));

        let mut again = Session::restored();
        again.handle(HostMessage::Project { folder: folder.display().to_string() });
        let messages = again.handle(HostMessage::Open { path: Some(path.display().to_string()) });
        let film = film_of(&messages);
        let names: Vec<&str> = film.items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, ["Sky", "Untitled Asset 2", "Untitled Asset 2 copy"]);
        assert_eq!(film.shown, 1);
        assert_eq!(layer_count(&messages), 2);
        let layers: Vec<usize> = again.images[0].gallery.frames.iter().map(|frame| frame.doc.picture.nodes.len()).collect();
        assert_eq!(layers, [1, 2, 3]);
        let _ = std::fs::remove_dir_all(folder);
    }

    /// Catches an assistant gallery call that the tab does not show, or that
    /// is not one step of the strip's history.
    #[test]
    fn the_assistants_gallery_calls_show_and_undo_one_by_one() {
        let mut session = Session::launch();
        let _ = session.greeting();
        let invoke = |session: &mut Session, tool: &str, input: Value| {
            session.handle(HostMessage::Invoke {
                call: "c".into(),
                tab: "1".into(),
                tool: protocol::tool_key("image-editor", "image", tool),
                input,
                group: "r1".into(),
            })
        };
        let messages = invoke(&mut session, "add_picture", json!({ "name": "Moon" }));
        let AppMessage::ToolResult(result) = messages.last().unwrap() else { panic!("no result last") };
        assert!(result.ok, "{:?}", result.output);
        assert_eq!(film_of(&messages).items[1].name, "Moon", "the strip does not show the added picture");
        invoke(&mut session, "duplicate_picture", json!({ "picture": 1 }));
        invoke(&mut session, "rename_picture", json!({ "picture": 0, "name": "Sun" }));
        invoke(&mut session, "move_picture", json!({ "picture": 0, "to": 2 }));
        invoke(&mut session, "remove_picture", json!({ "picture": 1 }));
        let messages = invoke(&mut session, "show_picture", json!({ "picture": 0 }));
        assert_eq!(film_of(&messages).shown, 0);
        let messages = invoke(&mut session, "list_pictures", json!({}));
        let AppMessage::ToolResult(result) = messages.last().unwrap() else { panic!("no result last") };
        assert_eq!(result.output["pictures"].as_array().unwrap().len(), 2);
        command(&mut session, "film_focus", json!({}));
        let mut lengths = Vec::new();
        while session.images[0].gallery.can_undo() {
            command(&mut session, "undo", json!({}));
            lengths.push(session.images[0].gallery.frames.len());
        }
        assert_eq!(lengths, [3, 3, 3, 2, 1], "the five calls are not five steps");
    }
}
