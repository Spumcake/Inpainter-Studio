//! The Image Editor's tabs, one layered image each, and everything the host
//! sends them: commands, gestures, panel edits, files, and the project.
//!
//! Inside a project, which the host names with `project`, every image is
//! written into the project's folder by itself, a second after it changes,
//! and before its tab closes, a `keep` is answered, or the host quits, as
//! Mission Control's notebooks are. Save and Save As are offered only
//! outside a project.

use crate::declare;
use crate::doc::{Doc, Setting};
use crate::files::{self, PngCache};
use crate::paint::{Brush, Combine, Mask};
use crate::picture::{Blend, Picture, Pixels};
use protocol::{
    AppMessage, Ask, AskButton, ContentsKind, DocumentCamera, FileFilter, FileKind, HostMessage, PanelSide,
    PropValue, RasterGesture, RasterTool, RasterView, Thumbnail,
};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

pub struct Image {
    pub id: String,
    pub label: String,
    pub path: Option<PathBuf>,
    pub doc: Doc,
    pub dirty: bool,
    /// Inside a project: when its change was first seen unwritten, and its
    /// last failed write, why and when.
    unwritten_since: Option<Instant>,
    write_failed: Option<(String, Instant)>,
    /// The crop box while the crop tool is chosen: `[x, y, width, height]`.
    pub crop: Option<[f64; 4]>,
    /// The camera as the host last reported it, or as this sent it.
    pub camera: Option<DocumentCamera>,
    /// A camera to send with the next picture: the image is new or opened.
    send_camera: bool,
    /// The host has reported moving this image's camera.
    host_camera: bool,
    /// The composite revision last written to the picture file.
    written: Option<u64>,
    /// Each layer's thumbnail, and the bytes written for it.
    pub thumbs: HashMap<u64, (Vec<u8>, Thumbnail)>,
    pngs: PngCache,
}

/// What a file dialog's answer is for.
#[derive(Debug, Clone)]
enum Purpose {
    Open,
    OpenLayer,
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
    /// Every revision this process sends starts above this, so a picture of
    /// an earlier run is never taken for a current one.
    seed: u64,
    next_thumb: u64,
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
        session.push_image(Doc::new(Picture::blank(NEW_SIZE[0], NEW_SIZE[1])), None, None);
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
            seed,
            next_thumb: seed,
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
                // Every picture is written again where the host now looks.
                for image in &mut self.images {
                    image.written = None;
                    image.thumbs.clear();
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
                self.command(&id, tab.as_deref(), arg.unwrap_or(Value::Null), &mut out);
            }
            HostMessage::RasterGesture { tab, gesture } => {
                self.activate(&tab);
                self.gesture(gesture);
                out.extend(self.emit());
            }
            HostMessage::Camera { tab, origin, zoom } => {
                if let Some(image) = self.images.iter_mut().find(|image| image.id == tab) {
                    image.camera = Some(DocumentCamera { origin, zoom });
                    image.host_camera = true;
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
                        if !image.host_camera {
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
        if self.project.is_some() && self.write_due(Instant::now()) {
            out.extend(self.declared());
        }
        out
    }

    /// The loop's wake with no message: writes what is due inside a project,
    /// and sends what that changed, such as a tab's new name.
    pub fn tick(&mut self) -> Vec<AppMessage> {
        if self.project.is_some() && self.write_due(Instant::now()) {
            return self.declared();
        }
        Vec::new()
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

    fn push_image(&mut self, doc: Doc, path: Option<PathBuf>, label: Option<String>) -> usize {
        let id = self.next_tab.to_string();
        self.next_tab += 1;
        let label = label
            .or_else(|| path.as_deref().map(file_title))
            .unwrap_or_else(|| String::from(UNTITLED));
        let camera = fit(doc.picture.width, doc.picture.height, self.viewport);
        self.images.push(Image {
            id,
            label,
            path,
            doc,
            dirty: false,
            unwritten_since: None,
            write_failed: None,
            crop: None,
            camera: Some(camera),
            send_camera: true,
            host_camera: false,
            written: None,
            thumbs: HashMap::new(),
            pngs: PngCache::default(),
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

    /// Runs `change` on the active image's document, and marks the image
    /// changed when it was.
    fn change<T>(&mut self, change: impl FnOnce(&mut Doc) -> T) -> Option<T> {
        let image = self.images.get_mut(self.active)?;
        let before = image.doc.edits;
        let result = change(&mut image.doc);
        if image.doc.edits != before {
            image.dirty = true;
        }
        Some(result)
    }

    fn command(&mut self, id: &str, tab: Option<&str>, arg: Value, out: &mut Vec<AppMessage>) {
        let number = |key: &str| arg.get(key).and_then(Value::as_f64);
        let node = arg.get("node").and_then(Value::as_u64);
        match id {
            "new_image" => {
                self.push_image(Doc::new(Picture::blank(NEW_SIZE[0], NEW_SIZE[1])), None, None);
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
                let name = format!("{}.{}", image.label, if jpeg { "jpg" } else { "png" });
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
            "undo" | "redo" => {
                let size = self.active_image().map(|image| (image.doc.picture.width, image.doc.picture.height));
                self.change(if id == "undo" { Doc::undo } else { Doc::redo });
                // A crop undone or redone: the box starts again from the
                // whole picture.
                let now = self.active_image().map(|image| (image.doc.picture.width, image.doc.picture.height));
                if self.tools.tool == ToolKind::Crop && size != now {
                    self.start_crop();
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
                    if let Some(rect) = self.active_image().and_then(|image| image.crop) {
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
                let target = node.or_else(|| self.active_image().and_then(|image| image.doc.active));
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
            image.crop = None;
        }
    }

    /// The crop tool starts with the whole picture's box, ready to drag.
    fn start_crop(&mut self) {
        if let Some(image) = self.images.get_mut(self.active) {
            image.crop = Some([0.0, 0.0, f64::from(image.doc.picture.width), f64::from(image.doc.picture.height)]);
        }
    }

    /// Sets the crop box, kept inside the picture and to the chosen ratio.
    fn set_crop(&mut self, rect: [f64; 4]) {
        let ratio = declare::RATIOS[self.tools.ratio].1;
        let Some(image) = self.images.get_mut(self.active) else {
            return;
        };
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
        let Some(rect) = self.active_image().and_then(|image| image.crop) else {
            return;
        };
        self.change(|doc| doc.crop(rect));
        self.start_crop();
    }

    fn gesture(&mut self, gesture: RasterGesture) {
        let tools = self.tools.clone();
        match gesture {
            RasterGesture::Stroke { points } => {
                let (settings, erase) = match tools.tool {
                    ToolKind::Brush => (tools.brush, false),
                    ToolKind::Eraser => (tools.eraser, true),
                    _ => return,
                };
                let brush = Brush {
                    rgb: tools.rgb,
                    size: f64::from(settings.size),
                    hardness: f64::from(settings.hardness) / 100.0,
                    opacity: f64::from(settings.opacity) / 100.0,
                    pressure: settings.pressure,
                    erase,
                };
                // A stroke on a locked or hidden layer changes nothing; the
                // host's preview goes when the picture comes back.
                self.change(|doc| doc.stroke(&points, &brush));
            }
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
                if let Some(rgba) = self.images.get_mut(self.active).and_then(|image| image.doc.pick(at)) {
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
            Purpose::Export { jpeg } => {
                let Some(image) = self.images.get_mut(self.active) else {
                    return;
                };
                let (width, height) = (image.doc.picture.width, image.doc.picture.height);
                let pixels = image.doc.composite();
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
            self.push_image(Doc::new(Picture::blank(NEW_SIZE[0], NEW_SIZE[1])), None, None);
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
                    let mut doc = Doc::new(read.picture);
                    if read.active.is_some() {
                        doc.active = read.active;
                    }
                    self.push_image(doc, Some(path), None);
                    self.declare_new(out);
                }
                Err(reason) => out.push(notice("This image could not be opened", &reason)),
            }
            return;
        }
        match files::import(&path) {
            Ok(pixels) => {
                let title = file_title(&path);
                let index = self.push_image(Doc::new(Picture::from_pixels(&title, pixels)), None, Some(title));
                // It is new work, so it is kept in the project as an image.
                self.images[index].dirty = true;
                self.declare_new(out);
            }
            Err(reason) => out.push(notice("This picture could not be opened", &reason)),
        }
    }

    /// Writes every image whose change is a second old, and tries a failed
    /// one again after a while. Returns whether any was written.
    fn write_due(&mut self, now: Instant) -> bool {
        let mut wrote = false;
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
            wrote |= self.write_into_project(index).is_ok();
        }
        wrote
    }

    /// Writes one image into the project's folder: to its own file, or, for
    /// one never written, to a new file named after it that replaces
    /// nothing. Returns the file, or none for an untouched image.
    fn write_into_project(&mut self, index: usize) -> Result<Option<PathBuf>, String> {
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
        image.doc.finish_edit();
        match files::write_ipx(&path, &image.doc.picture, image.doc.active, &mut image.pngs) {
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
        let Some(image) = self.images.get_mut(index) else {
            return;
        };
        image.doc.finish_edit();
        if let Err(reason) = files::write_ipx(path, &image.doc.picture, image.doc.active, &mut image.pngs) {
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
        let _ = std::fs::remove_file(self.cache.join(format!("{}.rgba", image.id)));
        for layer in image.thumbs.keys() {
            let _ = std::fs::remove_file(self.cache.join(format!("{}-{layer}.rgba", image.id)));
        }
        if self.images.is_empty() {
            self.push_image(Doc::new(Picture::blank(NEW_SIZE[0], NEW_SIZE[1])), None, None);
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
        let view = self.raster_view(index);
        self.write_thumbnails(index);
        view.map(AppMessage::Raster).into_iter().collect()
    }

    fn raster_view(&mut self, index: usize) -> Option<RasterView> {
        let seed = self.seed;
        let tool = self.raster_tool();
        let move_tool = self.tools.tool == ToolKind::Move;
        let crop_tool = self.tools.tool == ToolKind::Crop;
        let cache = self.cache.clone();
        let viewport = self.viewport;
        let image = self.images.get_mut(index)?;
        image.doc.refresh();
        let picture = cache.join(format!("{}.rgba", image.id));
        if image.written != Some(image.doc.revision) {
            let (width, height) = (image.doc.picture.width, image.doc.picture.height);
            let _ = files::write_atomic(&picture, image.doc.composite());
            debug_assert_eq!(image.doc.composite().len(), width as usize * height as usize * 4);
            image.written = Some(image.doc.revision);
        }
        let camera = if std::mem::take(&mut image.send_camera) {
            let camera = fit(image.doc.picture.width, image.doc.picture.height, viewport);
            image.camera = Some(camera.clone());
            Some(camera)
        } else {
            None
        };
        if crop_tool {
            // A tab chosen, or a crop undone, since the box was set: the box
            // starts again from the whole picture.
            let (width, height) = (f64::from(image.doc.picture.width), f64::from(image.doc.picture.height));
            let fits = image.crop.is_some_and(|rect| rect[0] + rect[2] <= width && rect[1] + rect[3] <= height);
            if !fits {
                image.crop = Some([0.0, 0.0, width, height]);
            }
        }
        let bounds = if move_tool {
            image.doc.active.and_then(|active| image.doc.picture.node_rect(active))
        } else {
            None
        };
        Some(RasterView {
            tab: image.id.clone(),
            width: image.doc.picture.width,
            height: image.doc.picture.height,
            picture: picture.display().to_string(),
            revision: seed + image.doc.revision,
            tool,
            selection: image.doc.outlines(),
            crop: if crop_tool { image.crop } else { None },
            bounds: bounds.map(|rect| [f64::from(rect.x), f64::from(rect.y), f64::from(rect.w), f64::from(rect.h)]),
            camera,
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
        let mut next = self.next_thumb;
        let Some(image) = self.images.get_mut(index) else {
            return;
        };
        let picture = &image.doc.picture;
        let (width, height) = thumb_size(picture.width, picture.height);
        let layers = picture.all_layers();
        image.thumbs.retain(|id, _| layers.contains(id));
        for id in layers {
            let (Some(node), Some(pixels)) = (picture.find(id), picture.pixels.get(&id)) else {
                continue;
            };
            let bytes = thumbnail(picture.width, picture.height, node.offset, pixels, width, height);
            if image.thumbs.get(&id).is_some_and(|(kept, _)| *kept == bytes) {
                continue;
            }
            let path = cache.join(format!("{}-{id}.rgba", image.id));
            if files::write_atomic(&path, &bytes).is_err() {
                continue;
            }
            next += 1;
            let thumb = Thumbnail { path: path.display().to_string(), width, height, revision: next };
            image.thumbs.insert(id, (bytes, thumb));
        }
        self.next_thumb = next;
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
        let image = &mut self.images[index];
        let before = image.doc.edits;
        let (outcome, look) = crate::assist::run(&mut image.doc, name, &input, group);
        if image.doc.edits != before {
            image.dirty = true;
        }
        let mut out = Vec::new();
        if image.doc.edits != before {
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
    let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let scale = (f64::from(THUMB_SIZE[0]) / w).min(f64::from(THUMB_SIZE[1]) / h);
    (((w * scale).round() as u32).max(1), ((h * scale).round() as u32).max(1))
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
        let width = image.doc.picture.width as usize;
        let composite = image.doc.composite();
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
        assert!(session.images[0].doc.selection.is_none());

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
        let background = session.images[0].doc.active.unwrap();
        let property = |session: &mut Session, prop: &str, value: PropValue, preview: bool| {
            session.handle(HostMessage::Property { prop: json!(prop), value: serde_json::to_value(value).unwrap(), preview })
        };
        property(&mut session, "blend", PropValue::Index(1), false);
        assert_eq!(session.images[0].doc.picture.nodes[0].blend, Blend::Multiply);
        for value in [0.8, 0.6, 0.4] {
            property(&mut session, "opacity", PropValue::Number(value), true);
        }
        command(&mut session, "finish_edit", json!({}));
        assert_eq!(session.images[0].doc.picture.nodes[0].opacity, 0.4);
        command(&mut session, "undo", json!({}));
        assert_eq!(session.images[0].doc.picture.nodes[0].opacity, 1.0, "the held slider was more than one step");
        assert_eq!(session.images[0].doc.picture.nodes[0].blend, Blend::Multiply);
        property(&mut session, "locks", PropValue::Index(1), false);
        assert!(session.images[0].doc.picture.nodes[0].locks.pixels);
        command(&mut session, "tool", json!({ "tool": "brush" }));
        stroke(&mut session, [10.0, 10.0], [40.0, 10.0]);
        assert_eq!(at(&mut session, 20, 10), [255, 255, 255, 255], "a pixel-locked layer took paint");

        command(&mut session, "add_layer", json!({}));
        let added = session.images[0].doc.active.unwrap();
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
        assert!(session.images[0].doc.picture.find(added).is_none());
        assert!(session.images[0].doc.picture.find(background).is_some());
        command(&mut session, "undo", json!({}));
        assert!(session.images[0].doc.picture.find(added).is_some());
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
        assert_eq!(files::read_ipx(Path::new(&expected)).unwrap().picture.nodes.len(), 2);

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
        let background = session.images[0].doc.active.unwrap();
        let messages = command(&mut session, "layer_lock", json!({ "node": background }));
        let row = options(&messages);
        assert_eq!(strong_labels(&row), ["Background is locked."]);
        assert!(matches!(row.last(), Some(ToolItem::Label { strong: true, .. })), "the reason is not after the settings");
        stroke(&mut session, [10.0, 10.0], [40.0, 10.0]);
        assert_eq!(at(&mut session, 20, 10), [255, 255, 255, 255], "a locked layer took paint");
        let messages = command(&mut session, "tool", json!({ "tool": "move" }));
        assert_eq!(strong_labels(&options(&messages)), ["Background is locked."]);
        command(&mut session, "add_group", json!({}));
        let group = session.images[0].doc.picture.nodes[0].id;
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
        let background = session.images[0].doc.active.unwrap();
        command(&mut session, "add_layer", json!({}));
        let top = session.images[0].doc.active.unwrap();
        command(&mut session, "add_group", json!({}));
        let group = session.images[0].doc.picture.place(top).unwrap().parent.unwrap();
        command(&mut session, "reorder", json!({ "node": background, "value": 0, "parent": group }));
        assert_eq!(session.images[0].doc.picture.place(background).unwrap().parent, Some(group));
        command(&mut session, "reorder", json!({ "node": background, "value": 1, "parent": null }));
        let place = session.images[0].doc.picture.place(background).unwrap();
        assert_eq!((place.parent, place.index), (None, 0), "the layer did not come out under the group");
        command(&mut session, "reorder", json!({ "node": background, "value": 0 }));
        let place = session.images[0].doc.picture.place(background).unwrap();
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
}
