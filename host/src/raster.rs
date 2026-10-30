//! A raster tab: a picture of pixels a sub-application composites, drawn
//! through a camera the host keeps, and the pointer on it. The host draws the
//! picture, its selection's marching edge, the crop box, and the active
//! layer's box, previews a paint drag while it is drawn, and sends each
//! finished gesture in picture pixels. It knows no editor: what a gesture
//! does is the sub-application's, and the next `raster` view shows it.

use crate::process::RasterDisplay;
use crate::tablet::Tablet;
use crate::widgets;
use draw::document::Camera;
use eframe::egui::{
    self, Color32, CursorIcon, Event, Key, MouseWheelUnit, PointerButton, Pos2, Rect,
    Sense, pos2, vec2,
};
use protocol::{RasterGesture, RasterTool, RasterView};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The gray around the picture, as 0.10's approved views show it.
pub(crate) const GROUND: Color32 = Color32::from_rgb(234, 236, 240);
/// The squares drawn under transparency.
const CHECK_LIGHT: Color32 = Color32::from_rgb(255, 255, 255);
const CHECK_DARK: Color32 = Color32::from_rgb(224, 226, 230);
/// A checker square's side, in points.
const CHECK: f32 = 8.0;
/// The move tool's box and handles, in the mockup's blue.
const BOUNDS: Color32 = Color32::from_rgb(37, 99, 235);
/// The eraser's preview: a light, see-through trail.
const ERASE_TRAIL: Color32 = Color32::from_rgba_premultiplied(150, 152, 156, 150);
/// What lies outside the crop box is dimmed this much, as crop.png shows.
const CROP_DIM: Color32 = Color32::from_black_alpha(110);

/// A raster tab zooms further than a drawing does, so single pixels can be
/// worked on and large photographs can be seen whole.
const MIN_ZOOM: f64 = 1.0 / 64.0;
const MAX_ZOOM: f64 = 64.0;
/// The space Fit leaves around the picture, in points.
const FIT_MARGIN: f64 = 40.0;
/// The toolbar's zoom buttons step as the drawing surface's do.
const BUTTON_STEP: f64 = 5.0 / 4.0;
/// A click of the magnifier doubles the zoom, or halves it with Alt.
const CLICK_STEP: f64 = 2.0;
const NOTCH_POINTS: f64 = 50.0;
/// How near the pointer must be to a crop box's edge to take it.
const GRAB: f32 = 8.0;
/// A sent gesture's preview ends with the next view, or after this long
/// when none comes, as when a locked layer refuses it.
const HELD_FOR: Duration = Duration::from_secs(3);
/// The marching edge's dash, in points, and how often it moves.
const DASH: f32 = 4.0;
const MARCH: Duration = Duration::from_millis(120);

/// What the pointer finished this frame. A paint drag is its own case: the
/// window names it as the protocol does.
pub(crate) enum Out {
    Gesture(RasterGesture),
    Painted(Vec<[f64; 3]>),
}

/// How a paint drag is previewed: its colour, or none for the eraser.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    rgb: Option<[u8; 3]>,
    size: f64,
    opacity: f64,
    pressure: bool,
}

/// What a drag shows while it is open, and after it is sent until the
/// sub-application answers.
#[derive(Clone, Debug, PartialEq)]
enum Live {
    Paint { points: Vec<[f64; 3]>, look: Look },
    Rect { from: [f64; 2], to: [f64; 2] },
    Lasso { points: Vec<[f64; 2]> },
    Move { from: [f64; 2], to: [f64; 2] },
    Crop { rect: [f64; 4] },
}

/// Which part of the crop box a drag took.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    /// A new box, from the press to the pointer.
    New,
    /// The whole box, moved.
    Inside,
    /// These edges of the box: left, top, right, bottom.
    Edges([bool; 4]),
}

struct Drag {
    live: Live,
    /// Where the press was, in picture pixels.
    press: [f64; 2],
    /// The crop box when the press took it.
    crop: Option<(Grab, [f64; 4])>,
    moved: bool,
}

struct Held {
    live: Live,
    /// The tab's view count when it was sent.
    version: u64,
    at: Instant,
}

/// The pointer's state between frames on the raster tab shown.
pub(crate) struct Pointer {
    drag: Option<Drag>,
    pan: Option<PointerButton>,
    /// The open paint drag takes its samples from the tablet.
    pen: bool,
    held: Vec<Held>,
    /// Where the tab was last drawn.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) shown: Rect,
}

impl Default for Pointer {
    fn default() -> Self {
        Self {
            drag: None,
            pan: None,
            pen: false,
            held: Vec::new(),
            shown: Rect::NOTHING,
        }
    }
}

impl Pointer {
    /// Another tab is shown: whatever was being dragged ends.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Draws the tab in `rect` and handles its pointer. Finished gestures go
    /// to `out`. Returns whether the camera moved.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        display: &mut RasterDisplay,
        rect: Rect,
        tablet: &Tablet,
        locked: bool,
        dismissing: bool,
        out: &mut Vec<Out>,
    ) -> bool {
        self.shown = rect;
        let viewport = [f64::from(rect.width()), f64::from(rect.height())];
        let mut moved = false;
        if display.camera.is_none_or(|camera| !usable(&camera)) {
            fit(display, viewport);
            moved = true;
        }
        let version = display.version;
        self.held
            .retain(|held| held.version == version && held.at.elapsed() < HELD_FOR);

        let response = ui.interact(rect, ui.id().with("raster"), Sense::click_and_drag());
        let hovered = response.hovered();
        let (pos, delta, space, alt) = ui.input(|input| {
            (
                input.pointer.latest_pos(),
                input.pointer.delta(),
                input.key_down(Key::Space),
                input.modifiers.alt,
            )
        });
        let pressed = |button| ui.input(|input| input.pointer.button_pressed(button));
        let down = |button| ui.input(|input| input.pointer.button_down(button));
        let released = |button| ui.input(|input| input.pointer.button_released(button));
        let over = hovered && pos.is_some_and(|pos| rect.contains(pos));
        // A press that closes a menu does nothing else.
        if dismissing && over && (pressed(PointerButton::Primary) || pressed(PointerButton::Middle)) {
            egui::Popup::close_all(ui.ctx());
        }
        let press_inside = over && !dismissing;
        let tool = display.view.tool.clone();

        if self.drag.is_some() && ui.input(|input| input.key_pressed(Key::Escape)) {
            self.drag = None;
            self.pen = false;
        }
        if self.pan.is_none() && self.drag.is_none() && press_inside {
            if pressed(PointerButton::Middle) {
                self.pan = Some(PointerButton::Middle);
            } else if pressed(PointerButton::Primary) {
                if space || tool == RasterTool::Hand {
                    self.pan = Some(PointerButton::Primary);
                } else if tool == RasterTool::Zoom {
                    if let Some(pos) = pos {
                        let factor = if alt { 1.0 / CLICK_STEP } else { CLICK_STEP };
                        zoom_toward(display, factor, pos - rect.min);
                        moved = true;
                    }
                } else if !locked && let Some(pos) = pos {
                    self.press(ui, display, rect, tablet, pos, out);
                }
            }
        }

        if let Some(button) = self.pan {
            if delta != egui::Vec2::ZERO
                && let Some(camera) = display.camera.as_mut()
            {
                pan(camera, delta);
                moved = true;
            }
            if released(button) || !down(button) {
                self.pan = None;
            }
        } else if self.drag.is_some() {
            self.update(ui, display, rect, tablet);
            if released(PointerButton::Primary) || !down(PointerButton::Primary) {
                self.finish(display, out);
            }
        }

        moved |= wheel(ui, display, rect, over || self.drag.is_some() || self.pan.is_some());

        let cursor = self.cursor(ui, display, rect, &tool, space, alt, locked);
        response.on_hover_cursor(cursor);
        self.paint(ui, display, rect);
        moved
    }

    /// A primary press with the declared tool.
    fn press(
        &mut self,
        ui: &egui::Ui,
        display: &RasterDisplay,
        rect: Rect,
        tablet: &Tablet,
        pos: Pos2,
        out: &mut Vec<Out>,
    ) {
        let Some(camera) = display.camera else {
            return;
        };
        let at = picture_at(&camera, rect, pos);
        let live = match &display.view.tool {
            RasterTool::Brush { rgb, size, opacity, pressure, .. } => Live::Paint {
                points: Vec::new(),
                look: Look { rgb: Some(*rgb), size: *size, opacity: *opacity, pressure: *pressure },
            },
            RasterTool::Erase { size, opacity, pressure, .. } => Live::Paint {
                points: Vec::new(),
                look: Look { rgb: None, size: *size, opacity: *opacity, pressure: *pressure },
            },
            RasterTool::Marquee => Live::Rect { from: at, to: at },
            RasterTool::Lasso => Live::Lasso { points: vec![at] },
            RasterTool::Move => Live::Move { from: at, to: at },
            RasterTool::Crop => {
                let (grab, start) = match display.view.crop {
                    Some(crop) => (crop_grab(&camera, rect, crop, pos), crop),
                    None => (Grab::New, [at[0], at[1], 0.0, 0.0]),
                };
                let mut drag = Drag {
                    live: Live::Crop { rect: start },
                    press: at,
                    crop: Some((grab, start)),
                    moved: false,
                };
                if grab == Grab::New {
                    drag.live = Live::Crop { rect: [at[0], at[1], 0.0, 0.0] };
                }
                self.drag = Some(drag);
                return;
            }
            RasterTool::Eyedropper => {
                out.push(Out::Gesture(RasterGesture::Pick { at }));
                return;
            }
            RasterTool::Hand | RasterTool::Zoom => return,
        };
        let paints = matches!(live, Live::Paint { .. });
        self.drag = Some(Drag { live, press: at, crop: None, moved: false });
        if paints {
            self.pen = tablet.input().down();
            self.sample(ui, &camera, rect, tablet);
        }
    }

    /// The open drag follows the pointer.
    fn update(&mut self, ui: &egui::Ui, display: &RasterDisplay, rect: Rect, tablet: &Tablet) {
        let Some(camera) = display.camera else {
            return;
        };
        let Some(pos) = ui.input(|input| input.pointer.latest_pos()) else {
            return;
        };
        let at = picture_at(&camera, rect, pos);
        let size = [f64::from(display.view.width), f64::from(display.view.height)];
        let paints = matches!(self.drag.as_ref().map(|drag| &drag.live), Some(Live::Paint { .. }));
        if paints {
            self.sample(ui, &camera, rect, tablet);
            return;
        }
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        if at != drag.press {
            drag.moved = true;
        }
        match &mut drag.live {
            Live::Rect { to, .. } | Live::Move { to, .. } => *to = at,
            Live::Lasso { points } => {
                if points.last() != Some(&at) {
                    points.push(at);
                }
            }
            Live::Crop { rect: crop } => {
                if let Some((grab, start)) = drag.crop {
                    *crop = dragged_crop(grab, start, drag.press, at, size);
                }
            }
            Live::Paint { .. } => {}
        }
    }

    /// Adds this frame's pointer or pen samples to the open paint drag, in
    /// picture pixels, with the pen's pressure or 1 with a mouse.
    fn sample(&mut self, ui: &egui::Ui, camera: &Camera, rect: Rect, tablet: &Tablet) {
        let Some(Drag { live: Live::Paint { points, .. }, .. }) = self.drag.as_mut() else {
            return;
        };
        let mut samples: Vec<(Pos2, f64)> = Vec::new();
        if self.pen {
            for sample in tablet.input().samples() {
                samples.push((sample.pos, sample.pressure.unwrap_or(1.0).clamp(0.0, 1.0)));
            }
        } else {
            ui.input(|input| {
                for event in &input.events {
                    match event {
                        Event::PointerMoved(pos) => samples.push((*pos, 1.0)),
                        Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, .. } => {
                            samples.push((*pos, 1.0));
                        }
                        _ => {}
                    }
                }
            });
            if samples.is_empty()
                && points.is_empty()
                && let Some(pos) = ui.input(|input| input.pointer.latest_pos())
            {
                samples.push((pos, 1.0));
            }
        }
        for (pos, pressure) in samples {
            let at = picture_at(camera, rect, pos);
            let point = [at[0], at[1], pressure];
            if points.last().is_none_or(|last| last[0] != point[0] || last[1] != point[1]) {
                points.push(point);
            }
        }
    }

    /// The drag is released: its gesture is sent, and its preview held.
    fn finish(&mut self, display: &RasterDisplay, out: &mut Vec<Out>) {
        self.pen = false;
        let Some(drag) = self.drag.take() else {
            return;
        };
        let gesture = match &drag.live {
            Live::Paint { points, .. } if !points.is_empty() => Out::Painted(points.clone()),
            Live::Rect { from, to } => Out::Gesture(RasterGesture::Rect { from: *from, to: *to }),
            Live::Lasso { points } if points.len() >= 3 => {
                Out::Gesture(RasterGesture::Lasso { points: points.clone() })
            }
            Live::Move { from, to } if drag.moved => Out::Gesture(RasterGesture::Drag { from: *from, to: *to }),
            Live::Crop { rect } if drag.moved => {
                let rect = whole_pixels(*rect, [display.view.width, display.view.height]);
                if rect[2] < 1.0 || rect[3] < 1.0 {
                    return;
                }
                self.held.push(Held { live: Live::Crop { rect }, version: display.version, at: Instant::now() });
                out.push(Out::Gesture(RasterGesture::Crop { rect }));
                return;
            }
            _ => return,
        };
        self.held.push(Held { live: drag.live, version: display.version, at: Instant::now() });
        out.push(gesture);
    }

    #[allow(clippy::too_many_arguments)]
    fn cursor(
        &self,
        ui: &egui::Ui,
        display: &RasterDisplay,
        rect: Rect,
        tool: &RasterTool,
        space: bool,
        alt: bool,
        locked: bool,
    ) -> CursorIcon {
        if self.pan.is_some() {
            return CursorIcon::Grabbing;
        }
        if space || *tool == RasterTool::Hand {
            return CursorIcon::Grab;
        }
        if *tool == RasterTool::Zoom {
            return if alt { CursorIcon::ZoomOut } else { CursorIcon::ZoomIn };
        }
        if locked {
            return CursorIcon::Default;
        }
        match tool {
            RasterTool::Move => CursorIcon::Move,
            // The brush's own ring stands for the pointer, however small.
            RasterTool::Brush { .. } | RasterTool::Erase { .. } => CursorIcon::None,
            RasterTool::Crop => {
                let grab = match (&self.drag, display.camera, display.view.crop, ui.input(|input| input.pointer.hover_pos())) {
                    (Some(drag), _, _, _) => drag.crop.map(|(grab, _)| grab),
                    (None, Some(camera), Some(crop), Some(pos)) => Some(crop_grab(&camera, rect, crop, pos)),
                    _ => None,
                };
                match grab {
                    Some(Grab::Edges([left, top, right, bottom])) => match (left || right, top || bottom) {
                        (true, false) => CursorIcon::ResizeHorizontal,
                        (false, true) => CursorIcon::ResizeVertical,
                        _ if (left && top) || (right && bottom) => CursorIcon::ResizeNwSe,
                        _ => CursorIcon::ResizeNeSw,
                    },
                    Some(Grab::Inside) => CursorIcon::Move,
                    _ => CursorIcon::Crosshair,
                }
            }
            _ => CursorIcon::Crosshair,
        }
    }

    /// The picture, then what lies over it: paint previews, the selection,
    /// the crop box, the layer's box, and the brush's ring.
    fn paint(&self, ui: &egui::Ui, display: &RasterDisplay, rect: Rect) {
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect_filled(rect, 0.0, GROUND);
        let Some(camera) = display.camera else {
            return;
        };
        let view = &display.view;
        let picture = picture_rect(&camera, rect, view);
        // The picture, then a border around it, as the owner set on
        // 2026-10-30, so a white picture stands apart from the grey.
        match raw_picture(ui.ctx(), &view.picture, view.width, view.height, view.revision) {
            Some((texture, opaque)) => {
                if !opaque {
                    checker(ui.ctx(), &painter, picture);
                }
                painter.image(texture, picture, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
            }
            None => checker(ui.ctx(), &painter, picture),
        }
        widgets::outlined(&painter, picture.expand(1.0), 0.0, Color32::TRANSPARENT, widgets::RAIL);

        let lives = self.held.iter().map(|held| &held.live).chain(self.drag.iter().map(|drag| &drag.live));
        let mut crop = view.crop;
        let mut offset = [0.0, 0.0];
        let mut marching = false;
        for live in lives {
            match live {
                Live::Paint { points, look } => paint_trail(&painter, &camera, rect, points, look),
                Live::Rect { from, to } => {
                    let corners = [*from, [to[0], from[1]], *to, [from[0], to[1]]];
                    let points: Vec<Pos2> = corners.iter().map(|point| screen_at(&camera, rect, *point)).collect();
                    marching_edge(ui, &painter, &points, true);
                    marching = true;
                }
                Live::Lasso { points } => {
                    let points: Vec<Pos2> = points.iter().map(|point| screen_at(&camera, rect, *point)).collect();
                    marching_edge(ui, &painter, &points, false);
                    marching = true;
                }
                Live::Move { from, to } => offset = [to[0] - from[0], to[1] - from[1]],
                Live::Crop { rect: box_ } => crop = Some(*box_),
            }
        }
        for outline in &view.selection {
            let points: Vec<Pos2> = outline.iter().map(|point| screen_at(&camera, rect, *point)).collect();
            marching_edge(ui, &painter, &points, true);
            marching = true;
        }
        if marching {
            ui.ctx().request_repaint_after(MARCH);
        }
        if let Some(crop) = crop {
            paint_crop(&painter, &camera, rect, picture, crop);
        }
        if view.tool == RasterTool::Move
            && let Some(bounds) = view.bounds
        {
            let moved = [bounds[0] + offset[0], bounds[1] + offset[1], bounds[2], bounds[3]];
            paint_bounds(&painter, box_on_screen(&camera, rect, moved));
        }
        if let RasterTool::Brush { size, .. } | RasterTool::Erase { size, .. } = view.tool
            && self.pan.is_none()
            && let Some(pos) = ui.input(|input| input.pointer.hover_pos())
            && rect.contains(pos)
        {
            // A small brush still shows a ring the eye can find.
            let radius = ((size * camera.zoom / 2.0) as f32).max(3.0);
            widgets::ring(&painter, pos, radius, 1.0, Color32::from_black_alpha(160));
            widgets::ring(&painter, pos, radius + 1.0, 1.0, Color32::from_white_alpha(200));
        }
    }
}

/// A camera that can draw: a finite origin and a zoom above nothing.
fn usable(camera: &Camera) -> bool {
    camera.origin[0].is_finite() && camera.origin[1].is_finite() && camera.zoom.is_finite() && camera.zoom > 0.0
}

/// The picture's place on screen.
fn picture_rect(camera: &Camera, rect: Rect, view: &RasterView) -> Rect {
    Rect::from_min_max(
        screen_at(camera, rect, [0.0, 0.0]),
        screen_at(camera, rect, [f64::from(view.width), f64::from(view.height)]),
    )
}

fn picture_at(camera: &Camera, rect: Rect, pos: Pos2) -> [f64; 2] {
    [
        camera.origin[0] + f64::from(pos.x - rect.min.x) / camera.zoom,
        camera.origin[1] + f64::from(pos.y - rect.min.y) / camera.zoom,
    ]
}

fn screen_at(camera: &Camera, rect: Rect, point: [f64; 2]) -> Pos2 {
    pos2(
        ((point[0] - camera.origin[0]) * camera.zoom) as f32 + rect.min.x,
        ((point[1] - camera.origin[1]) * camera.zoom) as f32 + rect.min.y,
    )
}

fn box_on_screen(camera: &Camera, rect: Rect, box_: [f64; 4]) -> Rect {
    Rect::from_two_pos(
        screen_at(camera, rect, [box_[0], box_[1]]),
        screen_at(camera, rect, [box_[0] + box_[2], box_[1] + box_[3]]),
    )
}

// The camera, which the host keeps while the tab is shown.

/// Frames the whole picture in the viewport, with room around it.
pub(crate) fn fit(display: &mut RasterDisplay, viewport: [f64; 2]) {
    let size = [f64::from(display.view.width.max(1)), f64::from(display.view.height.max(1))];
    frame(display, [0.0, 0.0, size[0], size[1]], viewport);
}

/// Frames the selection, or the whole picture when nothing is selected.
pub(crate) fn zoom_to_selection(display: &mut RasterDisplay, viewport: [f64; 2]) {
    let points = display.view.selection.iter().flatten();
    let mut bounds: Option<[f64; 4]> = None;
    for point in points {
        let [x0, y0, x1, y1] = bounds.unwrap_or([point[0], point[1], point[0], point[1]]);
        bounds = Some([x0.min(point[0]), y0.min(point[1]), x1.max(point[0]), y1.max(point[1])]);
    }
    match bounds {
        Some([x0, y0, x1, y1]) if x1 > x0 && y1 > y0 => frame(display, [x0, y0, x1 - x0, y1 - y0], viewport),
        _ => fit(display, viewport),
    }
}

fn frame(display: &mut RasterDisplay, box_: [f64; 4], viewport: [f64; 2]) {
    if viewport[0] <= 0.0 || viewport[1] <= 0.0 {
        return;
    }
    let room = [(viewport[0] - 2.0 * FIT_MARGIN).max(1.0), (viewport[1] - 2.0 * FIT_MARGIN).max(1.0)];
    let zoom = (room[0] / box_[2].max(1.0)).min(room[1] / box_[3].max(1.0)).clamp(MIN_ZOOM, MAX_ZOOM);
    let center = [box_[0] + box_[2] / 2.0, box_[1] + box_[3] / 2.0];
    display.camera = Some(Camera {
        origin: [center[0] - viewport[0] / (2.0 * zoom), center[1] - viewport[1] / (2.0 * zoom)],
        zoom,
    });
}

/// The toolbar's zoom in or out, about the viewport's center.
pub(crate) fn zoom_step(display: &mut RasterDisplay, zoom_in: bool, viewport: [f64; 2]) {
    let factor = if zoom_in { BUTTON_STEP } else { 1.0 / BUTTON_STEP };
    zoom_toward(display, factor, vec2((viewport[0] / 2.0) as f32, (viewport[1] / 2.0) as f32));
}

/// A zoom chosen by number, such as 1 for 100%, about the viewport's center.
pub(crate) fn set_zoom(display: &mut RasterDisplay, zoom: f64, viewport: [f64; 2]) {
    let Some(camera) = display.camera else {
        return;
    };
    if !zoom.is_finite() || zoom <= 0.0 {
        return;
    }
    zoom_toward(display, zoom / camera.zoom, vec2((viewport[0] / 2.0) as f32, (viewport[1] / 2.0) as f32));
}

/// Zooms by `factor`, keeping the picture point under `pointer`, which is
/// measured from the viewport's corner, where it is.
fn zoom_toward(display: &mut RasterDisplay, factor: f64, pointer: egui::Vec2) {
    let Some(camera) = display.camera.as_mut() else {
        return;
    };
    if !factor.is_finite() || factor <= 0.0 {
        return;
    }
    let old = camera.zoom;
    let new = (old * factor).clamp(MIN_ZOOM, MAX_ZOOM);
    let at = [f64::from(pointer.x), f64::from(pointer.y)];
    let point = [camera.origin[0] + at[0] / old, camera.origin[1] + at[1] / old];
    camera.zoom = new;
    camera.origin = [point[0] - at[0] / new, point[1] - at[1] / new];
}

fn pan(camera: &mut Camera, delta: egui::Vec2) {
    camera.origin[0] -= f64::from(delta.x) / camera.zoom;
    camera.origin[1] -= f64::from(delta.y) / camera.zoom;
}

/// The wheel pans, sideways with Shift, and zooms at the pointer with the
/// command key, as on the drawing surface.
fn wheel(ui: &egui::Ui, display: &mut RasterDisplay, rect: Rect, inside: bool) -> bool {
    if !inside {
        return false;
    }
    let hover = ui.input(|input| input.pointer.hover_pos());
    let events = ui.input(|input| input.events.clone());
    let mut moved = false;
    for event in events {
        let Event::MouseWheel { unit, delta, modifiers, .. } = event else {
            continue;
        };
        let scale = match unit {
            MouseWheelUnit::Point => 1.0,
            MouseWheelUnit::Line => NOTCH_POINTS,
            MouseWheelUnit::Page => f64::from(rect.height()),
        };
        let x = f64::from(delta.x) * scale;
        let y = f64::from(delta.y) * scale;
        if modifiers.command {
            if y != 0.0 {
                let pointer = hover.unwrap_or(rect.center()) - rect.min;
                zoom_toward(display, (5.0_f64 / 4.0).powf(y / NOTCH_POINTS), pointer);
                moved = true;
            }
        } else if let Some(camera) = display.camera.as_mut() {
            let step = if modifiers.shift { vec2((x + y) as f32, 0.0) } else { vec2(x as f32, y as f32) };
            pan(camera, step);
            moved = true;
        }
    }
    moved
}

// The crop box.

/// Which part of the shown crop box a press at `pos` takes.
fn crop_grab(camera: &Camera, rect: Rect, crop: [f64; 4], pos: Pos2) -> Grab {
    let frame = box_on_screen(camera, rect, crop);
    let along_y = pos.y >= frame.top() - GRAB && pos.y <= frame.bottom() + GRAB;
    let along_x = pos.x >= frame.left() - GRAB && pos.x <= frame.right() + GRAB;
    let edges = [
        along_y && (pos.x - frame.left()).abs() <= GRAB,
        along_x && (pos.y - frame.top()).abs() <= GRAB,
        along_y && (pos.x - frame.right()).abs() <= GRAB,
        along_x && (pos.y - frame.bottom()).abs() <= GRAB,
    ];
    if edges.iter().any(|edge| *edge) {
        Grab::Edges(edges)
    } else if frame.contains(pos) {
        Grab::Inside
    } else {
        Grab::New
    }
}

/// The crop box a drag from `press` to `at` makes of `start`, kept on the
/// picture.
fn dragged_crop(grab: Grab, start: [f64; 4], press: [f64; 2], at: [f64; 2], size: [f64; 2]) -> [f64; 4] {
    let clamp = |point: [f64; 2]| [point[0].clamp(0.0, size[0]), point[1].clamp(0.0, size[1])];
    let corners = |a: [f64; 2], b: [f64; 2]| {
        [a[0].min(b[0]), a[1].min(b[1]), (a[0] - b[0]).abs(), (a[1] - b[1]).abs()]
    };
    match grab {
        Grab::New => corners(clamp(press), clamp(at)),
        Grab::Inside => {
            let x = (start[0] + at[0] - press[0]).clamp(0.0, (size[0] - start[2]).max(0.0));
            let y = (start[1] + at[1] - press[1]).clamp(0.0, (size[1] - start[3]).max(0.0));
            [x, y, start[2], start[3]]
        }
        Grab::Edges([left, top, right, bottom]) => {
            let at = clamp(at);
            let mut a = [start[0], start[1]];
            let mut b = [start[0] + start[2], start[1] + start[3]];
            if left {
                a[0] = at[0];
            }
            if top {
                a[1] = at[1];
            }
            if right {
                b[0] = at[0];
            }
            if bottom {
                b[1] = at[1];
            }
            corners(a, b)
        }
    }
}

/// A crop box on whole pixels, inside the picture.
fn whole_pixels(box_: [f64; 4], size: [u32; 2]) -> [f64; 4] {
    let x0 = box_[0].round().clamp(0.0, f64::from(size[0]));
    let y0 = box_[1].round().clamp(0.0, f64::from(size[1]));
    let x1 = (box_[0] + box_[2]).round().clamp(0.0, f64::from(size[0]));
    let y1 = (box_[1] + box_[3]).round().clamp(0.0, f64::from(size[1]));
    [x0, y0, x1 - x0, y1 - y0]
}

/// The crop box as crop.png shows it: the picture outside it dimmed, a
/// white edge with thirds, and heavier corners.
fn paint_crop(painter: &egui::Painter, camera: &Camera, rect: Rect, picture: Rect, crop: [f64; 4]) {
    let frame = box_on_screen(camera, rect, crop);
    for dim in [
        Rect::from_min_max(picture.min, pos2(picture.right(), frame.top())),
        Rect::from_min_max(pos2(picture.left(), frame.bottom()), picture.max),
        Rect::from_min_max(pos2(picture.left(), frame.top()), pos2(frame.left(), frame.bottom())),
        Rect::from_min_max(pos2(frame.right(), frame.top()), pos2(picture.right(), frame.bottom())),
    ] {
        if dim.is_positive() {
            painter.rect_filled(dim, 0.0, CROP_DIM);
        }
    }
    let line = (1.0, Color32::WHITE);
    for third in [1.0 / 3.0, 2.0 / 3.0] {
        let x = frame.left() + frame.width() * third;
        let y = frame.top() + frame.height() * third;
        painter.line_segment([pos2(x, frame.top()), pos2(x, frame.bottom())], (1.0, Color32::from_white_alpha(190)));
        painter.line_segment([pos2(frame.left(), y), pos2(frame.right(), y)], (1.0, Color32::from_white_alpha(190)));
    }
    painter.line_segment([frame.left_top(), frame.right_top()], line);
    painter.line_segment([frame.right_top(), frame.right_bottom()], line);
    painter.line_segment([frame.right_bottom(), frame.left_bottom()], line);
    painter.line_segment([frame.left_bottom(), frame.left_top()], line);
    let arm = 14.0_f32.min(frame.width() / 2.0).min(frame.height() / 2.0);
    let heavy = (3.0, Color32::WHITE);
    for (corner, dx, dy) in [
        (frame.left_top(), 1.0, 1.0),
        (frame.right_top(), -1.0, 1.0),
        (frame.right_bottom(), -1.0, -1.0),
        (frame.left_bottom(), 1.0, -1.0),
    ] {
        painter.line_segment([corner, corner + vec2(dx * arm, 0.0)], heavy);
        painter.line_segment([corner, corner + vec2(0.0, dy * arm)], heavy);
    }
}

/// The active layer's box, as the mockup shows the move tool's: a thin blue
/// edge with white handles at the corners and the middle of each side.
fn paint_bounds(painter: &egui::Painter, frame: Rect) {
    let line = (1.0, BOUNDS);
    painter.line_segment([frame.left_top(), frame.right_top()], line);
    painter.line_segment([frame.right_top(), frame.right_bottom()], line);
    painter.line_segment([frame.right_bottom(), frame.left_bottom()], line);
    painter.line_segment([frame.left_bottom(), frame.left_top()], line);
    for handle in [
        frame.left_top(),
        frame.center_top(),
        frame.right_top(),
        frame.right_center(),
        frame.right_bottom(),
        frame.center_bottom(),
        frame.left_bottom(),
        frame.left_center(),
    ] {
        widgets::outlined(painter, Rect::from_center_size(handle, vec2(8.0, 8.0)), 1.0, Color32::WHITE, BOUNDS);
    }
}

/// An outline drawn as a selection's marching edge: white under black
/// dashes that move along it.
fn marching_edge(ui: &egui::Ui, painter: &egui::Painter, points: &[Pos2], closed: bool) {
    if points.len() < 2 {
        return;
    }
    let mut path = points.to_vec();
    if closed {
        path.push(points[0]);
    }
    painter.line(path.clone(), (1.0, Color32::WHITE));
    let phase = (ui.input(|input| input.time) / MARCH.as_secs_f64()) as f32 % (2.0 * DASH);
    // How far along the dash pattern the walk is: dark for the first half.
    let mut along = 2.0 * DASH - phase;
    for pair in path.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let length = from.distance(to);
        if length <= 0.0 {
            continue;
        }
        let step = (to - from) / length;
        let mut walked = 0.0;
        while walked < length {
            let in_dash = along % (2.0 * DASH);
            let (dark, left) = if in_dash < DASH { (true, DASH - in_dash) } else { (false, 2.0 * DASH - in_dash) };
            let next = (walked + left).min(length);
            if dark {
                painter.line_segment([from + step * walked, from + step * next], (1.0, Color32::BLACK));
            }
            along += next - walked;
            walked = next;
        }
    }
}

/// A paint drag as it is drawn, in the tool's colour, size, and opacity,
/// or as a light trail for the eraser.
fn paint_trail(painter: &egui::Painter, camera: &Camera, rect: Rect, points: &[[f64; 3]], look: &Look) {
    if points.is_empty() {
        return;
    }
    let color = match look.rgb {
        Some([r, g, b]) => Color32::from_rgba_unmultiplied(r, g, b, (look.opacity.clamp(0.0, 1.0) * 255.0).round() as u8),
        None => ERASE_TRAIL,
    };
    let width = |pressure: f64| {
        let scale = if look.pressure { pressure } else { 1.0 };
        ((look.size * scale * camera.zoom) as f32).max(1.0)
    };
    let screen: Vec<Pos2> = points.iter().map(|point| screen_at(camera, rect, [point[0], point[1]])).collect();
    if color.a() == 255 {
        // An opaque trail is dabs joined by bars, so pressure shows.
        for (index, point) in points.iter().enumerate() {
            widgets::disc(painter, screen[index], width(point[2]) / 2.0, color);
            if index > 0 {
                let w = (width(point[2]) + width(points[index - 1][2])) / 2.0;
                painter.line_segment([screen[index - 1], screen[index]], (w, color));
            }
        }
    } else if screen.len() == 1 {
        widgets::disc(painter, screen[0], width(points[0][2]) / 2.0, color);
    } else {
        // A see-through trail is one path, so its overlaps do not darken
        // where the finished drag would not.
        let mean = points.iter().map(|point| width(point[2])).sum::<f32>() / points.len() as f32;
        painter.line(screen, (mean, color));
    }
}

// Pictures read from raw RGBA files.

#[derive(Clone)]
struct Loaded {
    revision: u64,
    size: [u32; 2],
    texture: egui::TextureHandle,
    opaque: bool,
    /// The last pass that drew it, so pictures no longer drawn are let go.
    used: u64,
}

/// Pictures not drawn for this many passes are let go, and read again if
/// they are shown again.
const FORGET_AFTER: u64 = 600;

/// A file of raw RGBA, `width × height × 4` bytes with straight alpha, as a
/// texture, read again when `revision` changes. A file of any other length
/// is not whole yet: the last texture is kept and the file is tried again on
/// a later frame. Returns the texture and whether every pixel is opaque.
pub(crate) fn raw_picture(
    ctx: &egui::Context,
    path: &str,
    width: u32,
    height: u32,
    revision: u64,
) -> Option<(egui::TextureId, bool)> {
    let id = egui::Id::new("raster-pictures");
    let pass = ctx.cumulative_pass_nr();
    let known: Option<Loaded> = ctx.data(|data| data.get_temp::<HashMap<String, Loaded>>(id).and_then(|cache| cache.get(path).cloned()));
    let current = known.as_ref().is_some_and(|known| known.revision == revision && known.size == [width, height]);
    let loaded = if current {
        known
    } else {
        match read_raw(ctx, path, width, height) {
            Some((image, opaque)) => Some(Loaded {
                revision,
                size: [width, height],
                texture: ctx.load_texture(format!("raster:{path}"), image, picture_options()),
                opaque,
                used: pass,
            }),
            None => {
                ctx.request_repaint_after(Duration::from_millis(100));
                known
            }
        }
    };
    let loaded = loaded?;
    let answer = (loaded.texture.id(), loaded.opaque);
    ctx.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<HashMap<String, Loaded>>(id);
        cache.insert(path.to_string(), Loaded { used: pass, ..loaded });
        cache.retain(|_, picture| pass.saturating_sub(picture.used) < FORGET_AFTER);
    });
    Some(answer)
}

/// Pixels stay square when zoomed in, and blend smoothly when zoomed out.
fn picture_options() -> egui::TextureOptions {
    egui::TextureOptions {
        magnification: egui::TextureFilter::Nearest,
        minification: egui::TextureFilter::Linear,
        wrap_mode: egui::TextureWrapMode::ClampToEdge,
        mipmap_mode: Some(egui::TextureFilter::Linear),
    }
}

/// Reads a whole raw RGBA file, shrunk by whole steps when it is larger than
/// the graphics card's largest texture.
fn read_raw(ctx: &egui::Context, path: &str, width: u32, height: u32) -> Option<(egui::ColorImage, bool)> {
    let (width, height) = (width as usize, height as usize);
    let expected = width.checked_mul(height)?.checked_mul(4)?;
    if expected == 0 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() != expected {
        return None;
    }
    let opaque = bytes.chunks_exact(4).all(|pixel| pixel[3] == 255);
    let largest = ctx.input(|input| input.max_texture_side).max(1);
    let step = width.max(height).div_ceil(largest);
    if step <= 1 {
        return Some((egui::ColorImage::from_rgba_unmultiplied([width, height], &bytes), opaque));
    }
    let (small_w, small_h) = (width.div_ceil(step), height.div_ceil(step));
    let mut small = Vec::with_capacity(small_w * small_h * 4);
    for y in 0..small_h {
        for x in 0..small_w {
            let at = ((y * step).min(height - 1) * width + (x * step).min(width - 1)) * 4;
            small.extend_from_slice(&bytes[at..at + 4]);
        }
    }
    Some((egui::ColorImage::from_rgba_unmultiplied([small_w, small_h], &small), opaque))
}

/// Squares under a picture's transparency, fixed to the picture's corner.
pub(crate) fn checker(ctx: &egui::Context, painter: &egui::Painter, area: Rect) {
    let id = egui::Id::new("raster-checker");
    let texture = match ctx.data(|data| data.get_temp::<egui::TextureHandle>(id)) {
        Some(texture) => texture,
        None => {
            let image = egui::ColorImage::new([2, 2], vec![CHECK_LIGHT, CHECK_DARK, CHECK_DARK, CHECK_LIGHT]);
            let options = egui::TextureOptions {
                magnification: egui::TextureFilter::Nearest,
                minification: egui::TextureFilter::Nearest,
                wrap_mode: egui::TextureWrapMode::Repeat,
                mipmap_mode: None,
            };
            let texture = ctx.load_texture("raster-checker", image, options);
            ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
            texture
        }
    };
    let uv = Rect::from_min_max(Pos2::ZERO, pos2(area.width() / (2.0 * CHECK), area.height() / (2.0 * CHECK)));
    painter.image(texture.id(), area, uv, Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A crop drag keeps the box on the picture, whichever way it goes, and
    /// an edge crossed over the other swaps them instead of turning negative.
    #[test]
    fn crop_drags_stay_on_the_picture() {
        let size = [100.0, 50.0];
        assert_eq!(dragged_crop(Grab::New, [0.0; 4], [80.0, 40.0], [-20.0, 10.0], size), [0.0, 10.0, 80.0, 30.0]);
        assert_eq!(dragged_crop(Grab::Inside, [10.0, 10.0, 20.0, 20.0], [15.0, 15.0], [200.0, 15.0], size), [80.0, 10.0, 20.0, 20.0]);
        let left = Grab::Edges([true, false, false, false]);
        assert_eq!(dragged_crop(left, [10.0, 10.0, 20.0, 20.0], [10.0, 20.0], [40.0, 20.0], size), [30.0, 10.0, 10.0, 20.0]);
        assert_eq!(whole_pixels([0.4, 0.6, 10.2, 200.0], [100, 50]), [0.0, 1.0, 11.0, 49.0]);
    }
}
