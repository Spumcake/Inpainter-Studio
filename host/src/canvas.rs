//! The drawing surface: pointer and pen handling, painting, the marquee,
//! transform previews, and the text editor, from Simple Sketch 1.0.1's
//! `canvas.rs`. Where that file read or called the session, this one reads
//! the tab's display copy or its declared tool, or sends a message, through
//! [`Doc`]. A finished gesture, selection, or edit is sent; the next
//! `document` shows what came of it.

use crate::doc::{Doc, Mode, TextTarget};
use crate::tablet::Tablet;
use crate::widgets;
use draw::document::{BoxSide, GRID_SPACING, InkPaint, NodeId, Point, Sample, SceneMark, TextFit};
use draw::text;
use eframe::egui::{
    self, Color32, CursorIcon, Event, Key, MouseWheelUnit, PointerButton, Pos2, Rect, Sense, Shape,
    Stroke, TouchPhase, vec2,
};
use kurbo::{Affine, Vec2};

const NOTCH_POINTS: f64 = 50.0;
/// A hair darker than the white toolbar and active tab, as in the picture.
pub(crate) const CANVAS: Color32 = Color32::from_rgb(251, 251, 252);

/// The drawing surface's state between frames.
pub(crate) struct Surface {
    stroking: bool,
    /// A rectangle, ellipse, or line drag is open.
    shaping: bool,
    pan: Option<PanButton>,
    stroke_pressure: Option<f64>,
    pub(crate) tablet: Tablet,
    /// The open stroke is drawn from tablet samples rather than the pointer.
    pen_stroke: bool,
    /// What to send when the stylus eraser end lifts.
    restore_tool: Option<protocol::Action>,
    selecting: Option<SelectGesture>,
    /// Screen position where a text placement was pressed.
    text_press: Option<Pos2>,
    /// The object a select-tool right-click opened its menu on.
    menu_target: Option<NodeId>,
    /// Where the caret was drawn last frame, so a click outside it commits.
    editor_rect: Option<Rect>,
    /// The edit generation the caret has already been focused for.
    text_focus: u64,
    text_cache: text::Cache,
    /// The canvas's screen rectangle last frame, so a press elsewhere, such
    /// as on the panel, leaves the text being edited open.
    canvas_rect: Rect,
    /// The new text's box as laid out last frame, for its outline and
    /// handles.
    draft_fit: Option<TextFit>,
    /// The assistant is working: the pointer only pans and zooms.
    pub(crate) locked: bool,
}

/// A select-tool press that has not yet crossed the drag threshold, then one
/// of the gestures that follow. `previous` is the selection at the press, so
/// Escape can put it back. Transforms preview from `before` and are sent once.
enum SelectGesture {
    /// An edge handle dragged on one text: a side sets its box's width and
    /// its words re-wrap, and the top or bottom its height, instead of the
    /// text stretching. The select tool's handles act on a text object; the
    /// text tool's also on new text.
    TextFrame {
        target: TextTarget,
        before: TextFit,
        start: Point,
        side: BoxSide,
        previous: Vec<NodeId>,
    },
    Press {
        screen: Pos2,
        anchor: Point,
        hit: Option<NodeId>,
        inside: bool,
        before: Vec<(NodeId, Affine)>,
        previous: Vec<NodeId>,
    },
    Move {
        before: Vec<(NodeId, Affine)>,
        anchor: Point,
        previous: Vec<NodeId>,
    },
    Scale {
        before: Vec<(NodeId, Affine)>,
        opposite: Point,
        handle: Point,
        /// Which of x and y the drag scales: both for a corner, one for an edge.
        axes: [bool; 2],
        previous: Vec<NodeId>,
    },
    Rotate {
        before: Vec<(NodeId, Affine)>,
        center: Point,
        start: f64,
        previous: Vec<NodeId>,
    },
    Marquee {
        anchor: Point,
        current: Point,
        shift: bool,
        previous: Vec<NodeId>,
    },
}

/// A selection handle: a corner, the middle of an edge (top, right, bottom,
/// left), or the rotation dot above the top edge.
enum HandleHit {
    Corner(usize),
    Edge(usize),
    Rotate,
}

/// Selection handles, as in the shape renders: white squares with a dark
/// border at the corners and edge midpoints, and a dot above the top edge on
/// a short stem for rotation.
const HANDLE_SIDE: f32 = 9.0;
const HANDLE_REACH: f32 = 8.0;
/// How far a tool's dashed outline sits outside what it outlines, so it is
/// not hidden under a shape's stroke and the text tool's side handles are
/// clear of the caret's field.
const OUTLINE_OUTSET: f32 = 6.0;
const ROTATE_STEM: f32 = 18.0;
const ROTATE_DOT: f32 = 4.0;

#[derive(Clone, Copy)]
enum PanButton {
    Primary,
    Middle,
}

impl Surface {
    pub(crate) fn new(tablet: Tablet) -> Self {
        Self {
            stroking: false,
            shaping: false,
            pan: None,
            stroke_pressure: None,
            tablet,
            pen_stroke: false,
            restore_tool: None,
            selecting: None,
            text_press: None,
            menu_target: None,
            editor_rect: None,
            text_focus: 0,
            text_cache: text::Cache::new(),
            canvas_rect: Rect::NOTHING,
            draft_fit: None,
            locked: false,
        }
    }

    /// Another tab is shown: whatever was being dragged on this one ends.
    pub(crate) fn reset(&mut self) {
        self.stroking = false;
        self.shaping = false;
        self.pan = None;
        self.stroke_pressure = None;
        self.pen_stroke = false;
        self.restore_tool = None;
        self.selecting = None;
        self.text_press = None;
        self.menu_target = None;
        self.editor_rect = None;
        self.draft_fit = None;
    }

    /// A drag on the selection is open: undo, redo, and nudge wait.
    pub(crate) fn transforming(&self) -> bool {
        self.selecting.is_some()
    }

    fn end_stroke(&mut self, doc: &mut Doc) {
        self.stroking = false;
        self.pen_stroke = false;
        self.stroke_pressure = None;
        if let Some(tool) = self.restore_tool.take()
            && doc.mode() == Mode::Eraser
        {
            doc.run(&tool, None);
        }
    }

    /// Escape: placing text stops, text being typed is kept, a shape drag
    /// is dropped, and otherwise a select drag is undone or the selection
    /// let go.
    pub(crate) fn escape(&mut self, doc: &mut Doc) {
        if doc.text_placing() {
            doc.cancel();
            self.text_press = None;
            return;
        }
        if doc.text_editing() {
            doc.commit_text();
            self.editor_rect = None;
            return;
        }
        if self.shaping {
            doc.cancel();
            self.shaping = false;
        } else {
            self.escape_select(doc);
        }
    }

    /// `hovered` is egui's answer to whether the canvas itself is under the
    /// pointer. The sketch menu and the keep-this-sketch dialog float over the
    /// canvas rectangle, and a press on them must not start a stroke beneath.
    /// `dismissing` means a menu was open when this frame began: the press
    /// that closes it does nothing else.
    fn handle_pointer(
        &mut self,
        ui: &mut egui::Ui,
        doc: &mut Doc,
        rect: Rect,
        hovered: bool,
        dismissing: bool,
    ) {
        let space = ui.input(|input| input.key_down(Key::Space));
        let primary_pressed =
            ui.input(|input| input.pointer.button_pressed(PointerButton::Primary));
        let primary_down = ui.input(|input| input.pointer.button_down(PointerButton::Primary));
        let primary_released =
            ui.input(|input| input.pointer.button_released(PointerButton::Primary));
        let middle_pressed = ui.input(|input| input.pointer.button_pressed(PointerButton::Middle));
        let middle_down = ui.input(|input| input.pointer.button_down(PointerButton::Middle));
        let middle_released =
            ui.input(|input| input.pointer.button_released(PointerButton::Middle));
        let pos = ui.input(|input| input.pointer.latest_pos());
        let delta = ui.input(|input| input.pointer.delta());
        let over_canvas = hovered && pos.is_some_and(|pos| rect.contains(pos));
        // egui closes a menu only on a click outside it, and a drag is not a
        // click, so any press on the canvas closes it here and is swallowed.
        if dismissing && over_canvas && (primary_pressed || middle_pressed) {
            egui::Popup::close_all(ui.ctx());
        }
        let press_inside = over_canvas && !dismissing;

        let text_frame = matches!(self.selecting, Some(SelectGesture::TextFrame { .. }))
            && doc.mode() == Mode::Text;
        if self.selecting.is_some() && doc.mode() != Mode::Select && !text_frame {
            self.revert_select(doc);
        }
        // The text tool's side handles, before a press that would place or
        // edit text. They work while the text is being typed.
        if primary_pressed
            && press_inside
            && !space
            && !self.locked
            && self.pan.is_none()
            && self.selecting.is_none()
            && !doc.text_placing()
            && let Some(pos) = pos
        {
            self.selecting = self.begin_text_frame(doc, rect, pos);
        }

        if middle_pressed
            && press_inside
            && self.pan.is_none()
            && !self.stroking
            && !self.shaping
            && !doc.text_placing()
        {
            self.pan = Some(PanButton::Middle);
        }
        if primary_pressed
            && press_inside
            && self.pan.is_none()
            && !self.stroking
            && !self.shaping
            && self.selecting.is_none()
            && !doc.text_placing()
            && !doc.text_editing()
        {
            if space && !doc.text_editing() {
                self.pan = Some(PanButton::Primary);
            } else if self.locked {
                // While the assistant works, a press draws and selects nothing.
            } else if doc.mode() == Mode::Select {
                self.begin_select(ui, doc, rect);
            } else {
                if self.tablet.input().eraser_pressed()
                    && doc.mode() != Mode::Eraser
                    && let Some(end) = doc.canvas.and_then(|canvas| canvas.eraser_end.clone())
                {
                    // The eraser end draws with the declared eraser once the
                    // sub-application has switched to it.
                    self.restore_tool = Some(end.leave);
                    doc.run(&end.enter, None);
                }
                if matches!(doc.mode(), Mode::Brush | Mode::Eraser) {
                    self.stroke_pressure = None;
                    self.pen_stroke = self.tablet.input().down();
                    self.capture_force(ui);
                    if doc.begin().is_ok() {
                        self.stroking = true;
                        self.sample_pointer(ui, doc, rect);
                    } else {
                        self.end_stroke(doc);
                    }
                } else if doc.mode() == Mode::Shape
                    && let Some(pos) = ui.input(|input| input.pointer.latest_pos())
                    && doc.begin_shape(doc_at(doc, rect, pos)).is_ok()
                {
                    self.shaping = true;
                } else if doc.mode() == Mode::Text
                    && let Some(pos) = ui.input(|input| input.pointer.latest_pos())
                    && doc.begin_text(doc_at(doc, rect, pos)).is_ok()
                {
                    self.text_press = Some(pos);
                }
            }
        }

        if let Some(button) = self.pan {
            if delta.x != 0.0 || delta.y != 0.0 {
                doc.pan_screen([delta.x as f64, delta.y as f64]);
            }
            let released = match button {
                PanButton::Primary => primary_released || !primary_down,
                PanButton::Middle => middle_released || !middle_down,
            };
            if released {
                self.pan = None;
            }
        } else if self.selecting.is_some() {
            self.update_select(ui, doc, rect, primary_released || !primary_down);
        } else if self.stroking {
            self.capture_force(ui);
            self.sample_pointer(ui, doc, rect);
            if primary_released || !primary_down {
                doc.commit();
                self.end_stroke(doc);
            }
        } else if self.shaping {
            let shift = ui.input(|input| input.modifiers.shift);
            if let Some(pos) = ui.input(|input| input.pointer.latest_pos()) {
                let point = doc_at(doc, rect, pos);
                doc.move_shape(point, shift);
            }
            if primary_released || !primary_down {
                doc.commit();
                self.shaping = false;
            }
        } else if doc.text_placing() {
            if let Some(pos) = ui.input(|input| input.pointer.latest_pos()) {
                let point = doc_at(doc, rect, pos);
                doc.move_text(point);
            }
            if primary_released || !primary_down {
                let dragged = self.text_press.is_some_and(|start| {
                    ui.input(|input| input.pointer.latest_pos())
                        .is_some_and(|pos| start.distance(pos) >= 3.0)
                });
                doc.end_text(dragged);
                self.text_press = None;
            }
        }

        self.handle_scroll(ui, doc, rect, hovered);
    }

    /// Force from platforms that report it as a touch. Tablet strokes carry
    /// their own pressure per sample.
    fn capture_force(&mut self, ui: &egui::Ui) {
        if self.pen_stroke {
            return;
        }
        let events = ui.input(|input| input.events.clone());
        for event in events {
            if let Event::Touch { force, phase, .. } = event {
                if matches!(phase, TouchPhase::Cancel) {
                    continue;
                }
                if let Some(force) = force {
                    let force = f64::from(force);
                    if (0.0..=1.0).contains(&force) {
                        self.stroke_pressure = Some(force);
                    }
                }
            }
        }
    }

    fn sample_pointer(&mut self, ui: &egui::Ui, doc: &mut Doc, rect: Rect) {
        if self.pen_stroke {
            let origin = doc.camera_origin();
            let zoom = doc.zoom();
            for sample in self.tablet.input().samples() {
                let point = screen_to_document(rect, origin, zoom, sample.pos);
                doc.add_sample(Sample {
                    x: point[0],
                    y: point[1],
                    pressure: sample.pressure,
                });
            }
            return;
        }
        let events = ui.input(|input| input.events.clone());
        let mut positions = Vec::new();
        for event in &events {
            match event {
                Event::PointerMoved(pos) => positions.push(*pos),
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: true,
                    ..
                } => positions.push(*pos),
                _ => {}
            }
        }
        if positions.is_empty() {
            if let Some(pos) = ui.input(|input| input.pointer.latest_pos()) {
                positions.push(pos);
            }
        }
        let origin = doc.camera_origin();
        let zoom = doc.zoom();
        for pos in positions {
            let point = screen_to_document(rect, origin, zoom, pos);
            doc.add_sample(Sample {
                x: point[0],
                y: point[1],
                pressure: self.stroke_pressure,
            });
        }
    }

    fn handle_scroll(&mut self, ui: &egui::Ui, doc: &mut Doc, rect: Rect, hovered: bool) {
        let hover = ui.input(|input| input.pointer.hover_pos());
        let inside = (hovered && hover.is_some_and(|pos| rect.contains(pos)))
            || self.stroking
            || self.shaping
            || doc.text_placing()
            || self.pan.is_some();
        if !inside {
            return;
        }
        let events = ui.input(|input| input.events.clone());
        for event in events {
            let Event::MouseWheel {
                unit,
                delta,
                modifiers,
                ..
            } = event
            else {
                continue;
            };
            let scale = wheel_scale(unit, rect);
            let points_x = f64::from(delta.x) * scale;
            let points_y = f64::from(delta.y) * scale;
            if modifiers.command {
                if points_y != 0.0 {
                    let factor = (5.0_f64 / 4.0).powf(points_y / NOTCH_POINTS);
                    let pointer = hover.unwrap_or(rect.center());
                    doc.zoom_toward(
                        factor,
                        [
                            f64::from(pointer.x - rect.min.x),
                            f64::from(pointer.y - rect.min.y),
                        ],
                    );
                }
            } else if modifiers.shift {
                doc.pan_screen([points_x + points_y, 0.0]);
            } else {
                doc.pan_screen([points_x, points_y]);
            }
        }
    }

    /// With the select tool, a right-click on an object opens its menu, and
    /// a double-click on text edits it in place.
    fn select_clicks(&mut self, doc: &mut Doc, response: &egui::Response, rect: Rect) {
        if self.locked || doc.mode() != Mode::Select || doc.text_editing() {
            return;
        }
        if response.secondary_clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let point = doc_at(doc, rect, pos);
            self.menu_target = doc.pick_for_menu(point);
        }
        if let Some(node) = self.menu_target
            && let Some(canvas) = doc.canvas
        {
            let has_page = doc.object_page_available(node);
            let rows: Vec<protocol::MenuEntry> = canvas
                .object_menu
                .iter()
                .cloned()
                .map(|mut row| {
                    if canvas.properties_row.as_deref() == Some(row.label.as_str()) {
                        row.enabled = has_page;
                    }
                    row
                })
                .collect();
            if let Some(action) =
                widgets::action_menu(response, &rows).and_then(|row| row.action.clone())
            {
                doc.run(&action, Some(node.get()));
            }
        }
        if response.double_clicked()
            && let Some(pos) = response.interact_pointer_pos()
            && let Some(id) = doc.hit_test(doc_at(doc, rect, pos))
        {
            doc.edit_text(id);
        }
    }

    /// A press on the canvas outside the caret ends the edit. A press on the
    /// panel, toolbar, or a menu does not, so a color's switch changes the
    /// text being typed; nor does one on the text tool's side handles.
    pub(crate) fn commit_text_on_outside_press(&mut self, ui: &egui::Ui, doc: &mut Doc) {
        let pressed = ui.input(|input| input.pointer.button_pressed(PointerButton::Primary));
        let pos = ui.input(|input| input.pointer.interact_pos());
        if pressed
            && let (Some(editor), Some(pos)) = (self.editor_rect, pos)
            && !editor.contains(pos)
            && self.canvas_rect.contains(pos)
            && !egui::Popup::is_any_open(ui.ctx())
            && self.text_side_hit(doc, self.canvas_rect, pos).is_none()
        {
            doc.commit_text();
            self.editor_rect = None;
        }
    }

    fn show_text_editor(&mut self, ui: &mut egui::Ui, doc: &mut Doc, rect: Rect) {
        let Some(caret) = doc.text_caret() else {
            return;
        };
        let zoom = doc.zoom();
        let camera = doc.camera_origin();
        // The same scale the canvas draws this text at, so editing it does
        // not change its size on screen.
        let glyph = text::glyph_scale(caret.world);
        let across = text::horizontal_scale(caret.world);
        let font_px = (f64::from(caret.style.size.max(1)) * glyph * zoom) as f32;
        // The words sit inside the box's padding, on the canvas and here.
        // The padding is at the letters' scale on every side, as the canvas
        // draws it, so in the box's own units it is narrower along a
        // direction the box was stretched in.
        let pad = caret.style.padding.max(0.0) * glyph;
        let [_, _, c, d, _, _] = caret.world.as_coeffs();
        let down = (c * c + d * d).sqrt().max(1e-6);
        let mut inner_origin = [caret.origin[0] + pad / across, caret.origin[1] + pad / down];
        let inner_width = caret
            .box_width
            .map(|width| (width - 2.0 * pad / across).max(1.0));
        let preview = text::galley(
            ui.ctx(),
            &caret.content,
            &caret.style,
            &caret.look,
            font_px.max(1.0),
            inner_width.map(|width| (width * across * zoom) as f32),
            false,
            &mut self.text_cache,
        );
        // In a box taller than its lines, they sit by the vertical alignment.
        let natural = f64::from(preview.size().y) / zoom + 2.0 * pad;
        if let Some(height) = caret.box_height {
            let spare = (height * down - natural).max(0.0);
            inner_origin[1] += caret.style.valign.share() * spare / down;
        }
        let editor = text::editor_box(
            rect,
            camera,
            zoom,
            inner_origin,
            inner_width,
            caret.world,
            preview.size(),
        );
        let original = caret.content.clone();
        let mut buffer = original.clone();
        let generation = doc.text_generation();
        // A click on a toolbar or panel button takes the keys from the caret
        // without ending the edit, so the caret takes them back. A field
        // that wants keys, such as Hex, keeps them while it has them.
        let focus =
            generation != self.text_focus || ui.ctx().memory(|memory| memory.focused().is_none());
        let style = caret.style.clone();
        let look = caret.look;
        let box_width = inner_width;
        let width = if box_width.is_some() && style.wrap {
            editor.width
        } else {
            (preview.size().x + font_px).max(editor.width)
        };
        drop(caret);
        let drawn = {
            let cache = &mut self.text_cache;
            egui::Area::new(ui.id().with("text-caret"))
                .order(egui::Order::Foreground)
                .fixed_pos(editor.pos)
                .show(ui.ctx(), |ui| {
                    // An area offers its content last frame's size, and the
                    // field would not grow past it when the box widens.
                    ui.set_max_width(width.max(1.0));
                    let mut layouter =
                        |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                            let wrap = if style.wrap && box_width.is_some() {
                                Some(wrap_width)
                            } else {
                                None
                            };
                            text::galley(
                                ui.ctx(),
                                buf.as_str(),
                                &style,
                                &look,
                                font_px.max(1.0),
                                wrap,
                                false,
                                cache,
                            )
                        };
                    let output = egui::TextEdit::multiline(&mut buffer)
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin::ZERO)
                        .desired_width(width.max(1.0))
                        .desired_rows(1)
                        .layouter(&mut layouter)
                        .show(ui);
                    if focus {
                        output.response.request_focus();
                    }
                    output.response.rect
                })
                .inner
        };
        if focus {
            self.text_focus = generation;
        }
        self.editor_rect = Some(drawn);
        if buffer != original {
            doc.set_text_draft(buffer);
        }
    }

    fn paint_text_drag(&self, painter: &egui::Painter, ui: &egui::Ui, doc: &Doc, rect: Rect) {
        let Some((origin, _, line)) = doc.text_drag() else {
            return;
        };
        let Some(start) = self.text_press else {
            return;
        };
        let Some(pos) = ui.input(|input| input.pointer.latest_pos()) else {
            return;
        };
        if start.distance(pos) < 3.0 {
            return;
        }
        let point = doc_at(doc, rect, pos);
        let width = (point[0] - origin[0]).abs();
        if width < 1.0 {
            return;
        }
        let zoom = doc.zoom();
        let camera = doc.camera_origin();
        let min = document_to_screen(rect, camera, zoom, origin);
        let max = document_to_screen(rect, camera, zoom, [origin[0] + width, origin[1] + line]);
        painter.rect_stroke(
            Rect::from_min_max(min, max),
            0.0,
            Stroke::new(1.0, Color32::from_rgb(186, 188, 192)),
            egui::StrokeKind::Outside,
        );
    }

    fn paint(&mut self, ui: &egui::Ui, doc: &mut Doc, rect: Rect) {
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect_filled(rect, 0.0, CANVAS);
        let origin = doc.camera_origin();
        let zoom = doc.zoom();
        paint_grid(&painter, rect, origin, zoom);
        let caret = doc.text_caret();
        let editing = caret.as_ref().and_then(|caret| caret.id);
        // Text committed and not yet answered keeps its typed words, so it
        // does not blink back to what the document held.
        let committed = doc.committed_caret();
        let marks = doc.scene();
        for mark in marks {
            match mark {
                SceneMark::Ink(paint) => paint_mark(&painter, rect, origin, zoom, &paint),
                SceneMark::Text(mut view) => {
                    // Text being edited is measured with its typed words, so
                    // its background and outline follow the typing.
                    if editing == Some(view.id)
                        && let Some(caret) = caret.as_ref()
                    {
                        view.content.clone_from(&caret.content);
                    } else if let Some(committed) = committed.as_ref()
                        && committed.id == Some(view.id)
                    {
                        view.content.clone_from(&committed.content);
                    }
                    let laid = text::lay(
                        ui.ctx(),
                        &view.content,
                        &view.style,
                        &view.look,
                        view.origin,
                        view.box_width,
                        view.box_height,
                        view.world,
                        zoom,
                        true,
                        &mut self.text_cache,
                    );
                    doc.set_text_metrics(
                        view.id,
                        laid.local_width,
                        laid.local_height,
                        laid.natural_height,
                    );
                    if editing == Some(view.id) {
                        text::paint_box(
                            &painter,
                            rect,
                            origin,
                            zoom,
                            &text::Boxed::of(&view),
                            laid.local_width,
                            laid.local_height,
                        );
                    } else {
                        text::paint(&painter, rect, origin, zoom, &view, &laid);
                    }
                }
            }
        }
        // New text that was committed is drawn as it will be inserted until
        // the document with it arrives.
        if let Some(committed) =
            committed.filter(|caret| caret.id.is_none() && !caret.content.is_empty())
        {
            let view = draw::document::TextView {
                id: NodeId::new(u64::MAX),
                content: committed.content,
                style: committed.style,
                look: committed.look,
                origin: committed.origin,
                box_width: committed.box_width,
                box_height: committed.box_height,
                world: committed.world,
            };
            let laid = text::lay(
                ui.ctx(),
                &view.content,
                &view.style,
                &view.look,
                view.origin,
                view.box_width,
                view.box_height,
                view.world,
                zoom,
                true,
                &mut self.text_cache,
            );
            text::paint(&painter, rect, origin, zoom, &view, &laid);
        }
        self.draft_fit = None;
        // New text is not in the document yet. Its background is drawn here,
        // under the caret, from the text tool's palette.
        if let Some(caret) = caret.filter(|caret| caret.id.is_none()) {
            let laid = text::lay(
                ui.ctx(),
                &caret.content,
                &caret.style,
                &caret.look,
                caret.origin,
                caret.box_width,
                caret.box_height,
                caret.world,
                zoom,
                true,
                &mut self.text_cache,
            );
            text::paint_box(
                &painter,
                rect,
                origin,
                zoom,
                &text::Boxed {
                    look: &caret.look,
                    radius: caret.style.radius,
                    origin: caret.origin,
                    world: caret.world,
                },
                laid.local_width,
                laid.local_height,
            );
            if let Some(frame) = doc.draft_box() {
                self.draft_fit = Some(TextFit {
                    frame,
                    width: laid.local_width,
                    height: laid.local_height,
                    natural: laid.natural_height,
                });
            }
        }
        self.paint_text_drag(&painter, ui, doc, rect);
        self.paint_selection(&painter, doc, rect);
    }

    fn begin_select(&mut self, ui: &egui::Ui, doc: &mut Doc, rect: Rect) {
        let Some(pos) = ui.input(|input| input.pointer.latest_pos()) else {
            return;
        };
        let point = doc_at(doc, rect, pos);
        let previous = doc.selection();
        let shift = ui.input(|input| input.modifiers.shift);
        if !shift && let Some(handle) = self.hit_handle(doc, rect, pos) {
            let Some((min, max)) = doc.selection_bounds() else {
                return;
            };
            let before = doc.changeable_locals(None);
            if before.is_empty() {
                return;
            }
            let corners = box_corners(min, max);
            self.selecting = Some(match handle {
                HandleHit::Rotate => {
                    let center = box_center(min, max);
                    SelectGesture::Rotate {
                        before,
                        center,
                        start: angle_of(point, center),
                        previous,
                    }
                }
                HandleHit::Corner(index) => SelectGesture::Scale {
                    before,
                    opposite: corners[(index + 2) % 4],
                    handle: corners[index],
                    axes: [true, true],
                    previous,
                },
                // The edges of one text object set its box's width and height.
                HandleHit::Edge(index)
                    if let Some(id) = doc.sole_text()
                        && let Some(before) = doc.text_box(id) =>
                {
                    SelectGesture::TextFrame {
                        target: TextTarget::Node(id),
                        before,
                        start: point,
                        side: EDGE_SIDES[index],
                        previous,
                    }
                }
                HandleHit::Edge(index) => {
                    let edges = box_edges(min, max);
                    SelectGesture::Scale {
                        before,
                        opposite: edges[(index + 2) % 4],
                        handle: edges[index],
                        // The top and bottom edges scale y; the sides scale x.
                        axes: [index % 2 == 1, index % 2 == 0],
                        previous,
                    }
                }
            });
            return;
        }
        let hit = doc.hit_test(point);
        let inside = self.inside_selection(doc, point);
        // The press selects what it hit; the drag moves what will then be
        // selected, which the next document shows.
        let mut selected = None;
        if !shift
            && let Some(id) = hit
            && !previous.contains(&id)
        {
            doc.select_click(id, false);
            selected = Some(vec![id]);
        }
        self.selecting = Some(SelectGesture::Press {
            screen: pos,
            anchor: point,
            hit,
            inside,
            before: doc.changeable_locals(selected.as_deref()),
            previous,
        });
    }

    fn update_select(&mut self, ui: &egui::Ui, doc: &mut Doc, rect: Rect, released: bool) {
        let Some(pos) = ui.input(|input| input.pointer.latest_pos()) else {
            if released {
                self.revert_select(doc);
            }
            return;
        };
        let point = doc_at(doc, rect, pos);
        let shift = ui.input(|input| input.modifiers.shift);
        let Some(gesture) = self.selecting.take() else {
            return;
        };
        self.selecting = self.advance_select(doc, gesture, pos, point, shift, released);
    }

    fn advance_select(
        &mut self,
        doc: &mut Doc,
        gesture: SelectGesture,
        pos: Pos2,
        point: Point,
        shift: bool,
        released: bool,
    ) -> Option<SelectGesture> {
        match gesture {
            SelectGesture::Press {
                screen,
                anchor,
                hit,
                inside,
                before,
                previous,
            } => {
                if screen.distance(pos) < 4.0 {
                    if released {
                        self.finish_click(doc, hit, shift);
                    }
                    if released {
                        None
                    } else {
                        Some(SelectGesture::Press {
                            screen,
                            anchor,
                            hit,
                            inside,
                            before,
                            previous,
                        })
                    }
                } else if shift || (hit.is_none() && !inside) {
                    let gesture = SelectGesture::Marquee {
                        anchor,
                        current: point,
                        shift,
                        previous: previous.clone(),
                    };
                    self.apply_marquee(doc, &gesture);
                    if released { None } else { Some(gesture) }
                } else if before.is_empty() {
                    if released {
                        self.finish_click(doc, hit, shift);
                        None
                    } else {
                        Some(SelectGesture::Press {
                            screen,
                            anchor,
                            hit,
                            inside,
                            before,
                            previous,
                        })
                    }
                } else {
                    self.preview_move(doc, &before, anchor, point);
                    if released {
                        doc.commit_preview(&before);
                        None
                    } else {
                        Some(SelectGesture::Move {
                            before,
                            anchor,
                            previous,
                        })
                    }
                }
            }
            SelectGesture::Move {
                before,
                anchor,
                previous,
            } => {
                if released {
                    doc.commit_preview(&before);
                    None
                } else {
                    self.preview_move(doc, &before, anchor, point);
                    Some(SelectGesture::Move {
                        before,
                        anchor,
                        previous,
                    })
                }
            }
            SelectGesture::TextFrame {
                target,
                before,
                start,
                side,
                previous,
            } => {
                match target {
                    TextTarget::Node(id) => {
                        doc.preview_text_box(id, before, start, point, side);
                    }
                    TextTarget::Draft => doc.preview_draft_box(before, start, point, side),
                }
                if released {
                    match target {
                        TextTarget::Node(id) => doc.commit_text_box(id, before.frame),
                        TextTarget::Draft => doc.finish_draft_box(),
                    }
                    self.refocus_text(doc);
                    None
                } else {
                    Some(SelectGesture::TextFrame {
                        target,
                        before,
                        start,
                        side,
                        previous,
                    })
                }
            }
            SelectGesture::Scale {
                before,
                opposite,
                handle,
                axes,
                previous,
            } => {
                if released {
                    doc.commit_preview(&before);
                    None
                } else {
                    self.preview_scale(doc, &before, opposite, handle, axes, point, shift);
                    Some(SelectGesture::Scale {
                        before,
                        opposite,
                        handle,
                        axes,
                        previous,
                    })
                }
            }
            SelectGesture::Rotate {
                before,
                center,
                start,
                previous,
            } => {
                if released {
                    doc.commit_preview(&before);
                    None
                } else {
                    self.preview_rotate(doc, &before, center, start, point, shift);
                    Some(SelectGesture::Rotate {
                        before,
                        center,
                        start,
                        previous,
                    })
                }
            }
            SelectGesture::Marquee {
                anchor,
                shift,
                previous,
                ..
            } => {
                let gesture = SelectGesture::Marquee {
                    anchor,
                    current: point,
                    shift,
                    previous,
                };
                self.apply_marquee(doc, &gesture);
                if released { None } else { Some(gesture) }
            }
        }
    }

    fn finish_click(&mut self, doc: &mut Doc, hit: Option<NodeId>, shift: bool) {
        if shift {
            if let Some(id) = hit {
                doc.select_click(id, true);
            }
        } else if let Some(id) = hit {
            doc.select_click(id, false);
        } else {
            doc.clear_selection();
        }
    }

    fn preview_move(
        &mut self,
        doc: &mut Doc,
        before: &[(NodeId, Affine)],
        anchor: Point,
        point: Point,
    ) {
        let delta = Vec2::new(point[0] - anchor[0], point[1] - anchor[1]);
        doc.preview_world(before, |world| Affine::translate(delta) * world);
    }

    #[allow(clippy::too_many_arguments)]
    fn preview_scale(
        &mut self,
        doc: &mut Doc,
        before: &[(NodeId, Affine)],
        opposite: Point,
        handle: Point,
        axes: [bool; 2],
        point: Point,
        shift: bool,
    ) {
        let along = |axis: usize| {
            if axes[axis] {
                axis_scale(handle[axis], opposite[axis], point[axis])
            } else {
                Some(1.0)
            }
        };
        let (Some(sx), Some(sy)) = (along(0), along(1)) else {
            doc.restore_locals(before);
            return;
        };
        // An edge handle with Shift keeps proportions by giving the other
        // axis the same scale.
        let (sx, sy) = if shift && axes != [true, true] {
            let scale = if axes[0] { sx } else { sy };
            (scale, scale)
        } else if shift {
            let scale = if sx.abs() > sy.abs() {
                sx.abs()
            } else {
                sy.abs()
            };
            (scale.copysign(sx), scale.copysign(sy))
        } else {
            (sx, sy)
        };
        if !sx.is_finite() || !sy.is_finite() || sx.abs() < 1.0e-4 || sy.abs() < 1.0e-4 {
            doc.restore_locals(before);
            return;
        }
        let origin = kurbo::Point::new(opposite[0], opposite[1]);
        let map = scale_about(sx, sy, origin);
        doc.preview_world(before, |world| map * world);
    }

    fn preview_rotate(
        &mut self,
        doc: &mut Doc,
        before: &[(NodeId, Affine)],
        center: Point,
        start: f64,
        point: Point,
        shift: bool,
    ) {
        let mut delta = angle_of(point, center) - start;
        if shift {
            let step = std::f64::consts::PI / 12.0;
            delta = (delta / step).round() * step;
        }
        if !delta.is_finite() {
            doc.restore_locals(before);
            return;
        }
        let origin = kurbo::Point::new(center[0], center[1]);
        doc.preview_world(before, |world| Affine::rotate_about(delta, origin) * world);
    }

    fn apply_marquee(&mut self, doc: &mut Doc, gesture: &SelectGesture) {
        let SelectGesture::Marquee {
            anchor,
            current,
            shift,
            previous,
        } = gesture
        else {
            return;
        };
        let min = [anchor[0].min(current[0]), anchor[1].min(current[1])];
        let max = [anchor[0].max(current[0]), anchor[1].max(current[1])];
        let base: &[NodeId] = if *shift { previous } else { &[] };
        doc.marquee_select(min, max, *shift, base);
    }

    /// Escape cancels a drag, or lets go of the selection. With the text
    /// tool that hands the toolbar back to the next text's settings.
    fn escape_select(&mut self, doc: &mut Doc) {
        if self.selecting.is_some() {
            self.revert_select(doc);
        } else if doc.canvas.is_some_and(|canvas| canvas.escape_clears) {
            doc.clear_selection();
        }
    }

    fn revert_select(&mut self, doc: &mut Doc) {
        let Some(gesture) = self.selecting.take() else {
            return;
        };
        match gesture {
            SelectGesture::Press { previous, .. } => {
                doc.replace_selection(previous);
            }
            SelectGesture::Move {
                before, previous, ..
            }
            | SelectGesture::Scale {
                before, previous, ..
            }
            | SelectGesture::Rotate {
                before, previous, ..
            } => {
                doc.restore_locals(&before);
                doc.replace_selection(previous);
            }
            SelectGesture::Marquee { previous, .. } => {
                doc.replace_selection(previous);
            }
            SelectGesture::TextFrame {
                target,
                before,
                previous,
                ..
            } => {
                match target {
                    TextTarget::Node(id) => doc.restore_text_box(id, before.frame),
                    TextTarget::Draft => doc.set_draft_box(before.frame),
                }
                doc.replace_selection(previous);
                self.refocus_text(doc);
            }
        }
    }

    fn inside_selection(&self, doc: &Doc, point: Point) -> bool {
        let Some((min, max)) = doc.selection_bounds() else {
            return false;
        };
        point[0] >= min[0] && point[0] <= max[0] && point[1] >= min[1] && point[1] <= max[1]
    }

    fn hit_handle(&self, doc: &Doc, rect: Rect, pos: Pos2) -> Option<HandleHit> {
        let frame = selection_frame(doc, rect)?;
        if doc.selection_turns() && pos.distance(rotate_dot(frame)) <= HANDLE_REACH {
            return Some(HandleHit::Rotate);
        }
        let near = |points: [Pos2; 4]| {
            points
                .iter()
                .position(|point| pos.distance(*point) <= HANDLE_REACH)
        };
        near(frame_corners(frame))
            .map(HandleHit::Corner)
            .or_else(|| near(frame_edges(frame)).map(HandleHit::Edge))
    }

    /// The select tool's pointer: a resize arrow along the handle's axis over a
    /// corner or edge handle and while dragging one, a grab hand on the
    /// rotation dot and a closed hand while rotating, and the move arrows
    /// inside the selection and while moving it. egui has no rotation cursor,
    /// so the dot uses the hand.
    fn select_cursor(&self, ui: &egui::Ui, doc: &Doc, rect: Rect) -> CursorIcon {
        match &self.selecting {
            Some(SelectGesture::Scale {
                opposite,
                handle,
                axes,
                ..
            }) => return scale_cursor(*axes, *handle, *opposite),
            Some(SelectGesture::Rotate { .. }) => return CursorIcon::Grabbing,
            Some(SelectGesture::TextFrame { side, .. }) => return side_cursor(*side),
            Some(SelectGesture::Move { .. }) => return CursorIcon::Move,
            Some(SelectGesture::Marquee { .. }) => return CursorIcon::Default,
            Some(SelectGesture::Press { .. }) | None => {}
        }
        let Some(pos) = ui.input(|input| input.pointer.hover_pos()) else {
            return CursorIcon::Default;
        };
        match self.hit_handle(doc, rect, pos) {
            Some(HandleHit::Rotate) => CursorIcon::Grab,
            Some(HandleHit::Corner(index)) => {
                if index % 2 == 0 {
                    CursorIcon::ResizeNwSe
                } else {
                    CursorIcon::ResizeNeSw
                }
            }
            Some(HandleHit::Edge(index)) => {
                if index % 2 == 0 {
                    CursorIcon::ResizeVertical
                } else {
                    CursorIcon::ResizeHorizontal
                }
            }
            None if self.inside_selection(doc, doc_at(doc, rect, pos)) => CursorIcon::Move,
            None => CursorIcon::Default,
        }
    }

    fn paint_selection(&self, painter: &egui::Painter, doc: &Doc, rect: Rect) {
        if let Some(SelectGesture::Marquee {
            anchor, current, ..
        }) = &self.selecting
        {
            let a = screen_at(doc, rect, *anchor);
            let b = screen_at(doc, rect, *current);
            let mark = Rect::from_two_pos(a, b);
            painter.rect_filled(mark, 0.0, Color32::from_rgba_unmultiplied(40, 42, 46, 18));
            painter.rect_stroke(
                mark,
                0.0,
                Stroke::new(1.0, widgets::INK),
                egui::StrokeKind::Inside,
            );
        }
        let frame = if doc.mode() == Mode::Select {
            selection_frame(doc, rect)
        } else {
            self.outline_frame(doc, rect)
        };
        let Some(frame) = frame else {
            return;
        };
        if frame.width() < 1.0 && frame.height() < 1.0 {
            return;
        }
        match doc.mode() {
            Mode::Select => {
                let line = Stroke::new(1.5, widgets::INK);
                painter.rect_stroke(frame, 0.0, line, egui::StrokeKind::Middle);
                if doc.selection_turns() {
                    let dot = rotate_dot(frame);
                    let stem_top = frame.center_top() - vec2(0.0, HANDLE_SIDE / 2.0);
                    painter.line_segment([stem_top, dot + vec2(0.0, ROTATE_DOT)], line);
                    painter.circle_filled(dot, ROTATE_DOT, widgets::INK);
                }
                for point in frame_corners(frame).into_iter().chain(frame_edges(frame)) {
                    paint_handle(painter, point);
                }
            }
            // Every other tool outlines what it is working on: the canvas a
            // brush draws on, the shape just drawn, or the text being typed.
            mode => {
                paint_dashed_rect(
                    painter,
                    frame,
                    Color32::from_rgba_unmultiplied(40, 42, 46, 110),
                );
                if mode == Mode::Text && doc.text_box_handles() {
                    for point in frame_edges(frame) {
                        paint_handle(painter, point);
                    }
                }
            }
        }
    }

    /// The dashed outline a tool other than select draws, a little outside
    /// what it outlines: the selection, or with the text tool the new text
    /// being typed.
    fn outline_frame(&self, doc: &Doc, rect: Rect) -> Option<Rect> {
        let frame = match doc.draft_box() {
            Some(_) if doc.mode() == Mode::Text => {
                let fit = self.draft_fit?;
                let origin = fit.frame.origin;
                Rect::from_two_pos(
                    screen_at(doc, rect, origin),
                    screen_at(doc, rect, [origin[0] + fit.width, origin[1] + fit.height]),
                )
            }
            _ => selection_frame(doc, rect)?,
        };
        Some(frame.expand(OUTLINE_OUTSET))
    }

    /// Which of the text tool's edge handles is under `pos`.
    fn text_side_hit(&self, doc: &Doc, rect: Rect, pos: Pos2) -> Option<BoxSide> {
        if !doc.text_box_handles() {
            return None;
        }
        doc.text_target()?;
        let frame = self.outline_frame(doc, rect)?;
        frame_edges(frame)
            .iter()
            .position(|point| pos.distance(*point) <= HANDLE_REACH)
            .map(|index| EDGE_SIDES[index])
    }

    /// A press on one of the text tool's edge handles starts a drag on its
    /// text's box.
    fn begin_text_frame(&mut self, doc: &Doc, rect: Rect, pos: Pos2) -> Option<SelectGesture> {
        let side = self.text_side_hit(doc, rect, pos)?;
        let target = doc.text_target()?;
        let before = match target {
            TextTarget::Draft => self.draft_fit?,
            TextTarget::Node(id) => doc.text_box(id)?,
        };
        Some(SelectGesture::TextFrame {
            target,
            before,
            start: doc_at(doc, rect, pos),
            side,
            previous: doc.selection(),
        })
    }

    /// The text tool's pointer: the resize arrow over an edge handle and
    /// while dragging one, and the text beam elsewhere.
    fn text_cursor(&self, ui: &egui::Ui, doc: &Doc, rect: Rect) -> CursorIcon {
        if let Some(SelectGesture::TextFrame { side, .. }) = &self.selecting {
            return side_cursor(*side);
        }
        match ui
            .input(|input| input.pointer.hover_pos())
            .and_then(|pos| self.text_side_hit(doc, rect, pos))
        {
            Some(side) => side_cursor(side),
            None => CursorIcon::Text,
        }
    }

    /// Gives the caret its focus back after a side handle's drag took it.
    fn refocus_text(&mut self, doc: &Doc) {
        self.text_focus = doc.text_generation().wrapping_sub(1);
    }

    /// The canvas part of the reference's frame: the pointer, the text
    /// editor, the cursor, and the paint, in `rect`.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, doc: &mut Doc, rect: Rect, dismissing: bool) {
        self.canvas_rect = rect;
        doc.settle_sent();
        if self.stroking && !doc.gesture_open() {
            self.end_stroke(doc);
        }
        if self.shaping && !doc.gesture_open() {
            self.shaping = false;
        }
        if self.text_press.is_some() && !doc.text_placing() {
            self.text_press = None;
        }

        let response = ui.interact(rect, ui.id().with("canvas"), Sense::click_and_drag());
        let hovered = response.hovered();
        let space = ui.input(|input| input.key_down(Key::Space));
        let editing_before = doc.text_editing();
        if editing_before {
            self.show_text_editor(ui, doc, rect);
        }

        self.handle_pointer(ui, doc, rect, hovered, dismissing);
        self.select_clicks(doc, &response, rect);
        if doc.text_editing() && !editing_before {
            self.show_text_editor(ui, doc, rect);
        }
        if !doc.text_editing() {
            self.editor_rect = None;
        }
        let mut ring = None;
        if self.pan.is_some() {
            response.on_hover_cursor(CursorIcon::Grabbing);
        } else if space && !doc.text_editing() {
            response.on_hover_cursor(CursorIcon::Grab);
        } else if self.locked {
            response.on_hover_cursor(CursorIcon::Default);
        } else if doc.mode() == Mode::Select {
            response.on_hover_cursor(self.select_cursor(ui, doc, rect));
        } else if doc.mode() == Mode::Text {
            response.on_hover_cursor(self.text_cursor(ui, doc, rect));
        } else if let Some(width) = doc.tool_width() {
            // The brush and eraser show a ring of their size in place of the
            // pointer, as on a raster tab, as the owner asked on
            // 2026-10-30. It is drawn over the drawing, below.
            response.on_hover_cursor(CursorIcon::None);
            ring = ui
                .input(|input| input.pointer.hover_pos())
                .filter(|pos| rect.contains(*pos))
                // A small brush still shows a ring the eye can find.
                .map(|pos| (pos, ((width / 2.0) as f32).max(3.0)));
        } else {
            response.on_hover_cursor(CursorIcon::Crosshair);
        }
        self.paint(ui, doc, rect);
        if let Some((pos, radius)) = ring {
            let painter = ui.painter().with_clip_rect(rect);
            widgets::ring(&painter, pos, radius, 1.0, Color32::from_black_alpha(160));
            widgets::ring(&painter, pos, radius + 1.0, 1.0, Color32::from_white_alpha(200));
        }
    }
}

fn doc_at(doc: &Doc, rect: Rect, pos: Pos2) -> Point {
    screen_to_document(rect, doc.camera_origin(), doc.zoom(), pos)
}

fn screen_at(doc: &Doc, rect: Rect, point: Point) -> Pos2 {
    document_to_screen(rect, doc.camera_origin(), doc.zoom(), point)
}

fn selection_frame(doc: &Doc, rect: Rect) -> Option<Rect> {
    let (min, max) = doc.selection_bounds()?;
    Some(Rect::from_two_pos(
        screen_at(doc, rect, min),
        screen_at(doc, rect, max),
    ))
}

fn wheel_scale(unit: MouseWheelUnit, rect: Rect) -> f64 {
    match unit {
        MouseWheelUnit::Point => 1.0,
        MouseWheelUnit::Line => NOTCH_POINTS,
        MouseWheelUnit::Page => f64::from(rect.height()),
    }
}

fn screen_to_document(rect: Rect, origin: [f64; 2], zoom: f64, pos: Pos2) -> [f64; 2] {
    [
        origin[0] + f64::from(pos.x - rect.min.x) / zoom,
        origin[1] + f64::from(pos.y - rect.min.y) / zoom,
    ]
}

fn document_to_screen(rect: Rect, origin: [f64; 2], zoom: f64, point: [f64; 2]) -> Pos2 {
    Pos2::new(
        ((point[0] - origin[0]) * zoom) as f32 + rect.min.x,
        ((point[1] - origin[1]) * zoom) as f32 + rect.min.y,
    )
}

pub(crate) fn paint_grid(painter: &egui::Painter, rect: Rect, origin: [f64; 2], zoom: f64) {
    // As faint as the picture's grid: a guide, not a feature of the drawing.
    let stroke = Stroke::new(1.0, Color32::from_rgb(234, 235, 238));
    let top_left = screen_to_document(rect, origin, zoom, rect.min);
    let bottom_right = screen_to_document(rect, origin, zoom, rect.max);
    let mut x = (top_left[0] / GRID_SPACING).floor() * GRID_SPACING;
    while x <= bottom_right[0] {
        let start = document_to_screen(rect, origin, zoom, [x, top_left[1]]);
        let end = document_to_screen(rect, origin, zoom, [x, bottom_right[1]]);
        painter.line_segment([start, end], stroke);
        x += GRID_SPACING;
    }
    let mut y = (top_left[1] / GRID_SPACING).floor() * GRID_SPACING;
    while y <= bottom_right[1] {
        let start = document_to_screen(rect, origin, zoom, [top_left[0], y]);
        let end = document_to_screen(rect, origin, zoom, [bottom_right[0], y]);
        painter.line_segment([start, end], stroke);
        y += GRID_SPACING;
    }
}

fn paint_mark(painter: &egui::Painter, rect: Rect, origin: [f64; 2], zoom: f64, paint: &InkPaint) {
    if paint.triangles.is_empty() {
        return;
    }
    let color = if paint.eraser_preview {
        Color32::from_rgba_unmultiplied(90, 90, 90, 70)
    } else {
        let alpha = (paint.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        Color32::from_rgba_unmultiplied(paint.rgb[0], paint.rgb[1], paint.rgb[2], alpha)
    };
    let mut mesh = egui::Mesh::default();
    for triangle in &paint.triangles {
        let index = mesh.vertices.len() as u32;
        for point in triangle {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: document_to_screen(rect, origin, zoom, *point),
                uv: egui::epaint::WHITE_UV,
                color,
            });
        }
        mesh.add_triangle(index, index + 1, index + 2);
    }
    painter.add(Shape::mesh(mesh));
}

fn box_corners(min: Point, max: Point) -> [Point; 4] {
    [
        [min[0], min[1]],
        [max[0], min[1]],
        [max[0], max[1]],
        [min[0], max[1]],
    ]
}

/// Edge midpoints in document space: top, right, bottom, left.
fn box_edges(min: Point, max: Point) -> [Point; 4] {
    let center = box_center(min, max);
    [
        [center[0], min[1]],
        [max[0], center[1]],
        [center[0], max[1]],
        [min[0], center[1]],
    ]
}

fn box_center(min: Point, max: Point) -> Point {
    [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0]
}

fn angle_of(point: Point, center: Point) -> f64 {
    (point[1] - center[1]).atan2(point[0] - center[0])
}

fn axis_scale(handle: f64, opposite: f64, pointer: f64) -> Option<f64> {
    let span = handle - opposite;
    if span.abs() < 1.0e-6 {
        return None;
    }
    let scale = (pointer - opposite) / span;
    if scale.is_finite() { Some(scale) } else { None }
}

fn scale_about(sx: f64, sy: f64, center: kurbo::Point) -> Affine {
    let center = center.to_vec2();
    Affine::translate(-center)
        .then_scale_non_uniform(sx, sy)
        .then_translate(center)
}

fn paint_handle(painter: &egui::Painter, center: Pos2) {
    let handle = Rect::from_center_size(center, vec2(HANDLE_SIDE, HANDLE_SIDE));
    painter.rect(
        handle,
        1.5,
        Color32::WHITE,
        Stroke::new(1.5, widgets::INK),
        egui::StrokeKind::Inside,
    );
}

fn frame_corners(frame: Rect) -> [Pos2; 4] {
    [
        frame.left_top(),
        frame.right_top(),
        frame.right_bottom(),
        frame.left_bottom(),
    ]
}

/// Edge midpoints: top, right, bottom, left.
/// The box side each of [`frame_edges`] moves.
const EDGE_SIDES: [BoxSide; 4] = [BoxSide::Top, BoxSide::Right, BoxSide::Bottom, BoxSide::Left];

fn side_cursor(side: BoxSide) -> CursorIcon {
    match side {
        BoxSide::Left | BoxSide::Right => CursorIcon::ResizeHorizontal,
        BoxSide::Top | BoxSide::Bottom => CursorIcon::ResizeVertical,
    }
}

fn frame_edges(frame: Rect) -> [Pos2; 4] {
    [
        frame.center_top(),
        frame.right_center(),
        frame.center_bottom(),
        frame.left_center(),
    ]
}

/// The resize arrow for a scale drag: along x or y for an edge, and along the
/// diagonal from the fixed corner to the dragged one for a corner.
fn scale_cursor(axes: [bool; 2], handle: Point, opposite: Point) -> CursorIcon {
    let span = [handle[0] - opposite[0], handle[1] - opposite[1]];
    match axes {
        [true, false] => CursorIcon::ResizeHorizontal,
        [false, true] => CursorIcon::ResizeVertical,
        _ if span[0] * span[1] >= 0.0 => CursorIcon::ResizeNwSe,
        _ => CursorIcon::ResizeNeSw,
    }
}

fn rotate_dot(frame: Rect) -> Pos2 {
    frame.center_top() - vec2(0.0, ROTATE_STEM)
}

fn paint_dashed_rect(painter: &egui::Painter, rect: Rect, color: Color32) {
    dash_segment(painter, rect.left_top(), rect.right_top(), color);
    dash_segment(painter, rect.right_top(), rect.right_bottom(), color);
    dash_segment(painter, rect.right_bottom(), rect.left_bottom(), color);
    dash_segment(painter, rect.left_bottom(), rect.left_top(), color);
}

fn dash_segment(painter: &egui::Painter, from: Pos2, to: Pos2, color: Color32) {
    let delta = to - from;
    let length = delta.length();
    if length < 1.0 {
        return;
    }
    let step = delta / length;
    let mut cursor = 0.0;
    let mut draw = true;
    while cursor < length {
        let next = (cursor + if draw { 5.0 } else { 4.0 }).min(length);
        if draw {
            painter.line_segment(
                [from + step * cursor, from + step * next],
                Stroke::new(1.0, color),
            );
        }
        cursor = next;
        draw = !draw;
    }
}
