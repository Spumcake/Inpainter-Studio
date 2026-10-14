//! Pointer mapping and mesh painting. The session stays free of egui.

use sketch::document::{InkPaint, NodeId, Point, Sample, SceneMark, TextFit};
use sketch::registry::{self, Action, CommandId, Tool};
use sketch::session::{BoxSide, CloseEffect, CommandEffect, Session, TextTarget};
use crate::tablet::Tablet;
use crate::{panel, tabs, toolbar, widgets};
use eframe::egui::{
    self, Color32, Context, CursorIcon, Event, Key, MouseWheelUnit, PointerButton, Pos2, Rect,
    Sense, Shape, Stroke, TouchPhase, vec2,
};
use kurbo::{Affine, Vec2};

const NOTCH_POINTS: f64 = 50.0;
/// A hair darker than the white toolbar and active tab, as in the picture.
const CANVAS: Color32 = Color32::from_rgb(251, 251, 252);

pub struct SketchApp {
    session: Session,
    /// Tool settings are written to the preferences file. Off in the frame
    /// tests, so they never touch the person's own preferences.
    saves_prefs: bool,
    chrome: tabs::live::Chrome,
    view_ready: bool,
    /// Center the active sketch once its viewport is known. Set when a blank
    /// sketch is created.
    center_new: bool,
    /// The window title last sent: the active sketch's name, with `*` while
    /// it has unsaved changes.
    title: String,
    viewport: [f64; 2],
    stroking: bool,
    /// A rectangle, ellipse, or line drag is open.
    shaping: bool,
    pan: Option<PanButton>,
    stroke_pressure: Option<f64>,
    tablet: Tablet,
    /// The open stroke is drawn from tablet samples rather than the pointer.
    pen_stroke: bool,
    /// The tool to restore when the stylus eraser end lifts.
    restore_tool: Option<Tool>,
    selecting: Option<SelectGesture>,
    /// Screen position where a text placement was pressed.
    text_press: Option<Pos2>,
    /// The object a select-tool right-click opened its menu on.
    menu_target: Option<NodeId>,
    /// Where the caret was drawn last frame, so a click outside it commits.
    editor_rect: Option<Rect>,
    /// The edit generation the caret has already been focused for.
    text_focus: u64,
    text_cache: sketch::text::Cache,
    /// The canvas's screen rectangle last frame, so a press elsewhere, such
    /// as on the panel, leaves the text being edited open.
    canvas_rect: Rect,
    /// The new text's box as laid out last frame, for its outline and
    /// handles.
    draft_fit: Option<TextFit>,
    /// Ctrl/Cmd+C, X, or V already arrived as a copy, cut, or paste event.
    /// The key-up still comes through, and must not run the command again.
    copy_press: bool,
    cut_press: bool,
    paste_press: bool,
    /// File dialogs run in the real window. Frame tests pass a path instead.
    native_dialogs: bool,
}

/// A select-tool press that has not yet crossed the drag threshold, then one
/// of the gestures that follow. `previous` is the selection at the press, so
/// Escape can put it back. Transforms preview from `before` and record once.
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

impl SketchApp {
    pub(crate) fn new(creation: &eframe::CreationContext<'_>) -> Self {
        // Safety: `on_exit` disconnects before eframe closes the display.
        let tablet = unsafe { Tablet::connect(creation) };
        let mut app = Self::with_tablet(&creation.egui_ctx, tablet);
        app.native_dialogs = true;
        if let Some(dir) = sketch::prefs::data_dir() {
            app.session.set_data_dir(&dir);
            let (tools, recent) = sketch::prefs::load_dir(&dir);
            app.session.restore_tools(tools);
            app.session.set_recent(recent);
        }
        app.chrome.set_recoveries(app.session.recovery_offers());
        install_panic_hook();
        app.saves_prefs = true;
        app
    }

    fn with_tablet(ctx: &egui::Context, tablet: Tablet) -> Self {
        toolbar::install_fonts(ctx);
        // The window's own chrome is light. The title bar is left to the
        // desktop: egui would otherwise copy its light theme onto the window,
        // and with no theme set, winit's Wayland frame reads the desktop's
        // color scheme while other platforms draw their own.
        ctx.options_mut(|options| options.sync_window_theme = false);
        ctx.set_theme(egui::Theme::Light);
        Self {
            session: Session::new(),
            saves_prefs: false,
            chrome: tabs::live::Chrome::new(),
            view_ready: false,
            center_new: false,
            title: String::new(),
            viewport: [0.0, 0.0],
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
            text_cache: sketch::text::Cache::new(),
            canvas_rect: Rect::NOTHING,
            draft_fit: None,
            copy_press: false,
            cut_press: false,
            paste_press: false,
            native_dialogs: false,
        }
    }

    fn end_stroke(&mut self) {
        self.stroking = false;
        self.pen_stroke = false;
        self.stroke_pressure = None;
        if let Some(tool) = self.restore_tool.take()
            && self.session.tool() == Tool::Eraser
        {
            self.session.set_tool(tool);
        }
    }

    fn persist_tools(&mut self) {
        if self.saves_prefs
            && self.session.prefs_dirty()
            && let Some(dir) = self.session.data_dir()
            && sketch::prefs::save_tools(dir, &self.session.tool_snapshot())
        {
            self.session.clear_prefs_dirty();
        }
    }

    fn resolve_dialogs(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if !self.native_dialogs {
            return;
        }
        let Some(intent) = self.session.take_file_intent() else {
            return;
        };
        match intent {
            sketch::session::FileIntent::Open => {
                let start = self.session.dialog_directory();
                if let Some(path) = crate::dialogs::pick_open(frame, start.as_deref()) {
                    self.session.note_dialog_folder(&path);
                    self.session.open_path(&path);
                }
            }
            sketch::session::FileIntent::Save {
                id,
                copy,
                close_after,
            } => {
                let start = self.session.dialog_directory();
                let name = self.session.suggested_save_name(id);
                let Some(path) = crate::dialogs::pick_save(frame, start.as_deref(), &name) else {
                    if close_after {
                        self.chrome.cancel_save_as();
                    }
                    return;
                };
                self.session.note_dialog_folder(&path);
                match self.session.save_path(id, &path, copy, false) {
                    sketch::session::SaveResult::Wrote => {
                        if close_after && self.chrome.complete_save(ctx, &mut self.session, id) {
                            self.center_new = true;
                        }
                    }
                    sketch::session::SaveResult::NeedsReplace(path) => {
                        self.chrome.ask_replace(id, path, copy, close_after);
                    }
                    sketch::session::SaveResult::Failed => {
                        if close_after {
                            self.chrome.cancel_save_as();
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn handle_keys(&mut self, ctx: &Context) {
        // A name, the size field, or text being typed owns these keys.
        // The press is still noted, so the key-up after editing ends does not
        // copy the objects.
        let editing = ctx.text_edit_focused() || self.session.text_editing();
        let clipboard = self.clipboard_commands(ctx, !editing);
        if ctx.input(|input| input.key_pressed(Key::Escape)) {
            if self.session.text_placing() {
                let _ = self.session.cancel();
                self.text_press = None;
                return;
            }
            if self.session.text_editing() {
                self.session.commit_text();
                self.editor_rect = None;
                return;
            }
            if self.shaping {
                let _ = self.session.cancel();
                self.shaping = false;
            } else {
                self.escape_select();
            }
        }
        if editing {
            return;
        }
        let actions: Vec<Action> = ctx.input(|input| {
            registry::bindings()
                .into_iter()
                .filter(|binding| chord_pressed(input, &binding.chord))
                .map(|binding| binding.action)
                .collect()
        });
        for id in &clipboard {
            self.run_command(ctx, *id);
        }
        for action in actions {
            // The window never delivers the key-down for these three. When a
            // frame does, the clipboard path above already ran them.
            // Ctrl+S and Ctrl+O are not replaced: egui-winit delivers the
            // key-down, and the chord check runs save and open.
            if let Action::Command(id) = action
                && matches!(id, CommandId::Copy | CommandId::Cut | CommandId::Paste)
                && clipboard.contains(&id)
            {
                continue;
            }
            match action {
                Action::Tool(tool) => self.session.set_tool(tool),
                Action::Command(CommandId::Close) => {
                    let id = self.session.active_id();
                    if matches!(
                        self.chrome.close(&mut self.session, id),
                        CloseEffect::Closed { replacement: true }
                    ) {
                        self.center_new = true;
                    }
                }
                Action::Command(id) => self.run_command(ctx, id),
            }
        }
    }

    /// egui-winit replaces Ctrl/Cmd+C, X, and V with copy, cut, and paste
    /// events and drops the key-down, so the chord check never sees them.
    /// Paste is left out entirely when the operating-system clipboard is
    /// empty. The key-up still arrives. Run the in-app command from the
    /// event, or from that key-up when the event never came. `run` is false
    /// while a text field owns the keys.
    fn clipboard_commands(&mut self, ctx: &Context, run: bool) -> Vec<CommandId> {
        let frame = ctx.input(clipboard_frame);
        let mut commands = Vec::new();
        take_clipboard(
            &mut commands,
            CommandId::Copy,
            frame.copy,
            frame.copy_up,
            &mut self.copy_press,
            run,
        );
        take_clipboard(
            &mut commands,
            CommandId::Cut,
            frame.cut,
            frame.cut_up,
            &mut self.cut_press,
            run,
        );
        take_clipboard(
            &mut commands,
            CommandId::Paste,
            frame.paste,
            frame.paste_up,
            &mut self.paste_press,
            run,
        );
        commands
    }

    fn run_command(&mut self, ctx: &Context, id: CommandId) {
        let (maximized, shift) = ctx.input(|input| {
            (
                input.viewport().maximized.unwrap_or(false),
                input.modifiers.shift,
            )
        });
        match self.session.perform_shifted(id, self.viewport, shift) {
            CommandEffect::CenterView => self.center_new = true,
            CommandEffect::Closed { replacement: true } => self.center_new = true,
            CommandEffect::ToggleMaximize => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            CommandEffect::None | CommandEffect::Closed { replacement: false } => {}
        }
    }

    /// `hovered` is egui's answer to whether the canvas itself is under the
    /// pointer. The sketch menu and the keep-this-sketch dialog float over the
    /// canvas rectangle, and a press on them must not start a stroke beneath.
    /// `dismissing` means a menu was open when this frame began: the press
    /// that closes it does nothing else.
    fn handle_pointer(&mut self, ui: &mut egui::Ui, rect: Rect, hovered: bool, dismissing: bool) {
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
            && self.session.tool() == Tool::Text;
        if self.selecting.is_some() && self.session.tool() != Tool::Select && !text_frame {
            self.revert_select();
        }
        // The text tool's side handles, before a press that would place or
        // edit text. They work while the text is being typed.
        if primary_pressed
            && press_inside
            && !space
            && self.pan.is_none()
            && self.selecting.is_none()
            && !self.session.text_placing()
            && let Some(pos) = pos
        {
            self.selecting = self.begin_text_frame(rect, pos);
        }

        if middle_pressed
            && press_inside
            && self.pan.is_none()
            && !self.stroking
            && !self.shaping
            && !self.session.text_placing()
        {
            self.pan = Some(PanButton::Middle);
        }
        if primary_pressed
            && press_inside
            && self.pan.is_none()
            && !self.stroking
            && !self.shaping
            && self.selecting.is_none()
            && !self.session.text_placing()
            && !self.session.text_editing()
        {
            if space && !self.session.text_editing() {
                self.pan = Some(PanButton::Primary);
            } else if self.session.tool() == Tool::Select {
                self.begin_select(ui, rect);
            } else {
                if self.tablet.input().eraser_pressed() && self.session.tool() != Tool::Eraser {
                    self.restore_tool = Some(self.session.tool());
                    self.session.set_tool(Tool::Eraser);
                }
                if matches!(self.session.tool(), Tool::Brush | Tool::Eraser) {
                    self.stroke_pressure = None;
                    self.pen_stroke = self.tablet.input().down();
                    self.capture_force(ui);
                    if self.session.begin().is_ok() {
                        self.stroking = true;
                        self.sample_pointer(ui, rect);
                    } else {
                        self.end_stroke();
                    }
                } else if matches!(
                    self.session.tool(),
                    Tool::Rectangle | Tool::Ellipse | Tool::Line
                ) && let Some(pos) = ui.input(|input| input.pointer.latest_pos())
                    && self.session.begin_shape(self.doc_at(rect, pos)).is_ok()
                {
                    self.shaping = true;
                } else if self.session.tool() == Tool::Text
                    && let Some(pos) = ui.input(|input| input.pointer.latest_pos())
                    && self.session.begin_text(self.doc_at(rect, pos)).is_ok()
                {
                    self.text_press = Some(pos);
                }
            }
        }

        if let Some(button) = self.pan {
            if delta.x != 0.0 || delta.y != 0.0 {
                self.session.pan_screen([delta.x as f64, delta.y as f64]);
            }
            let released = match button {
                PanButton::Primary => primary_released || !primary_down,
                PanButton::Middle => middle_released || !middle_down,
            };
            if released {
                self.pan = None;
            }
        } else if self.selecting.is_some() {
            self.update_select(ui, rect, primary_released || !primary_down);
        } else if self.stroking {
            self.capture_force(ui);
            self.sample_pointer(ui, rect);
            if primary_released || !primary_down {
                let _ = self.session.commit();
                self.end_stroke();
            }
        } else if self.shaping {
            let shift = ui.input(|input| input.modifiers.shift);
            if let Some(pos) = ui.input(|input| input.pointer.latest_pos()) {
                let _ = self.session.move_shape(self.doc_at(rect, pos), shift);
            }
            if primary_released || !primary_down {
                let _ = self.session.commit();
                self.shaping = false;
            }
        } else if self.session.text_placing() {
            if let Some(pos) = ui.input(|input| input.pointer.latest_pos()) {
                let _ = self.session.move_text(self.doc_at(rect, pos));
            }
            if primary_released || !primary_down {
                let dragged = self.text_press.is_some_and(|start| {
                    ui.input(|input| input.pointer.latest_pos())
                        .is_some_and(|pos| start.distance(pos) >= 3.0)
                });
                let _ = self.session.end_text(dragged);
                self.text_press = None;
            }
        }

        self.handle_scroll(ui, rect, hovered);
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

    fn sample_pointer(&mut self, ui: &egui::Ui, rect: Rect) {
        if self.pen_stroke {
            let origin = self.session.camera_origin();
            let zoom = self.session.zoom();
            for sample in self.tablet.input().samples() {
                let point = crate::grid::screen_to_document(rect, origin, zoom, sample.pos);
                let _ = self.session.add_sample(Sample {
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
        let origin = self.session.camera_origin();
        let zoom = self.session.zoom();
        for pos in positions {
            let point = crate::grid::screen_to_document(rect, origin, zoom, pos);
            let _ = self.session.add_sample(Sample {
                x: point[0],
                y: point[1],
                pressure: self.stroke_pressure,
            });
        }
    }

    fn handle_scroll(&mut self, ui: &egui::Ui, rect: Rect, hovered: bool) {
        let hover = ui.input(|input| input.pointer.hover_pos());
        let inside = (hovered && hover.is_some_and(|pos| rect.contains(pos)))
            || self.stroking
            || self.shaping
            || self.session.text_placing()
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
                    self.session.zoom_toward(
                        factor,
                        [
                            f64::from(pointer.x - rect.min.x),
                            f64::from(pointer.y - rect.min.y),
                        ],
                    );
                }
            } else if modifiers.shift {
                self.session.pan_screen([points_x + points_y, 0.0]);
            } else {
                self.session.pan_screen([points_x, points_y]);
            }
        }
    }

    /// With the select tool, a right-click on an object opens its menu, and
    /// a double-click on text edits it in place.
    fn select_clicks(&mut self, response: &egui::Response, rect: Rect) {
        if self.session.tool() != Tool::Select || self.session.text_editing() {
            return;
        }
        if response.secondary_clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.menu_target = self.session.pick_for_menu(self.doc_at(rect, pos));
        }
        if let Some(node) = self.menu_target {
            let has_page = self.session.object_page_available(node);
            if let Some(action) =
                widgets::action_menu(response, registry::object_menu(), |_| has_page)
            {
                self.session.object_action(node, action);
            }
        }
        if response.double_clicked()
            && let Some(pos) = response.interact_pointer_pos()
            && let Some(id) = self.session.hit_test(self.doc_at(rect, pos))
        {
            self.session.edit_text(id);
        }
    }

    /// A press on the canvas outside the caret ends the edit. A press on the
    /// panel, toolbar, or a menu does not, so a color's switch changes the
    /// text being typed; nor does one on the text tool's side handles.
    pub(crate) fn commit_text_on_outside_press(&mut self, ui: &egui::Ui) {
        let pressed = ui.input(|input| input.pointer.button_pressed(PointerButton::Primary));
        let pos = ui.input(|input| input.pointer.interact_pos());
        if pressed
            && let (Some(editor), Some(pos)) = (self.editor_rect, pos)
            && !editor.contains(pos)
            && self.canvas_rect.contains(pos)
            && !egui::Popup::is_any_open(ui.ctx())
            && self.text_side_hit(self.canvas_rect, pos).is_none()
        {
            self.session.commit_text();
            self.editor_rect = None;
        }
    }

    fn show_text_editor(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let Some(caret) = self.session.text_caret() else {
            return;
        };
        let zoom = self.session.zoom();
        let camera = self.session.camera_origin();
        // The same scale the canvas draws this text at, so editing it does
        // not change its size on screen.
        let glyph = sketch::text::glyph_scale(caret.world);
        let across = sketch::text::horizontal_scale(caret.world);
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
        let preview = sketch::text::galley(
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
        let editor = sketch::text::editor_box(
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
        let generation = self.session.text_generation();
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
                            sketch::text::galley(
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
            self.session.set_text_draft(buffer);
        }
    }

    fn paint_text_drag(&self, painter: &egui::Painter, ui: &egui::Ui, rect: Rect) {
        let Some((origin, _, line)) = self.session.text_drag() else {
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
        let doc = self.doc_at(rect, pos);
        let width = (doc[0] - origin[0]).abs();
        if width < 1.0 {
            return;
        }
        let zoom = self.session.zoom();
        let camera = self.session.camera_origin();
        let min = crate::grid::document_to_screen(rect, camera, zoom, origin);
        let max = crate::grid::document_to_screen(rect, camera, zoom, [origin[0] + width, origin[1] + line]);
        painter.rect_stroke(
            Rect::from_min_max(min, max),
            0.0,
            Stroke::new(1.0, Color32::from_rgb(186, 188, 192)),
            egui::StrokeKind::Outside,
        );
    }

    fn paint(&mut self, ui: &egui::Ui, rect: Rect) {
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect_filled(rect, 0.0, CANVAS);
        let origin = self.session.camera_origin();
        let zoom = self.session.zoom();
        crate::grid::paint_grid(&painter, rect, origin, zoom);
        let caret = self.session.text_caret();
        let editing = caret.as_ref().and_then(|caret| caret.id);
        let marks = self.session.scene();
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
                    }
                    let laid = sketch::text::lay(
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
                    self.session.set_text_metrics(
                        view.id,
                        laid.local_width,
                        laid.local_height,
                        laid.natural_height,
                    );
                    if editing == Some(view.id) {
                        sketch::text::paint_box(
                            &painter,
                            rect,
                            origin,
                            zoom,
                            &sketch::text::Boxed::of(&view),
                            laid.local_width,
                            laid.local_height,
                        );
                    } else {
                        sketch::text::paint(&painter, rect, origin, zoom, &view, &laid);
                    }
                }
            }
        }
        self.draft_fit = None;
        // New text is not in the document yet. Its background is drawn here,
        // under the caret, from the text tool's palette.
        if let Some(caret) = caret.filter(|caret| caret.id.is_none()) {
            let laid = sketch::text::lay(
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
            sketch::text::paint_box(
                &painter,
                rect,
                origin,
                zoom,
                &sketch::text::Boxed {
                    look: &caret.look,
                    radius: caret.style.radius,
                    origin: caret.origin,
                    world: caret.world,
                },
                laid.local_width,
                laid.local_height,
            );
            if let Some(frame) = self.session.draft_box() {
                self.draft_fit = Some(TextFit {
                    frame,
                    width: laid.local_width,
                    height: laid.local_height,
                    natural: laid.natural_height,
                });
            }
        }
        self.paint_text_drag(&painter, ui, rect);
        self.paint_selection(&painter, rect);
    }

    fn doc_at(&self, rect: Rect, pos: Pos2) -> Point {
        crate::grid::screen_to_document(rect, self.session.camera_origin(), self.session.zoom(), pos)
    }

    fn screen_at(&self, rect: Rect, point: Point) -> Pos2 {
        crate::grid::document_to_screen(
            rect,
            self.session.camera_origin(),
            self.session.zoom(),
            point,
        )
    }

    fn begin_select(&mut self, ui: &egui::Ui, rect: Rect) {
        let Some(pos) = ui.input(|input| input.pointer.latest_pos()) else {
            return;
        };
        let doc = self.doc_at(rect, pos);
        let previous = self.session.selection();
        let shift = ui.input(|input| input.modifiers.shift);
        if !shift && let Some(handle) = self.hit_handle(rect, pos) {
            let Some((min, max)) = self.session.selection_bounds() else {
                return;
            };
            let before = self.session.changeable_locals();
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
                        start: angle_of(doc, center),
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
                    if let Some(id) = self.session.sole_text()
                        && let Some(before) = self.session.text_box(id) =>
                {
                    SelectGesture::TextFrame {
                        target: TextTarget::Node(id),
                        before,
                        start: doc,
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
        let hit = self.session.hit_test(doc);
        let inside = self.inside_selection(doc);
        if !shift
            && let Some(id) = hit
            && !previous.contains(&id)
        {
            self.session.select_click(id, false);
        }
        self.selecting = Some(SelectGesture::Press {
            screen: pos,
            anchor: doc,
            hit,
            inside,
            before: self.session.changeable_locals(),
            previous,
        });
    }

    fn update_select(&mut self, ui: &egui::Ui, rect: Rect, released: bool) {
        let Some(pos) = ui.input(|input| input.pointer.latest_pos()) else {
            if released {
                self.revert_select();
            }
            return;
        };
        let doc = self.doc_at(rect, pos);
        let shift = ui.input(|input| input.modifiers.shift);
        let Some(gesture) = self.selecting.take() else {
            return;
        };
        self.selecting = self.advance_select(gesture, pos, doc, shift, released);
    }

    fn advance_select(
        &mut self,
        gesture: SelectGesture,
        pos: Pos2,
        doc: Point,
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
                        self.finish_click(hit, shift);
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
                        current: doc,
                        shift,
                        previous: previous.clone(),
                    };
                    self.apply_marquee(&gesture);
                    if released { None } else { Some(gesture) }
                } else if before.is_empty() {
                    if released {
                        self.finish_click(hit, shift);
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
                    self.preview_move(&before, anchor, doc);
                    if released {
                        self.session.commit_preview(&before);
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
                    self.session.commit_preview(&before);
                    None
                } else {
                    self.preview_move(&before, anchor, doc);
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
                        self.session.preview_text_box(id, before, start, doc, side);
                    }
                    TextTarget::Draft => self.session.preview_draft_box(before, start, doc, side),
                }
                if released {
                    if let TextTarget::Node(id) = target {
                        self.session.commit_text_box(id, before.frame);
                    }
                    self.refocus_text();
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
                    self.session.commit_preview(&before);
                    None
                } else {
                    self.preview_scale(&before, opposite, handle, axes, doc, shift);
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
                    self.session.commit_preview(&before);
                    None
                } else {
                    self.preview_rotate(&before, center, start, doc, shift);
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
                    current: doc,
                    shift,
                    previous,
                };
                self.apply_marquee(&gesture);
                if released { None } else { Some(gesture) }
            }
        }
    }

    fn finish_click(&mut self, hit: Option<NodeId>, shift: bool) {
        if shift {
            if let Some(id) = hit {
                self.session.select_click(id, true);
            }
        } else if let Some(id) = hit {
            self.session.select_click(id, false);
        } else {
            self.session.clear_selection();
        }
    }

    fn preview_move(&mut self, before: &[(NodeId, Affine)], anchor: Point, doc: Point) {
        let delta = Vec2::new(doc[0] - anchor[0], doc[1] - anchor[1]);
        self.session
            .preview_world(before, |world| Affine::translate(delta) * world);
    }

    fn preview_scale(
        &mut self,
        before: &[(NodeId, Affine)],
        opposite: Point,
        handle: Point,
        axes: [bool; 2],
        doc: Point,
        shift: bool,
    ) {
        let along = |axis: usize| {
            if axes[axis] {
                axis_scale(handle[axis], opposite[axis], doc[axis])
            } else {
                Some(1.0)
            }
        };
        let (Some(sx), Some(sy)) = (along(0), along(1)) else {
            self.session.restore_locals(before);
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
            self.session.restore_locals(before);
            return;
        }
        let origin = kurbo::Point::new(opposite[0], opposite[1]);
        let map = scale_about(sx, sy, origin);
        self.session.preview_world(before, |world| map * world);
    }

    fn preview_rotate(
        &mut self,
        before: &[(NodeId, Affine)],
        center: Point,
        start: f64,
        doc: Point,
        shift: bool,
    ) {
        let mut delta = angle_of(doc, center) - start;
        if shift {
            let step = std::f64::consts::PI / 12.0;
            delta = (delta / step).round() * step;
        }
        if !delta.is_finite() {
            self.session.restore_locals(before);
            return;
        }
        let origin = kurbo::Point::new(center[0], center[1]);
        self.session
            .preview_world(before, |world| Affine::rotate_about(delta, origin) * world);
    }

    fn apply_marquee(&mut self, gesture: &SelectGesture) {
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
        self.session.marquee_select(min, max, *shift, base);
    }

    /// Escape cancels a drag, or lets go of the selection. With the text
    /// tool that hands the toolbar back to the next text's settings.
    fn escape_select(&mut self) {
        if self.selecting.is_some() {
            self.revert_select();
        } else if matches!(self.session.tool(), Tool::Select | Tool::Text) {
            self.session.clear_selection();
        }
    }

    fn revert_select(&mut self) {
        let Some(gesture) = self.selecting.take() else {
            return;
        };
        match gesture {
            SelectGesture::Press { previous, .. } => {
                self.session.replace_selection(previous);
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
                self.session.restore_locals(&before);
                self.session.replace_selection(previous);
            }
            SelectGesture::Marquee { previous, .. } => {
                self.session.replace_selection(previous);
            }
            SelectGesture::TextFrame {
                target,
                before,
                previous,
                ..
            } => {
                match target {
                    TextTarget::Node(id) => self.session.restore_text_box(id, before.frame),
                    TextTarget::Draft => self.session.set_draft_box(before.frame),
                }
                self.session.replace_selection(previous);
                self.refocus_text();
            }
        }
    }

    fn inside_selection(&self, doc: Point) -> bool {
        let Some((min, max)) = self.session.selection_bounds() else {
            return false;
        };
        doc[0] >= min[0] && doc[0] <= max[0] && doc[1] >= min[1] && doc[1] <= max[1]
    }

    fn hit_handle(&self, rect: Rect, pos: Pos2) -> Option<HandleHit> {
        let frame = self.selection_frame(rect)?;
        if self.session.selection_turns() && pos.distance(rotate_dot(frame)) <= HANDLE_REACH {
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
    fn select_cursor(&self, ui: &egui::Ui, rect: Rect) -> CursorIcon {
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
        match self.hit_handle(rect, pos) {
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
            None if self.inside_selection(self.doc_at(rect, pos)) => CursorIcon::Move,
            None => CursorIcon::Default,
        }
    }

    fn selection_frame(&self, rect: Rect) -> Option<Rect> {
        let (min, max) = self.session.selection_bounds()?;
        Some(Rect::from_two_pos(
            self.screen_at(rect, min),
            self.screen_at(rect, max),
        ))
    }

    fn paint_selection(&self, painter: &egui::Painter, rect: Rect) {
        if let Some(SelectGesture::Marquee {
            anchor, current, ..
        }) = &self.selecting
        {
            let a = self.screen_at(rect, *anchor);
            let b = self.screen_at(rect, *current);
            let mark = Rect::from_two_pos(a, b);
            painter.rect_filled(mark, 0.0, Color32::from_rgba_unmultiplied(40, 42, 46, 18));
            painter.rect_stroke(
                mark,
                0.0,
                Stroke::new(1.0, widgets::INK),
                egui::StrokeKind::Inside,
            );
        }
        let frame = if self.session.tool() == Tool::Select {
            self.selection_frame(rect)
        } else {
            self.outline_frame(rect)
        };
        let Some(frame) = frame else {
            return;
        };
        if frame.width() < 1.0 && frame.height() < 1.0 {
            return;
        }
        match self.session.tool() {
            Tool::Select => {
                let line = Stroke::new(1.5, widgets::INK);
                painter.rect_stroke(frame, 0.0, line, egui::StrokeKind::Middle);
                if self.session.selection_turns() {
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
            tool => {
                paint_dashed_rect(
                    painter,
                    frame,
                    Color32::from_rgba_unmultiplied(40, 42, 46, 110),
                );
                if tool == Tool::Text && self.session.text_box_handles() {
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
    fn outline_frame(&self, rect: Rect) -> Option<Rect> {
        let frame = match self.session.draft_box() {
            Some(_) if self.session.tool() == Tool::Text => {
                let fit = self.draft_fit?;
                let origin = fit.frame.origin;
                Rect::from_two_pos(
                    self.screen_at(rect, origin),
                    self.screen_at(rect, [origin[0] + fit.width, origin[1] + fit.height]),
                )
            }
            _ => self.selection_frame(rect)?,
        };
        Some(frame.expand(OUTLINE_OUTSET))
    }

    /// Which of the text tool's edge handles is under `pos`.
    fn text_side_hit(&self, rect: Rect, pos: Pos2) -> Option<BoxSide> {
        if !self.session.text_box_handles() {
            return None;
        }
        self.session.text_target()?;
        let frame = self.outline_frame(rect)?;
        frame_edges(frame)
            .iter()
            .position(|point| pos.distance(*point) <= HANDLE_REACH)
            .map(|index| EDGE_SIDES[index])
    }

    /// A press on one of the text tool's edge handles starts a drag on its
    /// text's box.
    fn begin_text_frame(&mut self, rect: Rect, pos: Pos2) -> Option<SelectGesture> {
        let side = self.text_side_hit(rect, pos)?;
        let target = self.session.text_target()?;
        let before = match target {
            TextTarget::Draft => self.draft_fit?,
            TextTarget::Node(id) => self.session.text_box(id)?,
        };
        Some(SelectGesture::TextFrame {
            target,
            before,
            start: self.doc_at(rect, pos),
            side,
            previous: self.session.selection(),
        })
    }

    /// The text tool's pointer: the resize arrow over an edge handle and
    /// while dragging one, and the text beam elsewhere.
    fn text_cursor(&self, ui: &egui::Ui, rect: Rect) -> CursorIcon {
        if let Some(SelectGesture::TextFrame { side, .. }) = &self.selecting {
            return side_cursor(*side);
        }
        match ui
            .input(|input| input.pointer.hover_pos())
            .and_then(|pos| self.text_side_hit(rect, pos))
        {
            Some(side) => side_cursor(side),
            None => CursorIcon::Text,
        }
    }

    /// Gives the caret its focus back after a side handle's drag took it.
    fn refocus_text(&mut self) {
        self.text_focus = self.session.text_generation().wrapping_sub(1);
    }
}

impl SketchApp {
    pub(crate) fn session(&mut self) -> &mut Session {
        &mut self.session
    }

    pub(crate) fn session_ref(&self) -> &Session {
        &self.session
    }

    pub(crate) fn viewport(&self) -> [f64; 2] {
        self.viewport
    }

    /// The next contents pass centers the sketch, as a new tab does.
    pub(crate) fn center_next_view(&mut self) {
        self.center_new = true;
    }

    /// A document replaced the camera. Center a blank again once the
    /// contents rectangle is known.
    pub(crate) fn forget_presented_view(&mut self) {
        self.view_ready = false;
    }

    /// Pointer, pen, and paint for one contents rectangle. The host window
    /// draws the shell around this. The in-process canvas tests call it
    /// from `ui` after their own chrome.
    pub(crate) fn drive(&mut self, ui: &mut egui::Ui, rect: Rect, dismissing: bool) {
        self.canvas_rect = rect;
        self.viewport = [f64::from(rect.width()), f64::from(rect.height())];
        if !self.view_ready && self.viewport[0] > 1.0 && self.viewport[1] > 1.0 {
            self.session.present_view(self.viewport);
            self.view_ready = true;
        }
        if self.center_new && self.viewport[0] > 1.0 && self.viewport[1] > 1.0 {
            self.session.present_view(self.viewport);
            self.center_new = false;
        }
        if self.stroking && !self.session.gesture_open() {
            self.end_stroke();
        }
        if self.shaping && !self.session.gesture_open() {
            self.shaping = false;
        }
        if self.text_press.is_some() && !self.session.text_placing() {
            self.text_press = None;
        }

        let response = ui.interact(rect, ui.id().with("canvas"), Sense::click_and_drag());
        let hovered = response.hovered();
        let space = ui.input(|input| input.key_down(Key::Space));
        let editing_before = self.session.text_editing();
        if editing_before {
            self.show_text_editor(ui, rect);
        }

        self.handle_pointer(ui, rect, hovered, dismissing);
        self.select_clicks(&response, rect);
        if self.session.text_editing() && !editing_before {
            self.show_text_editor(ui, rect);
        }
        if !self.session.text_editing() {
            self.editor_rect = None;
        }
        if self.pan.is_some() {
            response.on_hover_cursor(CursorIcon::Grabbing);
        } else if space && !self.session.text_editing() {
            response.on_hover_cursor(CursorIcon::Grab);
        } else if self.session.tool() == Tool::Select {
            response.on_hover_cursor(self.select_cursor(ui, rect));
        } else if self.session.tool() == Tool::Text {
            response.on_hover_cursor(self.text_cursor(ui, rect));
        } else {
            response.on_hover_cursor(CursorIcon::Crosshair);
        }
        self.paint(ui, rect);
        self.persist_tools();
    }

    pub(crate) fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.tablet.feed(ctx, raw_input);
    }

    pub(crate) fn on_exit(&mut self) {
        self.session.finish_size_edit();
        self.persist_tools();
        self.tablet.disconnect();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        CANVAS.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.commit_text_on_outside_press(ui);
        // Read before the tab bar runs: a click outside the menu closes it there.
        let dismissing = egui::Popup::is_any_open(ui.ctx());
        let mut visuals = egui::Visuals::light();
        visuals.panel_fill = Color32::WHITE;
        ui.ctx().set_visuals(visuals);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                if tabs::live::show(ui, &mut self.session, &mut self.chrome) {
                    self.center_new = true;
                }
                self.session.set_transform_preview(self.selecting.is_some());
                if toolbar::show_session(ui, &mut self.session, self.viewport) {
                    self.center_new = true;
                }

                let full = ui.available_rect_before_wrap();
                // F4 can open the panel. Split after the keys so that frame
                // gives the panel its width and narrows the canvas.
                self.handle_keys(ui.ctx());
                self.resolve_dialogs(ui.ctx(), frame);
                self.session.poll_recovery(std::time::Instant::now());
                self.session.publish_emergency();
                let panel_w = if self.session.panel_open() {
                    panel::WIDTH
                } else {
                    0.0
                };
                let right = if panel_w == 0.0 {
                    full.right()
                } else {
                    (full.right() - panel_w).max(full.left())
                };
                let rect = Rect::from_min_max(full.min, egui::pos2(right, full.bottom()));
                self.drive(ui, rect, dismissing);
                // A menu's Properties can open the panel after this frame was
                // split with no room for it. It then waits for the next frame,
                // which gives it its width.
                if self.session.panel_open() && panel_w > 0.0 {
                    let panel_rect =
                        Rect::from_min_max(egui::pos2(rect.right(), full.top()), full.max);
                    panel::page::show(ui, &mut self.session, panel_rect);
                } else if self.session.panel_open() {
                    ui.ctx().request_repaint();
                }
                panel::page::toggle_tab(ui.ctx(), &mut self.session, rect.right(), full.top());
            });
    }
}

fn wheel_scale(unit: MouseWheelUnit, rect: Rect) -> f64 {
    match unit {
        MouseWheelUnit::Point => 1.0,
        MouseWheelUnit::Line => NOTCH_POINTS,
        MouseWheelUnit::Page => f64::from(rect.height()),
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
                pos: crate::grid::document_to_screen(rect, origin, zoom, *point),
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

struct ClipboardFrame {
    copy: bool,
    cut: bool,
    paste: bool,
    copy_up: bool,
    cut_up: bool,
    paste_up: bool,
}

fn clipboard_frame(input: &egui::InputState) -> ClipboardFrame {
    let mut frame = ClipboardFrame {
        copy: false,
        cut: false,
        paste: false,
        copy_up: false,
        cut_up: false,
        paste_up: false,
    };
    for event in &input.events {
        match event {
            Event::Copy => frame.copy = true,
            Event::Cut => frame.cut = true,
            Event::Paste(_) => frame.paste = true,
            Event::Key {
                key,
                pressed: false,
                modifiers,
                ..
            } if modifiers.command => match key {
                Key::C => frame.copy_up = true,
                Key::X => frame.cut_up = true,
                Key::V => frame.paste_up = true,
                _ => {}
            },
            _ => {}
        }
    }
    frame
}

fn take_clipboard(
    commands: &mut Vec<CommandId>,
    id: CommandId,
    press: bool,
    release: bool,
    seen: &mut bool,
    run: bool,
) {
    if press {
        *seen = true;
        if run {
            commands.push(id);
        }
    }
    if release {
        let already = *seen;
        *seen = false;
        if run && !already {
            commands.push(id);
        }
    }
}

fn chord_pressed(input: &egui::InputState, chord: &registry::Chord) -> bool {
    if input.modifiers.command != chord.command || input.modifiers.alt != chord.alt {
        return false;
    }
    if let Some(shift) = chord.shift
        && input.modifiers.shift != shift
    {
        return false;
    }
    input.key_pressed(egui_key(chord.key))
}

fn egui_key(key: registry::Key) -> Key {
    match key {
        registry::Key::B => Key::B,
        registry::Key::C => Key::C,
        registry::Key::E => Key::E,
        registry::Key::G => Key::G,
        registry::Key::L => Key::L,
        registry::Key::N => Key::N,
        registry::Key::O => Key::O,
        registry::Key::R => Key::R,
        registry::Key::S => Key::S,
        registry::Key::T => Key::T,
        registry::Key::V => Key::V,
        registry::Key::W => Key::W,
        registry::Key::X => Key::X,
        registry::Key::Y => Key::Y,
        registry::Key::Z => Key::Z,
        registry::Key::Digit0 => Key::Num0,
        registry::Key::Digit2 => Key::Num2,
        registry::Key::Plus => Key::Plus,
        registry::Key::Equals => Key::Equals,
        registry::Key::Minus => Key::Minus,
        registry::Key::BracketLeft => Key::OpenBracket,
        registry::Key::BracketRight => Key::CloseBracket,
        registry::Key::F4 => Key::F4,
        registry::Key::Delete => Key::Delete,
        registry::Key::Backspace => Key::Backspace,
        registry::Key::Left => Key::ArrowLeft,
        registry::Key::Right => Key::ArrowRight,
        registry::Key::Up => Key::ArrowUp,
        registry::Key::Down => Key::ArrowDown,
    }
}

fn install_panic_hook() {
    use std::sync::Once;
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            sketch::store::flush_emergency();
            previous(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::App;

    const SCREEN: egui::Vec2 = egui::vec2(1280.0, 800.0);

    /// Runs whole frames of the app with synthetic pointer input.
    struct Harness {
        ctx: egui::Context,
        app: SketchApp,
        frame: eframe::Frame,
        /// Every command the frames sent to the native window.
        window_commands: Vec<egui::ViewportCommand>,
        /// The pointer the last frame asked for.
        cursor: CursorIcon,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            let app = SketchApp::with_tablet(&ctx, Tablet::default());
            let mut harness = Self {
                ctx,
                app,
                frame: eframe::Frame::_new_kittest(),
                window_commands: Vec::new(),
                cursor: CursorIcon::Default,
            };
            harness.step(Vec::new());
            harness.step(Vec::new());
            harness
        }

        fn step(&mut self, events: Vec<Event>) {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
                events,
                ..Default::default()
            };
            let Self {
                ctx,
                app,
                frame,
                window_commands,
                cursor,
            } = self;
            let mut output = ctx.run_ui(input, |ui| app.ui(ui, frame));
            *cursor = output.platform_output.cursor_icon;
            // Nothing renders here, so the texture uploads are dropped.
            output.textures_delta.clear();
            for viewport in output.viewport_output.into_values() {
                window_commands.extend(viewport.commands);
            }
        }

        /// Moves to `from`, presses, drags to `to`, and releases, a frame each.
        fn drag(&mut self, from: Pos2, to: Pos2) {
            self.step(vec![Event::PointerMoved(from)]);
            self.step(vec![button(from, true)]);
            self.step(vec![Event::PointerMoved(to)]);
            self.step(vec![button(to, false)]);
            self.step(Vec::new());
        }
    }

    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn right_click(harness: &mut Harness, at: Pos2) {
        let press = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        harness.step(vec![Event::PointerMoved(at)]);
        harness.step(vec![press(true)]);
        harness.step(vec![press(false)]);
        harness.step(Vec::new());
    }

    #[test]
    fn right_clicking_a_swatch_or_a_tool_opens_a_menu_not_the_panel() {
        let mut harness = Harness::new();
        assert!(!harness.app.session.panel_open());
        right_click(&mut harness, swatch_center(0));
        assert!(
            egui::Popup::is_any_open(&harness.ctx),
            "the swatch's menu opened"
        );
        assert!(
            !harness.app.session.panel_open(),
            "the panel waits for Properties"
        );
        harness.step(vec![key_press(Key::Escape)]);
        harness.step(Vec::new());

        // The brush follows the nine swatches, New sketch, Open, and Save,
        // undo and redo, and New canvas, with a divider after each group.
        let brush = Pos2::new(
            12.0 + toolbar::SWATCH_INSET
                + 9.0 * toolbar::SWATCH_W
                + 3.0 * toolbar::SEPARATOR_W
                + 6.5 * toolbar::BUTTON_W,
            40.0 + 5.0 + 20.0,
        );
        right_click(&mut harness, brush);
        assert!(
            egui::Popup::is_any_open(&harness.ctx),
            "the tool's menu opened"
        );
        assert!(!harness.app.session.panel_open());
        assert_eq!(harness.app.session.tool(), Tool::Brush);
    }

    #[test]
    fn select_double_clicks_text_to_edit_it_and_right_clicks_for_properties() {
        let mut harness = Harness::new();
        harness.step(vec![key_press(Key::T)]);
        let at = Pos2::new(400.0, 400.0);
        click(&mut harness, at);
        harness.step(vec![Event::Text("Hello".into())]);
        harness.step(vec![key_press(Key::Escape)]);
        harness.step(Vec::new());
        harness.step(vec![key_press(Key::V)]);
        assert_eq!(harness.app.session.tool(), Tool::Select);
        // Frames are a sixtieth of a second apart: the next two clicks are a
        // fresh double-click.
        for _ in 0..30 {
            harness.step(Vec::new());
        }
        let on_text = at + egui::vec2(8.0, 8.0);
        harness.step(vec![Event::PointerMoved(on_text)]);
        for _ in 0..2 {
            harness.step(vec![button(on_text, true)]);
            harness.step(vec![button(on_text, false)]);
        }
        harness.step(Vec::new());
        assert!(
            harness.app.session.text_editing(),
            "the double-click edits the text"
        );
        assert_eq!(harness.app.session.tool(), Tool::Select);
        harness.step(vec![key_press(Key::Escape)]);
        harness.step(Vec::new());
        assert!(!harness.app.session.text_editing());

        harness.app.session.clear_selection();
        right_click(&mut harness, on_text);
        assert!(
            egui::Popup::is_any_open(&harness.ctx),
            "the object's menu opened"
        );
        assert_eq!(
            harness.app.session.selection().len(),
            1,
            "the right-click picked it"
        );
        assert!(!harness.app.session.panel_open());
    }

    #[test]
    fn properties_from_the_canvas_opens_a_closed_panel_without_a_crash() {
        let mut harness = Harness::new();
        harness.step(vec![key_press(Key::R)]);
        harness.drag(Pos2::new(300.0, 300.0), Pos2::new(500.0, 450.0));
        harness.step(vec![key_press(Key::V)]);
        assert!(!harness.app.session.panel_open());
        let on_outline = Pos2::new(300.0, 380.0);
        right_click(&mut harness, on_outline);
        assert!(egui::Popup::is_any_open(&harness.ctx));
        // The menu's one row, Properties, sits just below and right of the
        // pointer. Choosing it opens the panel in the middle of a frame.
        let row = on_outline + egui::vec2(40.0, 20.0);
        harness.step(vec![Event::PointerMoved(row)]);
        harness.step(vec![button(row, true)]);
        harness.step(vec![button(row, false)]);
        harness.step(Vec::new());
        harness.step(Vec::new());
        assert!(
            harness.app.session.panel_open(),
            "Properties opened the panel"
        );
        assert!(
            !harness.app.session.shows_layers(),
            "on the rectangle's own page"
        );
    }

    #[test]
    fn dragging_texts_side_handle_rewraps_it_instead_of_stretching_it() {
        let mut harness = Harness::new();
        harness.step(vec![key_press(Key::T)]);
        click(&mut harness, Pos2::new(400.0, 400.0));
        harness.step(vec![Event::Text("Hello there".into())]);
        harness.step(vec![key_press(Key::Escape)]);
        harness.step(Vec::new());
        harness.step(vec![key_press(Key::V)]);
        harness.step(Vec::new());
        let id = harness.app.session.selection()[0];
        let local = harness.app.session.changeable_locals();
        let frame = harness.app.selection_frame(canvas_rect(&harness)).unwrap();
        let side = frame.right_center();
        harness.drag(side, side + egui::vec2(120.0, 0.0));
        let width = harness.app.session.text_box(id).unwrap().frame.width;
        assert!(width.is_some(), "the side handle gave the text a box");
        assert_eq!(
            harness.app.session.changeable_locals(),
            local,
            "the text was not stretched"
        );
    }

    #[test]
    fn the_text_tools_side_handle_widens_new_text_while_it_is_typed() {
        let mut harness = Harness::new();
        harness.step(vec![key_press(Key::T)]);
        // A drag places a box, which wraps.
        harness.drag(Pos2::new(300.0, 400.0), Pos2::new(420.0, 400.0));
        harness.step(vec![Event::Text("Hello there, this wraps".into())]);
        harness.step(Vec::new());
        let before = harness
            .app
            .session
            .draft_box()
            .expect("new text is open")
            .width;
        let undo_len = harness.app.session.undo_len();

        let frame = harness.app.outline_frame(canvas_rect(&harness)).unwrap();
        let side = frame.right_center();
        harness.step(vec![Event::PointerMoved(side)]);
        assert_eq!(harness.cursor, CursorIcon::ResizeHorizontal);
        harness.drag(side, side + egui::vec2(240.0, 0.0));
        let after = harness.app.session.draft_box().expect("still typing").width;
        let after = after.unwrap();
        assert!(
            after > before.unwrap() + 1.0,
            "the box widened: {before:?} to {after:?}"
        );
        assert_eq!(harness.app.session.undo_len(), undo_len);
        // The caret's field widens with the box, so the words wrap where
        // the box now ends, not at its old width.
        harness.step(Vec::new());
        let field = harness.app.editor_rect.unwrap().width();
        let box_px = (after * harness.app.session.zoom()) as f32;
        assert!(
            (field - box_px).abs() < 2.0,
            "the field is {field} wide for a {box_px} box"
        );
        let wrapped = harness.app.outline_frame(canvas_rect(&harness)).unwrap();
        assert!(
            wrapped.height() < frame.height() - 1.0,
            "the outline follows the re-wrapped words"
        );

        harness.step(vec![Event::Text("!".into())]);
        harness.step(Vec::new());
        // A click on a handle, too, leaves the words taking keys.
        let frame = harness.app.outline_frame(canvas_rect(&harness)).unwrap();
        click(&mut harness, frame.right_center());
        harness.step(vec![Event::Text("?".into())]);
        harness.step(Vec::new());
        assert_eq!(
            harness.app.session.text_caret().unwrap().content,
            "Hello there, this wraps!?",
            "the caret kept its focus"
        );
    }

    #[test]
    fn the_text_tools_side_handle_sets_a_finished_texts_width_in_one_step() {
        let mut harness = Harness::new();
        harness.step(vec![key_press(Key::T)]);
        click(&mut harness, Pos2::new(300.0, 400.0));
        harness.step(vec![Event::Text("Hello there".into())]);
        harness.step(vec![key_press(Key::Escape)]);
        harness.step(Vec::new());
        let id = harness.app.session.selection()[0];
        assert_eq!(harness.app.session.text_box(id).unwrap().frame.width, None);
        let frame = harness.app.outline_frame(canvas_rect(&harness)).unwrap();
        let undo_len = harness.app.session.undo_len();
        let side = frame.right_center();
        harness.drag(side, side + egui::vec2(-30.0, 0.0));
        assert!(
            harness
                .app
                .session
                .text_box(id)
                .unwrap()
                .frame
                .width
                .is_some()
        );
        assert_eq!(harness.app.session.undo_len(), undo_len + 1);
        assert!(
            !harness.app.session.text_editing(),
            "a handle does not edit"
        );

        // Escape lets go of the text, so the toolbar sets up the next one.
        assert_eq!(harness.app.session.active_text(), Some(id));
        harness.step(vec![key_press(Key::Escape)]);
        assert_eq!(harness.app.session.active_text(), None);
    }

    #[test]
    fn a_toolbar_button_leaves_the_caret_taking_keys() {
        let mut harness = Harness::new();
        harness.step(vec![key_press(Key::T)]);
        click(&mut harness, Pos2::new(300.0, 400.0));
        assert!(harness.app.session.text_editing());
        // Align center, the second alignment button in the text toolbar,
        // after its three swatches.
        click(&mut harness, Pos2::new(871.0, 65.0));
        assert_eq!(
            harness.app.session.bar_value(sketch::schema::PropId::Align),
            Some(sketch::schema::PropValue::Index(1)),
            "the click chose Align center"
        );
        assert!(harness.app.session.text_editing());
        harness.step(vec![Event::Text("Hi".into())]);
        harness.step(Vec::new());
        assert_eq!(harness.app.session.text_caret().unwrap().content, "Hi");
    }

    #[test]
    fn the_caret_sits_where_the_lines_do_in_a_taller_box() {
        let mut harness = Harness::new();
        harness.step(vec![key_press(Key::T)]);
        click(&mut harness, Pos2::new(300.0, 300.0));
        harness.step(vec![Event::Text("Hi".into())]);
        harness.step(Vec::new());
        let top = harness.app.editor_rect.unwrap().top();
        let fit = harness.app.draft_fit.unwrap();
        // The bottom handle makes the box 100 units taller than its lines.
        let frame = harness.app.outline_frame(canvas_rect(&harness)).unwrap();
        let zoom = harness.app.session.zoom() as f32;
        let bottom = frame.center_bottom();
        harness.drag(bottom, bottom + egui::vec2(0.0, 100.0 * zoom));
        let tall = harness.app.session.draft_box().unwrap().height.unwrap();
        assert!((tall - (fit.height + 100.0)).abs() < 0.5, "height {tall}");
        assert_eq!(harness.app.editor_rect.unwrap().top(), top, "top stays");

        assert!(harness.app.session.set_bar_value(
            sketch::schema::PropId::VAlign,
            sketch::schema::PropValue::Index(2),
            true,
        ));
        harness.step(Vec::new());
        harness.step(Vec::new());
        let moved = harness.app.editor_rect.unwrap().top() - top;
        assert!(
            (moved - 100.0 * zoom).abs() < 1.0,
            "the caret moved {moved} down with the lines"
        );
        assert_eq!(harness.app.session.text_caret().unwrap().content, "Hi");
    }

    #[test]
    fn a_press_on_the_panel_leaves_the_text_being_typed_open() {
        let mut harness = Harness::new();
        harness.step(vec![key_press(Key::T)]);
        click(&mut harness, Pos2::new(300.0, 400.0));
        harness.step(vec![Event::Text("Hi".into())]);
        harness.app.session.show_panel();
        harness.step(Vec::new());
        assert!(harness.app.session.panel_open());
        let panel = Pos2::new(canvas_rect(&harness).right() + 100.0, 760.0);
        click(&mut harness, panel);
        assert!(
            harness.app.session.text_editing(),
            "a press on the panel does not end the edit"
        );
        // A press on the canvas ends it, and this one starts new text.
        click(&mut harness, Pos2::new(700.0, 600.0));
        assert_eq!(
            harness.app.session.text_caret().map(|caret| caret.content),
            Some(String::new())
        );
    }

    /// The center of palette swatch `index` in the toolbar: the tab bar is 40
    /// points tall, the toolbar pads 12 at the left and 5 at the top, and its
    /// 40-point row holds swatches `SWATCH_W` wide.
    fn swatch_center(index: usize) -> Pos2 {
        Pos2::new(
            12.0 + toolbar::SWATCH_INSET + toolbar::SWATCH_W * (index as f32 + 0.5),
            40.0 + 5.0 + 20.0,
        )
    }

    #[test]
    fn dragging_a_brush_swatch_sorts_the_brush_palette_without_history() {
        let mut harness = Harness::new();
        let black = harness.app.session.swatches()[0].id;
        let red = harness.app.session.swatches()[1].id;
        let undo_len = harness.app.session.undo_len();

        harness.drag(swatch_center(1), swatch_center(0) - egui::vec2(12.0, 0.0));
        let order: Vec<_> = harness
            .app
            .session
            .swatches()
            .iter()
            .map(|swatch| swatch.id)
            .collect();
        assert_eq!(&order[..2], &[red, black]);
        assert_eq!(
            harness.app.session.undo_len(),
            undo_len,
            "the brush's palette is a tool setting"
        );
        assert!(
            harness.app.session.swatches()[1].active,
            "a drag does not pick a color"
        );

        let orange = harness.app.session.swatches()[2].id;
        let at = swatch_center(2);
        harness.step(vec![Event::PointerMoved(at)]);
        harness.step(vec![button(at, true)]);
        harness.step(vec![button(at, false)]);
        harness.step(Vec::new());
        let active = harness.app.session.swatches();
        assert!(
            active
                .iter()
                .any(|swatch| swatch.id == orange && swatch.active)
        );
        assert_eq!(
            harness.app.session.undo_len(),
            undo_len,
            "a click is not a move"
        );
    }

    #[test]
    fn the_title_bar_is_left_to_the_desktop_theme() {
        let mut harness = Harness::new();
        harness.drag(Pos2::new(400.0, 400.0), Pos2::new(480.0, 440.0));
        let themed = harness
            .window_commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::SetTheme(_)));
        assert!(!themed, "a frame set the window theme");
    }

    #[test]
    fn a_drag_on_the_canvas_draws_a_stroke() {
        let mut harness = Harness::new();
        harness.drag(Pos2::new(400.0, 400.0), Pos2::new(480.0, 440.0));
        assert_eq!(harness.app.session.stroke_count(), 1);
    }

    #[test]
    fn the_press_that_closes_the_menu_draws_nothing() {
        let mut harness = Harness::new();
        let menu = Pos2::new(24.0, 23.0);
        harness.step(vec![Event::PointerMoved(menu)]);
        harness.step(vec![button(menu, true)]);
        harness.step(vec![button(menu, false)]);
        harness.step(Vec::new());
        assert!(egui::Popup::is_any_open(&harness.ctx), "the menu opened");

        harness.drag(Pos2::new(600.0, 400.0), Pos2::new(680.0, 440.0));
        assert!(!egui::Popup::is_any_open(&harness.ctx), "the menu closed");
        assert_eq!(harness.app.session.stroke_count(), 0);

        harness.drag(Pos2::new(600.0, 400.0), Pos2::new(680.0, 440.0));
        assert_eq!(harness.app.session.stroke_count(), 1);
    }

    #[test]
    fn a_drag_over_the_keep_dialog_draws_nothing() {
        let mut harness = Harness::new();
        harness.drag(Pos2::new(300.0, 300.0), Pos2::new(380.0, 340.0));
        assert_eq!(harness.app.session.stroke_count(), 1);
        let id = harness.app.session.active_id();
        let effect = harness.app.chrome.close(&mut harness.app.session, id);
        assert_eq!(effect, CloseEffect::NeedsConfirm);
        harness.step(Vec::new());
        harness.step(Vec::new());

        // The dialog sits at the center of the window, over the canvas.
        harness.drag(Pos2::new(630.0, 400.0), Pos2::new(650.0, 404.0));
        assert_eq!(harness.app.session.stroke_count(), 1);
        assert_eq!(harness.app.session.undo_len(), 1);
    }

    fn click(harness: &mut Harness, at: Pos2) {
        harness.step(vec![Event::PointerMoved(at)]);
        harness.step(vec![button(at, true)]);
        harness.step(vec![button(at, false)]);
        harness.step(Vec::new());
    }

    /// The panel tab's center: 18 points below the canvas top, which is under
    /// the 40-point tab bar and the 50-point toolbar, and 11 points in from
    /// the canvas's right edge.
    fn tab_center(canvas_right: f32) -> Pos2 {
        Pos2::new(canvas_right - 11.0, 40.0 + 50.0 + 18.0 + 20.0)
    }

    #[test]
    fn the_panel_tab_toggles_the_panel_without_drawing() {
        let mut harness = Harness::new();
        assert!(!harness.app.session.panel_open());
        harness.drag(
            tab_center(SCREEN.x) - egui::vec2(6.0, 12.0),
            tab_center(SCREEN.x) + egui::vec2(6.0, 12.0),
        );
        assert_eq!(
            harness.app.session.stroke_count(),
            0,
            "a drag on the tab drew"
        );

        click(&mut harness, tab_center(SCREEN.x));
        assert!(harness.app.session.panel_open());
        assert_eq!(harness.app.session.stroke_count(), 0);

        click(&mut harness, tab_center(SCREEN.x - panel::WIDTH));
        assert!(!harness.app.session.panel_open());
        assert_eq!(harness.app.session.stroke_count(), 0);
    }

    #[test]
    fn f4_opens_the_panel_and_narrows_the_canvas() {
        let mut harness = Harness::new();
        let wide = harness.app.viewport[0];
        assert!(!harness.app.session.panel_open());
        assert!(wide > 1000.0, "canvas width {wide}");

        harness.step(vec![key_press(Key::F4)]);
        assert!(harness.app.session.panel_open());
        let narrow = harness.app.viewport[0];
        assert!(
            narrow < wide - 200.0,
            "open panel left the canvas at {narrow}, from {wide}"
        );

        harness.step(vec![key_press(Key::F4)]);
        assert!(!harness.app.session.panel_open());
        assert!((harness.app.viewport[0] - wide).abs() < 1.0);
    }

    /// The canvas area with the panel closed: under the 40-point tab bar and
    /// the 50-point toolbar, as wide and tall as the last frame's viewport.
    fn canvas_rect(harness: &Harness) -> Rect {
        Rect::from_min_size(
            Pos2::new(0.0, 40.0 + 50.0),
            egui::vec2(
                harness.app.viewport[0] as f32,
                harness.app.viewport[1] as f32,
            ),
        )
    }

    #[test]
    fn an_edge_handle_scales_one_axis_and_the_dot_rotates() {
        let mut harness = Harness::new();
        harness.drag(Pos2::new(400.0, 380.0), Pos2::new(520.0, 440.0));
        harness.step(vec![key_press(Key::V)]);
        let (min, max) = harness.app.session.selection_bounds().unwrap();
        let frame = harness.app.selection_frame(canvas_rect(&harness)).unwrap();

        let right = frame.right_center();
        harness.drag(right, right + egui::vec2(60.0, 0.0));
        let (after_min, after_max) = harness.app.session.selection_bounds().unwrap();
        let (width, height) = (max[0] - min[0], max[1] - min[1]);
        let (after_width, after_height) =
            (after_max[0] - after_min[0], after_max[1] - after_min[1]);
        assert!(
            after_width > width + 30.0,
            "width {width} became {after_width}"
        );
        assert!(
            (after_height - height).abs() < 0.5,
            "height {height} became {after_height}"
        );
        assert!(
            (after_min[0] - min[0]).abs() < 0.5,
            "the opposite edge moved"
        );

        let frame = harness.app.selection_frame(canvas_rect(&harness)).unwrap();
        let dot = rotate_dot(frame);
        harness.drag(dot, dot + egui::vec2(80.0, 40.0));
        let local = harness.app.session.changeable_locals()[0].1.as_coeffs();
        assert!(local[1].abs() > 0.05, "the dot did not rotate: {local:?}");
    }

    #[test]
    fn the_pointer_shows_what_each_handle_does() {
        let mut harness = Harness::new();
        harness.drag(Pos2::new(400.0, 380.0), Pos2::new(520.0, 440.0));
        harness.step(vec![key_press(Key::V)]);
        let frame = harness.app.selection_frame(canvas_rect(&harness)).unwrap();
        let mut hover = |at: Pos2| {
            harness.step(vec![Event::PointerMoved(at)]);
            harness.cursor
        };
        assert_eq!(hover(frame.left_top()), CursorIcon::ResizeNwSe);
        assert_eq!(hover(frame.right_top()), CursorIcon::ResizeNeSw);
        assert_eq!(hover(frame.center_top()), CursorIcon::ResizeVertical);
        assert_eq!(hover(frame.right_center()), CursorIcon::ResizeHorizontal);
        assert_eq!(hover(rotate_dot(frame)), CursorIcon::Grab);
        assert_eq!(hover(frame.center()), CursorIcon::Move);
        assert_eq!(
            hover(frame.right_top() + egui::vec2(80.0, -40.0)),
            CursorIcon::Default
        );

        let corner = frame.right_bottom();
        harness.step(vec![Event::PointerMoved(corner)]);
        harness.step(vec![button(corner, true)]);
        harness.step(vec![Event::PointerMoved(corner + egui::vec2(30.0, 30.0))]);
        assert_eq!(harness.cursor, CursorIcon::ResizeNwSe, "while dragging");
        harness.step(vec![button(corner + egui::vec2(30.0, 30.0), false)]);
    }

    fn canvas_name(harness: &Harness) -> String {
        harness.app.session.layers()[0].name.clone()
    }

    #[test]
    fn a_rename_in_the_outliner_keeps_its_place_and_escape_cancels_it() {
        let mut harness = Harness::new();
        harness.step(vec![key_press(Key::V)]);
        if !harness.app.session.panel_open() {
            harness.app.session.toggle_panel();
        }
        harness.step(Vec::new());
        let name = Pos2::new(
            canvas_rect(&harness).right() + 80.0,
            canvas_rect(&harness).top() + crate::panel::page::FIRST_LINE,
        );
        let double_click = |harness: &mut Harness| {
            harness.step(vec![Event::PointerMoved(name)]);
            for _ in 0..2 {
                harness.step(vec![button(name, true)]);
                harness.step(vec![button(name, false)]);
            }
            harness.step(Vec::new());
        };
        let type_text = |harness: &mut Harness, text: &str| {
            harness.step(vec![Event::Text(text.to_string())]);
        };

        double_click(&mut harness);
        type_text(&mut harness, "Zed");
        harness.step(vec![key_press(Key::Escape)]);
        harness.step(Vec::new());
        assert_eq!(canvas_name(&harness), "Canvas 1", "Escape kept the name");
        type_text(&mut harness, "Zed");
        harness.step(Vec::new());
        assert_eq!(canvas_name(&harness), "Canvas 1", "Escape closed the field");

        // Frames are a sixtieth of a second apart: wait long enough that the
        // next two clicks are a fresh double-click, not a third and fourth.
        for _ in 0..30 {
            harness.step(Vec::new());
        }
        double_click(&mut harness);
        type_text(&mut harness, " sky");
        harness.step(vec![key_press(Key::Enter)]);
        harness.step(Vec::new());
        assert_eq!(canvas_name(&harness), "Canvas 1 sky");
    }

    #[test]
    fn undo_and_nudge_wait_while_a_selection_is_dragged() {
        let mut harness = Harness::new();
        harness.drag(Pos2::new(400.0, 400.0), Pos2::new(520.0, 400.0));
        harness.step(vec![key_press(Key::V)]);
        assert_eq!(harness.app.session.tool(), Tool::Select);
        let undo_len = harness.app.session.undo_len();

        let at = Pos2::new(460.0, 400.0);
        harness.step(vec![Event::PointerMoved(at)]);
        harness.step(vec![button(at, true)]);
        harness.step(vec![Event::PointerMoved(Pos2::new(530.0, 400.0))]);
        let dragged = translation(harness.app.session.changeable_locals()[0].1);
        assert!(
            dragged[0].abs() > 1.0,
            "the drag did not move the canvas ({dragged:?})"
        );
        assert_eq!(harness.app.session.undo_len(), undo_len);

        harness.step(vec![key_press(Key::ArrowRight)]);
        harness.step(vec![Event::Key {
            key: Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                command: true,
                ..Default::default()
            },
        }]);
        assert_eq!(
            harness.app.session.undo_len(),
            undo_len,
            "undo or nudge during the drag recorded a step"
        );
        let held = translation(harness.app.session.changeable_locals()[0].1);
        assert!(
            (held[0] - dragged[0]).abs() < 1.0e-6,
            "a key during the drag replaced the preview"
        );

        harness.step(vec![button(Pos2::new(530.0, 400.0), false)]);
        harness.step(Vec::new());
        assert_eq!(harness.app.session.undo_len(), undo_len + 1);
        assert_eq!(harness.app.session.redo_len(), 0);
    }

    #[test]
    fn ctrl_c_then_ctrl_v_duplicates_the_selected_object() {
        let mut harness = Harness::new();
        harness.step(vec![key_press(Key::R)]);
        harness.drag(Pos2::new(300.0, 300.0), Pos2::new(500.0, 450.0));
        let original = harness.app.session.selection()[0];
        let before = harness.app.session.selection_bounds().unwrap();
        let undo_len = harness.app.session.undo_len();
        assert_eq!(rectangles(&harness), 1);

        // The window's Ctrl+C: egui-winit emits Copy and drops the key-down.
        // The key-up still arrives. A pressed C never does.
        harness.step(vec![Event::Copy]);
        harness.step(vec![command_key(Key::C, false)]);
        assert_eq!(rectangles(&harness), 1, "copy leaves the sketch");
        assert_eq!(harness.app.session.selection(), vec![original]);
        assert_eq!(harness.app.session.undo_len(), undo_len);

        // Ctrl+V arrives as Paste, carrying operating-system text, then the key-up.
        harness.step(vec![Event::Paste("operating system clipboard".into())]);
        harness.step(vec![command_key(Key::V, false)]);
        assert_eq!(rectangles(&harness), 2, "one paste, including the key-up");
        assert!(
            harness
                .app
                .session
                .layers()
                .iter()
                .all(|node| node.kind != sketch::document::LayerKind::Text),
            "the operating-system text was not pasted"
        );
        let pasted = harness.app.session.selection()[0];
        assert_ne!(pasted, original);
        let after = harness.app.session.selection_bounds().unwrap();
        assert!(
            (after.0[0] - before.0[0] - 16.0).abs() < 1.0e-6
                && (after.0[1] - before.0[1] - 16.0).abs() < 1.0e-6,
            "the duplicate sits 16 right and down ({before:?} -> {after:?})"
        );
        assert_eq!(harness.app.session.undo_len(), undo_len + 1);

        // An empty operating-system clipboard emits no Paste event, only the key-up.
        harness.step(vec![command_key(Key::V, false)]);
        assert_eq!(rectangles(&harness), 3, "a key-up still pastes");
        let again = harness.app.session.selection_bounds().unwrap();
        assert!(
            (again.0[0] - before.0[0] - 32.0).abs() < 1.0e-6
                && (again.0[1] - before.0[1] - 32.0).abs() < 1.0e-6,
            "the next paste steps another 16 ({again:?})"
        );

        harness.step(vec![Event::Cut]);
        harness.step(vec![command_key(Key::X, false)]);
        assert_eq!(rectangles(&harness), 2, "cut removes the selection once");
        assert_eq!(harness.app.session.undo_len(), undo_len + 3);
    }

    #[test]
    fn ctrl_s_and_ctrl_o_ask_for_a_path() {
        let dir = std::env::temp_dir().join(format!("simple-sketch-keys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut harness = Harness::new();
        harness.app.session.set_data_dir(&dir);

        // The window sends ModifiersChanged for Ctrl, then a key-down for the
        // letter. It does not replace S or O the way it replaces C, X, and V.
        harness.step(command_chord(Key::O));
        assert_eq!(
            harness.app.session.file_intent(),
            Some(sketch::session::FileIntent::Open)
        );
        assert!(
            std::fs::read_dir(&dir).unwrap().next().is_none(),
            "Ctrl+O wrote a file"
        );

        harness.drag(Pos2::new(400.0, 400.0), Pos2::new(560.0, 420.0));
        assert!(harness.app.session.tabs().iter().any(|tab| tab.needs_keep));
        let id = harness.app.session.active_id();

        // A never-saved sketch's Ctrl+S is Save As. The dialog is not opened.
        harness.step(command_chord(Key::S));
        assert_eq!(
            harness.app.session.file_intent(),
            Some(sketch::session::FileIntent::Save {
                id,
                copy: false,
                close_after: false,
            })
        );
        assert!(
            std::fs::read_dir(&dir).unwrap().flatten().all(|entry| entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                != Some("smx")),
            "Ctrl+S wrote a sketch before a path was chosen"
        );

        let path = dir.join("notes.smx");
        assert_eq!(
            harness.app.session.save_path(id, &path, false, true),
            sketch::session::SaveResult::Wrote
        );
        assert!(path.is_file());
        assert!(harness.app.session.tabs().iter().all(|tab| !tab.needs_keep));
        assert_eq!(harness.app.session.active_name(), "notes");

        harness.drag(Pos2::new(400.0, 440.0), Pos2::new(520.0, 460.0));
        let before = std::fs::read(&path).unwrap();
        harness.step(command_chord(Key::S));
        assert!(harness.app.session.file_intent().is_none());
        let after = std::fs::read(&path).unwrap();
        assert_ne!(before, after, "Ctrl+S did not write the sketch's file");
        assert!(harness.app.session.tabs().iter().all(|tab| !tab.needs_keep));
        assert!(
            registry::command(sketch::registry::CommandId::SaveCopy)
                .shortcuts
                .is_empty()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn rectangles(harness: &Harness) -> usize {
        harness
            .app
            .session
            .layers()
            .iter()
            .filter(|node| node.kind == sketch::document::LayerKind::Rectangle)
            .count()
    }

    /// Ctrl/Cmd held, then the letter pressed: the events egui-winit emits for
    /// a shortcut it does not replace.
    fn command_chord(key: Key) -> Vec<Event> {
        let modifiers = egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        vec![
            Event::ModifiersChanged(modifiers),
            Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            },
        ]
    }

    fn command_key(key: Key, pressed: bool) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers {
                ctrl: true,
                command: true,
                ..Default::default()
            },
        }
    }

    fn translation(transform: kurbo::Affine) -> [f64; 2] {
        let coeffs = transform.as_coeffs();
        [coeffs[4], coeffs[5]]
    }

    fn key_press(key: Key) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }
}
