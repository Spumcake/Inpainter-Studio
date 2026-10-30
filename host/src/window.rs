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
use crate::workspace::{self, Settings};
use crate::{ask, grid, icons, panel, tabs, toolbar, widgets};
use assistant::{Assistant, Command};
use eframe::egui::{self, Color32, Event, Key, Rect};
use protocol::{Action, ContentsKind, FileKind, HostMessage, MenuEntry, PanelSide};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

mod flows;
mod settings;
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

/// An open file being offered to sub-applications, one at a time.
struct ClaimJob {
    id: String,
    path: String,
    request_id: String,
    requester: usize,
    /// The requesting application first, then the others.
    order: Vec<usize>,
    cursor: usize,
    waiting_since: Option<Instant>,
}

/// How long a sub-application may stay silent about a file before it counts
/// as not claiming it, so Open still finishes.
const CLAIM_WAIT: std::time::Duration = std::time::Duration::from_millis(400);

/// The project open in this window. The host owns it; no sub-application sees it.
struct Project {
    id: String,
    name: String,
    /// The project's folder, which holds its activities and `.inpainter`.
    folder: PathBuf,
    /// The open session: its id, name, and when it was made.
    session: String,
    session_name: String,
    session_created: u64,
    /// The workspace panel is shown beside the conversation.
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
    /// The pointer on a raster tab, and the previews of what it sent.
    raster: crate::raster::Pointer,
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
    /// Cells being typed or run. The host draws every cell from here and
    /// from what a sub-application reported.
    board: crate::cells::Board,
    /// Endpoint keys, read once each from the keyring.
    keys: crate::keys::Keys,
    /// Connection tests and model lists, by endpoint.
    probes: crate::endpoints::Probes,
    /// The assistant's endpoint and prompt as last sent, so a change in
    /// settings is sent again.
    configured: Option<String>,
    /// The level this session's composer chose, by id. None is the default.
    level: Option<String>,
    settings_panel: crate::settings_panel::SettingsPanel,
    /// A cell's level picker, open under its chevron: the cell's id and where.
    model_menu: Option<(String, Rect)>,
    /// A conversation cell the person is dragging.
    dragging: Option<protocol::Cell>,
    /// A notebook cell's "…" menu, open under that rect.
    cell_menu: Option<(String, Rect)>,
    /// The notebook has the keyboard: its keys act on the active cell.
    notebook_focus: bool,
    /// The active cell is selected and shows its border: a click on it,
    /// or the active cell changing, selects it; any other click clears it.
    /// The bar stays either way.
    cell_selected: bool,
    /// The active cell last drawn, so a change of it selects it.
    shown_active: Option<String>,
    /// Where the open "…" menu was drawn, so a click in it keeps the
    /// border.
    cell_menu_rect: Option<Rect>,
    /// A cell whose prompt takes the keyboard on the next frame.
    focus_prompt: Option<String>,
    /// A declared key sequence partly pressed: the binding, how many of its
    /// chords are done, and when the last one was.
    sequence: Option<(usize, usize, std::time::Instant)>,
    /// The pointer was released on this cells tab.
    drop_target: Option<(usize, String)>,
    claim: Option<ClaimJob>,
    claim_seq: u64,
    settings: Settings,
    /// Where projects are kept. None in frame tests, which never write one.
    data: Option<PathBuf>,
    project: Option<Project>,
    /// Where Studio makes a project by itself, and where the New Project
    /// form starts: `~/Inpainter`. A frame test leaves it unset, and its
    /// projects go beside its data folder instead.
    projects_root: Option<PathBuf>,
    /// The session menu's state.
    sessions: crate::sessions::SessionMenu,
    /// When the project file was last written, so a quiet change waits.
    last_project_write: Option<Instant>,
    /// The change a write failed on. It is tried again only after a new one.
    failed_signature: Option<String>,
    flows: flows::Flows,
    /// The gray bar, last drawn, so a check can press it.
    bar_rect: Rect,
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
        window.projects_root = workspace::default_projects_dir();
        window.settings = Settings::load(Settings::default_file());
        window.keys = crate::keys::Keys::new(window.data.clone());
        window.first_start();
        if let Some(root) = &window.settings.projects_root {
            window.projects_root = Some(PathBuf::from(root));
        }
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
            raster: crate::raster::Pointer::default(),
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
            board: crate::cells::Board::default(),
            keys: crate::keys::Keys::default(),
            probes: crate::endpoints::Probes::default(),
            configured: None,
            level: None,
            settings_panel: crate::settings_panel::SettingsPanel::default(),
            model_menu: None,
            dragging: None,
            cell_menu: None,
            notebook_focus: false,
            cell_selected: false,
            shown_active: None,
            cell_menu_rect: None,
            focus_prompt: None,
            sequence: None,
            drop_target: None,
            claim: None,
            claim_seq: 0,
            settings: Settings::load(None),
            data: None,
            project: None,
            projects_root: None,
            sessions: crate::sessions::SessionMenu::default(),
            last_project_write: None,
            failed_signature: None,
            flows: flows::Flows::default(),
            bar_rect: Rect::NOTHING,
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
            // A raster tab's camera answers the drawing surface's own
            // commands, and `canvas.zoom` sets a zoom by number.
            if let Some(display) = declared.rasters.get_mut(&tab)
                && Doc::moves_camera_only(&action.id)
            {
                match action.id.as_str() {
                    "canvas.zoom_in" => crate::raster::zoom_step(display, true, viewport),
                    "canvas.zoom_out" => crate::raster::zoom_step(display, false, viewport),
                    "canvas.fit" => crate::raster::fit(display, viewport),
                    "canvas.zoom_to_selection" => crate::raster::zoom_to_selection(display, viewport),
                    _ => {
                        if let Some(zoom) = asked_zoom(action) {
                            crate::raster::set_zoom(display, zoom, viewport);
                        }
                    }
                }
                self.live.moved = true;
                return self.send_camera();
            }
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
        let Some(declared) = self.apps.get(app).map(|started| &started.running.declared) else {
            return;
        };
        let Some(camera) = declared
            .documents
            .get(&tab)
            .map(|display| display.camera)
            .or_else(|| declared.rasters.get(&tab).and_then(|display| display.camera))
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
                let folder = self.dialog_folder(request.folder.as_deref());
                let path = match request.kind {
                    FileKind::Open => crate::dialogs::pick_open(
                        frame,
                        folder.as_deref(),
                        request.filter.as_ref(),
                    ),
                    FileKind::Save => crate::dialogs::pick_save(
                        frame,
                        folder.as_deref(),
                        request.name.as_deref().unwrap_or_default(),
                        request.filter.as_ref(),
                    ),
                };
                self.deliver_file(app, request, path);
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
        let pressed: Vec<Action> = self.pressed_bindings(ctx, &keys.bindings);
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

impl HostWindow {
    /// The declared actions whose keys were pressed this frame. A binding
    /// with chords to follow, such as D D, waits for each in turn, at most a
    /// second apart; any other key in between cancels it.
    fn pressed_bindings(&mut self, ctx: &egui::Context, bindings: &[protocol::Binding]) -> Vec<Action> {
        const BETWEEN: std::time::Duration = std::time::Duration::from_secs(1);
        let now = std::time::Instant::now();
        let mut fired = Vec::new();
        let any_key = ctx.input(|input| input.events.iter().any(|event| matches!(event, egui::Event::Key { pressed: true, .. })));
        if let Some((index, done, at)) = self.sequence.take()
            && now.duration_since(at) <= BETWEEN
            && let Some(binding) = bindings.get(index)
        {
            let next = &binding.then[done - 1];
            if ctx.input(|input| icons::chord_pressed(input, next)) {
                if done == binding.then.len() {
                    fired.push(binding.action.clone());
                } else {
                    self.sequence = Some((index, done + 1, now));
                }
                return fired;
            }
            if !any_key {
                self.sequence = Some((index, done, at));
            }
        }
        for (index, binding) in bindings.iter().enumerate() {
            if !ctx.input(|input| icons::chord_pressed(input, &binding.chord)) {
                continue;
            }
            if binding.then.is_empty() {
                fired.push(binding.action.clone());
            } else {
                self.sequence = Some((index, 1, now));
            }
        }
        fired
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
        // A card names its model as the picker does, without the provider.
        let known = self.probes.known_models();
        let names: Vec<(String, String)> = known
            .iter()
            .map(|model| (model.id.clone(), chat::short_name(&model.name).to_string()))
            .collect();
        crate::cells::set_model_names(&ctx, &names);
        crate::cells::set_picture_root(&ctx, self.project.as_ref().map(|project| project.folder.clone()));
        self.show_levels(&known);
        self.configure_assistant();
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
        self.take_cell_requests();
        self.tick_cells();
        self.poll_claim();
        self.project_frame(&ctx, frame);
        self.resolve_dialogs(frame);

        let app = self.selected.min(self.apps.len().saturating_sub(1));
        self.selected = app;
        let tab = self.active_tab();
        let shown = tab.clone().map(|tab| (app, tab));
        if shown != self.shown {
            self.surface.reset();
            self.live.reset();
            self.raster.reset();
            self.shown = shown;
        }

        // Read before the tab bar runs: a click outside the menu closes it there.
        let dismissing = egui::Popup::is_any_open(&ctx);
        let mut visuals = egui::Visuals::light();
        visuals.panel_fill = Color32::WHITE;
        ctx.set_visuals(visuals);
        ctx.style_mut_of(egui::Theme::Light, widgets::menu_card);

        let mut chosen: Vec<Action> = Vec::new();
        let mut sent: Vec<Sent> = Vec::new();
        let mut talk: Vec<ChatEvent> = Vec::new();
        let workspace_open = self.workspace_shown();
        if let Some(project) = &self.project {
            self.sessions.session = project.session_name.clone();
            self.sessions.project = project.id.clone();
        }
        let mut menu = std::mem::take(&mut self.sessions);
        let mut bar_pressed = false;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let full = ui.max_rect();
                let left = if workspace_open {
                    chat::width(full.width(), self.settings.chat_width)
                } else {
                    full.width()
                };
                let chat_rect =
                    Rect::from_min_max(full.min, egui::pos2(full.left() + left, full.bottom()));
                let shell = Rect::from_min_max(egui::pos2(chat_rect.right(), full.top()), full.max);
                let mut cell_actions = Vec::new();
                talk = chat::show(
                    ui,
                    chat_rect,
                    &mut self.chat,
                    &mut menu,
                    !workspace_open,
                    &mut self.board,
                    &mut cell_actions,
                );
                self.note_cell_actions(cell_actions);
                if workspace_open {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(shell), |ui| {
                        self.shell(ui, &ctx, app, tab.clone(), dismissing, &mut chosen, &mut sent);
                    });
                } else {
                    self.viewport = hidden_viewport(full, self.settings.chat_width, self.panel_width(app, tab.as_deref()));
                }
                let edge = if workspace_open { chat_rect.right() } else { full.right() };
                if workspace_open {
                    self.drag_panel_edge(ui, edge, full);
                }
                let (rect, pressed) = workspace_bar(ui, edge, workspace_open, full);
                self.bar_rect = rect;
                bar_pressed = pressed;
            });
        self.sessions = menu;
        self.draw_settings(&ctx, frame);
        self.draw_model_menu(&ctx);
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
        self.finish_drag(&ctx);
    }
}

impl HostWindow {
    /// The folder a file dialog starts in when the request names none: the
    /// project's own folder.
    fn project_files_dir(&self) -> Option<PathBuf> {
        Some(self.project.as_ref()?.folder.clone())
    }

    /// The folder a file dialog opens in: the one the request names, or the
    /// project's own folder.
    pub(crate) fn dialog_folder(&self, requested: Option<&str>) -> Option<PathBuf> {
        let files = self.project_files_dir();
        workspace::dialog_directory(requested.map(std::path::Path::new), files.as_deref())
    }

    /// A file dialog's answer. An open is offered to the sub-applications
    /// before anyone receives the path. A save goes straight back.
    fn deliver_file(
        &mut self,
        app: usize,
        request: crate::process::FileRequest,
        path: Option<PathBuf>,
    ) {
        let Some(path) = path else {
            self.send(
                app,
                HostMessage::FileResult {
                    id: request.id,
                    path: None,
                },
            );
            return;
        };
        if request.kind == FileKind::Open {
            // A file from outside the project is never opened in place, and
            // no sub-application sees its path: it is imported first.
            let outside = self
                .project
                .as_ref()
                .is_some_and(|project| workspace::relative_to_project(&project.folder, &path).is_none());
            if outside {
                self.ask_import(app, request.id, path);
                return;
            }
            self.begin_claim(app, request.id, path.display().to_string());
            return;
        }
        self.send(
            app,
            HostMessage::FileResult {
                id: request.id,
                path: Some(path.display().to_string()),
            },
        );
    }

    /// An imported copy goes on as a picked file inside the project would.
    pub(super) fn claim_imported(&mut self, requester: usize, request_id: String, copy: PathBuf) {
        self.begin_claim(requester, request_id, copy.display().to_string());
    }

    fn begin_claim(&mut self, requester: usize, request_id: String, path: String) {
        self.claim_seq += 1;
        let mut order = vec![requester];
        order.extend((0..self.apps.len()).filter(|index| *index != requester));
        self.claim = Some(ClaimJob {
            id: format!("claim-{}", self.claim_seq),
            path,
            request_id,
            requester,
            order,
            cursor: 0,
            waiting_since: None,
        });
        self.poll_claim();
    }

    /// One step of asking who owns an opened file.
    fn poll_claim(&mut self) {
        let Some(job) = self.claim.as_ref() else {
            return;
        };
        if job.cursor >= job.order.len() {
            self.claim_unclaimed();
            return;
        }
        if job.waiting_since.is_some() {
            let app = job.order[job.cursor];
            let id = job.id.clone();
            let waited = job.waiting_since.unwrap_or_else(Instant::now);
            let reply = self.apps.get_mut(app).and_then(|started| {
                let queued = &mut started.running.declared.claimed;
                queued
                    .iter()
                    .position(|(claim, _)| claim == &id)
                    .and_then(|position| queued.remove(position))
                    .map(|(_, yours)| yours)
            });
            if let Some(yours) = reply {
                self.claim_answered(yours);
                return;
            }
            if waited.elapsed() >= CLAIM_WAIT {
                self.claim_answered(false);
            }
            return;
        }
        let app = job.order[job.cursor];
        let id = job.id.clone();
        let path = job.path.clone();
        self.send(app, HostMessage::Claim { id, path });
        if let Some(job) = self.claim.as_mut() {
            job.waiting_since = Some(Instant::now());
        }
    }

    fn claim_answered(&mut self, yours: bool) {
        let Some(job) = self.claim.as_ref() else {
            return;
        };
        let app = job.order[job.cursor];
        let requester = job.requester;
        if yours {
            let path = job.path.clone();
            let request_id = job.request_id.clone();
            self.claim = None;
            if app == requester {
                self.send(
                    requester,
                    HostMessage::FileResult {
                        id: request_id,
                        path: Some(path),
                    },
                );
            } else {
                self.send(
                    app,
                    HostMessage::Open {
                        path: Some(path),
                    },
                );
                self.send(
                    requester,
                    HostMessage::FileResult {
                        id: request_id,
                        path: None,
                    },
                );
            }
            return;
        }
        if let Some(job) = self.claim.as_mut() {
            job.cursor += 1;
            job.waiting_since = None;
        }
        self.poll_claim();
    }

    fn claim_unclaimed(&mut self) {
        let Some(job) = self.claim.take() else {
            return;
        };
        self.send(
            job.requester,
            HostMessage::FileResult {
                id: job.request_id,
                path: Some(job.path),
            },
        );
    }

    /// Sends one message, with the files chosen for it, to the assistant.
    fn send_message(&mut self, text: String, chips: Vec<PathBuf>) {
        let mut unread = Vec::new();
        let attachments = chips
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
        let level = self.current_level();
        let request = assistant::Request {
            text,
            attachments,
            model: level.as_ref().map(|level| level.model.clone()).unwrap_or_default(),
            effort: level.and_then(|level| level.effort).map(|effort| effort.word().to_string()),
            selected,
            targets: route::targets(&self.apps, &self.settings),
        };
        self.assistant.send(Command::Send(request));
        for path in unread {
            self.assistant
                .send(Command::Note(format!("{path} could not be read, so it was not sent.")));
        }
    }

    /// Text that arrived but never became an entry, as when a request is
    /// stopped or fails midway, stays in the conversation as it was.
    fn keep_live(&mut self) {
        let text = std::mem::take(&mut self.chat.live);
        if !text.trim().is_empty() {
            self.chat.conversation.push(assistant::Entry {
                role: assistant::Role::Assistant,
                text: text.trim().to_string(),
                time: crate::workspace::now_seconds(),
                attachments: Vec::new(),
                tool: None,
                ok: None,
                cell: None,
                aside: false,
            });
        }
    }

    fn note_entry(&mut self, entry: assistant::Entry) {
        match entry.role {
            // The reply's entry replaces the pieces shown while it arrived.
            assistant::Role::Assistant => self.chat.live.clear(),
            assistant::Role::Note => self.keep_live(),
            _ => {}
        }
        if entry.role == assistant::Role::Tool
            && entry.ok.is_some()
            && let Some(pending) = self.chat.conversation.iter_mut().rev().find(|existing| {
                existing.role == assistant::Role::Tool && existing.ok.is_none() && existing.tool == entry.tool
            })
        {
            pending.ok = entry.ok;
            return;
        }
        self.chat.conversation.push(entry);
    }

    /// The host's own tool was called: a prompt, or one of the person's
    /// skills, runs as a cell in the conversation.
    fn begin_host_cell(&mut self, call: String, tool: &str, input: serde_json::Value) {
        let text = |key: &str| input.get(key).and_then(|value| value.as_str()).unwrap_or("").to_string();
        let (prompt, run_on, picture) = match route::skill_for_tool(&self.settings, tool) {
            Some(skill) => {
                // An image skill's tool is asked with the picture's prompt.
                let picture = skill.makes == workspace::Makes::Images;
                let asked = text(if picture { "prompt" } else { "input" });
                let prompt = if skill.prompt.trim().is_empty() {
                    asked
                } else {
                    format!("{}\n\n{asked}", skill.prompt.trim_end())
                };
                let keys = &mut self.keys;
                (prompt, crate::endpoints::skill_route(&self.settings, &skill, |id| keys.get(id)), picture)
            }
            None => (text("prompt"), self.level_route(), false),
        };
        let mut cell = protocol::Cell::text(protocol::cell_id(), &prompt);
        if picture {
            cell.kind = protocol::IMAGE_KIND.to_string();
        }
        if let Ok(route) = &run_on {
            cell.endpoint = Some(route.endpoint.clone());
            cell.model = Some(route.model.clone());
            cell.effort = route.effort.map(|effort| effort.word().to_string());
        }
        if let Some(entry) = self.chat.conversation.iter_mut().rev().find(|entry| {
            entry.role == assistant::Role::Tool && entry.ok.is_none()
        }) {
            entry.cell = Some(cell.clone());
        }
        self.board.drafts.insert(cell.id.clone(), prompt.clone());
        self.launch_run(crate::cells::Place::Conversation, cell.id, Some(call), run_on, prompt, picture);
    }

    fn note_cell_actions(&mut self, actions: Vec<crate::cells::CardAction>) {
        for action in actions {
            match action {
                // Only a conversation cell is dragged, into a notebook; a
                // notebook's own cells move from their menu.
                crate::cells::CardAction::Drag(cell) => {
                    if self.cell_home(&cell.id).is_none() {
                        self.dragging = Some(cell);
                    }
                }
                crate::cells::CardAction::Edit(id, input) => self.edit_prompt(&id, input),
                crate::cells::CardAction::Run(id) => self.run_from_card(&id),
                crate::cells::CardAction::Stop(id) => self.stop_one(&id),
                crate::cells::CardAction::Select(id) => self.select_one(&id),
                // The "…" opens the cell's own menu, in a notebook only.
                crate::cells::CardAction::More(id, at) => {
                    if self.cell_home(&id).is_some() {
                        self.cell_menu = Some((id, at));
                    }
                }
                crate::cells::CardAction::ShowSource(id) => self.show_source(&id),
                crate::cells::CardAction::ShowVersion(id, index) => self.show_version(&id, index),
                crate::cells::CardAction::Models(id, at) => {
                    let open = self.model_menu.as_ref().is_some_and(|(open, _)| *open == id);
                    self.model_menu = if open { None } else { Some((id, at)) };
                }
            }
        }
    }

    fn edit_prompt(&mut self, id: &str, input: Vec<protocol::CellPart>) {
        if let Some(entry) = self.chat.conversation.iter_mut().find(|entry| {
            entry.cell.as_ref().is_some_and(|cell| cell.id == id)
        }) {
            if let Some(cell) = entry.cell.as_mut() {
                cell.input = input;
            }
            return;
        }
        let Some((app, tab)) = self.cell_home(id) else {
            return;
        };
        self.send(
            app,
            HostMessage::EditCell {
                tab,
                cell: id.to_string(),
                input,
            },
        );
    }

    fn run_from_card(&mut self, id: &str) {
        let prompt = self.board.drafts.get(id).cloned().unwrap_or_default();
        let recorded = self.chat.conversation.iter().find_map(|entry| {
            entry.cell.as_ref().filter(|cell| cell.id == id).cloned()
        });
        if let Some(cell) = recorded {
            self.edit_prompt(id, vec![protocol::CellPart::Text { text: prompt.clone() }]);
            let run_on = self.cell_route(&cell);
            let picture = cell.kind == protocol::IMAGE_KIND;
            self.launch_run(crate::cells::Place::Conversation, id.to_string(), None, run_on, prompt, picture);
            return;
        }
        let Some((app, tab)) = self.cell_home(id) else {
            return;
        };
        self.start_book_run(app, &tab, id);
    }

    /// A rendered Markdown cell shows its source, and its prompt takes the
    /// keyboard.
    fn show_source(&mut self, id: &str) {
        let Some((app, tab)) = self.cell_home(id) else {
            return;
        };
        self.send(app, HostMessage::SelectCell { tab: tab.clone(), cell: id.to_string() });
        self.send(app, HostMessage::ShowSource { tab, cell: id.to_string() });
        self.focus_prompt = Some(id.to_string());
    }

    fn select_one(&mut self, id: &str) {
        let Some((app, tab)) = self.cell_home(id) else {
            return;
        };
        self.send(
            app,
            HostMessage::SelectCell {
                tab,
                cell: id.to_string(),
            },
        );
    }

    fn cell_home(&self, id: &str) -> Option<(usize, String)> {
        for (app, started) in self.apps.iter().enumerate() {
            for (tab, (cells, _)) in &started.running.declared.cells {
                if cells.iter().any(|cell| cell.id == id) {
                    return Some((app, tab.clone()));
                }
            }
        }
        None
    }

    fn launch_run(
        &mut self,
        place: crate::cells::Place,
        id: String,
        call: Option<String>,
        run_on: Result<crate::endpoints::Route, String>,
        prompt: String,
        picture: bool,
    ) {
        self.board.runs.retain(|run| run.id != id);
        let (endpoint, model, effort) = match &run_on {
            Ok(route) => (route.endpoint.clone(), route.model.clone(), route.effort.map(|effort| effort.word().to_string())),
            Err(_) => (String::new(), String::new(), None),
        };
        let rx = crate::cells::start_run(run_on, &prompt, picture);
        self.board.runs.push(crate::cells::Run {
            place,
            id,
            call,
            endpoint,
            model,
            effort,
            picture: None,
            prompt,
            text: String::new(),
            rx,
            settled: false,
        });
    }

    fn take_cell_requests(&mut self) {
        let mut runs = Vec::new();
        let mut stops = Vec::new();
        for (app, started) in self.apps.iter_mut().enumerate() {
            runs.extend(
                started
                    .running
                    .declared
                    .run_cells
                    .drain(..)
                    .map(|(tab, cell)| (app, tab, cell)),
            );
            stops.extend(
                started
                    .running
                    .declared
                    .stop_cells
                    .drain(..)
                    .map(|(tab, cell)| (app, tab, cell)),
            );
        }
        for (app, tab, cell) in runs {
            self.start_book_run(app, &tab, &cell);
        }
        for (app, tab, cell) in stops {
            self.stop_one(&cell);
            let _ = (app, tab);
        }
    }

    fn start_book_run(&mut self, app: usize, tab: &str, id: &str) {
        let Some(cell) = self.apps.get(app).and_then(|started| {
            started.running.declared.cells.get(tab).and_then(|(cells, _)| {
                cells.iter().find(|cell| cell.id == id).cloned()
            })
        }) else {
            return;
        };
        if !crate::cells::runs(&cell.kind) {
            return;
        }
        // A cell that has run goes back to its endpoint and model, or to the
        // model chosen from its chevron; a new one runs on the session's level.
        let mut cell = cell;
        if let Some((endpoint, model, effort)) = self.board.chosen.get(id).cloned() {
            cell.endpoint = Some(endpoint);
            cell.model = Some(model);
            cell.effort = effort;
        }
        let run_on = self.cell_route(&cell);
        let prompt = self.board.drafts.get(id).cloned().unwrap_or_else(|| cell.prompt());
        if prompt.trim().is_empty() {
            return;
        }
        if prompt != cell.prompt() {
            self.send(
                app,
                HostMessage::EditCell {
                    tab: tab.to_string(),
                    cell: id.to_string(),
                    input: vec![protocol::CellPart::Text { text: prompt.clone() }],
                },
            );
        }
        // A Markdown cell runs here and now: it shows rendered, calls
        // nothing, and makes no version.
        if cell.kind == protocol::MARKDOWN_KIND {
            self.send(
                app,
                HostMessage::CellResult {
                    tab: tab.to_string(),
                    cell: id.to_string(),
                    input: Vec::new(),
                    endpoint: String::new(),
                    model: String::new(),
                    effort: None,
                    state: protocol::CellState::Done,
                    output: Vec::new(),
                    error: None,
                },
            );
            return;
        }
        self.launch_run(
            crate::cells::Place::Book {
                app,
                tab: tab.to_string(),
            },
            id.to_string(),
            None,
            run_on,
            prompt,
            cell.kind == protocol::IMAGE_KIND,
        );
    }

    fn stop_one(&mut self, id: &str) {
        let Some(index) = self.board.runs.iter().position(|run| run.id == id && !run.settled) else {
            return;
        };
        self.finish_run(index, protocol::CellState::Stopped, None);
    }

    fn stop_conversation_cells(&mut self) {
        let ids: Vec<usize> = self
            .board
            .runs
            .iter()
            .enumerate()
            .filter(|(_, run)| matches!(run.place, crate::cells::Place::Conversation) && !run.settled)
            .map(|(index, _)| index)
            .rev()
            .collect();
        for index in ids {
            self.finish_run(index, protocol::CellState::Stopped, None);
        }
    }

    fn tick_cells(&mut self) {
        let Some((index, piece)) = self.board.poll() else {
            return;
        };
        match piece {
            crate::cells::Piece::Text(_) => self.mirror_live(index),
            crate::cells::Piece::Picture(png) => self.keep_picture(index, png),
            crate::cells::Piece::End => self.finish_run(index, protocol::CellState::Done, None),
            crate::cells::Piece::Failed(reason) => {
                self.finish_run(index, protocol::CellState::Failed, Some(reason))
            }
        }
    }

    fn mirror_live(&mut self, index: usize) {
        let Some(run) = self.board.runs.get(index) else {
            return;
        };
        if !matches!(run.place, crate::cells::Place::Conversation) {
            return;
        }
        let id = run.id.clone();
        let text = run.text.clone();
        if let Some(cell) = self.chat.conversation.iter_mut().find_map(|entry| {
            entry.cell.as_mut().filter(|cell| cell.id == id)
        }) {
            cell.output = vec![protocol::CellPart::Text { text }];
        }
    }

    fn finish_run(&mut self, index: usize, state: protocol::CellState, error: Option<String>) {
        let Some(run) = self.board.runs.get(index) else {
            return;
        };
        if run.settled {
            return;
        }
        let id = run.id.clone();
        let text = run.text.clone();
        let model = run.model.clone();
        let endpoint = run.endpoint.clone();
        let effort = run.effort.clone();
        let picture = run.picture.clone();
        let ran = vec![protocol::CellPart::Text { text: run.prompt.clone() }];
        let call = run.call.clone();
        let place = match &run.place {
            crate::cells::Place::Conversation => None,
            crate::cells::Place::Book { app, tab } => Some((*app, tab.clone())),
        };
        self.board.runs.remove(index);
        // The run's result records the model a chevron chose.
        self.board.chosen.remove(&id);
        // An Image cell's output is its picture, named relative to the project.
        let output = match &picture {
            Some((path, _)) => vec![protocol::CellPart::Path { path: path.clone(), media_type: String::from("image/png") }],
            None => vec![protocol::CellPart::Text { text: text.clone() }],
        };
        match place {
            None => {
                if let Some(cell) = self.chat.conversation.iter_mut().find_map(|entry| {
                    entry.cell.as_mut().filter(|cell| cell.id == id)
                }) {
                    // A conversation cell keeps every run as a version, as a
                    // notebook's cell does, for the Image cell's filmstrip.
                    cell.versions.push(protocol::CellVersion {
                        id: protocol::cell_id(),
                        input: ran.clone(),
                        endpoint: (!endpoint.is_empty()).then(|| endpoint.clone()).or_else(|| cell.endpoint.clone()),
                        model: (!endpoint.is_empty()).then(|| model.clone()).or_else(|| cell.model.clone()),
                        effort: if endpoint.is_empty() { cell.effort.clone() } else { effort.clone() },
                        output: output.clone(),
                        state,
                        error: error.clone(),
                        time: workspace::now_seconds(),
                    });
                    cell.current = Some(cell.versions.len() - 1);
                    cell.agree();
                }
                if let Some(call) = call {
                    let ok = state != protocol::CellState::Failed;
                    let (output, images) = if let Some(reason) = &error {
                        (serde_json::json!({ "error": reason }), Vec::new())
                    } else if let Some((_, png)) = &picture {
                        // The assistant sees the picture it asked for, as the
                        // owner set on 2026-10-30.
                        (serde_json::json!("Made the picture."), vec![assistant::picture_base64(png)])
                    } else {
                        (serde_json::json!(text), Vec::new())
                    };
                    self.assistant.send(Command::Result(protocol::ToolResult {
                        call,
                        ok,
                        output,
                        images,
                    }));
                }
            }
            Some((app, tab)) => {
                if !self.tab_open(app, &tab) {
                    return;
                }
                self.send(
                    app,
                    HostMessage::CellResult {
                        tab,
                        cell: id,
                        input: ran,
                        endpoint,
                        model,
                        effort,
                        state,
                        output,
                        error,
                    },
                );
            }
        }
    }

    fn tab_open(&self, app: usize, tab: &str) -> bool {
        self.apps.get(app).is_some_and(|started| {
            started.running.tabs().0.iter().any(|known| known.id == tab)
        })
    }

    /// A tab that keeps cells, drawn as a notebook: a white column at most
    /// 880 wide on gray, each cell with its "[n]" in the margin, the active
    /// cell marked by a blue bar its full height and, until a click away
    /// from it, a blue border. Cells are the conversation's cards; a
    /// Markdown cell that has run shows as rendered text.
    fn show_cells(&mut self, ui: &mut egui::Ui, app: usize, tab: &str, rect: Rect) {
        let (mut listed, selected) = self
            .apps
            .get(app)
            .and_then(|started| started.running.declared.cells.get(tab).cloned())
            .unwrap_or_default();
        // A model chosen from a chevron shows before the run records it.
        for cell in &mut listed {
            if let Some((endpoint, model, effort)) = self.board.chosen.get(&cell.id) {
                cell.endpoint = Some(endpoint.clone());
                cell.model = Some(model.clone());
                cell.effort = effort.clone();
            }
        }
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect_filled(rect, 0.0, NOTEBOOK_GRAY);
        let width = (rect.width() - 2.0 * crate::typescale::GUTTER).min(NOTEBOOK_COLUMN).max(200.0);
        let column = Rect::from_min_max(
            egui::pos2(rect.center().x - width / 2.0, rect.top() + 16.0),
            egui::pos2(rect.center().x + width / 2.0, rect.bottom()),
        );
        painter.rect_filled(column, 0.0, Color32::WHITE);
        self.board.drop_rect = Some(column);
        if ui.input(|input| input.pointer.any_released())
            && ui.input(|input| input.pointer.interact_pos()).is_some_and(|pos| column.contains(pos))
        {
            self.drop_target = Some((app, tab.to_string()));
        }
        // A press inside the notebook gives it the keyboard; one elsewhere
        // takes it away.
        if let Some(pos) = ui.input(|input| input.pointer.press_origin()).filter(|_| ui.input(|input| input.pointer.any_pressed())) {
            self.notebook_focus = rect.contains(pos);
        }
        let active = selected.as_deref().and_then(|id| listed.iter().position(|cell| cell.id == id));
        let editing = listed
            .iter()
            .any(|cell| ui.memory(|memory| memory.has_focus(egui::Id::new(("cell-prompt", &cell.id)))));
        self.notebook_keys(ui, app, tab, &listed, active, editing);
        if let Some(id) = self.focus_prompt.take() {
            ui.memory_mut(|memory| memory.request_focus(egui::Id::new(("cell-prompt", &id))));
            self.notebook_focus = true;
        }
        // A new active cell, by a click, a key, or adding one, is selected.
        if selected != self.shown_active {
            self.shown_active = selected.clone();
            self.cell_selected = true;
        }
        let press = ui
            .input(|input| input.pointer.press_origin())
            .filter(|_| ui.input(|input| input.pointer.any_pressed()));
        let mut pressed_cell = false;

        let mut actions = Vec::new();
        self.board.cards.clear();
        self.board.bordered = None;
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(column));
        egui::ScrollArea::vertical()
            .id_salt(("cells", tab))
            .auto_shrink([false, false])
            .show(&mut child, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.add_space(NOTEBOOK_GAP);
                let inner = ui.available_width();
                for (index, cell) in listed.iter().enumerate() {
                    if index > 0 {
                        ui.add_space(NOTEBOOK_GAP);
                    }
                    let focused = ui.memory(|memory| memory.has_focus(egui::Id::new(("cell-prompt", &cell.id))));
                    let prompt = cell.prompt();
                    let committed = self
                        .board
                        .committed
                        .entry(cell.id.clone())
                        .or_insert_with(|| prompt.clone())
                        .clone();
                    let mut draft = self.board.drafts.get(&cell.id).cloned().unwrap_or_else(|| prompt.clone());
                    if !focused && prompt != committed {
                        draft = prompt.clone();
                        self.board.committed.insert(cell.id.clone(), prompt);
                    }
                    let live = self.board.running(&cell.id).map(|run| run.text.clone());
                    let card_width = inner - NOTEBOOK_MARGIN - NOTEBOOK_RIGHT;
                    let rendered = crate::cells::rendered(cell) && !focused;
                    let height = if rendered {
                        crate::cells::rendered_height(ui, cell, card_width)
                    } else {
                        crate::cells::card_height(ui, cell, &draft, live.as_deref(), live.is_some(), card_width, false)
                    };
                    let (row, _) = ui.allocate_exact_size(egui::vec2(inner, height), egui::Sense::hover());
                    if press.is_some_and(|pos| row.contains(pos) && ui.clip_rect().contains(pos)) {
                        pressed_cell = true;
                    }
                    let card_row = Rect::from_min_size(
                        egui::pos2(row.left() + NOTEBOOK_MARGIN, row.top()),
                        egui::vec2(card_width, height),
                    );
                    let card = if rendered {
                        crate::cells::paint_rendered(ui, card_row, cell, &mut actions)
                    } else {
                        crate::cells::paint_card(ui, card_row, cell, &mut draft, live.as_deref(), live.is_some(), None, &mut actions)
                    };
                    self.board.drafts.insert(cell.id.clone(), draft);
                    self.board.cards.push((cell.id.clone(), card));
                    let painter = ui.painter();
                    painter.text(
                        egui::pos2(card.left() - 10.0, card.top() + 16.0),
                        egui::Align2::RIGHT_CENTER,
                        format!("[{}]", index + 1),
                        crate::typescale::code(),
                        crate::typescale::MUTED,
                    );
                    if Some(index) == active {
                        // The bar marks the active cell always; the border,
                        // while it is selected or edited.
                        painter.rect_filled(
                            Rect::from_min_size(egui::pos2(row.left() + 6.0, card.top()), egui::vec2(4.0, card.height())),
                            2.0,
                            NOTEBOOK_BLUE,
                        );
                        if self.cell_selected || focused {
                            self.board.bordered = Some(cell.id.clone());
                            painter.rect_stroke(
                                card,
                                12.0,
                                egui::Stroke::new(1.0, NOTEBOOK_BLUE),
                                egui::StrokeKind::Inside,
                            );
                        }
                    }
                    // A click anywhere on a cell makes it active.
                    if ui.input(|input| input.pointer.primary_clicked())
                        && ui.input(|input| input.pointer.interact_pos()).is_some_and(|pos| row.contains(pos))
                        && Some(index) != active
                    {
                        actions.push(crate::cells::CardAction::Select(cell.id.clone()));
                    }
                }
                ui.add_space(NOTEBOOK_GAP);
            });
        // A click on a cell selects it; one anywhere else, in the notebook
        // or out of it, clears the border, except in the open "…" menu.
        if let Some(pos) = press {
            let in_menu = self.cell_menu.is_some() && self.cell_menu_rect.is_some_and(|menu| menu.contains(pos));
            if !in_menu && self.cell_selected != pressed_cell {
                // The cells were drawn before the click was known.
                self.cell_selected = pressed_cell;
                ui.ctx().request_repaint();
            }
        }
        self.note_cell_actions(actions);
        self.cell_menu_popup(ui, app, tab, &listed);
    }

    /// The notebook's own keys, as Jupyter has them, acting on any tab that
    /// keeps cells through the protocol alone. While a prompt is edited,
    /// Escape stops editing and Shift+Enter or Ctrl+Enter runs; otherwise
    /// Enter edits, Up and Down change the active cell, and Shift+Enter and
    /// Ctrl+Enter run.
    fn notebook_keys(
        &mut self,
        ui: &egui::Ui,
        app: usize,
        tab: &str,
        listed: &[protocol::Cell],
        active: Option<usize>,
        editing: bool,
    ) {
        // While a cell's level picker is open, keys are not the notebook's.
        if (!self.notebook_focus && !editing) || self.model_menu.is_some() {
            return;
        }
        let Some(index) = active else {
            return;
        };
        let id = listed[index].id.clone();
        let take = |modifiers: egui::Modifiers, key: egui::Key| ui.input_mut(|input| input.consume_key(modifiers, key));
        let run_down = take(egui::Modifiers::SHIFT, egui::Key::Enter);
        let run_here = take(egui::Modifiers::COMMAND, egui::Key::Enter);
        if run_down || run_here {
            if editing {
                // The edit ends first, so the run takes the typed prompt.
                ui.memory_mut(|memory| memory.surrender_focus(egui::Id::new(("cell-prompt", &id))));
                self.commit_draft(app, tab, &listed[index]);
            }
            self.start_book_run(app, tab, &id);
            if run_down {
                match listed.get(index + 1) {
                    Some(next) => self.send(app, HostMessage::SelectCell { tab: tab.to_string(), cell: next.id.clone() }),
                    None => self.send(app, HostMessage::InsertCell { tab: tab.to_string(), index: listed.len(), cell: None }),
                }
            }
            return;
        }
        if editing {
            if take(egui::Modifiers::NONE, egui::Key::Escape) {
                ui.memory_mut(|memory| memory.surrender_focus(egui::Id::new(("cell-prompt", &id))));
                self.commit_draft(app, tab, &listed[index]);
            }
            return;
        }
        if take(egui::Modifiers::NONE, egui::Key::Enter) {
            if crate::cells::rendered(&listed[index]) {
                self.send(app, HostMessage::ShowSource { tab: tab.to_string(), cell: id.clone() });
            }
            self.focus_prompt = Some(id);
            return;
        }
        let step: isize = if take(egui::Modifiers::NONE, egui::Key::ArrowUp) {
            -1
        } else if take(egui::Modifiers::NONE, egui::Key::ArrowDown) {
            1
        } else {
            0
        };
        if step != 0
            && let Some(next) = index.checked_add_signed(step).and_then(|next| listed.get(next))
        {
            self.send(app, HostMessage::SelectCell { tab: tab.to_string(), cell: next.id.clone() });
        }
    }

    /// Sends a cell's typed prompt to its owner, when it differs.
    fn commit_draft(&mut self, app: usize, tab: &str, cell: &protocol::Cell) {
        let Some(draft) = self.board.drafts.get(&cell.id).cloned() else {
            return;
        };
        if draft != cell.prompt() {
            self.send(
                app,
                HostMessage::EditCell {
                    tab: tab.to_string(),
                    cell: cell.id.clone(),
                    input: vec![protocol::CellPart::Text { text: draft }],
                },
            );
        }
    }

    /// The "…" menu of a notebook cell: back, "n/N", forward, up, down, and
    /// delete, each a protocol message to the cell's owner.
    fn cell_menu_popup(&mut self, ui: &mut egui::Ui, app: usize, tab: &str, listed: &[protocol::Cell]) {
        let Some((id, at)) = self.cell_menu.clone() else {
            return;
        };
        let Some(index) = listed.iter().position(|cell| cell.id == id) else {
            self.cell_menu = None;
            return;
        };
        let cell = &listed[index];
        let count = cell.versions.len();
        let current = cell.current.unwrap_or(0);
        let mut chosen: Option<HostMessage> = None;
        let mut close = false;
        let mut buttons: Vec<(&'static str, Rect)> = Vec::new();
        let area = egui::Area::new(egui::Id::new(("cell-menu", &id)))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(at.right() - CELL_MENU_W + 8.0, at.bottom() + 6.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .fill(Color32::WHITE)
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let mut icon = |ui: &mut egui::Ui, name: &'static str, tip: &str, enabled: bool| {
                                let response = crate::toolbar::icon_button(ui, name, Some(tip), enabled, false);
                                buttons.push((name, response.rect));
                                response.clicked() && enabled
                            };
                            if icon(ui, "nav-arrow-left", "Previous version", count > 0 && current > 0) {
                                chosen = Some(HostMessage::SetVersion { tab: tab.to_string(), cell: id.clone(), index: current - 1 });
                            }
                            let shown = if count == 0 { String::from("0/0") } else { format!("{}/{}", current + 1, count) };
                            ui.label(egui::RichText::new(shown).font(crate::typescale::small()).color(crate::typescale::MUTED));
                            if icon(ui, "nav-arrow-right", "Next version", current + 1 < count) {
                                chosen = Some(HostMessage::SetVersion { tab: tab.to_string(), cell: id.clone(), index: current + 1 });
                            }
                            ui.separator();
                            if icon(ui, "arrow-up", "Move up", index > 0) {
                                chosen = Some(HostMessage::MoveCell { tab: tab.to_string(), cell: id.clone(), to: index - 1 });
                            }
                            if icon(ui, "arrow-down", "Move down", index + 1 < listed.len()) {
                                chosen = Some(HostMessage::MoveCell { tab: tab.to_string(), cell: id.clone(), to: index + 1 });
                            }
                            ui.separator();
                            if icon(ui, "trash", "Delete cell", true) {
                                chosen = Some(HostMessage::DeleteCell { tab: tab.to_string(), cell: id.clone() });
                                close = true;
                            }
                        });
                    });
            });
        // A press outside the menu, and away from the "…" that opened it,
        // closes it.
        if ui.input(|input| input.pointer.any_pressed())
            && ui
                .input(|input| input.pointer.interact_pos())
                .is_some_and(|pos| !area.response.rect.contains(pos) && !at.contains(pos))
        {
            close = true;
        }
        self.board.menu_buttons = buttons;
        self.cell_menu_rect = Some(area.response.rect);
        if let Some(message) = chosen {
            self.send(app, message);
        }
        if close {
            self.cell_menu = None;
            self.board.menu_buttons.clear();
        }
    }

    fn finish_drag(&mut self, ctx: &egui::Context) {
        if !ctx.input(|input| input.pointer.any_released()) {
            return;
        }
        let Some(source) = self.dragging.take() else {
            self.drop_target = None;
            return;
        };
        let Some((app, tab)) = self.drop_target.take() else {
            return;
        };
        let mut cell = source;
        let from = cell.id.clone();
        cell.id = protocol::cell_id();
        cell.from = Some(from);
        let index = self
            .apps
            .get(app)
            .and_then(|started| started.running.declared.cells.get(&tab))
            .map(|(cells, _)| cells.len())
            .unwrap_or(0);
        self.send(
            app,
            HostMessage::InsertCell { tab, index, cell: Some(cell) },
        );
    }
}

/// The tab strip and the toolbar, which the contents area sits under.
const WORKSPACE_CHROME: f32 = TAB_STRIP + crate::toolbar::HEIGHT;
/// The tab strip's height.
const TAB_STRIP: f32 = 40.0;
/// A notebook: gray around a white column at most this wide, a margin for
/// the bar and the "[n]", the gap between cells, and the active cell's blue.
const NOTEBOOK_COLUMN: f32 = 880.0;
const NOTEBOOK_MARGIN: f32 = 52.0;
const NOTEBOOK_RIGHT: f32 = 20.0;
const NOTEBOOK_GAP: f32 = 16.0;
const NOTEBOOK_GRAY: Color32 = Color32::from_rgb(0xF3, 0xF4, 0xF6);
const NOTEBOOK_BLUE: Color32 = Color32::from_rgb(0x25, 0x63, 0xEB);
const CELL_MENU_W: f32 = 236.0;
/// The short gray bar that shows or hides the workspace.
const BAR_GRAY: Color32 = Color32::from_rgb(186, 188, 192);

/// The contents size the workspace would have if it were shown: the window
/// minus the conversation column, this chrome, and an open properties panel.
fn hidden_viewport(window: Rect, chosen: Option<f32>, panel: f32) -> [f64; 2] {
    let column = chat::width(window.width(), chosen);
    let shell_w = (window.width() - column).max(0.0);
    let width = (shell_w - panel).max(0.0);
    let height = (window.height() - WORKSPACE_CHROME).max(0.0);
    [f64::from(width), f64::from(height)]
}

impl HostWindow {
    /// The panel's right edge, dragged to make the panel a little wider or
    /// narrower. The width is remembered when the drag ends.
    fn drag_panel_edge(&mut self, ui: &egui::Ui, edge_x: f32, bounds: Rect) {
        let zone = Rect::from_min_max(egui::pos2(edge_x - 3.0, bounds.top()), egui::pos2(edge_x + 3.0, bounds.bottom()));
        let response = ui
            .interact(zone, egui::Id::new("chat-panel-edge"), egui::Sense::drag())
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        if response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            let now = chat::width(bounds.width(), self.settings.chat_width);
            let wanted = (now + response.drag_delta().x).clamp(chat::PANEL_MIN, chat::PANEL_MAX);
            self.settings.chat_width = Some(wanted);
        }
        if response.drag_stopped()
            && let Some(width) = self.settings.chat_width
        {
            self.settings.choose_chat_width(width);
        }
    }
}

/// A short gray bar by `edge_x`, vertically centered. With the workspace
/// panel open it sits just inside the panel, right of the conversation's
/// edge, so it is never taken for the conversation's scroll bar, as the
/// owner set on 2026-10-29; closed, it sits just inside the window's right
/// edge. Returns its rect and whether it was clicked.
fn workspace_bar(ui: &egui::Ui, edge_x: f32, inside_workspace: bool, bounds: Rect) -> (Rect, bool) {
    let x = if inside_workspace { edge_x + 8.0 } else { edge_x - 8.0 };
    let center = egui::pos2(x, bounds.center().y);
    let bar = Rect::from_center_size(center, egui::vec2(5.0, 56.0));
    let hit = bar.expand2(egui::vec2(8.0, 6.0));
    // Its own layer, over the workspace's contents and under menus and the
    // session panel. It takes presses and drags, so no tool underneath,
    // such as a brush, ever starts from it, as the owner set on 2026-10-29.
    let clicked = egui::Area::new(egui::Id::new("workspace-bar"))
        .order(egui::Order::Middle)
        .fixed_pos(hit.min)
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            let (rect, response) = ui.allocate_exact_size(hit.size(), egui::Sense::click_and_drag());
            ui.painter().rect_filled(Rect::from_center_size(rect.center(), bar.size()), 3.0, BAR_GRAY);
            // The ordinary arrow over it, as the owner set on 2026-10-29. A
            // press that drifts, as a pen's does, still counts.
            let response = response.on_hover_cursor(egui::CursorIcon::Default);
            response.clicked() || response.drag_stopped()
        })
        .inner;
    (bar, clicked)
}

/// The zoom a `canvas.zoom` action asks for, such as 1 for 100%.
fn asked_zoom(action: &Action) -> Option<f64> {
    action
        .arg
        .as_ref()?
        .get("zoom")?
        .as_f64()
        .filter(|zoom| zoom.is_finite() && *zoom > 0.0)
}

/// While the assistant works, these wait: commands, gestures, selections,
/// and property edits. Selecting a tab is a command that still goes.
fn waits_while_locked(message: &HostMessage) -> bool {
    match message {
        HostMessage::Command { id, .. } => id != "select_tab",
        HostMessage::Gesture { .. }
        | HostMessage::RasterGesture { .. }
        | HostMessage::Select { .. }
        | HostMessage::Property { .. } => true,
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
        // The settings row's right-hand group ends above the contents' right
        // edge, beside the panel, as 0.10's approved views show.
        let panel_inset = self.panel_width(app, tab.as_deref());
        let picked = ui
            .scope(|ui| {
                if locked {
                    ui.style_mut().visuals.disabled_alpha = LOCKED_ALPHA;
                    ui.disable();
                }
                toolbar::show(ui, toolbar.as_ref(), panel_inset)
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
            Some(ContentsKind::Cells) => self.show_cells(ui, app, &tab, rect),
            Some(ContentsKind::Raster) => self.show_raster(ui, app, &tab, rect, dismissing),
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
        while let Some(event) = self.assistant.try_recv() {
            match event {
                assistant::Event::Entry(entry) => self.note_entry(entry),
                assistant::Event::Partial(text) => self.chat.live.push_str(&text),
                assistant::Event::Cleared => self.chat.conversation.clear(),
                assistant::Event::Working(working) => {
                    self.chat.working = working;
                    if working {
                        self.chat.started = Some(crate::workspace::now_seconds());
                    } else {
                        self.keep_live();
                        self.routing.end();
                        // A message sent while this one ran goes next.
                        if !self.chat.queued.is_empty() {
                            let (text, chips) = self.chat.queued.remove(0);
                            self.send_message(text, chips);
                        }
                    }
                }
                // The picker shows levels; each endpoint's models are
                // listed by the Settings page that needs them.
                assistant::Event::Models(_) => {}
                assistant::Event::Call {
                    call,
                    app,
                    tab,
                    tool,
                    input,
                    group,
                } => {
                    if app == route::HOST_ID {
                        self.begin_host_cell(call, &tool, input);
                        continue;
                    }
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
        if let Some(targets) = self.routing.changed_targets(&self.apps, &self.settings) {
            self.assistant.send(Command::Targets(targets));
        }
    }

    /// What the person did in the chat panel.
    fn chat_event(&mut self, event: ChatEvent, ctx: &egui::Context, frame: &mut eframe::Frame) {
        match event {
            ChatEvent::Send => {
                let text = std::mem::take(&mut self.chat.input).trim().to_string();
                let chips = std::mem::take(&mut self.chat.chips);
                if self.chat.working {
                    // It waits for the running request, shown as queued.
                    self.chat.queued.push((text, chips));
                    return;
                }
                self.send_message(text, chips);
            }
            ChatEvent::Retry(text) => {
                if self.chat.working {
                    self.chat.queued.push((text, Vec::new()));
                } else {
                    self.send_message(text, Vec::new());
                }
            }
            ChatEvent::Stop => {
                self.chat.queued.clear();
                self.stop_conversation_cells();
                self.assistant.send(Command::Stop);
            }
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
                // Each file is copied into the project when it is attached, so
                // the session never points outside the project.
                let folder = self.project.as_ref().map(|project| project.folder.clone());
                for path in picked {
                    let path = match &folder {
                        Some(folder) => match workspace::import_copy(&path, folder, "attachments") {
                            Ok(copy) => copy,
                            Err(reason) => {
                                self.assistant.send(Command::Note(format!(
                                    "{} could not be attached: {reason}",
                                    path.display()
                                )));
                                continue;
                            }
                        },
                        None => path,
                    };
                    if !self.chat.chips.contains(&path) {
                        self.chat.chips.push(path);
                    }
                }
            }
            ChatEvent::Level(id) => self.level = Some(id),
            ChatEvent::LevelSettings => self.settings_panel.open_at(crate::settings_panel::Page::Discussion),
            ChatEvent::Logo(choice) => self.logo(choice, ctx, frame),
            ChatEvent::Session(choice) => self.session_choice(choice, frame),
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
        let folder = workspace::default_project_folder(&data.join("Inpainter"));
        let (record, session) = workspace::create_project_at(&data, &folder, workspace::UNTITLED).expect("a project");
        self.project = Some(Project {
            id: record.id,
            name: record.name,
            folder,
            session: session.id,
            session_name: session.name,
            session_created: session.created_at,
            workspace_open: true,
        });
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

    /// A raster tab: its picture through the host's camera, and the pointer's
    /// gestures sent to the sub-application.
    fn show_raster(&mut self, ui: &mut egui::Ui, app: usize, tab: &str, rect: Rect, dismissing: bool) {
        let mut out = Vec::new();
        {
            let Some(started) = self.apps.get_mut(app) else {
                return;
            };
            let Some(display) = started.running.declared.rasters.get_mut(tab) else {
                ui.painter().with_clip_rect(rect).rect_filled(rect, 0.0, crate::raster::GROUND);
                return;
            };
            let locked = self.surface.locked;
            if self.raster.show(ui, display, rect, &self.surface.tablet, locked, dismissing, &mut out) {
                self.live.moved = true;
            }
        }
        for item in out {
            let gesture = match item {
                crate::raster::Out::Gesture(gesture) => gesture,
                crate::raster::Out::Painted(points) => protocol::RasterGesture::Stroke { points },
            };
            self.send(
                app,
                HostMessage::RasterGesture {
                    tab: tab.to_string(),
                    gesture,
                },
            );
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
    /// the tab bar and the toolbar, and 11 points in from the canvas's right
    /// edge.
    fn tab_center(canvas_right: f32) -> Pos2 {
        Pos2::new(canvas_right - 11.0, super::WORKSPACE_CHROME + 18.0 + 20.0)
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

    /// Catches a conversation panel that cannot be resized, or one dragged
    /// past its limits: its edge drags it a little wider, and the
    /// workspace narrows by as much.
    #[test]
    fn the_panel_edge_drags_a_little() {
        let mut harness = Harness::new();
        let wide = harness.app.viewport[0];
        let edge = crate::chat::width(SCREEN.x, None);
        harness.drag(egui::pos2(edge, 200.0), egui::pos2(edge + 60.0, 200.0));
        assert_eq!(harness.app.settings.chat_width, Some(edge + 60.0));
        assert!((harness.app.viewport[0] - (wide - 60.0)).abs() < 1.0, "the workspace did not narrow");
        let edge = edge + 60.0;
        harness.drag(egui::pos2(edge, 200.0), egui::pos2(edge + 400.0, 200.0));
        assert_eq!(harness.app.settings.chat_width, Some(crate::chat::PANEL_MAX));
    }

    #[test]
    fn f4_opens_the_panel_and_narrows_the_canvas() {
        let mut harness = Harness::new();
        let wide = harness.app.viewport[0];
        assert!(!harness.app.panel_open);
        // The chat panel takes a quarter of the window on the left.
        let shell = f64::from(SCREEN.x - crate::chat::width(SCREEN.x, None));
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
            cell: None,
            aside: false,
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

        let project = harness.app.project.as_ref().unwrap();
        let path = crate::workspace::session_path(&project.folder, &project.session);
        let text = std::fs::read_to_string(&path).expect("the session was written");
        let file = crate::workspace::SessionFile::parse(&text).unwrap();
        // The fixture's file is outside the project, so the session never
        // points to it.
        assert!(file.tabs.is_empty(), "a file outside the project was written:\n{text}");
        assert!(!text.contains("/somewhere"));
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
        let project = changed.app.project.as_ref().unwrap();
        let text = std::fs::read_to_string(crate::workspace::session_path(&project.folder, &project.session)).unwrap();
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
        let older = folder.join("Inpainter").join("Older");
        let (record, mut session) = crate::workspace::create_project_at(&folder, &older, "Older").unwrap();
        session.tabs = vec![crate::workspace::SessionTab {
            app: "no-such-app".into(),
            path: None,
            label: "Gone".into(),
            camera: None,
        }];
        session.selected = Some(0);
        session.conversation = vec![entry(assistant::Role::Person, "hello")];
        session.write(&older).unwrap();

        let Harness { app, frame, .. } = &mut harness;
        app.session_choice(
            crate::sessions::SessionChoice::Open { project: record.id.clone(), session: session.id.clone() },
            frame,
        );
        assert!(harness.app.flows.closing_started());
        // The sub-application has resolved its unsaved work.
        harness.app.apps[0].running.declared.quit_ready = true;
        harness.until(|app| app.chat.conversation.len() == 2);
        assert_eq!(harness.app.chat.conversation[0].text, "hello");
        let left_out = &harness.app.chat.conversation[1];
        assert_eq!(left_out.role, assistant::Role::Note);
        assert!(left_out.text.contains("Gone"), "{}", left_out.text);
        assert_eq!(harness.app.project.as_ref().map(|project| project.id.as_str()), Some(record.id.as_str()));
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

        let targets = crate::route::targets(&apps, &crate::workspace::Settings::default());
        assert!(targets.iter().all(|target| target.app != "gone"), "an exited process is offered");
        let here = targets.iter().find(|target| target.app == "here").expect("the living process was dropped");
        assert_eq!(here.tabs[0].label, "Template");
        assert!(targets.iter().any(|target| target.app == crate::route::HOST_ID));
    }
}
