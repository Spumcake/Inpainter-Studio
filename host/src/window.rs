//! The host window. The title is always Inpainter.

use crate::{canvas, grid, icons, panel, tabs, toolbar};
use eframe::egui::{self, Color32, Rect};
use protocol::AppMessage;
use sketch::registry::{CommandId, Tool};

/// A hair darker than the white toolbar and active tab, as in the picture.
const CANVAS: Color32 = Color32::from_rgb(251, 251, 252);

/// The application id: the desktop file, icon, and file-type names on Linux.
pub const APP_ID: &str = "io.github.spumcake.Inpainter";

/// The window icon, for desktops that show one. Wayland desktops take the
/// icon from the launcher entry instead.
fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon/128.png")).unwrap_or_default()
}

pub fn run(child: crate::process::Running) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Inpainter")
            // The desktop matches the window to its launcher entry and icon by
            // this id, which is the desktop file's name.
            .with_app_id(APP_ID)
            .with_icon(window_icon())
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 480.0]),
        persist_window: false,
        // Ink is painted as raw meshes, which egui does not feather.
        multisampling: 4,
        ..Default::default()
    };
    eframe::run_native(
        "Inpainter",
        options,
        Box::new(move |creation| Ok(Box::new(HostWindow::new(creation, child)))),
    )
}

struct HostWindow {
    /// Kept until the window closes. `on_exit` takes it and shuts the child down.
    child: Option<crate::process::Running>,
    /// The panel starts closed.
    panel_open: bool,
    viewport: [f64; 2],
    /// The live copy. Pen samples stay in this process. Absent in frame
    /// tests that only draw a declaration.
    sketch: Option<canvas::SketchApp>,
    /// The first document from the app is a blank camera. Center it once.
    saw_document: bool,
}

impl HostWindow {
    fn new(creation: &eframe::CreationContext<'_>, child: crate::process::Running) -> Self {
        let mut window = Self::with_child(&creation.egui_ctx, child);
        window.sketch = Some(canvas::SketchApp::new(creation));
        window
    }

    fn with_child(ctx: &egui::Context, child: crate::process::Running) -> Self {
        toolbar::install_fonts(ctx);
        // The window's own chrome is light. The title bar is left to the
        // desktop: egui would otherwise copy its light theme onto the window,
        // and with no theme set, winit's Wayland frame reads the desktop's
        // color scheme while other platforms draw their own.
        ctx.options_mut(|options| options.sync_window_theme = false);
        ctx.set_theme(egui::Theme::Light);
        Self {
            child: Some(child),
            panel_open: false,
            viewport: [0.0, 0.0],
            sketch: None,
            saw_document: false,
        }
    }

    fn running(&self) -> &crate::process::Running {
        self.child
            .as_ref()
            .expect("the child stays until the window closes")
    }

    fn running_mut(&mut self) -> &mut crate::process::Running {
        self.child
            .as_mut()
            .expect("the child stays until the window closes")
    }

    /// Documents replace nodes and camera. Results are dialogs; this slice
    /// does not draw a status from a stopped child, and it drains results
    /// so they do not pile up unread.
    fn apply_remote(&mut self) {
        let documents = self.running_mut().declaration_mut().take_documents();
        let _results = self.running_mut().declaration_mut().take_results();
        let Some(sketch) = self.sketch.as_mut() else {
            return;
        };
        let first = !self.saw_document;
        let mut installed = false;
        for document in documents {
            let AppMessage::Document { sketch: value, dirty } = document else {
                continue;
            };
            if sketch.session().install_document(&value, dirty) {
                installed = true;
            }
            self.saw_document = true;
        }
        if first && installed {
            sketch.forget_presented_view();
        }
    }

    fn publish_sketch(&mut self, undo_before: usize, redo_before: usize, tool_before: Tool) {
        let Some(sketch) = self.sketch.as_ref() else {
            return;
        };
        let session = sketch.session_ref();
        let tool_after = session.tool();
        let viewport = sketch.viewport();
        let undid = session.undo_len() < undo_before;
        let redid = session.redo_len() < redo_before;
        if tool_after != tool_before {
            self.running_mut().send(&protocol::HostMessage::Command {
                id: None,
                viewport: Some(viewport),
                path: None,
                replace: None,
                discard: None,
                tool: Some(tool_name(tool_after).to_string()),
            });
        }
        if undid {
            self.send_command(CommandId::Undo, viewport);
        } else if redid {
            self.send_command(CommandId::Redo, viewport);
        }
    }

    fn publish_document(&mut self, step: bool) {
        let Some(sketch) = self.sketch.as_ref() else {
            return;
        };
        let snapshot = sketch.session_ref().active_snapshot();
        self.running_mut()
            .send(&protocol::HostMessage::Document {
                sketch: snapshot,
                step,
            });
    }

    fn send_command(&mut self, id: CommandId, viewport: [f64; 2]) {
        self.running_mut().send(&protocol::HostMessage::Command {
            id: Some(command_name(id).to_string()),
            viewport: Some(viewport),
            path: None,
            replace: None,
            discard: None,
            tool: None,
        });
    }
}

fn tool_name(tool: Tool) -> &'static str {
    match tool {
        Tool::Brush => "brush",
        Tool::Eraser => "eraser",
        Tool::Select => "select",
        Tool::Rectangle => "rectangle",
        Tool::Ellipse => "ellipse",
        Tool::Line => "line",
        Tool::Text => "text",
    }
}

fn command_name(id: CommandId) -> &'static str {
    match id {
        CommandId::NewSketch => "new_sketch",
        CommandId::Open => "open",
        CommandId::Save => "save",
        CommandId::SaveAs => "save_as",
        CommandId::SaveCopy => "save_copy",
        CommandId::ClearRecent => "clear_recent",
        CommandId::Close => "close",
        CommandId::Undo => "undo",
        CommandId::Redo => "redo",
        CommandId::Cut => "cut",
        CommandId::Copy => "copy",
        CommandId::Paste => "paste",
        CommandId::Delete => "delete",
        CommandId::NudgeLeft => "nudge_left",
        CommandId::NudgeRight => "nudge_right",
        CommandId::NudgeUp => "nudge_up",
        CommandId::NudgeDown => "nudge_down",
        CommandId::ZoomIn => "zoom_in",
        CommandId::ZoomOut => "zoom_out",
        CommandId::Fit => "fit",
        CommandId::ZoomToSelection => "zoom_to_selection",
        CommandId::Maximize => "maximize",
        CommandId::NewCanvas => "new_canvas",
        CommandId::Group => "group",
        CommandId::Ungroup => "ungroup",
        CommandId::BringForward => "bring_forward",
        CommandId::SendBackward => "send_backward",
        CommandId::BringToFront => "bring_to_front",
        CommandId::SendToBack => "send_to_back",
        CommandId::AlignLeft => "align_left",
        CommandId::AlignCenter => "align_center",
        CommandId::AlignRight => "align_right",
        CommandId::RotateClockwise => "rotate_clockwise",
        CommandId::RotateCounterclockwise => "rotate_counterclockwise",
        CommandId::RotateHalf => "rotate_half",
        CommandId::FlipHorizontal => "flip_horizontal",
        CommandId::FlipVertical => "flip_vertical",
        CommandId::TogglePanel => "toggle_panel",
        CommandId::SizeDown => "size_down",
        CommandId::SizeUp => "size_up",
    }
}

fn shows_grid(declaration: &protocol::Declaration) -> bool {
    let Some(tab) = declaration.tabs.first() else {
        return false;
    };
    declaration.contents.iter().any(|contents| {
        contents.tab == tab.id && matches!(contents.kind, protocol::ContentsKind::Grid)
    })
}

/// The properties panel is the host's, drawn only for a tab whose
/// sub-application declares it. Only the right side has a reference.
fn shows_panel(declaration: &protocol::Declaration) -> bool {
    let Some(tab) = declaration.tabs.first() else {
        return false;
    };
    declaration.panels.iter().any(|panel| {
        panel.tab == tab.id && matches!(panel.side, protocol::PanelSide::Right)
    })
}

impl eframe::App for HostWindow {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if let Some(sketch) = self.sketch.as_mut() {
            sketch.raw_input_hook(ctx, raw_input);
        }
    }

    fn on_exit(&mut self) {
        if let Some(sketch) = self.sketch.as_mut() {
            sketch.on_exit();
        }
        if let Some(child) = self.child.take() {
            child.shutdown();
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        CANVAS.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.running_mut().poll();
        self.apply_remote();
        let pending = self.running().pending();
        if pending {
            ui.ctx().request_repaint();
        }

        let live = self.sketch.is_some() && self.running().declaration().toolbar.is_some();
        let before = self.sketch.as_ref().filter(|_| live).map(|sketch| {
            let session = sketch.session_ref();
            (
                session.undo_len(),
                session.redo_len(),
                session.tool(),
                session.active_snapshot(),
            )
        });

        let mut visuals = egui::Visuals::light();
        visuals.panel_fill = Color32::WHITE;
        ui.ctx().set_visuals(visuals);

        let dismissing = egui::Popup::is_any_open(ui.ctx());
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                if live {
                    self.sketch
                        .as_mut()
                        .expect("live")
                        .commit_text_on_outside_press(ui);
                }
                {
                    let declaration = self.running().declaration();
                    tabs::show(ui, declaration);
                }
                if live {
                    let viewport = self.sketch.as_ref().expect("live").viewport();
                    let center = toolbar::show_session(
                        ui,
                        self.sketch.as_mut().expect("live").session(),
                        viewport,
                    );
                    if center {
                        self.sketch.as_mut().expect("live").center_next_view();
                    }
                } else {
                    toolbar::show(ui);
                }

                let full = ui.available_rect_before_wrap();
                // A tab without a declared panel has no panel tab, and F4
                // does nothing there.
                let has_panel = shows_panel(self.running().declaration());
                if !has_panel {
                    self.panel_open = false;
                }
                if live {
                    self.sketch.as_mut().expect("live").handle_keys(ui.ctx());
                }
                // F4 can open the panel. Split after the key so that frame
                // gives the panel its width and narrows the contents.
                if has_panel && ui.input(|input| icons::panel_shortcut_pressed(input)) {
                    self.panel_open = !self.panel_open;
                }
                let panel_w = if self.panel_open { panel::WIDTH } else { 0.0 };
                let right = if panel_w == 0.0 {
                    full.right()
                } else {
                    (full.right() - panel_w).max(full.left())
                };
                let rect = Rect::from_min_max(full.min, egui::pos2(right, full.bottom()));
                self.viewport = [f64::from(rect.width()), f64::from(rect.height())];

                if live {
                    self.sketch
                        .as_mut()
                        .expect("live")
                        .drive(ui, rect, dismissing);
                } else {
                    let show_grid = shows_grid(self.running().declaration());
                    let painter = ui.painter().with_clip_rect(rect);
                    painter.rect_filled(rect, 0.0, CANVAS);
                    if show_grid {
                        // Until the first sketch document, the grid stays the
                        // centered grid the scaffold already draws.
                        let origin = [-self.viewport[0] / 2.0, -self.viewport[1] / 2.0];
                        let zoom = 1.0;
                        grid::paint_grid(&painter, rect, origin, zoom);
                    }
                }

                if self.panel_open && panel_w > 0.0 {
                    let panel_rect =
                        Rect::from_min_max(egui::pos2(rect.right(), full.top()), full.max);
                    if live {
                        panel::page::show(
                            ui,
                            self.sketch.as_mut().expect("live").session(),
                            panel_rect,
                        );
                    } else {
                        panel::show(ui, panel_rect);
                    }
                } else if self.panel_open {
                    ui.ctx().request_repaint();
                }
                if has_panel {
                    panel::toggle_tab(ui.ctx(), &mut self.panel_open, rect.right(), full.top());
                }
            });

        if let Some((undo_before, redo_before, tool_before, snap_before)) = before {
            let (changed, nodes_changed, undid, redid) = {
                let session = self.sketch.as_ref().expect("live").session_ref();
                let snap_after = session.active_snapshot();
                (
                    snap_after != snap_before,
                    snap_after.get("nodes") != snap_before.get("nodes"),
                    session.undo_len() < undo_before,
                    session.redo_len() < redo_before,
                )
            };
            self.publish_sketch(undo_before, redo_before, tool_before);
            if changed && !undid && !redid {
                self.publish_document(nodes_changed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HostWindow;
    use eframe::App;
    use eframe::egui::{self, Event, Key, PointerButton, Pos2, Rect};

    const SCREEN: egui::Vec2 = egui::vec2(1280.0, 800.0);

    /// Runs whole frames of the host window with synthetic pointer input.
    struct Harness {
        ctx: egui::Context,
        app: HostWindow,
        frame: eframe::Frame,
        /// Every command the frames sent to the native window.
        window_commands: Vec<egui::ViewportCommand>,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            let app = HostWindow::with_child(&ctx, crate::process::Running::declared(template()));
            let mut harness = Self {
                ctx,
                app,
                frame: eframe::Frame::_new_kittest(),
                window_commands: Vec::new(),
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
            } = self;
            let mut output = ctx.run_ui(input, |ui| app.ui(ui, frame));
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

    /// What the template declares: one tab, the grid, and the panel on the right.
    fn template() -> Vec<protocol::AppMessage> {
        use protocol::{AppMessage, ContentsKind, PanelSide};
        vec![
            AppMessage::Hello {
                protocol: protocol::PROTOCOL_VERSION,
            },
            AppMessage::Tab {
                id: "template".into(),
                label: "Template".into(),
            },
            AppMessage::Contents {
                tab: "template".into(),
                kind: ContentsKind::Grid,
            },
            AppMessage::Panel {
                tab: "template".into(),
                side: PanelSide::Right,
            },
            AppMessage::End,
        ]
    }

    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
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

    fn key_press(key: Key) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
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
    fn the_panel_tab_toggles_the_panel_without_drawing() {
        let mut harness = Harness::new();
        assert!(!harness.app.panel_open);
        harness.drag(
            tab_center(SCREEN.x) - egui::vec2(6.0, 12.0),
            tab_center(SCREEN.x) + egui::vec2(6.0, 12.0),
        );

        click(&mut harness, tab_center(SCREEN.x));
        assert!(harness.app.panel_open);

        click(&mut harness, tab_center(SCREEN.x - crate::panel::WIDTH));
        assert!(!harness.app.panel_open);
    }

    #[test]
    fn f4_opens_the_panel_and_narrows_the_canvas() {
        let mut harness = Harness::new();
        let wide = harness.app.viewport[0];
        assert!(!harness.app.panel_open);
        assert!(wide > 1000.0, "canvas width {wide}");

        harness.step(vec![key_press(Key::F4)]);
        assert!(harness.app.panel_open);
        let narrow = harness.app.viewport[0];
        assert!(
            narrow < wide - 200.0,
            "open panel left the canvas at {narrow}, from {wide}"
        );

        harness.step(vec![key_press(Key::F4)]);
        assert!(!harness.app.panel_open);
        assert!((harness.app.viewport[0] - wide).abs() < 1.0);
    }
}
