//! The window's side of Settings: the first start, where each run goes,
//! telling the assistant its endpoint and prompt, the composer's levels,
//! and writing what the Settings panel changed.

use super::HostWindow;
use crate::chat::LevelView;
use crate::endpoints::{self, Route};
use crate::settings_panel::{self, Env, SettingsChoice};
use crate::workspace::{DISCUSSION, Effort, EndpointSetting, Level};
use assistant::Command;
use eframe::egui;
use std::hash::{Hash, Hasher};

/// Said when there is no level to run on.
const NO_LEVEL: &str = "No level is set. Add one in Settings, under Discussion.";
const OPENROUTER: &str = "https://openrouter.ai/api/v1";

impl HostWindow {
    /// Settings from before 0.8 gain the OpenRouter endpoint, Discussion,
    /// and the default levels, and the OpenRouter key from the environment
    /// or `.env` moves into the keyring.
    pub(super) fn first_start(&mut self) {
        let address = std::env::var("INPAINTER_OPENROUTER_URL")
            .ok()
            .filter(|address| !address.trim().is_empty())
            .unwrap_or_else(|| OPENROUTER.to_string());
        let made = self.settings.first_start(&address);
        // 0.9's first start: the skill that makes pictures, once.
        self.settings.add_image_skill();
        let Some(endpoint) = made else {
            return;
        };
        if let Some(key) = std::env::var("OPENROUTER_API_KEY").ok().filter(|key| !key.trim().is_empty())
            && let Err(reason) = self.keys.set(&endpoint, &key)
        {
            self.assistant
                .send(Command::Note(format!("The OpenRouter key could not be kept: {reason}")));
        }
    }

    /// The level this session uses: the one it chose, or the default.
    pub(super) fn current_level(&self) -> Option<Level> {
        self.settings.level_or_default(self.level.as_deref()).cloned()
    }

    /// Where the session's level sends a run: its model at its effort, on
    /// its endpoint, with that endpoint's key.
    pub(super) fn level_route(&mut self) -> Result<Route, String> {
        let level = self.current_level().ok_or_else(|| NO_LEVEL.to_string())?;
        let keys = &mut self.keys;
        endpoints::level_route(&self.settings, &level, |id| keys.get(id))
    }

    /// A cell that ran goes back to the endpoint, model, and effort it
    /// recorded, so Run again repeats what ran and a cell made on a local
    /// server never moves by itself, as the owner set on 2026-10-29. A new
    /// cell, or one whose endpoint is gone, runs on the session's level.
    pub(super) fn recorded_route(&mut self, cell: &protocol::Cell) -> Result<Route, String> {
        let recorded = cell.endpoint.as_deref().and_then(|id| self.settings.endpoint(id)).cloned();
        match (recorded, cell.model.as_deref().filter(|model| !model.trim().is_empty())) {
            (Some(endpoint), Some(model)) => {
                let effort = match self.older_level(cell) {
                    Some(level) => level.effort,
                    None => cell.effort.as_deref().and_then(Effort::from_word),
                };
                let key = self.keys.get(&endpoint.id);
                Ok(Route::new(&endpoint, key, model, effort))
            }
            _ => self.level_route(),
        }
    }

    /// Tells the assistant the level's endpoint and key, the endpoint's
    /// kind, and the Discussion prompt, whenever any of them changed. A
    /// request already running keeps what it began with.
    pub(super) fn configure_assistant(&mut self) {
        let voice = self.settings.discussion().map(|skill| skill.prompt.clone()).unwrap_or_default();
        let endpoint = match self.level_route() {
            Ok(route) => assistant::Endpoint {
                base_url: route.base.clone(),
                api_key: route.key.clone(),
                reasoning: route.reasoning(),
                voice,
                refusal: route.blocked(),
            },
            Err(reason) => assistant::Endpoint {
                base_url: String::new(),
                api_key: None,
                reasoning: assistant::Reasoning::OpenRouter,
                voice,
                refusal: Some(reason),
            },
        };
        // Compared by a hash, so the key is held nowhere new.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        endpoint.base_url.hash(&mut hasher);
        endpoint.api_key.hash(&mut hasher);
        (endpoint.reasoning == assistant::Reasoning::OpenRouter).hash(&mut hasher);
        endpoint.voice.hash(&mut hasher);
        endpoint.refusal.hash(&mut hasher);
        let signature = hasher.finish().to_string();
        if self.configured.as_deref() != Some(signature.as_str()) {
            self.configured = Some(signature);
            self.assistant.send(Command::Configure(endpoint));
        }
    }

    /// The composer's levels, with the session's marked. The real window
    /// asks the Discussion endpoint for its models, so the picker can name
    /// them; frame tests never reach a network.
    pub(super) fn show_levels(&mut self, known: &[assistant::Model]) {
        if self.native_dialogs
            && let Some(endpoint) = self.settings.discussion_endpoint().cloned()
        {
            let key = self.keys.get(&endpoint.id);
            self.probes.models(&self.ctx, &endpoint, key);
        }
        // The image skill's models too, so an Image cell names its model.
        if self.native_dialogs
            && let Some(endpoint) = self
                .settings
                .image_skill()
                .and_then(|skill| skill.endpoint.clone())
                .and_then(|id| self.settings.endpoint(&id).cloned())
        {
            let key = self.keys.get(&endpoint.id);
            self.probes.image_models(&self.ctx, &endpoint, key);
        }
        self.chat.levels = self
            .settings
            .levels
            .iter()
            .map(|level| LevelView {
                id: level.id.clone(),
                name: level.name.clone(),
                model: settings_panel::model_label(known, &level.model),
                effort: level
                    .effort
                    .map(|effort| Effort::label(Some(effort)).trim_end_matches(" effort").to_string())
                    .unwrap_or_default(),
            })
            .collect();
        let current = self.current_level().map(|level| level.id);
        self.chat.current = current.and_then(|id| self.settings.levels.iter().position(|level| level.id == id));
    }

    /// Draws Settings when open, under the session's name, and writes what
    /// the person changed.
    pub(super) fn draw_settings(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if !self.settings_panel.open {
            return;
        }
        let top = self
            .chat
            .anchor
            .unwrap_or_else(|| egui::pos2(ctx.content_rect().center().x, 50.0));
        let projects_root = self.projects_root().map(|root| root.display().to_string()).unwrap_or_default();
        let uses_file = self.data.is_none() || crate::keys::uses_file();
        let mut env = Env {
            settings: &self.settings,
            probes: &mut self.probes,
            keys: &mut self.keys,
            uses_file,
            projects_root,
        };
        let choices = settings_panel::show(ctx, top, &mut self.settings_panel, &mut env);
        for choice in choices {
            self.apply_setting(choice, frame);
        }
    }

    pub(super) fn apply_setting(&mut self, choice: SettingsChoice, frame: &mut eframe::Frame) {
        match choice {
            SettingsChoice::ChooseProjectsRoot => {
                let picked = if self.native_dialogs {
                    crate::dialogs::pick_folder(frame)
                } else {
                    self.scripted_path()
                };
                if let Some(path) = picked {
                    let text = path.display().to_string();
                    self.settings.update(|settings| settings.projects_root = Some(text.clone()));
                    self.sessions.form_root = text;
                    self.projects_root = Some(path);
                }
            }
            SettingsChoice::SaveEndpoint { id, kind, name, address, key } => {
                let id = id.unwrap_or_else(protocol::cell_id);
                let endpoint = EndpointSetting { id: id.clone(), name, kind, address };
                self.settings.update(|settings| {
                    match settings.endpoints.iter_mut().find(|known| known.id == endpoint.id) {
                        Some(known) => *known = endpoint,
                        None => settings.endpoints.push(endpoint),
                    }
                });
                if let Some(key) = key
                    && let Err(reason) = self.keys.set(&id, &key)
                {
                    self.settings_panel.notice = Some(format!("The key could not be kept: {reason}"));
                }
                self.probes.forget(&id);
            }
            SettingsChoice::DeleteEndpoint(id) => {
                self.settings.update(|settings| settings.endpoints.retain(|endpoint| endpoint.id != id));
                self.keys.delete(&id);
                self.probes.forget(&id);
            }
            SettingsChoice::SaveSkill(skill) => {
                self.settings.update(|settings| {
                    let mut moved = None::<String>;
                    match settings.skills.iter_mut().find(|known| known.id == skill.id) {
                        Some(known) => {
                            // Discussion keeps its name: it is built in.
                            let name = if skill.id == DISCUSSION { known.name.clone() } else { skill.name.clone() };
                            if skill.id == DISCUSSION && known.endpoint != skill.endpoint {
                                moved = skill.endpoint.clone();
                            }
                            *known = skill;
                            known.name = name;
                        }
                        None => settings.skills.push(skill),
                    }
                    // Discussion's endpoint is where the replies come from:
                    // every level follows it, as the owner set on
                    // 2026-10-29. A model it does not list shows a warning.
                    if let Some(new) = moved {
                        for level in &mut settings.levels {
                            level.endpoint = new.clone();
                        }
                    }
                });
            }
            SettingsChoice::DeleteSkill(id) => {
                if id != DISCUSSION {
                    self.settings.update(|settings| settings.skills.retain(|skill| skill.id != id));
                }
            }
            SettingsChoice::SaveLevel(level) => {
                self.settings.update(|settings| {
                    match settings.levels.iter_mut().find(|known| known.id == level.id) {
                        Some(known) => *known = level,
                        None => settings.levels.push(level),
                    }
                });
            }
            SettingsChoice::MoveLevel { from, to } => {
                self.settings.update(|settings| {
                    if from < settings.levels.len() {
                        let level = settings.levels.remove(from);
                        let to = to.min(settings.levels.len());
                        settings.levels.insert(to, level);
                    }
                });
            }
            SettingsChoice::MakeDefault(id) => {
                self.settings.update(|settings| settings.default_level = Some(id));
            }
            SettingsChoice::DeleteLevel(id) => {
                self.settings.update(|settings| {
                    if settings.levels.len() <= 1 {
                        return;
                    }
                    settings.levels.retain(|level| level.id != id);
                    if settings.default_level.as_deref() == Some(id.as_str()) {
                        settings.default_level = settings.levels.first().map(|level| level.id.clone());
                    }
                });
                if self.level.as_deref() == Some(id.as_str()) {
                    self.level = None;
                }
            }
        }
    }

    /// A cell, from the conversation or a notebook, by id.
    fn find_cell(&self, id: &str) -> Option<protocol::Cell> {
        if let Some(cell) = self.chat.conversation.iter().find_map(|entry| entry.cell.as_ref().filter(|cell| cell.id == id)) {
            return Some(cell.clone());
        }
        let (app, tab) = self.cell_home(id)?;
        let (cells, _) = self.apps.get(app)?.running.declared.cells.get(&tab)?;
        cells.iter().find(|cell| cell.id == id).cloned()
    }

    /// A level chosen from a cell's chevron: the cell runs on that level's
    /// model and effort, on the Discussion skill's endpoint, as the
    /// composer's level does. A conversation cell is the host's own and
    /// takes it at once; a notebook cell's next run uses it, and its result
    /// records it.
    pub(super) fn choose_cell_level(&mut self, id: &str, level: &str) {
        let Some(level) = self.settings.levels.iter().find(|known| known.id == level).cloned() else {
            return;
        };
        let Some(endpoint) = self.settings.discussion_endpoint().map(|endpoint| endpoint.id.clone()).or(Some(level.endpoint.clone())) else {
            return;
        };
        let effort = level.effort.map(|effort| effort.word().to_string());
        if let Some(cell) = self
            .chat
            .conversation
            .iter_mut()
            .find_map(|entry| entry.cell.as_mut().filter(|cell| cell.id == id))
        {
            cell.endpoint = Some(endpoint);
            cell.model = Some(level.model);
            cell.effort = effort;
            return;
        }
        self.board.chosen.insert(id.to_string(), (endpoint, level.model, effort));
    }

    /// The level a cell is on: the one whose model and effort it runs
    /// with, or, for a cell that never ran, the session's.
    pub(super) fn cell_level(&self, id: &str, cell: &protocol::Cell) -> Option<usize> {
        let (model, effort) = match self.board.chosen.get(id) {
            Some((_, model, effort)) => (Some(model.clone()), effort.clone()),
            None => (cell.model.clone(), cell.effort.clone()),
        };
        let Some(model) = model else {
            return self.chat.current;
        };
        let exact = self
            .settings
            .levels
            .iter()
            .position(|level| level.model == model && level.effort.map(|effort| effort.word().to_string()) == effort);
        if exact.is_some() || self.board.chosen.contains_key(id) {
            return exact;
        }
        let older = self.older_level(cell)?;
        self.settings.levels.iter().position(|level| level.id == older.id)
    }

    /// The level a cell from before 0.8 counts as being on, as the owner
    /// set on 2026-10-29. Such a cell ran with no effort on the Discussion
    /// endpoint, so no level names it exactly: it takes the session's level
    /// when that is on its model, or else the first level on its model. Its
    /// picker shows that level, and Run again runs at its effort. Any other
    /// cell has none.
    fn older_level(&self, cell: &protocol::Cell) -> Option<Level> {
        if cell.effort.is_some() {
            return None;
        }
        let model = cell.model.as_deref()?;
        // Before 0.8 a cell named its endpoint "openrouter", which names no
        // endpoint in Settings; such a cell runs on the session's level.
        let discussion = self.settings.discussion_endpoint()?;
        let recorded = cell.endpoint.as_deref().unwrap_or_default();
        if recorded != discussion.id && self.settings.endpoint(recorded).is_some() {
            return None;
        }
        let on_model = |level: &&Level| level.model == model;
        if self.settings.levels.iter().filter(on_model).any(|level| level.effort.is_none()) {
            return None;
        }
        self.current_level()
            .filter(|level| level.model == model)
            .or_else(|| self.settings.levels.iter().find(on_model).cloned())
    }

    /// A cell's level picker, open under its chevron, on the menus' card:
    /// the same picker the composer opens. A press outside it, or Escape,
    /// closes it.
    pub(super) fn draw_model_menu(&mut self, ctx: &egui::Context) {
        let Some((id, at)) = self.model_menu.clone() else {
            return;
        };
        let Some(cell) = self.find_cell(&id) else {
            self.model_menu = None;
            return;
        };
        let chosen = self.cell_level(&id, &cell);
        // A cell on a model no level names still shows what it runs on.
        let own = (
            cell.model.as_deref().map(|model| settings_panel::model_label(&self.probes.known_models(), model)).unwrap_or_default(),
            cell.effort.as_deref().and_then(Effort::from_word).map(|effort| Effort::label(Some(effort)).trim_end_matches(" effort").to_string()).unwrap_or_default(),
        );
        const WIDTH: f32 = 324.0;
        let screen = ctx.content_rect();
        let left = (at.right() - WIDTH).clamp(screen.left() + 8.0, (screen.right() - WIDTH - 24.0).max(screen.left() + 8.0));
        let levels = self.chat.levels.clone();
        let shown = egui::Area::new(egui::Id::new("cell-level-picker"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(left, at.bottom() + 8.0))
            .fade_in(false)
            .show(ctx, |ui| {
                crate::widgets::floating_card()
                    .show(ui, |ui| crate::chat::level_picker(ui, &format!("cell-{id}"), &levels, chosen, Some(own)))
                    .inner
            });
        match shown.inner {
            Some(crate::chat::LevelPick::Level(level)) => self.choose_cell_level(&id, &level),
            Some(crate::chat::LevelPick::Settings) => {
                self.model_menu = None;
                self.settings_panel.open_at(settings_panel::Page::Discussion);
                return;
            }
            None => {}
        }
        let pressed_outside = ctx.input(|input| {
            input.pointer.any_pressed()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|pos| !shown.response.rect.contains(pos) && !at.contains(pos))
        });
        if pressed_outside || ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.model_menu = None;
        }
    }

    /// Where a cell's run goes. An Image cell goes back to the endpoint and
    /// model it recorded, or, new, to the first image skill; any other cell
    /// as `recorded_route` says.
    pub(super) fn cell_route(&mut self, cell: &protocol::Cell) -> Result<Route, String> {
        if cell.kind != protocol::IMAGE_KIND {
            return self.recorded_route(cell);
        }
        let recorded = cell.endpoint.as_deref().and_then(|id| self.settings.endpoint(id)).cloned();
        if let (Some(endpoint), Some(model)) = (recorded, cell.model.as_deref().filter(|model| !model.trim().is_empty())) {
            let key = self.keys.get(&endpoint.id);
            return Ok(Route::new(&endpoint, key, model, None));
        }
        let skill = self
            .settings
            .image_skill()
            .cloned()
            .ok_or_else(|| String::from("No skill makes pictures. Add one in Settings, under Skills."))?;
        let keys = &mut self.keys;
        endpoints::skill_route(&self.settings, &skill, |id| keys.get(id))
    }

    /// Keeps a run's picture in the project's `images` folder, one file per
    /// run, and names it relative to the project. With no project, as in a
    /// frame test with none, it goes to a scratch folder.
    pub(super) fn keep_picture(&mut self, index: usize, png: Vec<u8>) {
        let Some(id) = self.board.runs.get(index).map(|run| run.id.clone()) else {
            return;
        };
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |time| time.as_millis());
        let name = format!("{id}-{stamp}.png");
        let (folder, named) = match &self.project {
            Some(project) => (project.folder.join("images"), format!("images/{name}")),
            None => {
                let folder = std::env::temp_dir().join("inpainter-pictures");
                (folder.clone(), folder.join(&name).display().to_string())
            }
        };
        match std::fs::create_dir_all(&folder).and_then(|_| std::fs::write(folder.join(&name), &png)) {
            Ok(()) => self.board.runs[index].picture = Some((named, png)),
            // The run ends here, failed; the end that follows finds it gone.
            Err(error) => self.finish_run(index, protocol::CellState::Failed, Some(format!("The picture could not be kept: {error}."))),
        }
    }

    /// Shows one of a cell's versions, from its filmstrip.
    pub(super) fn show_version(&mut self, id: &str, index: usize) {
        if let Some(cell) = self
            .chat
            .conversation
            .iter_mut()
            .find_map(|entry| entry.cell.as_mut().filter(|cell| cell.id == id))
        {
            if cell.show_version(index) {
                self.board.drafts.insert(id.to_string(), cell.prompt());
            }
            return;
        }
        if let Some((app, tab)) = self.cell_home(id) {
            self.send(app, protocol::HostMessage::SetVersion { tab, cell: id.to_string(), index });
        }
    }
}
