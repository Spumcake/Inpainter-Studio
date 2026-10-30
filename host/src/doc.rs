//! What the drawing surface reads and does in place of Simple Sketch 1.0.1's
//! session. Each method stands where `canvas.rs` called the session: it reads
//! the tab's display copy or the declared tool, previews on the display copy,
//! or sends a message. The host keeps no history and allocates no id; a
//! preview lasts until the next `document` replaces the display copy.

use crate::process::Display;
use draw::camera;
use draw::document::snapshot::{
    box_from_value, dynamics_from_value, palette_from_value, stroke_look_from, style_from_value,
};
use draw::document::{
    BoxSide, Cap, ColorId, Dash, InkPaint, LiveStroke, NodeId, Point, Sample, SceneMark, ShapeKind,
    TextBox, TextFit, TextLook, TextStyle, dragged_box, map_samples, preview_shape_paints,
    resolve_drag,
};
use draw::gesture::{GestureKind, OpenGesture};
use draw::ink;
use kurbo::Affine;
use protocol::{Action, Canvas, CanvasTool, HostMessage, SelectOp};
use std::collections::{HashMap, VecDeque};

/// The current tool's kind, as the drawing surface handles its pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Select,
    Brush,
    Eraser,
    Shape,
    Text,
    /// No tool is declared: nothing is drawn by the pointer.
    None,
}

/// The text the text tool's side handles act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextTarget {
    Draft,
    Node(NodeId),
}

/// What the frame draws while text is being edited.
pub(crate) struct TextCaret {
    pub id: Option<NodeId>,
    pub content: String,
    pub style: TextStyle,
    pub look: TextLook,
    pub origin: Point,
    pub box_width: Option<f64>,
    pub box_height: Option<f64>,
    pub world: Affine,
}

/// The surface's own state across frames: the gesture in progress and what
/// it has shown before the sub-application answered.
#[derive(Default)]
pub(crate) struct Live {
    open: Option<OpenGesture>,
    /// Words typed into the caret that the sub-application has not yet echoed.
    typed: Option<(u64, String)>,
    /// The caret generation that was committed; it is hidden until another arrives.
    closed: Option<u64>,
    /// New text's box while its side handle is dragged.
    draft: Option<(u64, TextBox)>,
    /// Strokes and shapes sent and not yet answered, oldest first. Each stays
    /// on screen until a document answers it, so a mark does not vanish while
    /// the sub-application commits it.
    sent: VecDeque<OpenGesture>,
    /// The document the oldest of `sent` waits to be replaced.
    sent_over: u64,
    /// Text sizes already sent, by object.
    metrics: HashMap<u64, [f64; 3]>,
    /// The camera moved this frame.
    pub moved: bool,
}

impl Live {
    /// A tab switch drops the gesture and what it previewed.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

pub(crate) struct Doc<'a> {
    pub tab: &'a str,
    pub display: &'a mut Display,
    pub canvas: Option<&'a Canvas>,
    pub out: &'a mut Vec<HostMessage>,
    pub live: &'a mut Live,
    pub viewport: [f64; 2],
}

impl Doc<'_> {
    pub(crate) fn mode(&self) -> Mode {
        match self.canvas.map(|canvas| &canvas.tool) {
            Some(CanvasTool::Select) => Mode::Select,
            Some(CanvasTool::Brush { .. }) => Mode::Brush,
            Some(CanvasTool::Erase { .. }) => Mode::Eraser,
            Some(CanvasTool::Shape { .. }) => Mode::Shape,
            Some(CanvasTool::Text { .. }) => Mode::Text,
            None => Mode::None,
        }
    }

    /// The drawing surface's own commands, which only move the tab's camera.
    pub(crate) fn moves_camera_only(id: &str) -> bool {
        matches!(
            id,
            "canvas.zoom_in"
                | "canvas.zoom_out"
                | "canvas.fit"
                | "canvas.zoom_to_selection"
                | "canvas.zoom"
        )
    }

    /// Sends a declared action, as the window ran a session command.
    pub(crate) fn run(&mut self, action: &Action, node: Option<u64>) {
        match action.id.as_str() {
            "canvas.zoom_in" => self.zoom_at_center(true),
            "canvas.zoom_out" => self.zoom_at_center(false),
            "canvas.fit" => self.fit(),
            "canvas.zoom_to_selection" => self.zoom_to_selection(),
            "canvas.zoom" => self.set_zoom(action),
            _ => self.out.push(HostMessage::Command {
                id: action.id.clone(),
                tab: Some(self.tab.to_string()),
                arg: action.arg.clone(),
                node,
                viewport: self.viewport,
            }),
        }
    }

    // The camera, which the host keeps while the tab is shown.

    pub(crate) fn camera_origin(&self) -> Point {
        self.display.camera.origin
    }

    pub(crate) fn zoom(&self) -> f64 {
        self.display.camera.zoom
    }

    /// How wide the brush or eraser draws, in screen points, for the ring
    /// that follows the pointer: the tool's size, or else its layer's.
    pub(crate) fn tool_width(&self) -> Option<f64> {
        let width = match self.canvas.map(|canvas| &canvas.tool)? {
            CanvasTool::Brush { canvas, layer, size, look, .. } => {
                let layer_size = canvas
                    .filter(|id| *id != 0)
                    .zip(layer.filter(|id| *id != 0))
                    .and_then(|(canvas, layer)| self.display.sketch.color_style(NodeId::new(canvas), ColorId::new(layer)))
                    .map(|style| style.size)
                    .or_else(|| draw::document::snapshot::stroke_look_from(look).map(|look| look.0));
                f64::from(size.or(layer_size)?)
            }
            CanvasTool::Erase { size, .. } => f64::from(*size),
            _ => return None,
        };
        Some(width * self.display.camera.zoom)
    }

    pub(crate) fn pan_screen(&mut self, delta: Point) {
        camera::pan_screen(&mut self.display.camera, delta);
        self.live.moved = true;
    }

    pub(crate) fn zoom_toward(&mut self, factor: f64, pointer: Point) {
        camera::zoom_toward(&mut self.display.camera, factor, pointer);
        self.live.moved = true;
    }

    fn zoom_at_center(&mut self, zoom_in: bool) {
        camera::zoom_at_center(&mut self.display.camera, zoom_in, self.viewport);
        self.live.moved = true;
    }

    /// `canvas.zoom` with `{"zoom": n}`, such as 1 for 100%, about the
    /// viewport's center.
    fn set_zoom(&mut self, action: &Action) {
        let Some(zoom) = action
            .arg
            .as_ref()
            .and_then(|arg| arg.get("zoom"))
            .and_then(serde_json::Value::as_f64)
            .filter(|zoom| zoom.is_finite() && *zoom > 0.0)
        else {
            return;
        };
        let center = [self.viewport[0] / 2.0, self.viewport[1] / 2.0];
        let factor = zoom / self.display.camera.zoom;
        camera::zoom_toward(&mut self.display.camera, factor, center);
        self.live.moved = true;
    }

    fn fit(&mut self) {
        camera::fit(
            &mut self.display.camera,
            &self.display.sketch,
            self.viewport,
        );
        self.live.moved = true;
    }

    fn zoom_to_selection(&mut self) {
        let sketch = &mut self.display.sketch;
        sketch.set_camera(self.display.camera);
        if sketch.zoom_to_selection(self.viewport) {
            self.display.camera = sketch.camera();
            self.live.moved = true;
        }
    }

    // The selection, read from the display copy and changed by the
    // sub-application.

    pub(crate) fn selection(&self) -> Vec<NodeId> {
        self.display.sketch.selection().to_vec()
    }

    pub(crate) fn selection_bounds(&self) -> Option<(Point, Point)> {
        self.display.sketch.selection_bounds()
    }

    pub(crate) fn hit_test(&self, point: Point) -> Option<NodeId> {
        self.display.sketch.hit_test(point)
    }

    pub(crate) fn selection_turns(&self) -> bool {
        self.display.sketch.selection_turns()
    }

    /// The one selected object, when it is a text object that can be changed.
    pub(crate) fn sole_text(&self) -> Option<NodeId> {
        let [id] = self.display.sketch.selection() else {
            return None;
        };
        (self.display.sketch.is_text(*id) && self.display.sketch.transform_target(*id))
            .then_some(*id)
    }

    pub(crate) fn text_box(&self, id: NodeId) -> Option<TextFit> {
        self.display.sketch.text_box(id)
    }

    fn select(&mut self, op: SelectOp) {
        self.out.push(HostMessage::Select {
            tab: self.tab.to_string(),
            op,
        });
    }

    pub(crate) fn select_click(&mut self, id: NodeId, shift: bool) {
        self.select(SelectOp::Click {
            node: id.get(),
            add: shift,
        });
    }

    pub(crate) fn clear_selection(&mut self) {
        self.select(SelectOp::Clear);
    }

    pub(crate) fn replace_selection(&mut self, ids: Vec<NodeId>) {
        self.select(SelectOp::Replace {
            nodes: ids.into_iter().map(NodeId::get).collect(),
        });
    }

    pub(crate) fn marquee_select(&mut self, min: Point, max: Point, shift: bool, base: &[NodeId]) {
        self.select(SelectOp::Marquee {
            min,
            max,
            add: shift,
            base: base.iter().map(|id| id.get()).collect(),
        });
    }

    /// A right-click with the select tool: anywhere inside the selection's
    /// box keeps it; otherwise the object under the pointer is selected.
    pub(crate) fn pick_for_menu(&mut self, point: Point) -> Option<NodeId> {
        if let Some((min, max)) = self.selection_bounds()
            && (min[0]..=max[0]).contains(&point[0])
            && (min[1]..=max[1]).contains(&point[1])
        {
            return self.display.sketch.selection().first().copied();
        }
        let hit = self.hit_test(point)?;
        if !self.display.sketch.selection().contains(&hit) {
            self.select_click(hit, false);
        }
        Some(hit)
    }

    pub(crate) fn object_page_available(&self, node: NodeId) -> bool {
        self.canvas
            .is_some_and(|canvas| canvas.object_pages.contains(&node.get()))
    }

    pub(crate) fn edit_text(&mut self, id: NodeId) {
        if let Some(action) = self.canvas.and_then(|canvas| canvas.edit_text.clone()) {
            self.run(&action, Some(id.get()));
        }
    }

    // Transforms, previewed on the display copy and sent once on release.

    /// The selection's local transforms that a drag may change, as the
    /// selection will be once `select` arrives: `predicted`, when given.
    pub(crate) fn changeable_locals(&self, predicted: Option<&[NodeId]>) -> Vec<(NodeId, Affine)> {
        let sketch = &self.display.sketch;
        predicted
            .unwrap_or(sketch.selection())
            .iter()
            .filter(|id| sketch.transform_target(**id))
            .filter_map(|id| {
                sketch
                    .local_transform(*id)
                    .map(|transform| (*id, transform))
            })
            .collect()
    }

    pub(crate) fn preview_world(
        &mut self,
        before: &[(NodeId, Affine)],
        map: impl Fn(Affine) -> Affine,
    ) {
        let mut locals = Vec::new();
        for (id, local) in before {
            let Some(parent) = self.display.sketch.parent_of_world(*id) else {
                continue;
            };
            locals.push((*id, parent.inverse() * map(parent * *local)));
        }
        self.show_locals(&locals);
    }

    pub(crate) fn restore_locals(&mut self, before: &[(NodeId, Affine)]) {
        self.show_locals(before);
    }

    fn show_locals(&mut self, locals: &[(NodeId, Affine)]) {
        if let Some(step) = self.display.sketch.set_locals(locals) {
            self.display.sketch.apply(&step);
        }
    }

    /// Sends the drag as one gesture: each object's transform before it and now.
    pub(crate) fn commit_preview(&mut self, before: &[(NodeId, Affine)]) {
        let after: Vec<(NodeId, Affine)> = before
            .iter()
            .filter_map(|(id, _)| {
                self.display
                    .sketch
                    .local_transform(*id)
                    .map(|transform| (*id, transform))
            })
            .collect();
        if after.iter().zip(before).all(|(now, was)| now.1 == was.1) {
            return;
        }
        self.gesture(GestureKind::Transform {
            before: before.to_vec(),
            after,
        });
    }

    fn gesture(&mut self, kind: GestureKind) {
        let gesture = OpenGesture {
            kind,
            samples: Vec::new(),
        }
        .finish();
        self.out.push(HostMessage::Gesture {
            tab: self.tab.to_string(),
            gesture: gesture.to_value(),
        });
    }

    /// A handle's drag on text: a side handle sets the box's width, so the
    /// words re-wrap, and a top or bottom handle its height. The opposite
    /// side stays put.
    pub(crate) fn preview_text_box(
        &mut self,
        id: NodeId,
        before: TextFit,
        from: Point,
        to: Point,
        side: BoxSide,
    ) {
        let sketch = &self.display.sketch;
        let (Some(from), Some(to)) = (sketch.local_point(id, from), sketch.local_point(id, to))
        else {
            return;
        };
        let em = sketch.text_size(id).map_or(8.0, f64::from);
        let after = dragged_box(before, [to[0] - from[0], to[1] - from[1]], em, side);
        self.restore_text_box(id, after);
    }

    pub(crate) fn restore_text_box(&mut self, id: NodeId, frame: TextBox) {
        if let Some(step) = self.display.sketch.place_text_box(id, frame) {
            self.display.sketch.apply(&step);
        }
    }

    pub(crate) fn commit_text_box(&mut self, id: NodeId, before: TextBox) {
        let Some(after) = self.display.sketch.text_box(id).map(|fit| fit.frame) else {
            return;
        };
        if after == before {
            return;
        }
        self.gesture(GestureKind::TextFrame {
            node: id,
            before,
            after,
        });
    }

    /// A handle's drag on the new text, in the document's units.
    pub(crate) fn preview_draft_box(
        &mut self,
        before: TextFit,
        from: Point,
        to: Point,
        side: BoxSide,
    ) {
        let em = match self.canvas.map(|canvas| &canvas.tool) {
            Some(CanvasTool::Text { em, .. }) => *em,
            _ => 8.0,
        };
        let after = dragged_box(before, [to[0] - from[0], to[1] - from[1]], em, side);
        self.live.draft = Some((self.text_generation(), after));
    }

    /// Puts the new text's box back, as Escape during its drag does.
    pub(crate) fn set_draft_box(&mut self, _before: TextBox) {
        self.live.draft = None;
    }

    /// Sends the new text's box when its drag ends. It is not history.
    pub(crate) fn finish_draft_box(&mut self) {
        let Some(frame) = self.dragged_draft(self.text_generation()) else {
            return;
        };
        if let Some(caret) = self.canvas.and_then(|canvas| canvas.caret.as_ref()) {
            let action = caret
                .draft_box
                .with_value(draw::document::snapshot::box_value(frame));
            self.run(&action, None);
        }
    }

    // Text being typed, which the sub-application owns.

    fn caret(&self) -> Option<&protocol::Caret> {
        let caret = self.canvas?.caret.as_ref()?;
        (self.live.closed != Some(caret.generation)).then_some(caret)
    }

    pub(crate) fn text_editing(&self) -> bool {
        self.caret().is_some()
    }

    pub(crate) fn text_generation(&self) -> u64 {
        self.caret().map_or(0, |caret| caret.generation)
    }

    pub(crate) fn text_placing(&self) -> bool {
        matches!(
            self.live.open.as_ref().map(|open| &open.kind),
            Some(GestureKind::Text { .. })
        )
    }

    /// The in-progress drag, when it is placing a new box rather than editing.
    pub(crate) fn text_drag(&self) -> Option<(Point, Point, f64)> {
        let GestureKind::Text {
            origin,
            pointer,
            hit: None,
        } = self.live.open.as_ref()?.kind
        else {
            return None;
        };
        let line = match self.canvas.map(|canvas| &canvas.tool) {
            Some(CanvasTool::Text { line, .. }) => *line,
            _ => return None,
        };
        Some((origin, pointer, line))
    }

    pub(crate) fn text_box_handles(&self) -> bool {
        self.canvas.is_some_and(|canvas| canvas.text_handles)
    }

    pub(crate) fn text_target(&self) -> Option<TextTarget> {
        match self.caret() {
            Some(caret) => Some(match caret.id {
                Some(id) => TextTarget::Node(NodeId::new(id)),
                None => TextTarget::Draft,
            }),
            None => self.sole_text().map(TextTarget::Node),
        }
    }

    /// The new text's box, while it is being typed.
    pub(crate) fn draft_box(&self) -> Option<TextBox> {
        let caret = self.caret()?;
        if caret.id.is_some() {
            return None;
        }
        self.dragged_draft(caret.generation)
            .or_else(|| caret.frame.as_ref().and_then(box_from_value))
    }

    /// The new text's box as its side handle left it, for that edit only.
    fn dragged_draft(&self, generation: u64) -> Option<TextBox> {
        self.live
            .draft
            .filter(|(edit, _)| *edit == generation)
            .map(|(_, frame)| frame)
    }

    pub(crate) fn text_caret(&self) -> Option<TextCaret> {
        self.caret_view(self.caret()?)
    }

    /// Text committed and not yet answered: drawn as it was typed until the
    /// next declaration says the edit ended, so the words do not blink.
    pub(crate) fn committed_caret(&self) -> Option<TextCaret> {
        let caret = self.canvas?.caret.as_ref()?;
        if self.live.closed != Some(caret.generation) {
            return None;
        }
        self.caret_view(caret)
    }

    fn caret_view(&self, caret: &protocol::Caret) -> Option<TextCaret> {
        let content = match &self.live.typed {
            Some((generation, typed)) if *generation == caret.generation => typed.clone(),
            _ => caret.content.clone(),
        };
        match caret.id {
            None => {
                let frame = self
                    .dragged_draft(caret.generation)
                    .or_else(|| caret.frame.as_ref().and_then(box_from_value))?;
                let style = caret.style.as_ref().and_then(style_from_value)?;
                let palette = caret.palette.as_ref().and_then(palette_from_value)?;
                Some(TextCaret {
                    id: None,
                    content,
                    style,
                    look: TextLook::of(&palette),
                    origin: frame.origin,
                    box_width: frame.width,
                    box_height: frame.height,
                    world: Affine::IDENTITY,
                })
            }
            Some(id) => {
                let id = NodeId::new(id);
                let SceneMark::Text(view) = self
                    .display
                    .sketch
                    .scene(None)
                    .into_iter()
                    .find(|mark| matches!(mark, SceneMark::Text(view) if view.id == id))?
                else {
                    return None;
                };
                Some(TextCaret {
                    id: Some(id),
                    content,
                    style: view.style,
                    look: view.look,
                    origin: view.origin,
                    box_width: view.box_width,
                    box_height: view.box_height,
                    world: view.world,
                })
            }
        }
    }

    /// The words in the caret changed. They show at once and are sent.
    pub(crate) fn set_text_draft(&mut self, content: String) {
        let Some(caret) = self.caret().cloned() else {
            return;
        };
        self.live.typed = Some((caret.generation, content.clone()));
        let action = caret.edit.with_value(serde_json::Value::String(content));
        self.run(&action, None);
    }

    pub(crate) fn commit_text(&mut self) {
        let Some(caret) = self.caret().cloned() else {
            return;
        };
        self.live.closed = Some(caret.generation);
        self.run(&caret.commit, None);
    }

    /// A text's size as this frame laid it out: stored on the display copy,
    /// and sent when it changed.
    pub(crate) fn set_text_metrics(&mut self, id: NodeId, width: f64, height: f64, natural: f64) {
        self.display
            .sketch
            .set_text_metrics(id, width, height, natural);
        let value = [width, height, natural];
        if self.live.metrics.get(&id.get()) == Some(&value) {
            return;
        }
        self.live.metrics.insert(id.get(), value);
        if let Some(action) = self.canvas.and_then(|canvas| canvas.metrics.clone()) {
            let action = Action {
                arg: Some(serde_json::json!({ "node": id.get() })),
                ..action
            }
            .with_value(serde_json::json!(value));
            self.run(&action, None);
        }
    }

    // The open gesture: the host previews it, and sends it once it ends.

    pub(crate) fn gesture_open(&self) -> bool {
        self.live.open.is_some()
    }

    /// Opens a stroke from the declared brush or eraser. A brush whose layer
    /// is hidden or locked, or an eraser with no canvas, draws nothing.
    pub(crate) fn begin(&mut self) -> Result<(), ()> {
        if self.live.open.is_some() {
            return Err(());
        }
        let open = match self.canvas.map(|canvas| &canvas.tool) {
            Some(CanvasTool::Brush {
                canvas,
                source,
                layer,
                size,
                dynamics,
                ready,
                ..
            }) => {
                if !*ready || *source == 0 {
                    return Err(());
                }
                OpenGesture::brush(
                    canvas.filter(|id| *id != 0).map(NodeId::new),
                    ColorId::new(*source),
                    layer.filter(|id| *id != 0).map(ColorId::new),
                    *size,
                    dynamics_from_value(dynamics).ok_or(())?,
                )
            }
            Some(CanvasTool::Erase {
                canvas,
                size,
                dynamics,
            }) => {
                let canvas = canvas.filter(|id| *id != 0).ok_or(())?;
                OpenGesture::erase(
                    NodeId::new(canvas),
                    *size,
                    dynamics_from_value(dynamics).ok_or(())?,
                )
            }
            _ => return Err(()),
        };
        self.live.open = Some(open);
        Ok(())
    }

    pub(crate) fn add_sample(&mut self, sample: Sample) {
        if let Some(open) = self.live.open.as_mut() {
            let _ = open.add_sample(sample);
        }
    }

    pub(crate) fn begin_shape(&mut self, point: Point) -> Result<(), ()> {
        if self.live.open.is_some() || !point[0].is_finite() || !point[1].is_finite() {
            return Err(());
        }
        let Some(CanvasTool::Shape { shape, .. }) = self.canvas.map(|canvas| &canvas.tool) else {
            return Err(());
        };
        self.live.open = Some(OpenGesture::shape(shape_kind(shape).ok_or(())?, point));
        Ok(())
    }

    pub(crate) fn move_shape(&mut self, point: Point, shift: bool) {
        if let Some(OpenGesture {
            kind:
                GestureKind::Shape {
                    pointer,
                    shift: held,
                    ..
                },
            ..
        }) = self.live.open.as_mut()
        {
            *pointer = point;
            *held = shift;
        }
    }

    /// Starts a text placement. A press on existing text is remembered so the
    /// release edits it. Text already being typed is committed first.
    pub(crate) fn begin_text(&mut self, point: Point) -> Result<(), ()> {
        if self.live.open.is_some() || self.mode() != Mode::Text {
            return Err(());
        }
        if !point[0].is_finite() || !point[1].is_finite() {
            return Err(());
        }
        self.commit_text();
        let hit = self
            .hit_test(point)
            .filter(|id| self.display.sketch.is_text(*id));
        self.live.open = Some(OpenGesture::text(point, hit));
        Ok(())
    }

    pub(crate) fn move_text(&mut self, point: Point) {
        if let Some(OpenGesture {
            kind: GestureKind::Text { pointer, .. },
            ..
        }) = self.live.open.as_mut()
        {
            *pointer = point;
        }
    }

    /// Ends the placement and sends it. A click sends no drag.
    pub(crate) fn end_text(&mut self, dragged: bool) {
        let Some(mut open) = self.live.open.take() else {
            return;
        };
        if let GestureKind::Text {
            origin, pointer, ..
        } = &mut open.kind
            && !dragged
        {
            *pointer = *origin;
        }
        self.send_open(open);
    }

    /// Sends the open stroke or shape. Its preview ends here; the next
    /// document shows what the sub-application made of it.
    pub(crate) fn commit(&mut self) {
        if matches!(
            self.live.open.as_ref().map(|open| &open.kind),
            Some(GestureKind::Text { .. })
        ) {
            return;
        }
        if let Some(open) = self.live.open.take() {
            self.send_open(open);
        }
    }

    fn send_open(&mut self, open: OpenGesture) {
        if !matches!(open.kind, GestureKind::Text { .. }) {
            if self.live.sent.is_empty() {
                self.live.sent_over = self.display.version;
            }
            self.live.sent.push_back(open.clone());
        }
        let gesture = open.finish();
        self.out.push(HostMessage::Gesture {
            tab: self.tab.to_string(),
            gesture: gesture.to_value(),
        });
    }

    pub(crate) fn cancel(&mut self) {
        self.live.open = None;
    }

    // Painting.

    /// The display copy's marks, with the open gesture painted where its
    /// commit will put it.
    pub(crate) fn scene(&self) -> Vec<SceneMark> {
        let shown: Vec<&OpenGesture> = self
            .shown_gestures()
            .filter(|open| {
                !open.samples.is_empty() || matches!(open.kind, GestureKind::Shape { .. })
            })
            .collect();
        // Strokes on an existing canvas paint in their color layer, under
        // what is above it. The document takes one live stroke; the others
        // waiting paint above, where they will land in a moment.
        let mut lives = shown.iter().filter_map(|open| self.live_stroke(open));
        let first = lives.next();
        let mut marks = self.display.sketch.scene(first.as_ref());
        let rest: Vec<LiveStroke> = lives.collect();
        // A stroke that will make a new canvas paints above everything, as
        // that canvas will be the top of the sketch.
        for live in first
            .iter()
            .filter(|live| live.canvas.is_none())
            .chain(rest.iter())
        {
            if let Ok(mesh) = ink::mesh_from_shapes(live.shapes.clone()) {
                marks.push(SceneMark::Ink(InkPaint {
                    rgb: live.look.0,
                    opacity: live.look.1,
                    eraser_preview: false,
                    triangles: mesh.triangles,
                }));
            }
        }
        for open in &shown {
            if let Some(trail) = eraser_trail(open) {
                marks.push(SceneMark::Ink(trail));
            }
            marks.extend(self.shape_preview(open).into_iter().map(SceneMark::Ink));
        }
        marks
    }

    /// The open brush stroke, in its canvas's space and outlined as it will
    /// be committed, so it paints in its color layer while it is drawn.
    fn live_stroke(&self, open: &OpenGesture) -> Option<LiveStroke> {
        let GestureKind::Brush {
            canvas,
            layer,
            size,
            dynamics,
            ..
        } = &open.kind
        else {
            return None;
        };
        let sketch = &self.display.sketch;
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
                let Some(CanvasTool::Brush { look, .. }) = self.canvas.map(|canvas| &canvas.tool)
                else {
                    return None;
                };
                (stroke_look_from(look)?, None)
            }
        };
        let (layer_size, cap, dash, look) = style;
        let transform = canvas
            .and_then(|canvas| sketch.world_transform(canvas))
            .unwrap_or(Affine::IDENTITY);
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

    /// Each document that arrived since the last frame answers the oldest
    /// gesture still waiting. A gesture is always answered with a document.
    pub(crate) fn settle_sent(&mut self) {
        let arrived = self.display.version.saturating_sub(self.live.sent_over);
        for _ in 0..arrived {
            self.live.sent.pop_front();
        }
        self.live.sent_over = self.display.version;
    }

    /// The gestures sent and not yet answered, then the one in progress.
    fn shown_gestures(&self) -> impl Iterator<Item = &OpenGesture> {
        self.live.sent.iter().chain(self.live.open.as_ref())
    }

    fn shape_preview(&self, open: &OpenGesture) -> Vec<InkPaint> {
        let OpenGesture {
            kind:
                GestureKind::Shape {
                    kind,
                    origin,
                    pointer,
                    shift,
                },
            ..
        } = open
        else {
            return Vec::new();
        };
        let Some(CanvasTool::Shape {
            constrain,
            snap,
            palette,
            corner_radius,
            ..
        }) = self.canvas.map(|canvas| &canvas.tool)
        else {
            return Vec::new();
        };
        let Some(palette) = palette_from_value(palette) else {
            return Vec::new();
        };
        let Some(placed) = resolve_drag(*kind, *origin, *pointer, constrain ^ shift, *snap) else {
            return Vec::new();
        };
        preview_shape_paints(*kind, placed, palette, *corner_radius)
    }
}

/// The eraser's path while it is dragged, a translucent trail over the ink.
fn eraser_trail(open: &OpenGesture) -> Option<InkPaint> {
    let GestureKind::Erase { size, dynamics, .. } = &open.kind else {
        return None;
    };
    let shapes = ink::stroke_visible(
        &open.samples,
        dynamics,
        &[],
        f64::from(*size),
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

fn shape_kind(name: &str) -> Option<ShapeKind> {
    Some(match name {
        "rectangle" => ShapeKind::Rectangle,
        "ellipse" => ShapeKind::Ellipse,
        "line" => ShapeKind::Line,
        _ => return None,
    })
}
