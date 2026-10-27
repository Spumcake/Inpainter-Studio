//! The host window. The title is always Inpainter. It runs every started
//! sub-application, shows all of their tabs, and draws the selected tab's
//! toolbar, menu, panel, and contents from what that tab's sub-application
//! declared. Its frame never waits on a sub-application.

use crate::canvas::{CANVAS, Surface};
use crate::chat::{self, Chat, ChatEvent};
use crate::doc::{Doc, Live};
use crate::panel::Sent;
use crate::process::Running;
use crate::route::{self, Routing};
use crate::tablet::Tablet;
use crate::tabs::{StripTab, TabEvent};
use crate::workspace::{self, ModelChoice, Settings};
use crate::{ask, grid, icons, panel, tabs, toolbar, widgets};
use assistant::{Assistant, Command};
use eframe::egui::{self, Color32, Event, Key, Rect};
use protocol::{Action, ContentsKind, FileKind, HostMessage, MenuEntry, PanelSide};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

mod flows;
#[cfg(test)]
mod checks;

/// How strongly a locked control is faded: the toolbar's own waiting gray.
const LOCKED_ALPHA: f32 = 0.33;

/// The application id: the desktop file, icon, and file-type names on Linux.
pub const APP_ID: &str = "io.github.spumcake.Inpainter";

/// The window icon, for desktops that show one. Wayland desktops take the
/// icon from the launcher entry instead.
fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon/128.png")).unwrap_or_default()
}

/// A started sub-application: its manifest's id and name, its program, and
/// its process.
pub struct Started {
    pub id: String,
    pub name: String,
    /// None in frame tests, which start no program.
    pub executable: Option<PathBuf>,
    pub running: Running,
}

/// A sub-application to start: what [`Started`] keeps besides its process.
#[derive(Clone)]
pub(crate) struct AppSpec {
    pub id: String,
    pub name: String,
    pub executable: Option<PathBuf>,
}

/// The project open in this window. The host owns it; no sub-application sees it.
struct Project {
    id: String,
    name: String,
    /// Absolute paths, in the order the person attached them.
    folders: Vec<String>,
    /// The workspace is shown beside the conversation.
    workspace_open: bool,
}

pub fn run(specs: Vec<AppSpec>) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Inpainter")
            // The desktop matches the window to its launcher entry and icon by
            // this id, which is the desktop file's name.
            .with_app_id(APP_ID)
            .with_icon(window_icon())
            // The shell keeps the 1280-point width it had before the chat
            // panel, which takes a quarter of the window, joined it.
            .with_inner_size([1712.0, 800.0])
            .with_min_inner_size([640.0, 480.0]),
        persist_window: false,
        // Ink is painted as raw meshes, which egui does not feather.
        multisampling: 4,
        ..Default::default()
    };
    eframe::run_native(
        "Inpainter",
        options,
        Box::new(move |creation| Ok(Box::new(HostWindow::new(creation, specs)))),
    )
}

struct HostWindow {
    apps: Vec<Started>,
    /// The sub-application whose tab is shown.
    selected: usize,
    surface: Surface,
    live: Live,
    /// The tab drawn last frame, so a switch ends what was dragged on it.
    shown: Option<(usize, String)>,
    /// The panel of a tab that declares the panel but no properties.
    panel_open: bool,
    viewport: [f64; 2],
    /// The contents size last sent to the sub-applications. Sent again when
    /// the window's size changes, shown or hidden.
    sent_viewport: Option<[f64; 2]>,
    /// File dialogs open in the real window. Frame tests answer them instead.
    native_dialogs: bool,
    /// Ctrl/Cmd+C, X, or V already arrived as a copy, cut, or paste event.
    /// The key-up still comes through, and must not run the command again.
    copy_press: bool,
    cut_press: bool,
    paste_press: bool,
    ctx: egui::Context,
    /// The sub-applications installed at launch. Every project starts all of them.
    launch: Vec<AppSpec>,
    /// The contents and panel each sub-application declared for its first
    /// tab, by id, so its tabs keep them when it is started again to
    /// restore a project and reports its tabs only later.
    kinds: HashMap<String, (Option<ContentsKind>, bool)>,
    assistant: Assistant,
    chat: Chat,
    routing: Routing,
    settings: Settings,
    /// Where projects are kept. None in frame tests, which never write one.
    data: Option<PathBuf>,
    project: Option<Project>,
    /// The name is being typed.
    renaming: bool,
    rename_draft: String,
    /// When the project file was last written, so a quiet change waits.
    last_project_write: Option<Instant>,
    /// The change a write failed on. It is tried again only after a new one.
    failed_signature: Option<String>,
    flows: flows::Flows,
    /// The model list has been asked for.
    listed: bool,
    /// The gray bar, last drawn, so a check can press it.
    bar_rect: Rect,
    /// Other projects for the menu. Read when the list changes, not each frame.
    listed_projects: Option<Vec<(String, String)>>,
    /// Paths that frame tests give in place of file dialogs.
    #[cfg(test)]
    scripted_paths: std::collections::VecDeque<PathBuf>,
}

impl HostWindow {
    fn new(creation: &eframe::CreationContext<'_>, specs: Vec<AppSpec>) -> Self {
        // Safety: `on_exit` disconnects before eframe closes the display.
        let tablet = unsafe { Tablet::connect(creation) };
        let mut window = Self::with_apps(&creation.egui_ctx, Vec::new(), tablet);
        window.launch = specs;
        window.native_dialogs = true;
        window.data = workspace::data_folder();
        window.settings = Settings::load(Settings::default_file());
        window.chat.model = window.settings.model.clone();
        window.open_current();
        window
    }

    fn with_apps(ctx: &egui::Context, apps: Vec<Started>, tablet: Tablet) -> Self {
        toolbar::install_fonts(ctx);
        // The window's own chrome is light. The title bar is left to the
        // desktop: egui would otherwise copy its light theme onto the window,
        // and with no theme set, winit's Wayland frame reads the desktop's
        // color scheme while other platforms draw their own.
        ctx.options_mut(|options| options.sync_window_theme = false);
        ctx.set_theme(egui::Theme::Light);
        for app in &apps {
            app.running.attach(ctx);
        }
        let launch = apps
            .iter()
            .map(|started| AppSpec {
                id: started.id.clone(),
                name: started.name.clone(),
                executable: started.executable.clone(),
            })
            .collect();
        let wake_ctx = ctx.clone();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || wake_ctx.request_repaint());
        let assistant = Assistant::spawn(assistant::Config::from_env(wake));
        Self {
            apps,
            selected: 0,
            surface: Surface::new(tablet),
            live: Live::default(),
            shown: None,
            panel_open: false,
            viewport: [0.0, 0.0],
            sent_viewport: None,
            native_dialogs: false,
            copy_press: false,
            cut_press: false,
            paste_press: false,
            ctx: ctx.clone(),
            launch,
            kinds: HashMap::new(),
            assistant,
            chat: Chat::default(),
            routing: Routing::default(),
            settings: Settings::load(None),
            data: None,
            project: None,
            renaming: false,
            rename_draft: String::new(),
            last_project_write: None,
            failed_signature: None,
            flows: flows::Flows::default(),
            listed: false,
            bar_rect: Rect::NOTHING,
            listed_projects: None,
            #[cfg(test)]
            scripted_paths: std::collections::VecDeque::new(),
        }
    }

    /// A request is running: every sub-application control waits.
    fn locked(&self) -> bool {
        self.chat.working
    }

    /// The selected sub-application's shown tab id.
    fn active_tab(&self) -> Option<String> {
        self.apps.get(self.selected)?.running.tabs().1
    }

    /// The contents a tab shows. A tab a sub-application reported after its
    /// greeting shows what the greeting declared for its first tab.
    fn contents(&self, app: usize, tab: &str) -> Option<ContentsKind> {
        let started = self.apps.get(app)?;
        let declaration = started.running.declaration();
        declaration
            .contents
            .iter()
            .find(|contents| contents.tab == tab)
            .or_else(|| declaration.contents.first())
            .map(|contents| contents.kind)
            .or_else(|| self.kinds.get(&started.id).and_then(|kind| kind.0))
    }

    /// Remembers what each sub-application declared for its first tab.
    fn note_kinds(&mut self) {
        for started in &self.apps {
            let declaration = started.running.declaration();
            if let Some(contents) = declaration.contents.first() {
                let panel = declaration
                    .panels
                    .first()
                    .is_some_and(|panel| matches!(panel.side, PanelSide::Right));
                self.kinds
                    .insert(started.id.clone(), (Some(contents.kind), panel));
            }
        }
    }

    /// The properties panel is the host's, drawn only for a tab whose
    /// sub-application declares it. Only the right side has a reference.
    fn has_panel(&self, app: usize, tab: &str) -> bool {
        let Some(started) = self.apps.get(app) else {
            return false;
        };
        let declaration = started.running.declaration();
        if declaration.panels.is_empty() && declaration.contents.is_empty() {
            return self.kinds.get(&started.id).is_some_and(|kind| kind.1);
        }
        declaration
            .panels
            .iter()
            .find(|panel| panel.tab == tab)
            .or_else(|| declaration.panels.first())
            .is_some_and(|panel| matches!(panel.side, PanelSide::Right))
    }

    /// Writes one message to a sub-application. While the assistant works,
    /// nothing from the toolbar, menus, panel, drawing surface, or shortcuts
    /// is sent; selecting a tab and moving its camera still are.
    fn send(&mut self, app: usize, message: HostMessage) {
        if self.locked() && waits_while_locked(&message) {
            return;
        }
        match &message {
            HostMessage::FileResult { path: Some(path), .. } => self.note_save_path(app, path.clone()),
            HostMessage::FileResult { path: None, .. } => self.note_save_cancelled(),
            _ => {}
        }
        if let Some(started) = self.apps.get_mut(app) {
            started.running.send(&message);
        }
    }

    /// Runs a declared action of the selected sub-application. The drawing
    /// surface's own commands act on the tab's camera.
    fn run_action(&mut self, action: &Action) {
        let app = self.selected;
        let Some(tab) = self.active_tab() else {
            return;
        };
        if action.id == "window.maximize" {
            return;
        }
        let viewport = self.viewport;
        let mut out = Vec::new();
        {
            let Some(started) = self.apps.get_mut(app) else {
                return;
            };
            let declared = &mut started.running.declared;
            let canvas = declared.canvas.as_ref().filter(|canvas| canvas.tab == tab);
            match declared.documents.get_mut(&tab) {
                Some(display) => {
                    let mut doc = Doc {
                        tab: &tab,
                        display,
                        canvas,
                        out: &mut out,
                        live: &mut self.live,
                        viewport,
                    };
                    doc.run(action, None);
                }
                None => out.push(HostMessage::Command {
                    id: action.id.clone(),
                    tab: Some(tab.clone()),
                    arg: action.arg.clone(),
                    node: None,
                    viewport,
                }),
            }
        }
        for message in out {
            self.send(app, message);
        }
        self.send_camera();
    }

    /// The camera the drawing surface moved this frame, reported once.
    fn send_camera(&mut self) {
        if !self.live.moved {
            return;
        }
        self.live.moved = false;
        let app = self.selected;
        let Some(tab) = self.active_tab() else {
            return;
        };
        let Some(camera) = self
            .apps
            .get(app)
            .and_then(|started| started.running.declared.documents.get(&tab))
            .map(|display| display.camera)
        else {
            return;
        };
        self.send(
            app,
            HostMessage::Camera {
                tab,
                origin: camera.origin,
                zoom: camera.zoom,
            },
        );
        self.flows.moved = true;
    }

    /// Opens the file dialogs sub-applications asked for, and answers them.
    fn resolve_dialogs(&mut self, frame: &mut eframe::Frame) {
        if !self.native_dialogs {
            return;
        }
        for app in 0..self.apps.len() {
            while let Some(request) = self.apps[app].running.declared.files.pop_front() {
                let folder = request.folder.as_deref().map(std::path::Path::new);
                let path = match request.kind {
                    FileKind::Open => {
                        crate::dialogs::pick_open(frame, folder, request.filter.as_ref())
                    }
                    FileKind::Save => crate::dialogs::pick_save(
                        frame,
                        folder,
                        request.name.as_deref().unwrap_or_default(),
                        request.filter.as_ref(),
                    ),
                };
                self.send(
                    app,
                    HostMessage::FileResult {
                        id: request.id,
                        path: path.map(|path| path.display().to_string()),
                    },
                );
            }
        }
    }

    /// The one question shown: the host's own, or else the selected
    /// sub-application's first.
    fn show_asks(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.show_question(ctx, frame) {
            return;
        }
        let order: Vec<usize> = std::iter::once(self.selected)
            .chain((0..self.apps.len()).filter(|app| *app != self.selected))
            .filter(|app| *app < self.apps.len())
            .collect();
        for app in order {
            let Some(question) = self.apps[app].running.declared.ask.clone() else {
                continue;
            };
            if let Some(button) = ask::show(ctx, app, &question) {
                self.apps[app].running.declared.ask = None;
                self.note_answer(&question, &button);
                self.send(
                    app,
                    HostMessage::Answer {
                        id: question.id,
                        button,
                    },
                );
            }
            return;
        }
    }

    /// Shortcuts the selected sub-application declared, and the window's
    /// clipboard events. Escape ends what the drawing surface is doing.
    fn handle_keys(&mut self, ctx: &egui::Context, editing_text: bool) -> Vec<Action> {
        let Some(started) = self.apps.get(self.selected) else {
            return Vec::new();
        };
        let keys = started.running.declared.keys.clone();
        // A name, the size field, or text being typed owns these keys.
        // The press is still noted, so the key-up after editing ends does not
        // copy the objects.
        let editing = ctx.text_edit_focused() || editing_text;
        let frame = ctx.input(clipboard_frame);
        let mut clipboard = Vec::new();
        take_clipboard(
            &mut clipboard,
            keys.copy.as_ref(),
            frame.copy,
            frame.copy_up,
            &mut self.copy_press,
            !editing,
        );
        take_clipboard(
            &mut clipboard,
            keys.cut.as_ref(),
            frame.cut,
            frame.cut_up,
            &mut self.cut_press,
            !editing,
        );
        take_clipboard(
            &mut clipboard,
            keys.paste.as_ref(),
            frame.paste,
            frame.paste_up,
            &mut self.paste_press,
            !editing,
        );
        if editing {
            return Vec::new();
        }
        let shift = ctx.input(|input| input.modifiers.shift);
        let mut actions = clipboard.clone();
        let pressed: Vec<Action> = ctx.input(|input| {
            keys.bindings
                .iter()
                .filter(|binding| icons::chord_pressed(input, &binding.chord))
                .map(|binding| binding.action.clone())
                .collect()
        });
        for action in pressed {
            // The window never delivers the key-down for these three. When a
            // frame does, the clipboard path above already ran them.
            if clipboard.iter().any(|done| done.id == action.id) {
                continue;
            }
            if action.id == "window.maximize" {
                let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                continue;
            }
            // A command with no argument of its own is sent with Shift, so a
            // nudge can step farther.
            let action = if action.arg.is_none() {
                action.with_value(serde_json::json!(shift))
            } else {
                action
            };
            actions.push(action);
        }
        // Undo, redo, and nudge wait while a selection is dragged.
        if self.surface.transforming() {
            actions.clear();
        }
        // While the assistant works, only the camera's own shortcuts run.
        if self.locked() {
            actions.retain(|action| Doc::moves_camera_only(&action.id));
        }
        actions
    }
}

impl eframe::App for HostWindow {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.surface.tablet.feed(ctx, raw_input);
    }

    fn on_exit(&mut self) {
        self.surface.tablet.disconnect();
        for started in std::mem::take(&mut self.apps) {
            started.running.shutdown();
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        CANVAS.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut pending = false;
        for started in &mut self.apps {
            started.running.poll();
            pending |= started.running.pending();
        }
        if pending {
            ctx.request_repaint();
        }
        // Pings go out every second, and a missing answer shows a notice.
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
        self.note_kinds();
        self.pump_assistant();
        self.project_frame(&ctx, frame);
        self.resolve_dialogs(frame);

        let app = self.selected.min(self.apps.len().saturating_sub(1));
        self.selected = app;
        let tab = self.active_tab();
        let shown = tab.clone().map(|tab| (app, tab));
        if shown != self.shown {
            self.surface.reset();
            self.live.reset();
            self.shown = shown;
        }

        // Read before the tab bar runs: a click outside the menu closes it there.
        let dismissing = egui::Popup::is_any_open(&ctx);
        let mut visuals = egui::Visuals::light();
        visuals.panel_fill = Color32::WHITE;
        ctx.set_visuals(visuals);

        let mut chosen: Vec<Action> = Vec::new();
        let mut sent: Vec<Sent> = Vec::new();
        let mut talk: Vec<ChatEvent> = Vec::new();
        let workspace_open = self.workspace_shown();
        let mut menu = chat::ProjectMenu {
            name: self
                .project
                .as_ref()
                .map(|project| project.name.clone())
                .unwrap_or_default(),
            folders: self
                .project
                .as_ref()
                .map(|project| project.folders.clone())
                .unwrap_or_default(),
            others: self.other_projects(),
            renaming: self.renaming,
            draft: self.rename_draft.clone(),
        };
        let mut bar_pressed = false;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let full = ui.max_rect();
                let left = if workspace_open {
                    chat::width(full.width())
                } else {
                    full.width()
                };
                let chat_rect =
                    Rect::from_min_max(full.min, egui::pos2(full.left() + left, full.bottom()));
                let shell = Rect::from_min_max(egui::pos2(chat_rect.right(), full.top()), full.max);
                talk = chat::show(ui, chat_rect, &mut self.chat, &mut menu, !workspace_open);
                if workspace_open {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(shell), |ui| {
                        self.shell(ui, &ctx, app, tab.clone(), dismissing, &mut chosen, &mut sent);
                    });
                } else {
                    self.viewport = hidden_viewport(full, self.panel_width(app, tab.as_deref()));
                }
                let edge = if workspace_open { chat_rect.right() } else { full.right() };
                let (rect, pressed) = workspace_bar(ui, edge, full);
                self.bar_rect = rect;
                bar_pressed = pressed;
            });
        self.rename_draft = menu.draft;
        if bar_pressed {
            self.toggle_workspace();
        }
        self.publish_viewport();

        for action in chosen {
            self.run_action(&action);
        }
        for item in sent {
            match item {
                Sent::Run(action) => self.run_action(&action),
                Sent::Property {
                    prop,
                    value,
                    preview,
                } => {
                    let value = serde_json::to_value(value).unwrap_or_default();
                    self.send(
                        app,
                        HostMessage::Property {
                            prop,
                            value,
                            preview,
                        },
                    );
                }
            }
        }
        self.send_camera();
        for event in talk {
            self.chat_event(event, &ctx, frame);
        }
        self.show_asks(&ctx, frame);
    }
}

/// The tab strip and the toolbar, which the contents area sits under.
const WORKSPACE_CHROME: f32 = 90.0;
/// The short gray bar that shows or hides the workspace.
const BAR_GRAY: Color32 = Color32::from_rgb(186, 188, 192);

/// The contents size the workspace would have if it were shown: the window
/// minus the conversation column, this chrome, and an open properties panel.
fn hidden_viewport(window: Rect, panel: f32) -> [f64; 2] {
    let column = chat::width(window.width());
    let shell_w = (window.width() - column).max(0.0);
    let width = (shell_w - panel).max(0.0);
    let height = (window.height() - WORKSPACE_CHROME).max(0.0);
    [f64::from(width), f64::from(height)]
}

/// A short gray bar on `edge_x`, vertically centered. Returns its rect and
/// whether it was clicked.
fn workspace_bar(ui: &egui::Ui, edge_x: f32, bounds: Rect) -> (Rect, bool) {
    let center = egui::pos2(edge_x - 8.0, bounds.center().y);
    let bar = Rect::from_center_size(center, egui::vec2(5.0, 56.0));
    let hit = bar.expand2(egui::vec2(8.0, 6.0));
    ui.painter().rect_filled(bar, 3.0, BAR_GRAY);
    let clicked = ui
        .interact(hit, egui::Id::new("workspace-bar"), egui::Sense::click())
        .clicked();
    (bar, clicked)
}

/// While the assistant works, these wait: commands, gestures, selections,
/// and property edits. Selecting a tab is a command that still goes.
fn waits_while_locked(message: &HostMessage) -> bool {
    match message {
        HostMessage::Command { id, .. } => id != "select_tab",
        HostMessage::Gesture { .. } | HostMessage::Select { .. } | HostMessage::Property { .. } => {
            true
        }
        _ => false,
    }
}

/// Menu rows drawn in their waiting look, with their submenus.
fn waiting_rows(rows: &[MenuEntry]) -> Vec<MenuEntry> {
    rows.iter()
        .cloned()
        .map(|mut row| {
            row.enabled = false;
            row.submenu = waiting_rows(&row.submenu);
            row
        })
        .collect()
}

impl HostWindow {
    /// The shell to the right of the chat panel: the tab strip, toolbar,
    /// contents, and properties panel, as version 0.2 drew them. While the
    /// assistant works, every sub-application control is in its waiting look.
    #[allow(clippy::too_many_arguments)]
    fn shell(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        app: usize,
        tab: Option<String>,
        dismissing: bool,
        chosen: &mut Vec<Action>,
        sent: &mut Vec<Sent>,
    ) {
        let locked = self.locked();
        self.surface.locked = locked;
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut strip = self.strip();
        if locked {
            for tab in &mut strip {
                tab.close_waits = true;
            }
        }
        let (menu, plus_tip) = match self.apps.get(app) {
            Some(started) => (
                started.running.declared.menu.clone(),
                started
                    .running
                    .declared
                    .tabs
                    .as_ref()
                    .filter(|tabs| tabs.new_tab.is_some())
                    .map(|tabs| tabs.new_tab_tip.clone()),
            ),
            None => (Vec::new(), None),
        };
        let menu = if locked { waiting_rows(&menu) } else { menu };
        for event in tabs::show(ui, &strip, &menu, plus_tip.as_deref(), locked) {
            match event {
                TabEvent::Menu(action) => chosen.push(action),
                TabEvent::Select { app, tab } => {
                    self.selected = app;
                    self.send(
                        app,
                        HostMessage::Command {
                            id: "select_tab".to_string(),
                            tab: Some(tab),
                            arg: None,
                            node: None,
                            viewport: self.viewport,
                        },
                    );
                }
                TabEvent::Close { app, tab } => self.send(
                    app,
                    HostMessage::Command {
                        id: "close_tab".to_string(),
                        tab: Some(tab),
                        arg: None,
                        node: None,
                        viewport: self.viewport,
                    },
                ),
                TabEvent::Plus => {
                    if let Some(action) = self.apps.get(app).and_then(|started| {
                        started.running.declared.tabs.as_ref()?.new_tab.clone()
                    }) {
                        chosen.push(action);
                    }
                }
            }
        }
        let toolbar = self
            .apps
            .get(app)
            .and_then(|started| started.running.declared.toolbar.clone());
        let picked = ui
            .scope(|ui| {
                if locked {
                    ui.style_mut().visuals.disabled_alpha = LOCKED_ALPHA;
                    ui.disable();
                }
                toolbar::show(ui, toolbar.as_ref())
            })
            .inner;
        if !locked {
            chosen.extend(picked);
        }

        let full = ui.available_rect_before_wrap();
        let Some(tab) = tab else {
            self.viewport = [f64::from(full.width()), f64::from(full.height())];
            ui.painter().rect_filled(full, 0.0, CANVAS);
            return;
        };
        let properties = self
            .apps
            .get(app)
            .and_then(|started| started.running.declared.properties.clone());
        // A tab without a declared panel has no panel tab, and F4
        // does nothing there.
        let has_panel = self.has_panel(app, &tab);
        let editing_text = self.editing_text(app, &tab);
        // F4 can open the panel. Split after the keys so that frame
        // gives the panel its width and narrows the contents.
        chosen.extend(self.handle_keys(ctx, editing_text));
        if properties.is_none() {
            if !has_panel {
                self.panel_open = false;
            }
            if has_panel && ui.input(icons::panel_shortcut_pressed) {
                self.panel_open = !self.panel_open;
            }
        }
        let open = has_panel
            && properties
                .as_ref()
                .map_or(self.panel_open, |properties| properties.open);
        let panel_w = if open { panel::WIDTH } else { 0.0 };
        let right = if panel_w == 0.0 {
            full.right()
        } else {
            (full.right() - panel_w).max(full.left())
        };
        let rect = Rect::from_min_max(full.min, egui::pos2(right, full.bottom()));
        self.viewport = [f64::from(rect.width()), f64::from(rect.height())];

        match self.contents(app, &tab) {
            Some(ContentsKind::Canvas) => self.show_canvas(ui, app, &tab, rect, dismissing),
            Some(ContentsKind::Grid) => {
                let painter = ui.painter().with_clip_rect(rect);
                painter.rect_filled(rect, 0.0, CANVAS);
                // As `Camera::centered` sets a new sketch: origin in
                // the middle of the area, zoom 1.
                let origin = [-self.viewport[0] / 2.0, -self.viewport[1] / 2.0];
                grid::paint_grid(&painter, rect, origin, 1.0);
            }
            None => {
                ui.painter()
                    .with_clip_rect(rect)
                    .rect_filled(rect, 0.0, CANVAS);
            }
        }
        self.paint_status(ui, app, rect);

        // A menu's Properties can open the panel after this frame was
        // split with no room for it. It then waits for the next frame,
        // which gives it its width.
        if open && panel_w > 0.0 {
            let panel_rect = Rect::from_min_max(egui::pos2(rect.right(), full.top()), full.max);
            let mut column = ui.new_child(egui::UiBuilder::new().max_rect(panel_rect));
            if locked {
                column.style_mut().visuals.disabled_alpha = LOCKED_ALPHA;
                column.disable();
            }
            let edits = panel::show(&mut column, properties.as_ref(), panel_rect);
            if !locked {
                sent.extend(edits);
            }
        } else if open {
            ui.ctx().request_repaint();
        }
        if has_panel {
            let tip = properties
                .as_ref()
                .map_or_else(icons::panel_tip, |properties| properties.tip.clone());
            if panel::toggle_tab(ui.ctx(), open, &tip, rect.right(), full.top()) {
                match &properties {
                    Some(properties) if !locked => chosen.push(properties.toggle.clone()),
                    Some(_) => {}
                    None => self.panel_open = !self.panel_open,
                }
            }
        }
    }

    /// Takes what the assistant sent: entries to show, tool calls to route,
    /// the start and end of a request, and the model list. Then passes back
    /// the sub-applications' answers, and tells the assistant when the
    /// running sub-applications' tabs or skills changed.
    fn pump_assistant(&mut self) {
        if !self.listed && self.native_dialogs {
            self.listed = true;
            self.assistant.send(Command::ListModels);
        }
        while let Some(event) = self.assistant.try_recv() {
            match event {
                assistant::Event::Entry(entry) => self.chat.conversation.push(entry),
                assistant::Event::Cleared => self.chat.conversation.clear(),
                assistant::Event::Working(working) => {
                    self.chat.working = working;
                    if !working {
                        self.routing.end();
                    }
                }
                assistant::Event::Models(models) => {
                    // The picker offers its short list, each model able to see
                    // the pictures the assistant looks at.
                    let models = models.map(crate::workspace::picker_models);
                    // A remembered model the list no longer offers, such as a
                    // batch variant, gives way to the default.
                    let offered = |chosen: &ModelChoice, list: &[assistant::Model]| {
                        list.iter().any(|model| model.id == chosen.id)
                    };
                    if let Ok(list) = &models
                        && self.chat.model.as_ref().is_none_or(|chosen| !offered(chosen, list))
                        && let Some(model) = crate::workspace::default_model(list)
                    {
                        self.chat.model = Some(ModelChoice {
                            id: model.id.clone(),
                            name: model.name.clone(),
                        });
                    }
                    self.chat.models = Some(models);
                }
                assistant::Event::Call {
                    call,
                    app,
                    tab,
                    tool,
                    input,
                    group,
                } => {
                    let message = HostMessage::Invoke {
                        call: call.clone(),
                        tab,
                        tool,
                        input,
                        group,
                    };
                    if let Some(answer) = self.routing.call(&mut self.apps, call, &app, message) {
                        self.assistant.send(answer);
                    }
                }
            }
        }
        for answer in self.routing.poll(&mut self.apps) {
            self.assistant.send(answer);
        }
        if let Some(targets) = self.routing.changed_targets(&self.apps) {
            self.assistant.send(Command::Targets(targets));
        }
    }

    /// What the person did in the chat panel.
    fn chat_event(&mut self, event: ChatEvent, ctx: &egui::Context, frame: &mut eframe::Frame) {
        match event {
            ChatEvent::Send => {
                if self.chat.working {
                    return;
                }
                let text = std::mem::take(&mut self.chat.input).trim().to_string();
                let mut unread = Vec::new();
                let attachments = std::mem::take(&mut self.chat.chips)
                    .iter()
                    .filter_map(|path| {
                        let loaded = chat::load_attachment(path);
                        if loaded.is_none() {
                            unread.push(path.display().to_string());
                        }
                        loaded
                    })
                    .collect();
                let selected = self.apps.get(self.selected).and_then(|started| {
                    Some((started.id.clone(), started.running.tabs().1?))
                });
                let request = assistant::Request {
                    text,
                    attachments,
                    model: self
                        .chat
                        .model
                        .as_ref()
                        .map(|model| model.id.clone())
                        .unwrap_or_default(),
                    selected,
                    targets: route::targets(&self.apps),
                };
                self.assistant.send(Command::Send(request));
                for path in unread {
                    self.assistant
                        .send(Command::Note(format!("{path} could not be read, so it was not sent.")));
                }
            }
            ChatEvent::Stop => self.assistant.send(Command::Stop),
            ChatEvent::Attach => {
                let filter = protocol::FileFilter {
                    name: "Pictures and text files".to_string(),
                    extensions: chat::PICTURES
                        .iter()
                        .chain(chat::TEXTS)
                        .map(|ext| ext.to_string())
                        .collect(),
                };
                let picked = if self.native_dialogs {
                    crate::dialogs::pick_files(frame, &filter)
                } else {
                    self.scripted_path().into_iter().collect()
                };
                for path in picked {
                    if !self.chat.chips.contains(&path) {
                        self.chat.chips.push(path);
                    }
                }
            }
            ChatEvent::ListModels => {
                self.chat.models = None;
                self.assistant.send(Command::ListModels);
            }
            ChatEvent::Model(model) => {
                let choice = ModelChoice {
                    id: model.id,
                    name: model.name,
                };
                self.chat.model = Some(choice.clone());
                self.settings.choose_model(choice);
            }
            ChatEvent::Logo(choice) => self.logo(choice, ctx, frame),
            ChatEvent::Project(choice) => self.project_menu(choice, frame),
            ChatEvent::Renamed(name) => self.commit_rename(name),
            ChatEvent::CancelRename => self.renaming = false,
        }
    }

    /// The workspace is shown. With no project, the shell stays up, which is
    /// how the frame checks draw it.
    fn workspace_shown(&self) -> bool {
        self.project
            .as_ref()
            .map(|project| project.workspace_open)
            .unwrap_or(true)
    }

    /// Other projects, newest change first, for the menu's switch list.
    /// The files are read when the list is first needed and after a project
    /// opens, not on every frame.
    fn other_projects(&mut self) -> Vec<(String, String)> {
        if let Some(listed) = &self.listed_projects {
            return listed.clone();
        }
        let listed = self.read_other_projects();
        self.listed_projects = Some(listed.clone());
        listed
    }

    fn read_other_projects(&self) -> Vec<(String, String)> {
        let Some(data) = &self.data else {
            return Vec::new();
        };
        let current = self.project.as_ref().map(|project| project.id.as_str());
        workspace::list_projects(data)
            .into_iter()
            .filter(|listed| Some(listed.id.as_str()) != current)
            .map(|listed| (listed.id, listed.file.name))
            .collect()
    }

    /// The properties panel's width when the workspace is shown and it is open.
    fn panel_width(&self, app: usize, tab: Option<&str>) -> f32 {
        let Some(tab) = tab else {
            return 0.0;
        };
        if !self.has_panel(app, tab) {
            return 0.0;
        }
        let open = self
            .apps
            .get(app)
            .and_then(|started| started.running.declared.properties.as_ref())
            .map_or(self.panel_open, |properties| properties.open);
        if open { panel::WIDTH } else { 0.0 }
    }

    /// Sends the contents size to every sub-application when it changes.
    fn publish_viewport(&mut self) {
        let viewport = self.viewport;
        if viewport[0] <= 1.0 || viewport[1] <= 1.0 {
            return;
        }
        let changed = self.sent_viewport.is_none_or(|sent| {
            (sent[0] - viewport[0]).abs() > 0.5 || (sent[1] - viewport[1]).abs() > 0.5
        });
        if !changed {
            return;
        }
        self.sent_viewport = Some(viewport);
        for app in 0..self.apps.len() {
            self.send(app, HostMessage::Viewport { viewport });
        }
    }

    /// Keeps the running tabs as a project in `data`, without restarting them.
    #[cfg(test)]
    fn remember_in(&mut self, data: PathBuf) {
        let _ = std::fs::create_dir_all(&data);
        self.data = Some(data.clone());
        self.settings = Settings::load(Some(data.join("settings.json")));
        let (id, file) = workspace::create_project(&data).expect("a project");
        self.project = Some(Project {
            id: id.clone(),
            name: file.name,
            folders: file.folders,
            workspace_open: true,
        });
        self.settings.choose_project(&id);
        self.write_project(true);
    }

    /// The path a frame test gives in place of a file dialog.
    fn scripted_path(&mut self) -> Option<PathBuf> {
        #[cfg(test)]
        {
            self.scripted_paths.pop_front()
        }
        #[cfg(not(test))]
        {
            None
        }
    }
    /// Every running sub-application's tabs, in the order their programs
    /// started, then in the order each reports them.
    fn strip(&self) -> Vec<StripTab> {
        let mut strip = Vec::new();
        for (index, started) in self.apps.iter().enumerate() {
            let (tabs, active) = started.running.tabs();
            for tab in tabs {
                strip.push(StripTab {
                    app: index,
                    active: index == self.selected && active.as_deref() == Some(tab.id.as_str()),
                    id: tab.id,
                    label: tab.label,
                    close_waits: tab.close_waits,
                    unsaved: tab.dirty,
                });
            }
        }
        strip
    }

    fn editing_text(&self, app: usize, tab: &str) -> bool {
        self.apps
            .get(app)
            .and_then(|started| started.running.declared.canvas.as_ref())
            .filter(|canvas| canvas.tab == tab)
            .is_some_and(|canvas| canvas.caret.is_some())
    }

    /// The drawing surface for the tab's display copy.
    fn show_canvas(
        &mut self,
        ui: &mut egui::Ui,
        app: usize,
        tab: &str,
        rect: Rect,
        dismissing: bool,
    ) {
        let viewport = self.viewport;
        let mut out = Vec::new();
        let escape = ui.input(|input| input.key_pressed(Key::Escape));
        {
            let Some(started) = self.apps.get_mut(app) else {
                return;
            };
            let declared = &mut started.running.declared;
            let canvas = declared.canvas.as_ref().filter(|canvas| canvas.tab == tab);
            let Some(display) = declared.documents.get_mut(tab) else {
                let painter = ui.painter().with_clip_rect(rect);
                painter.rect_filled(rect, 0.0, CANVAS);
                return;
            };
            let mut doc = Doc {
                tab,
                display,
                canvas,
                out: &mut out,
                live: &mut self.live,
                viewport,
            };
            if !self.surface.locked {
                self.surface.commit_text_on_outside_press(ui, &mut doc);
                if escape && !ui.ctx().text_edit_focused() || escape && doc.text_editing() {
                    self.surface.escape(&mut doc);
                }
            }
            self.surface.show(ui, &mut doc, rect, dismissing);
        }
        for message in out {
            self.send(app, message);
        }
    }

    /// The approved notices: the sub-application has exited, or is not
    /// responding. The rest of the window stays usable under them.
    fn paint_status(&self, ui: &egui::Ui, app: usize, rect: Rect) {
        let Some(started) = self.apps.get(app) else {
            return;
        };
        let running = &started.running;
        let (text, ring) = if running.exited() && !running.pending() && self.has_started(app) {
            (format!("{} has exited.", started.name), false)
        } else if running.not_responding() {
            (format!("{} is not responding.", started.name), true)
        } else {
            return;
        };
        let painter = ui.painter().with_clip_rect(rect);
        let font = egui::FontId::proportional(17.0);
        let galley = painter.layout_no_wrap(text, font, widgets::MUTED);
        let ring_w = if ring { 30.0 } else { 0.0 };
        let left = rect.center().x - (galley.size().x + ring_w) / 2.0;
        let top = rect.center().y - galley.size().y / 2.0;
        if ring {
            painter.circle_stroke(
                egui::pos2(left + 9.0, rect.center().y),
                8.0,
                egui::Stroke::new(2.0, widgets::MUTED),
            );
        }
        painter.galley(egui::pos2(left + ring_w, top), galley, widgets::MUTED);
    }

    /// A sub-application that declared itself and then exited, as opposed to
    /// none started at all.
    fn has_started(&self, app: usize) -> bool {
        self.apps
            .get(app)
            .is_some_and(|started| !started.running.declaration().tabs.is_empty())
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

/// egui-winit replaces Ctrl/Cmd+C, X, and V with copy, cut, and paste
/// events and drops the key-down, so the chord check never sees them.
/// Paste is left out entirely when the operating-system clipboard is
/// empty. The key-up still arrives. Run the declared action from the
/// event, or from that key-up when the event never came. `run` is false
/// while a text field owns the keys.
fn take_clipboard(
    actions: &mut Vec<Action>,
    action: Option<&Action>,
    press: bool,
    release: bool,
    seen: &mut bool,
    run: bool,
) {
    let Some(action) = action else {
        return;
    };
    if press {
        *seen = true;
        if run {
            actions.push(action.clone());
        }
    }
    if release {
        let already = *seen;
        *seen = false;
        if run && !already {
            actions.push(action.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HostWindow, Started};
    use crate::process::Running;
    use crate::tablet::Tablet;
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
            Self::with(template())
        }

        /// A window running one sub-application that declared `messages`.
        fn with(messages: Vec<protocol::AppMessage>) -> Self {
            let ctx = egui::Context::default();
            let started = Started {
                id: "fixture".to_string(),
                name: "Template".to_string(),
                executable: None,
                running: Running::declared(messages),
            };
            let app = HostWindow::with_apps(&ctx, vec![started], Tablet::none());
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

        /// The window's close is pressed.
        fn close(&mut self) {
            let mut input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
                ..Default::default()
            };
            input
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .events
                .push(egui::ViewportEvent::Close);
            let Self { ctx, app, frame, .. } = self;
            let mut output = ctx.run_ui(input, |ui| app.ui(ui, frame));
            output.textures_delta.clear();
        }

        /// Frames until `done`, waiting for the assistant's thread.
        fn until(&mut self, done: impl Fn(&HostWindow) -> bool) {
            for _ in 0..200 {
                if done(&self.app) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
                self.step(Vec::new());
            }
            panic!("the window never got there");
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
        // The chat panel takes a quarter of the window on the left.
        let shell = f64::from(SCREEN.x - crate::chat::width(SCREEN.x));
        assert!((wide - shell).abs() < 1.0, "canvas width {wide}");

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

    fn entry(role: assistant::Role, text: &str) -> assistant::Entry {
        assistant::Entry {
            role,
            text: text.to_string(),
            time: 1_760_000_000,
            attachments: Vec::new(),
            tool: None,
            ok: None,
        }
    }

    /// A sub-application showing one document from a file, with a camera.
    fn with_a_document() -> Vec<protocol::AppMessage> {
        use protocol::{AppMessage, ContentsKind, DocumentCamera, TabEntry, Tabs};
        vec![
            AppMessage::Hello {
                protocol: protocol::PROTOCOL_VERSION,
            },
            AppMessage::Tab {
                id: "7".into(),
                label: "notes".into(),
            },
            AppMessage::Contents {
                tab: "7".into(),
                kind: ContentsKind::Canvas,
            },
            AppMessage::End,
            AppMessage::Tabs(Tabs {
                tabs: vec![TabEntry {
                    id: "7".into(),
                    label: "notes".into(),
                    dirty: false,
                    close_waits: false,
                    path: Some("/somewhere/notes.file".into()),
                    saves: true,
                }],
                active: "7".into(),
                new_tab: None,
                new_tab_tip: String::new(),
            }),
            AppMessage::Document {
                tab: "7".into(),
                sketch: serde_json::json!({
                    "format_version": 1,
                    "id": 41,
                    "name": "Private Words Of The Work",
                    "camera": {"origin": [0.0, 0.0], "zoom": 1.0},
                    "nodes": [],
                }),
                selection: Vec::new(),
                active_canvas: None,
                camera: Some(DocumentCamera {
                    origin: [-30.0, 12.5],
                    zoom: 2.0,
                }),
            },
        ]
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let folder = std::env::temp_dir().join(format!("inpainter-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// Catches a project that keeps its own copy of the work: the file holds
    /// the tab's path, label, and camera, and none of the document.
    #[test]
    fn a_project_holds_paths_not_work() {
        let mut harness = Harness::with(with_a_document());
        let mut tool = entry(assistant::Role::Tool, "");
        tool.tool = Some("fixture__edit__grow".into());
        tool.ok = Some(true);
        harness.app.chat.conversation = vec![
            entry(assistant::Role::Person, "make it larger"),
            tool,
            entry(assistant::Role::Assistant, "Done."),
        ];
        let folder = scratch("save");
        harness.app.remember_in(folder.clone());

        let id = harness.app.project.as_ref().unwrap().id.clone();
        let path = crate::workspace::project_path(&folder, &id);
        let text = std::fs::read_to_string(&path).expect("the project was written");
        let file = crate::workspace::ProjectFile::parse(&text).unwrap();
        assert_eq!(file.tabs.len(), 1);
        assert_eq!(file.tabs[0].app, "fixture");
        assert_eq!(file.tabs[0].path.as_deref(), Some("/somewhere/notes.file"));
        let camera = file.tabs[0].camera.expect("the camera is kept");
        assert_eq!((camera.origin, camera.zoom), ([-30.0, 12.5], 2.0));
        assert_eq!(file.selected, Some(0));
        assert_eq!(file.conversation, harness.app.chat.conversation);
        for word in ["Private Words", "nodes", "format_version", "\"41\"", "selection"] {
            assert!(!text.contains(word), "the project holds {word:?}:\n{text}");
        }
        assert!(!text.contains("\"input\"") && !text.contains("\"output\""));
        assert!(!harness.app.project_dirty(), "a written project has no changes");
        std::fs::remove_dir_all(folder).unwrap();
    }

    /// Closing does not ask to keep the project. The quit flow starts, and
    /// the project is written when the sub-applications are ready.
    #[test]
    fn closing_writes_the_project_without_asking() {
        let mut fresh = Harness::new();
        fresh.step(Vec::new());
        fresh.close();
        assert!(fresh.app.flows.question_shown().is_none());
        assert!(fresh.app.flows.closing_started(), "the quit flow did not start");

        let mut changed = Harness::with(with_a_document());
        let folder = scratch("close");
        changed.app.remember_in(folder.clone());
        changed
            .app
            .chat
            .conversation
            .push(entry(assistant::Role::Person, "hello"));
        changed.close();
        assert!(changed.app.flows.question_shown().is_none());
        assert!(changed.app.flows.closing_started());
        changed.app.apps[0].running.declared.quit_ready = true;
        changed.step(Vec::new());
        let id = changed.app.project.as_ref().unwrap().id.clone();
        let text = std::fs::read_to_string(crate::workspace::project_path(&folder, &id)).unwrap();
        assert!(text.contains("hello"), "closing did not keep the conversation");
        std::fs::remove_dir_all(folder).unwrap();
    }

    /// Catches a reopened project that loses its conversation, or that
    /// silently drops a tab whose sub-application is not installed.
    #[test]
    fn opening_a_project_restores_the_conversation_and_says_what_was_left_out() {
        let folder = scratch("open");
        let mut harness = Harness::new();
        harness.step(Vec::new());
        harness.app.remember_in(folder.clone());
        let file = crate::workspace::ProjectFile {
            version: crate::workspace::VERSION,
            name: "Older".into(),
            folders: Vec::new(),
            tabs: vec![crate::workspace::ProjectTab {
                app: "no-such-app".into(),
                path: None,
                label: "Gone".into(),
                camera: None,
            }],
            selected: Some(0),
            workspace_open: false,
            conversation: vec![entry(assistant::Role::Person, "hello")],
        };
        file.write(&crate::workspace::project_path(&folder, "older")).unwrap();

        let Harness { app, frame, .. } = &mut harness;
        app.project_menu(crate::chat::ProjectChoice::Switch("older".into()), frame);
        assert!(harness.app.flows.closing_started());
        // The sub-application has resolved its unsaved work.
        harness.app.apps[0].running.declared.quit_ready = true;
        harness.until(|app| app.chat.conversation.len() == 2);
        assert_eq!(harness.app.chat.conversation[0].text, "hello");
        let left_out = &harness.app.chat.conversation[1];
        assert_eq!(left_out.role, assistant::Role::Note);
        assert!(left_out.text.contains("Gone"), "{}", left_out.text);
        assert_eq!(harness.app.project.as_ref().map(|project| project.id.as_str()), Some("older"));
        std::fs::remove_dir_all(folder).unwrap();
    }

    /// Catches a lock that lets the work be changed by hand during a
    /// request, or one that stops tabs from being switched or watched.
    #[test]
    fn the_lock_holds_changes_and_lets_tabs_and_cameras_through() {
        use protocol::{HostMessage, SelectOp};
        let command = |id: &str| HostMessage::Command {
            id: id.into(),
            tab: Some("7".into()),
            arg: None,
            node: None,
            viewport: [1.0, 1.0],
        };
        assert!(!super::waits_while_locked(&command("select_tab")));
        assert!(super::waits_while_locked(&command("close_tab")));
        assert!(super::waits_while_locked(&command("undo")));
        assert!(super::waits_while_locked(&HostMessage::Gesture {
            tab: "7".into(),
            gesture: serde_json::json!({}),
        }));
        assert!(super::waits_while_locked(&HostMessage::Select {
            tab: "7".into(),
            op: SelectOp::Clear,
        }));
        assert!(super::waits_while_locked(&HostMessage::Property {
            prop: serde_json::json!("size"),
            value: serde_json::json!(3),
            preview: false,
        }));
        assert!(!super::waits_while_locked(&HostMessage::Camera {
            tab: "7".into(),
            origin: [0.0, 0.0],
            zoom: 1.0,
        }));
        assert!(!super::waits_while_locked(&HostMessage::Invoke {
            call: "c".into(),
            tab: "7".into(),
            tool: "a__b__c".into(),
            input: serde_json::json!({}),
            group: "g".into(),
        }));
    }

    /// Catches a call routed to the wrong process, a late answer passed on,
    /// or a call to an exited process left waiting forever.
    #[test]
    fn tool_calls_reach_their_process_and_fail_when_it_is_gone() {
        use assistant::Command;
        let mut apps = vec![
            Started {
                id: "gone".into(),
                name: "Gone".into(),
                executable: None,
                running: Running::not_started(),
            },
            Started {
                id: "here".into(),
                name: "Here".into(),
                executable: None,
                running: Running::declared(template()),
            },
        ];
        let invoke = |call: &str| protocol::HostMessage::Invoke {
            call: call.into(),
            tab: "template".into(),
            tool: "here__s__t".into(),
            input: serde_json::json!({}),
            group: "g".into(),
        };
        let mut routing = crate::route::Routing::default();
        let Some(Command::Result(result)) = routing.call(&mut apps, "c1".into(), "gone", invoke("c1"))
        else {
            panic!("an exited process's call was not answered");
        };
        assert!(!result.ok);
        assert_eq!(result.call, "c1");

        assert!(routing.call(&mut apps, "c2".into(), "here", invoke("c2")).is_none());
        let answer = |call: &str| protocol::ToolResult {
            call: call.into(),
            ok: true,
            output: serde_json::json!({"made": 3}),
            images: Vec::new(),
        };
        apps[1].running.declared.results.push_back(answer("late"));
        apps[1].running.declared.results.push_back(answer("c2"));
        let out = routing.poll(&mut apps);
        assert_eq!(out, vec![Command::Result(answer("c2"))]);
        assert!(routing.poll(&mut apps).is_empty());

        let targets = crate::route::targets(&apps);
        assert_eq!(targets.len(), 1, "an exited process is not offered");
        assert_eq!(targets[0].app, "here");
        assert_eq!(targets[0].tabs[0].label, "Template");
    }
}
