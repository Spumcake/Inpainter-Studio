//! Settings, as the approved settings views show them: a panel at the top
//! centre, opened from the menu beside the logo, on the session menu's card,
//! overlay, and controls. The left column holds the search and the pages:
//! General, Endpoints, and Skills, a divider, then Discussion. What the
//! person changes is returned as a `SettingsChoice`; the window writes it.

use crate::endpoints::{Models, Probes, Status};
use crate::keys::Keys;
use crate::sessions::{self, COLUMN_W, CONTROL_H, DARK, HINT, INSET, NAV_GAP, NAV_ROW, OVERLAY, PAD};
use crate::toolbar;
use crate::typescale as ts;
use crate::widgets::{self, BORDER, MUTED, SELECTED, TEXT};
use crate::workspace::{DISCUSSION, Effort, EndpointKind, EndpointSetting, Level, Settings, SkillSetting};
use eframe::egui::{self, Align2, Color32, Key, Pos2, Rect, Sense, UiBuilder, pos2, vec2};

/// Wider and taller than the session menu, as the views show.
const PANEL_W: f32 = 880.0;
const PANEL_H: f32 = 600.0;
/// The space around a page.
const PAGE_PAD: f32 = 28.0;
/// One row of a card: a label over a gray line, and its control.
const ROW_H: f32 = 64.0;
/// The widest a row's control is.
const CONTROL_W: f32 = 320.0;
const CARD_RADIUS: f32 = 12.0;
const CONNECTED: Color32 = Color32::from_rgb(34, 160, 84);
const WARNING: Color32 = Color32::from_rgb(202, 120, 20);
const TAG_FILL: Color32 = Color32::from_rgb(241, 242, 244);

/// Which page the panel shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) enum Page {
    #[default]
    General,
    Endpoints,
    /// Add Endpoint, or Edit Endpoint.
    EndpointForm,
    Skills,
    /// One skill's page, by id.
    Skill(String),
    Discussion,
    /// Add Level, or Edit Level.
    LevelForm,
}

impl Page {
    /// The left column's page this page belongs to.
    fn section(&self) -> Page {
        match self {
            Page::EndpointForm => Page::Endpoints,
            Page::Skill(_) => Page::Skills,
            Page::LevelForm => Page::Discussion,
            page => page.clone(),
        }
    }
}

/// What the person changed. The window writes it into settings, or the
/// keyring, at once.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SettingsChoice {
    /// General's Change: choose a folder.
    ChooseProjectsRoot,
    /// A new endpoint, with no id, or an edited one. A key of None leaves
    /// the stored key as it is.
    SaveEndpoint {
        id: Option<String>,
        kind: EndpointKind,
        name: String,
        address: String,
        key: Option<String>,
    },
    DeleteEndpoint(String),
    SaveSkill(SkillSetting),
    DeleteSkill(String),
    /// A new level or an edited one, by its id.
    SaveLevel(Level),
    MoveLevel { from: usize, to: usize },
    MakeDefault(String),
    DeleteLevel(String),
}

/// The endpoint form, as it is being filled in.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EndpointForm {
    /// The endpoint being edited, or none for a new one.
    pub editing: Option<String>,
    pub kind: EndpointKind,
    pub name: String,
    pub address: String,
    /// A key typed here. Empty keeps the stored one.
    pub key: String,
    pub has_key: bool,
    pub error: Option<String>,
}

impl Default for EndpointForm {
    fn default() -> Self {
        let (name, address) = EndpointKind::Openrouter.suggested();
        Self {
            editing: None,
            kind: EndpointKind::Openrouter,
            name: name.to_string(),
            address: address.to_string(),
            key: String::new(),
            has_key: false,
            error: None,
        }
    }
}

/// The level form, as it is being filled in.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct LevelForm {
    pub editing: Option<String>,
    pub name: String,
    pub endpoint: String,
    pub model: String,
    pub effort: Option<Effort>,
    pub error: Option<String>,
}

/// The panel's state, kept by the window between frames.
#[derive(Default)]
pub(crate) struct SettingsPanel {
    pub open: bool,
    pub page: Page,
    pub search: String,
    /// The panel has just opened and is raised above everything this frame.
    pub raise: bool,
    pub endpoint_form: EndpointForm,
    pub level_form: LevelForm,
    /// The skill whose page is open, as it is being edited.
    pub skill: Option<SkillSetting>,
    /// Why the last action was refused, such as deleting an endpoint in use.
    pub notice: Option<String>,
    /// The level whose handle is being dragged, by index.
    dragging: Option<usize>,
    /// Statuses are asked again when the Endpoints page opens.
    fresh: bool,
    /// What the model lists' searches hold, by list.
    model_search: String,
    /// The pages' buttons last drawn, by name, so a check can press them.
    pub buttons: Vec<(String, Rect)>,
    /// What runs on a model its endpoint does not list, as last found.
    pub unlisted: Unlisted,
}

/// The levels and skills, by id, whose model their endpoint does not list,
/// as after the endpoint was switched. Each shows a warning until the
/// person chooses a model, as the owner approved on 2026-10-29.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Unlisted {
    pub levels: Vec<String>,
    pub skills: Vec<String>,
}

/// Finds what is unlisted. `listed` gives an endpoint's model ids, or none
/// while its list is unknown, as when it is not running; nothing is warned
/// about then.
pub(crate) fn unlisted(settings: &Settings, listed: &mut dyn FnMut(&EndpointSetting) -> Option<Vec<String>>) -> Unlisted {
    let mut missing = |endpoint: Option<&EndpointSetting>, model: &str| {
        let Some(endpoint) = endpoint.filter(|endpoint| endpoint.kind.chats()) else {
            return false;
        };
        !model.trim().is_empty() && listed(endpoint).is_some_and(|ids| !ids.iter().any(|id| id == model))
    };
    let discussion = settings.discussion_endpoint();
    let levels = settings
        .levels
        .iter()
        .filter(|level| missing(discussion.or_else(|| settings.endpoint(&level.endpoint)), &level.model))
        .map(|level| level.id.clone())
        .collect();
    let skills = settings
        .skills
        .iter()
        .filter(|skill| skill.id != DISCUSSION)
        .filter(|skill| missing(skill.endpoint.as_deref().and_then(|id| settings.endpoint(id)), skill.model.as_deref().unwrap_or_default()))
        .map(|skill| skill.id.clone())
        .collect();
    Unlisted { levels, skills }
}

/// The warning a row shows: "Not on Ollama. Choose a model."
fn not_on(endpoint: &str) -> String {
    format!("Not on {endpoint}. Choose a model.")
}

/// The hazard and its words, ending at `right`. Returns where it starts.
fn warning(ui: &egui::Ui, right: f32, center_y: f32, words: &str) -> f32 {
    let galley = ui.painter().layout_no_wrap(words.to_string(), ts::small(), WARNING);
    let left = right - galley.size().x;
    ui.painter().galley(pos2(left, center_y - galley.size().y / 2.0), galley, WARNING);
    toolbar::paint_icon(ui.painter(), pos2(left - 12.0, center_y), HAZARD, 14.0, WARNING);
    left - 22.0
}

const HAZARD: &str = "warning-triangle";

/// A Model row's label, with the warning in place of its gray line when
/// the endpoint does not list the model.
fn model_row(ui: &egui::Ui, row: Rect, right: f32, line: &str, missing_on: Option<&str>) {
    match missing_on {
        Some(endpoint) => {
            row_text(ui, row, row.left() + PAD, right, "Model", "");
            let galley = ui.painter().layout_no_wrap(not_on(endpoint), ts::small(), WARNING);
            let y = row.center().y + 10.0;
            toolbar::paint_icon(ui.painter(), pos2(row.left() + PAD + 7.0, y), HAZARD, 14.0, WARNING);
            ui.painter().galley(pos2(row.left() + PAD + 18.0, y - galley.size().y / 2.0), galley, WARNING);
        }
        None => {
            row_text(ui, row, row.left() + PAD, right, "Model", line);
        }
    }
}

impl SettingsPanel {
    /// Opens the panel at `page`.
    pub(crate) fn open_at(&mut self, page: Page) {
        self.open = true;
        self.raise = true;
        self.search.clear();
        self.notice = None;
        self.go(page);
    }

    fn go(&mut self, page: Page) {
        if page == Page::Endpoints {
            self.fresh = true;
        }
        self.notice = None;
        self.dragging = None;
        self.page = page;
    }

    /// Opens a skill's page, editing a copy of it.
    fn open_skill(&mut self, skill: &SkillSetting) {
        self.skill = Some(skill.clone());
        self.go(Page::Skill(skill.id.clone()));
    }

    pub(crate) fn add_endpoint(&mut self) {
        self.endpoint_form = EndpointForm::default();
        self.go(Page::EndpointForm);
    }

    fn edit_endpoint(&mut self, endpoint: &EndpointSetting, has_key: bool) {
        self.endpoint_form = EndpointForm {
            editing: Some(endpoint.id.clone()),
            kind: endpoint.kind,
            name: endpoint.name.clone(),
            address: endpoint.address.clone(),
            key: String::new(),
            has_key,
            error: None,
        };
        self.go(Page::EndpointForm);
    }

    fn add_level(&mut self, settings: &Settings) {
        self.level_form = LevelForm {
            endpoint: settings.discussion_endpoint().map(|endpoint| endpoint.id.clone()).unwrap_or_default(),
            name: String::from("New Level"),
            effort: Some(Effort::Medium),
            ..Default::default()
        };
        self.model_search.clear();
        self.go(Page::LevelForm);
    }

    fn edit_level(&mut self, level: &Level) {
        self.level_form = LevelForm {
            editing: Some(level.id.clone()),
            name: level.name.clone(),
            endpoint: level.endpoint.clone(),
            model: level.model.clone(),
            effort: level.effort,
            error: None,
        };
        self.model_search.clear();
        self.go(Page::LevelForm);
    }
}

/// What the panel reads while it draws.
pub(crate) struct Env<'a> {
    pub settings: &'a Settings,
    pub probes: &'a mut Probes,
    pub keys: &'a mut Keys,
    /// Keys go to the private file, as no keyring answers.
    pub uses_file: bool,
    /// Where new projects go, as General shows it.
    pub projects_root: String,
}

/// A model's name as a row shows it: its listed name without the maker,
/// or else its id without the maker.
pub(crate) fn model_label(known: &[assistant::Model], id: &str) -> String {
    match known.iter().find(|model| model.id == id) {
        Some(model) => crate::chat::short_name(&model.name).to_string(),
        None => id.split_once('/').map_or(id, |(_, name)| name).to_string(),
    }
}

/// The level's line: its model, its effort, and its endpoint.
pub(crate) fn level_line(settings: &Settings, known: &[assistant::Model], level: &Level) -> String {
    let endpoint = settings.endpoint(&level.endpoint).map_or("No endpoint", |endpoint| endpoint.name.as_str());
    format!("{} · {} · {}", model_label(known, &level.model), Effort::label(level.effort), endpoint)
}

/// The words each page answers to, for the search.
fn page_words(settings: &Settings, page: &Page) -> String {
    let mut words = match page {
        Page::General => "General Project location".to_string(),
        Page::Endpoints => "Endpoints keys address".to_string(),
        Page::Skills => "Skills prompt".to_string(),
        Page::Discussion => "Discussion levels effort model".to_string(),
        _ => String::new(),
    };
    match page {
        Page::Endpoints => {
            for endpoint in &settings.endpoints {
                words.push_str(&format!(" {} {} {}", endpoint.name, endpoint.address, endpoint.kind.label()));
            }
        }
        Page::Skills => {
            for skill in &settings.skills {
                words.push_str(&format!(" {} {}", skill.name, skill.description));
            }
        }
        Page::Discussion => {
            for level in &settings.levels {
                words.push_str(&format!(" {} {}", level.name, level.model));
            }
        }
        _ => {}
    }
    words.to_lowercase()
}

/// The pages the left column shows for `search`.
pub(crate) fn matching_pages(settings: &Settings, search: &str) -> Vec<Page> {
    let search = search.trim().to_lowercase();
    [Page::General, Page::Endpoints, Page::Skills, Page::Discussion]
        .into_iter()
        .filter(|page| search.is_empty() || page_words(settings, page).contains(&search))
        .collect()
}

/// Draws the panel, its top at `top`, when it is open. A press outside it,
/// or Escape, closes it.
pub(crate) fn show(ctx: &egui::Context, top: Pos2, panel: &mut SettingsPanel, env: &mut Env) -> Vec<SettingsChoice> {
    let mut choices = Vec::new();
    if !panel.open {
        return choices;
    }
    panel.buttons.clear();
    if std::mem::take(&mut panel.fresh) {
        env.probes.forget_statuses();
    }
    {
        let Env { settings, probes, keys, .. } = env;
        panel.unlisted = unlisted(settings, &mut |endpoint| {
            let key = keys.get(&endpoint.id);
            match probes.models(ctx, endpoint, key) {
                Some(Ok(list)) => Some(list.iter().map(|model| model.id.clone()).collect()),
                _ => None,
            }
        });
    }
    let screen = ctx.content_rect();
    let width = PANEL_W.min(screen.width() - 32.0);
    let height = PANEL_H.min(screen.height() - top.y - 16.0).max(240.0);
    let left = (top.x - width / 2.0).clamp(screen.left() + 16.0, (screen.right() - 16.0 - width).max(screen.left() + 16.0));
    let rect = Rect::from_min_size(pos2(left, top.y), vec2(width, height));
    // As the session menu's: under the panel and over everything else,
    // taking every press outside the panel.
    let overlay = egui::Area::new(egui::Id::new("settings-overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .fade_in(false)
        .show(ctx, |ui| {
            ui.painter().rect_filled(screen, 0.0, OVERLAY);
            ui.allocate_rect(screen, Sense::click_and_drag());
        })
        .response
        .layer_id;
    let area = egui::Area::new(egui::Id::new("settings-panel"))
        .order(egui::Order::Foreground)
        .fade_in(false)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.set_min_size(rect.size());
            let painter = ui.painter().clone();
            widgets::outlined(&painter, rect, 12.0, Color32::WHITE, BORDER);
            let column = Rect::from_min_size(rect.min, vec2(COLUMN_W, rect.height()));
            painter.rect_filled(
                Rect::from_min_max(pos2(column.right(), rect.top() + 1.0), pos2(column.right() + 1.0, rect.bottom() - 1.0)),
                0.0,
                BORDER,
            );
            nav(ui, column, panel, env.settings);
            let right = Rect::from_min_max(pos2(column.right() + 1.0, rect.top()), rect.max);
            let mut child = ui.new_child(UiBuilder::new().max_rect(right.shrink(PAGE_PAD)));
            egui::ScrollArea::vertical()
                .id_salt("settings-page")
                .auto_shrink([false, false])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(&mut child, |ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                    page(ui, panel, env, &mut choices);
                });
        });
    ctx.set_sublayer(overlay, area.response.layer_id);
    if panel.raise {
        panel.raise = false;
        ctx.move_to_top(overlay);
    }
    // Only a press that lands on the overlay closes the panel: a list that
    // opened from a control may reach past the panel's edge.
    let pressed_outside = ctx.input(|input| {
        input.pointer.any_pressed() && input.pointer.interact_pos().is_some_and(|pos| !area.response.rect.contains(pos))
    }) && ctx
        .input(|input| input.pointer.interact_pos())
        .is_some_and(|pos| ctx.layer_id_at(pos).is_none_or(|layer| layer == overlay));
    let escape = ctx.input(|input| input.key_pressed(Key::Escape));
    if pressed_outside || escape {
        panel.open = false;
        panel.dragging = None;
    }
    choices
}

fn nav(ui: &mut egui::Ui, column: Rect, panel: &mut SettingsPanel, settings: &Settings) {
    let inner = column.shrink2(vec2(12.0, PAD));
    let mut y = inner.top();
    let mut next = |height: f32| {
        let rect = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), height));
        y += height + NAV_GAP;
        rect
    };
    // The search, as the session list's: the left rows' height and insets.
    let field = next(NAV_ROW);
    widgets::outlined(ui.painter(), field, 8.0, Color32::WHITE, BORDER);
    toolbar::paint_icon(ui.painter(), pos2(field.left() + 20.0, field.center().y), "search", 18.0, MUTED);
    let text = Rect::from_min_max(pos2(field.left() + 42.0, field.top() + 2.0), pos2(field.right() - 10.0, field.bottom() - 2.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(text).layout(egui::Layout::left_to_right(egui::Align::Center)));
    child.add(
        egui::TextEdit::singleline(&mut panel.search)
            .id(egui::Id::new("settings-search"))
            .hint_text(egui::RichText::new("Search settings").color(HINT))
            .frame(egui::Frame::NONE)
            .font(ts::body())
            .desired_width(text.width()),
    );
    next(6.0);
    let shown = matching_pages(settings, &panel.search);
    let rows: [(&str, &str, Page); 4] = [
        ("settings", "General", Page::General),
        ("ev-plug", "Endpoints", Page::Endpoints),
        ("puzzle", "Skills", Page::Skills),
        ("chat-bubble", "Discussion", Page::Discussion),
    ];
    for (icon, label, page) in rows {
        if !shown.contains(&page) {
            continue;
        }
        // Skills and Discussion are set apart, as the owner set: other
        // skills' own pages may come between them later.
        if page == Page::Discussion {
            let divider = next(14.0);
            ui.painter().rect_filled(
                Rect::from_min_max(pos2(divider.left() + 8.0, divider.center().y), pos2(divider.right() - 8.0, divider.center().y + 1.0)),
                0.0,
                BORDER,
            );
        }
        let rect = next(NAV_ROW);
        let chosen = panel.page.section() == page;
        if sessions::nav_row(ui, rect, Some(icon), label, chosen, &format!("settings-{label}")).clicked() {
            panel.go(page.clone());
        }
        // A page with something to resolve carries the hazard at its right.
        let warns = match &page {
            Page::Discussion => !panel.unlisted.levels.is_empty(),
            Page::Skills => !panel.unlisted.skills.is_empty(),
            _ => false,
        };
        if warns {
            toolbar::paint_icon(ui.painter(), pos2(rect.right() - 20.0, rect.center().y), HAZARD, 16.0, WARNING);
        }
    }
}

fn page(ui: &mut egui::Ui, panel: &mut SettingsPanel, env: &mut Env, choices: &mut Vec<SettingsChoice>) {
    match panel.page.clone() {
        Page::General => general(ui, panel, env, choices),
        Page::Endpoints => endpoints(ui, panel, env, choices),
        Page::EndpointForm => endpoint_form(ui, panel, env, choices),
        Page::Skills => skills(ui, panel, env, choices),
        Page::Skill(id) => skill_page(ui, panel, env, &id, choices),
        Page::Discussion => discussion(ui, panel, env, choices),
        Page::LevelForm => level_form(ui, panel, env, choices),
    }
}

/// The page's title and gray line, with its dark button at the right, if
/// any. Returns whether the button was clicked.
fn header(ui: &mut egui::Ui, panel: &mut SettingsPanel, title: &str, line: &str, button: Option<&str>) -> bool {
    let width = ui.available_width();
    let button_w = button.map_or(0.0, |label| sessions::label_width(ui, label) + 30.0);
    let line_w = (width - button_w - 16.0).max(120.0);
    let galley = ui.painter().layout(line.to_string(), ts::body(), MUTED, line_w);
    let below = if line.is_empty() { 0.0 } else { galley.size().y };
    let (rect, _) = ui.allocate_exact_size(vec2(width, 34.0 + below + 22.0), Sense::hover());
    ui.painter().text(rect.left_top() + vec2(0.0, 15.0), Align2::LEFT_CENTER, title, widgets::semibold(20.0), TEXT);
    ui.painter().galley(rect.left_top() + vec2(0.0, 34.0), galley, MUTED);
    let Some(label) = button else {
        return false;
    };
    let button = Rect::from_min_size(pos2(rect.right() - button_w, rect.top()), vec2(button_w, CONTROL_H));
    dark_button(ui, panel, button, Some("plus"), label)
}

/// "‹ Endpoints" over a form's title. Returns whether it was clicked.
fn back(ui: &mut egui::Ui, panel: &mut SettingsPanel, label: &str) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    let width = ui.painter().layout_no_wrap(label.to_string(), ts::body(), MUTED).size().x + 18.0;
    let hit = Rect::from_min_size(rect.min, vec2(width, rect.height()));
    let response = ui
        .interact(hit, egui::Id::new(("settings-back", label)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let color = if response.hovered() { TEXT } else { MUTED };
    widgets::chevron_side(ui.painter(), pos2(hit.left() + 4.0, hit.center().y), false, color);
    ui.painter().text(pos2(hit.left() + 14.0, hit.center().y), Align2::LEFT_CENTER, label, ts::body(), color);
    panel.buttons.push((format!("‹ {label}"), hit));
    response.clicked()
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    ui.painter().text(pos2(rect.left(), rect.top() + 10.0), Align2::LEFT_CENTER, text, ts::title(), TEXT);
}

fn gap(ui: &mut egui::Ui, height: f32) {
    ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
}

/// A bordered card of `rows` rows, each `ROW_H` tall, with hairlines between
/// them. Returns each row's rect.
fn card(ui: &mut egui::Ui, rows: usize) -> Vec<Rect> {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_H * rows.max(1) as f32), Sense::hover());
    widgets::outlined(ui.painter(), rect, CARD_RADIUS, Color32::WHITE, BORDER);
    (0..rows)
        .map(|index| {
            let row = Rect::from_min_size(pos2(rect.left(), rect.top() + ROW_H * index as f32), vec2(width, ROW_H));
            if index > 0 {
                ui.painter().rect_filled(
                    Rect::from_min_max(pos2(row.left() + PAD, row.top()), pos2(row.right() - PAD, row.top() + 1.0)),
                    0.0,
                    ts::HAIRLINE,
                );
            }
            row
        })
        .collect()
}

/// A row's name, with a gray line under it, from `left`. Returns where the
/// name ends.
fn row_text(ui: &egui::Ui, row: Rect, left: f32, right: f32, name: &str, line: &str) -> f32 {
    let clip = ui.painter().with_clip_rect(Rect::from_min_max(pos2(left, row.top()), pos2(right, row.bottom())));
    let name = clip.text(pos2(left, row.center().y - 10.0), Align2::LEFT_CENTER, name, ts::title(), TEXT);
    clip.text(pos2(left, row.center().y + 10.0), Align2::LEFT_CENTER, line, ts::small(), MUTED);
    name.right()
}

/// A gray tag such as "Default" or "Built in", after `left`.
fn tag(ui: &egui::Ui, left: f32, center_y: f32, text: &str) -> Rect {
    let galley = ui.painter().layout_no_wrap(text.to_string(), ts::small(), MUTED);
    let rect = Rect::from_min_size(pos2(left, center_y - 10.0), vec2(galley.size().x + 16.0, 20.0));
    ui.painter().rect_filled(rect, 10.0, TAG_FILL);
    ui.painter().galley(pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0), galley, MUTED);
    rect
}

/// A row's control: right-aligned, the panel's one height.
fn control(row: Rect, width: f32, trailing: f32) -> Rect {
    let width = width.min(row.width() * 0.5);
    Rect::from_min_size(pos2(row.right() - PAD - trailing - width, row.center().y - CONTROL_H / 2.0), vec2(width, CONTROL_H))
}

fn dark_button(ui: &mut egui::Ui, panel: &mut SettingsPanel, rect: Rect, icon: Option<&str>, label: &str) -> bool {
    let response = ui
        .interact(rect, egui::Id::new(("settings-dark", label)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    ui.painter().rect_filled(rect, 8.0, DARK);
    match icon {
        Some(icon) => {
            toolbar::paint_icon(ui.painter(), pos2(rect.left() + 20.0, rect.center().y), icon, 18.0, Color32::WHITE);
            ui.painter().text(pos2(rect.left() + 38.0, rect.center().y), Align2::LEFT_CENTER, label, ts::body(), Color32::WHITE);
        }
        None => {
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, ts::body(), Color32::WHITE);
        }
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    panel.buttons.push((label.to_string(), rect));
    response.clicked()
}

fn plain(ui: &mut egui::Ui, panel: &mut SettingsPanel, rect: Rect, label: &str) -> bool {
    panel.buttons.push((label.to_string(), rect));
    sessions::plain_button(ui, rect, label, &format!("settings-plain-{label}"))
}

/// A one-line field in a row. `code` sets it in the monospace face, as an
/// address is; `secret` masks it.
fn field(ui: &mut egui::Ui, rect: Rect, id: &str, text: &mut String, code: bool, secret: bool, hint: &str) -> egui::Response {
    widgets::outlined(ui.painter(), rect, 8.0, Color32::WHITE, BORDER);
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink2(vec2(INSET, 2.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.add(
        egui::TextEdit::singleline(text)
            .id(egui::Id::new(("settings-field", id)))
            .frame(egui::Frame::NONE)
            .font(if code { ts::code() } else { ts::body() })
            .password(secret)
            .hint_text(egui::RichText::new(hint).color(HINT))
            .desired_width(rect.width() - 2.0 * INSET),
    )
}

/// A control that opens a list of `options`. Returns the one chosen.
fn select(ui: &mut egui::Ui, rect: Rect, id: &str, shown: &str, options: &[String], selected: Option<usize>) -> Option<usize> {
    let response = ui
        .interact(rect, egui::Id::new(("settings-select", id)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    widgets::outlined(ui.painter(), rect, 8.0, if response.hovered() || open { SELECTED } else { Color32::WHITE }, BORDER);
    let clip = ui.painter().with_clip_rect(rect.shrink2(vec2(INSET, 0.0)).with_max_x(rect.right() - 28.0));
    clip.text(pos2(rect.left() + INSET, rect.center().y), Align2::LEFT_CENTER, shown, ts::body(), TEXT);
    widgets::chevron(ui.painter(), pos2(rect.right() - 16.0, rect.center().y), MUTED, widgets::Scale(1.0));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, shown));
    let mut chosen = None;
    egui::Popup::menu(&response).width(rect.width()).gap(4.0).show(|ui| {
        ui.set_width(rect.width() - 12.0);
        ui.spacing_mut().item_spacing.y = 2.0;
        for (index, option) in options.iter().enumerate() {
            let (row, row_response) = ui.allocate_exact_size(vec2(rect.width() - 12.0, 30.0), Sense::click());
            if Some(index) == selected || row_response.hovered() {
                ui.painter().rect_filled(row, 6.0, SELECTED);
            }
            ui.painter().with_clip_rect(row).text(pos2(row.left() + 10.0, row.center().y), Align2::LEFT_CENTER, option, ts::body(), TEXT);
            if row_response.clicked() {
                chosen = Some(index);
                ui.close();
            }
        }
    });
    chosen
}

/// The model control: a list of the endpoint's models with a search. A
/// model the list lacks can still be typed and used.
fn model_select(ui: &mut egui::Ui, rect: Rect, id: &str, current: &str, known: &[assistant::Model], models: Option<&Models>, search: &mut String) -> Option<String> {
    let response = ui
        .interact(rect, egui::Id::new(("settings-model", id)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    widgets::outlined(ui.painter(), rect, 8.0, if response.hovered() || open { SELECTED } else { Color32::WHITE }, BORDER);
    let shown = if current.is_empty() { "Choose a model".to_string() } else { model_label(known, current) };
    let clip = ui.painter().with_clip_rect(rect.shrink2(vec2(INSET, 0.0)).with_max_x(rect.right() - 28.0));
    clip.text(pos2(rect.left() + INSET, rect.center().y), Align2::LEFT_CENTER, &shown, ts::body(), if current.is_empty() { HINT } else { TEXT });
    widgets::chevron(ui.painter(), pos2(rect.right() - 16.0, rect.center().y), MUTED, widgets::Scale(1.0));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, shown.as_str()));
    let mut chosen = None;
    let width = rect.width().max(300.0);
    egui::Popup::menu(&response)
        .width(width)
        .gap(4.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            if let Some(model) = model_list(ui, id, current, models, search, width - 12.0) {
                chosen = Some(model);
                ui.close();
            }
        });
    if chosen.is_some() {
        search.clear();
    }
    chosen
}

/// A model list with a search, as a model control or a cell's chevron opens
/// it: the endpoint's models, and a model the list lacks as typed. Returns
/// the model chosen.
pub(crate) fn model_list(ui: &mut egui::Ui, id: &str, current: &str, models: Option<&Models>, search: &mut String, width: f32) -> Option<String> {
    let mut chosen = None;
    ui.set_width(width);
    ui.spacing_mut().item_spacing.y = 2.0;
    // The search, as the panel's fields: the one height, a rounded hairline,
    // and the search mark.
    let (field, _) = ui.allocate_exact_size(vec2(width, CONTROL_H), Sense::hover());
    widgets::outlined(ui.painter(), field, 8.0, Color32::WHITE, BORDER);
    toolbar::paint_icon(ui.painter(), pos2(field.left() + 18.0, field.center().y), "search", 16.0, MUTED);
    let text = Rect::from_min_max(pos2(field.left() + 36.0, field.top() + 2.0), pos2(field.right() - 10.0, field.bottom() - 2.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(text).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let edit = child.add(
        egui::TextEdit::singleline(search)
            .id(egui::Id::new(("settings-model-search", id)))
            .hint_text(egui::RichText::new("Search models").color(HINT))
            .frame(egui::Frame::NONE)
            .font(ts::body())
            .desired_width(text.width()),
    );
    edit.request_focus();
    ui.add_space(4.0);
    let wanted = search.trim().to_lowercase();
    let mut row = |ui: &mut egui::Ui, label: &str, line: &str, value: &str| {
        let (rect, row_response) = ui.allocate_exact_size(vec2(width, 40.0), Sense::click());
        if value == current || row_response.hovered() {
            ui.painter().rect_filled(rect, 6.0, SELECTED);
        }
        let clip = ui.painter().with_clip_rect(rect);
        clip.text(pos2(rect.left() + 10.0, rect.center().y - 8.0), Align2::LEFT_CENTER, label, ts::body(), TEXT);
        clip.text(pos2(rect.left() + 10.0, rect.center().y + 9.0), Align2::LEFT_CENTER, line, ts::small(), MUTED);
        if row_response.clicked() {
            chosen = Some(value.to_string());
        }
    };
    egui::ScrollArea::vertical().id_salt(("settings-models", id)).max_height(280.0).show(ui, |ui| {
        match models {
            None => {
                ui.label(egui::RichText::new("Finding the models…").font(ts::body()).color(MUTED));
            }
            Some(Err(reason)) => {
                ui.add(egui::Label::new(egui::RichText::new(reason.as_str()).font(ts::small()).color(MUTED)).wrap());
            }
            Some(Ok(list)) => {
                let mut any = false;
                for model in list {
                    let hay = format!("{} {}", model.name, model.id).to_lowercase();
                    if !wanted.is_empty() && !hay.contains(&wanted) {
                        continue;
                    }
                    any = true;
                    row(ui, crate::chat::short_name(&model.name), &model.id, &model.id);
                }
                if !any && wanted.is_empty() {
                    ui.label(egui::RichText::new("This endpoint lists no models.").font(ts::body()).color(MUTED));
                }
            }
        }
        // A model the list does not name, as typed.
        let typed = search.trim().to_string();
        let listed = matches!(models, Some(Ok(list)) if list.iter().any(|model| model.id == typed));
        if !typed.is_empty() && !listed {
            row(ui, &format!("Use “{typed}”"), "As typed", &typed);
        }
    });
    if chosen.is_some() {
        search.clear();
    }
    chosen
}

/// A row's "…". Returns its response, for its menu.
fn more(ui: &mut egui::Ui, row: Rect, id: &str) -> egui::Response {
    let rect = Rect::from_center_size(pos2(row.right() - PAD - 14.0, row.center().y), vec2(28.0, 28.0));
    let response = ui.interact(rect, egui::Id::new(("settings-more", id)), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 6.0, SELECTED);
    }
    toolbar::paint_icon(ui.painter(), rect.center(), "more-horiz", 18.0, MUTED);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "More"));
    response
}

fn menu_item(label: &str, enabled: bool) -> widgets::MenuItem<'_> {
    widgets::MenuItem { label, icon: None, enabled, submenu: false, divider: false }
}

fn notice(ui: &mut egui::Ui, panel: &SettingsPanel) {
    if let Some(text) = &panel.notice {
        gap(ui, 10.0);
        ui.add(egui::Label::new(egui::RichText::new(text.as_str()).font(ts::small()).color(ts::FAILED)).wrap());
    }
}

fn general(ui: &mut egui::Ui, panel: &mut SettingsPanel, env: &mut Env, choices: &mut Vec<SettingsChoice>) {
    header(ui, panel, "General", "How Studio starts new work.", None);
    let rows = card(ui, 1);
    let row = rows[0];
    let change_w = sessions::label_width(ui, "Change");
    let change = control(row, change_w, 0.0);
    let path = Rect::from_min_max(pos2(row.center().x - 20.0, change.top()), pos2(change.left() - 8.0, change.bottom()));
    row_text(ui, row, row.left() + PAD, path.left() - 8.0, "Project location", "Where New Project suggests new projects");
    widgets::outlined(ui.painter(), path, 8.0, Color32::from_rgb(246, 247, 249), BORDER);
    ui.painter().with_clip_rect(path.shrink(2.0)).text(
        pos2(path.left() + INSET, path.center().y),
        Align2::LEFT_CENTER,
        sessions::shorten_home(&env.projects_root),
        ts::code(),
        MUTED,
    );
    if plain(ui, panel, change, "Change") {
        choices.push(SettingsChoice::ChooseProjectsRoot);
    }
}

fn status_mark(ui: &egui::Ui, right: f32, center_y: f32, status: &Status) -> f32 {
    let color = match status {
        Status::Connected => CONNECTED,
        Status::KeyMissing | Status::Answered(_) => WARNING,
        Status::Checking | Status::NotRunning => HINT,
    };
    let label = status.label();
    let galley = ui.painter().layout_no_wrap(label, ts::small(), MUTED);
    let text_left = right - galley.size().x;
    ui.painter().galley(pos2(text_left, center_y - galley.size().y / 2.0), galley, MUTED);
    widgets::disc(ui.painter(), pos2(text_left - 10.0, center_y), 3.5, color);
    text_left - 18.0
}

fn endpoints(ui: &mut egui::Ui, panel: &mut SettingsPanel, env: &mut Env, choices: &mut Vec<SettingsChoice>) {
    let line = if env.uses_file {
        "Where Studio sends prompts. No system keyring answered, so keys are kept in a private file in Studio's data folder."
    } else {
        "Where Studio sends prompts. Keys are kept in your system keyring."
    };
    if header(ui, panel, "Endpoints", line, Some("Add Endpoint")) {
        panel.add_endpoint();
        return;
    }
    section_label(ui, "Your endpoints");
    let settings = env.settings;
    if settings.endpoints.is_empty() {
        ui.label(egui::RichText::new("No endpoints yet.").font(ts::body()).color(MUTED));
        return;
    }
    let rows = card(ui, settings.endpoints.len());
    for (endpoint, row) in settings.endpoints.iter().zip(rows) {
        let key = env.keys.get(&endpoint.id);
        let has_key = key.is_some();
        let status = env.probes.status(ui.ctx(), endpoint, key);
        let menu = more(ui, row, &endpoint.id);
        let status_left = status_mark(ui, menu.rect.left() - 14.0, row.center().y, &status);
        let line = format!("{} · {}", endpoint.address, if has_key { "Key saved" } else { "No key" });
        row_text(ui, row, row.left() + PAD, status_left - 8.0, &endpoint.name, &line);
        egui::Popup::menu(&menu).show(|ui| match widgets::menu_rows(ui, &[menu_item("Edit", true), menu_item("Delete", true)]) {
            Some(0) => panel.edit_endpoint(endpoint, has_key),
            Some(1) => {
                let mut users = settings.users_of(&endpoint.id);
                users.extend(settings.levels.iter().filter(|level| level.endpoint == endpoint.id).map(|level| level.name.clone()));
                if users.is_empty() {
                    choices.push(SettingsChoice::DeleteEndpoint(endpoint.id.clone()));
                } else {
                    panel.notice = Some(format!(
                        "“{}” can't be deleted while {} use it. Choose another endpoint for them first.",
                        endpoint.name,
                        users.join(", ")
                    ));
                }
            }
            _ => {}
        });
    }
    notice(ui, panel);
}

fn endpoint_form(ui: &mut egui::Ui, panel: &mut SettingsPanel, env: &mut Env, choices: &mut Vec<SettingsChoice>) {
    if back(ui, panel, "Endpoints") {
        env.probes.clear_form();
        panel.go(Page::Endpoints);
        return;
    }
    let editing = panel.endpoint_form.editing.is_some();
    header(ui, panel, if editing { "Edit Endpoint" } else { "Add Endpoint" }, "", None);
    let rows = card(ui, 4);
    let form = &mut panel.endpoint_form;
    // Type
    let kinds: Vec<String> = EndpointKind::ALL.iter().map(|kind| kind.label().to_string()).collect();
    let chosen = EndpointKind::ALL.iter().position(|kind| *kind == form.kind);
    let type_rect = control(rows[0], CONTROL_W, 0.0);
    row_text(ui, rows[0], rows[0].left() + PAD, type_rect.left() - 8.0, "Type", "What kind of service this is");
    if let Some(index) = select(ui, type_rect, "endpoint-type", form.kind.label(), &kinds, chosen) {
        let old = form.kind.suggested();
        let kind = EndpointKind::ALL[index];
        // The name and address follow the type until the person changes them.
        if form.name.trim().is_empty() || form.name == old.0 {
            form.name = kind.suggested().0.to_string();
        }
        if form.address.trim().is_empty() || form.address == old.1 {
            form.address = kind.suggested().1.to_string();
        }
        form.kind = kind;
        env.probes.clear_form();
    }
    let name_rect = control(rows[1], CONTROL_W, 0.0);
    row_text(ui, rows[1], rows[1].left() + PAD, name_rect.left() - 8.0, "Name", "How it is shown in Studio");
    field(ui, name_rect, "endpoint-name", &mut form.name, false, false, "");
    let address_rect = control(rows[2], CONTROL_W, 0.0);
    row_text(ui, rows[2], rows[2].left() + PAD, address_rect.left() - 8.0, "Address", "The service's base URL");
    if field(ui, address_rect, "endpoint-address", &mut form.address, true, false, "").changed() {
        env.probes.clear_form();
    }
    let key_rect = control(rows[3], CONTROL_W, 0.0);
    let where_kept = if env.uses_file { "Kept in a private file, never in a project" } else { "Kept in your system keyring, never in a project" };
    row_text(ui, rows[3], rows[3].left() + PAD, key_rect.left() - 8.0, "API key", where_kept);
    let hint = if form.has_key { "Saved. Type to replace it" } else if form.kind == EndpointKind::Openrouter { "Required" } else { "Optional" };
    field(ui, key_rect, "endpoint-key", &mut form.key, false, true, hint);
    if let Some(error) = &form.error {
        gap(ui, 10.0);
        ui.label(egui::RichText::new(error.as_str()).font(ts::small()).color(ts::FAILED));
    }
    gap(ui, 16.0);
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), CONTROL_H), Sense::hover());
    let save_label = if editing { "Save" } else { "Add Endpoint" };
    let save_w = sessions::label_width(ui, save_label);
    let save = Rect::from_min_size(pos2(bar.right() - save_w, bar.top()), vec2(save_w, CONTROL_H));
    let cancel_w = sessions::label_width(ui, "Cancel");
    let cancel = Rect::from_min_size(pos2(save.left() - 8.0 - cancel_w, bar.top()), vec2(cancel_w, CONTROL_H));
    let test_w = sessions::label_width(ui, "Test Connection");
    let test = Rect::from_min_size(pos2(cancel.left() - 8.0 - test_w, bar.top()), vec2(test_w, CONTROL_H));
    if let Some(status) = env.probes.form_status() {
        status_mark(ui, test.left() - 12.0, bar.center().y, &status);
    }
    let form = panel.endpoint_form.clone();
    if plain(ui, panel, test, "Test Connection") {
        let key = if form.key.trim().is_empty() {
            form.editing.as_deref().and_then(|id| env.keys.get(id))
        } else {
            Some(form.key.trim().to_string())
        };
        env.probes.test_form(ui.ctx(), form.kind, form.address.trim(), key);
    }
    if plain(ui, panel, cancel, "Cancel") {
        env.probes.clear_form();
        panel.go(Page::Endpoints);
        return;
    }
    if dark_button(ui, panel, save, None, save_label) {
        if form.name.trim().is_empty() || form.address.trim().is_empty() {
            panel.endpoint_form.error = Some(String::from("An endpoint needs a name and an address."));
            return;
        }
        let key = Some(form.key.trim().to_string()).filter(|key| !key.is_empty());
        choices.push(SettingsChoice::SaveEndpoint {
            id: form.editing.clone(),
            kind: form.kind,
            name: form.name.trim().to_string(),
            address: form.address.trim().to_string(),
            key,
        });
        env.probes.clear_form();
        panel.go(Page::Endpoints);
    }
}

/// The endpoint choices a skill or level offers, and which is chosen.
fn endpoint_options(settings: &Settings, current: Option<&str>, chats_only: bool) -> (Vec<String>, Vec<String>, Option<usize>) {
    let offered: Vec<&EndpointSetting> = settings.endpoints.iter().filter(|endpoint| !chats_only || endpoint.kind.chats()).collect();
    let names = offered.iter().map(|endpoint| endpoint.name.clone()).collect();
    let ids: Vec<String> = offered.iter().map(|endpoint| endpoint.id.clone()).collect();
    let chosen = current.and_then(|current| ids.iter().position(|id| id == current));
    (names, ids, chosen)
}

fn skills(ui: &mut egui::Ui, panel: &mut SettingsPanel, env: &mut Env, choices: &mut Vec<SettingsChoice>) {
    if header(ui, panel, "Skills", "Each skill sends its prompts to the endpoint you choose for it.", Some("New Skill")) {
        let skill = SkillSetting {
            id: protocol::cell_id(),
            name: String::from("New Skill"),
            description: String::new(),
            endpoint: env.settings.discussion().and_then(|skill| skill.endpoint.clone()),
            model: None,
            prompt: String::new(),
        };
        choices.push(SettingsChoice::SaveSkill(skill.clone()));
        panel.open_skill(&skill);
        return;
    }
    section_label(ui, "Your skills");
    let settings = env.settings;
    let rows = card(ui, settings.skills.len());
    for (skill, row) in settings.skills.iter().zip(rows) {
        let built_in = skill.id == DISCUSSION;
        // Discussion has no "…", and its dropdown stays where the others' are.
        let trailing = 36.0;
        let menu = (!built_in).then(|| more(ui, row, &skill.id));
        let select_rect = control(row, 200.0, trailing);
        let (names, ids, chosen) = endpoint_options(settings, skill.endpoint.as_deref(), built_in);
        let shown = chosen.map_or("No endpoint", |index| names[index].as_str()).to_string();
        let body = Rect::from_min_max(row.min, pos2(select_rect.left() - 8.0, row.bottom()));
        let response = ui
            .interact(body, egui::Id::new(("settings-skill", &skill.id)), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        // The warning sits on the name's line; the description below keeps
        // the row's whole width.
        let words = not_on(&shown);
        let warned = panel.unlisted.skills.contains(&skill.id);
        let warning_w = ui.painter().layout_no_wrap(words.clone(), ts::small(), WARNING).size().x + 30.0;
        let name_right = if warned { body.right() - 8.0 - warning_w } else { body.right() };
        let clip = ui.painter().with_clip_rect(Rect::from_min_max(row.min, pos2(name_right, row.bottom())));
        let name = clip.text(pos2(row.left() + PAD, row.center().y - 10.0), Align2::LEFT_CENTER, &skill.name, ts::title(), TEXT);
        ui.painter().with_clip_rect(Rect::from_min_max(row.min, pos2(body.right(), row.bottom()))).text(
            pos2(row.left() + PAD, row.center().y + 10.0),
            Align2::LEFT_CENTER,
            &skill.description,
            ts::small(),
            MUTED,
        );
        if warned {
            warning(ui, body.right() - 8.0, row.center().y - 10.0, &words);
        }
        let name_right = name.right();
        if built_in {
            toolbar::paint_icon(ui.painter(), pos2(name_right + 14.0, row.center().y - 10.0), "lock", 14.0, MUTED);
            tag(ui, name_right + 28.0, row.center().y - 10.0, "Built in");
        }
        panel.buttons.push((skill.name.clone(), body));
        if let Some(index) = select(ui, select_rect, &format!("skill-endpoint-{}", skill.id), &shown, &names, chosen) {
            let mut changed = skill.clone();
            changed.endpoint = Some(ids[index].clone());
            choices.push(SettingsChoice::SaveSkill(changed));
        }
        if response.clicked() {
            panel.open_skill(skill);
        }
        if let Some(menu) = menu {
            egui::Popup::menu(&menu).show(|ui| {
                if widgets::menu_rows(ui, &[menu_item("Delete", true)]) == Some(0) {
                    choices.push(SettingsChoice::DeleteSkill(skill.id.clone()));
                }
            });
        }
    }
}

/// A tall prompt box in the monospace face, with line numbers in its
/// margin. Returns whether the text changed.
fn prompt_box(ui: &mut egui::Ui, id: &str, text: &mut String, height: f32) -> bool {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    widgets::outlined(ui.painter(), rect, CARD_RADIUS, Color32::WHITE, BORDER);
    let margin = 44.0;
    ui.painter().rect_filled(
        Rect::from_min_max(pos2(rect.left() + margin, rect.top() + 1.0), pos2(rect.left() + margin + 1.0, rect.bottom() - 1.0)),
        0.0,
        ts::HAIRLINE,
    );
    let inner = Rect::from_min_max(pos2(rect.left() + margin + 12.0, rect.top() + 12.0), pos2(rect.right() - 12.0, rect.bottom() - 12.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner));
    let mut changed = false;
    egui::ScrollArea::vertical()
        .id_salt(("settings-prompt-scroll", id))
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            let output = egui::TextEdit::multiline(text)
                .id(egui::Id::new(("settings-prompt", id)))
                .frame(egui::Frame::NONE)
                .font(ts::code())
                .desired_width(inner.width())
                .desired_rows(((inner.height() / ts::code_line()) as usize).max(4))
                .show(ui);
            changed = output.response.changed();
            // A number beside the first row of each line.
            let mut number = 1;
            let mut starts = true;
            let painter = ui.painter().with_clip_rect(Rect::from_min_max(pos2(rect.left(), inner.top()), pos2(inner.left(), inner.bottom())));
            for row in &output.galley.rows {
                if starts {
                    let y = output.galley_pos.y + row.rect().center().y;
                    painter.text(pos2(rect.left() + margin - 12.0, y), Align2::RIGHT_CENTER, number.to_string(), ts::code(), MUTED);
                    number += 1;
                }
                starts = row.ends_with_newline;
            }
        });
    changed
}

fn skill_page(ui: &mut egui::Ui, panel: &mut SettingsPanel, env: &mut Env, id: &str, choices: &mut Vec<SettingsChoice>) {
    if back(ui, panel, "Skills") {
        panel.go(Page::Skills);
        return;
    }
    let settings = env.settings;
    let Some(stored) = settings.skill(id) else {
        panel.go(Page::Skills);
        return;
    };
    if panel.skill.as_ref().is_none_or(|skill| skill.id != id) {
        panel.skill = Some(stored.clone());
    }
    let mut skill = panel.skill.clone().unwrap_or_else(|| stored.clone());
    let built_in = skill.id == DISCUSSION;
    let before = skill.clone();
    if built_in {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, 34.0 + 20.0 + 22.0), Sense::hover());
        let title = ui.painter().text(rect.left_top() + vec2(0.0, 15.0), Align2::LEFT_CENTER, &skill.name, widgets::semibold(20.0), TEXT);
        toolbar::paint_icon(ui.painter(), pos2(title.right() + 16.0, title.center().y), "lock", 16.0, MUTED);
        ui.painter().text(rect.left_top() + vec2(0.0, 44.0), Align2::LEFT_CENTER, "Built in. This skill can't be removed.", ts::body(), MUTED);
    } else {
        header(ui, panel, &skill.name, "Runs its prompt on its own endpoint when the assistant calls it.", None);
    }
    let rows = card(ui, if built_in { 1 } else { 4 });
    let mut index = 0;
    if !built_in {
        let name_rect = control(rows[0], CONTROL_W, 0.0);
        row_text(ui, rows[0], rows[0].left() + PAD, name_rect.left() - 8.0, "Name", "The tool's name, as the assistant sees it");
        field(ui, name_rect, "skill-name", &mut skill.name, false, false, "");
        let description_rect = control(rows[1], CONTROL_W, 0.0);
        row_text(ui, rows[1], rows[1].left() + PAD, description_rect.left() - 8.0, "Description", "When the assistant should use it");
        field(ui, description_rect, "skill-description", &mut skill.description, false, false, "");
        index = 2;
    }
    let endpoint_rect = control(rows[index], CONTROL_W, 0.0);
    let line = if built_in { "Where the assistant's replies come from" } else { "Where its prompts go" };
    row_text(ui, rows[index], rows[index].left() + PAD, endpoint_rect.left() - 8.0, "Endpoint", line);
    let (names, ids, chosen) = endpoint_options(settings, skill.endpoint.as_deref(), built_in);
    let shown = chosen.map_or("No endpoint", |index| names[index].as_str()).to_string();
    if let Some(picked) = select(ui, endpoint_rect, &format!("skill-page-endpoint-{id}"), &shown, &names, chosen) {
        skill.endpoint = Some(ids[picked].clone());
    }
    if !built_in {
        let model_rect = control(rows[3], CONTROL_W, 0.0);
        let endpoint = skill.endpoint.as_deref().and_then(|id| settings.endpoint(id)).cloned();
        let line = match &endpoint {
            Some(endpoint) if !endpoint.kind.chats() => "Image skills run from a later version",
            _ => "From the endpoint's models",
        };
        let name = endpoint.as_ref().map_or("the endpoint", |endpoint| endpoint.name.as_str());
        let missing = panel.unlisted.skills.contains(&skill.id).then_some(name);
        model_row(ui, rows[3], model_rect.left() - 8.0, line, missing);
        let known = env.probes.known_models();
        let models = match &endpoint {
            Some(endpoint) => {
                let key = env.keys.get(&endpoint.id);
                env.probes.models(ui.ctx(), endpoint, key).cloned()
            }
            None => None,
        };
        let current = skill.model.clone().unwrap_or_default();
        if let Some(model) = model_select(ui, model_rect, &format!("skill-model-{id}"), &current, &known, models.as_ref(), &mut panel.model_search) {
            skill.model = Some(model);
        }
    }
    gap(ui, 22.0);
    section_label(ui, "Prompt");
    let line = if built_in {
        "How the assistant speaks with you. Studio sends this with every message, after its own rules."
    } else {
        "Sent first, then a blank line, then what the assistant asks the skill for."
    };
    ui.add(egui::Label::new(egui::RichText::new(line).font(ts::body()).color(MUTED)).wrap());
    gap(ui, 10.0);
    let height = ui.available_height().max(220.0);
    prompt_box(ui, &skill.id, &mut skill.prompt, height);
    if skill != before {
        choices.push(SettingsChoice::SaveSkill(skill.clone()));
    }
    panel.skill = Some(skill);
}

fn discussion(ui: &mut egui::Ui, panel: &mut SettingsPanel, env: &mut Env, choices: &mut Vec<SettingsChoice>) {
    let settings = env.settings;
    let source = settings.discussion_endpoint().map_or("no endpoint yet", |endpoint| endpoint.name.as_str());
    let line = format!("The assistant's levels, lowest first. Models come from {source}, the endpoint the Discussion skill uses.");
    if header(ui, panel, "Discussion", &line, Some("Add Level")) {
        panel.add_level(settings);
        return;
    }
    section_label(ui, "Levels");
    // Ask for the Discussion endpoint's models, so rows show their names.
    if let Some(endpoint) = settings.discussion_endpoint() {
        let key = env.keys.get(&endpoint.id);
        env.probes.models(ui.ctx(), endpoint, key);
    }
    let known = env.probes.known_models();
    let rows = card(ui, settings.levels.len());
    let only = settings.levels.len() <= 1;
    let mut drop_at = None;
    for (index, (level, row)) in settings.levels.iter().zip(rows.iter().copied()).enumerate() {
        let handle = Rect::from_center_size(pos2(row.left() + PAD + 12.0, row.center().y), vec2(24.0, 36.0));
        let grip = ui
            .interact(handle, egui::Id::new(("settings-level-handle", &level.id)), Sense::drag())
            .on_hover_cursor(egui::CursorIcon::Grab);
        // The view's grip: two columns of three dots.
        for column in [-3.5, 3.5] {
            for row in [-6.0, 0.0, 6.0] {
                widgets::disc(ui.painter(), handle.center() + vec2(column, row), 1.6, MUTED);
            }
        }
        if grip.drag_started() {
            panel.dragging = Some(index);
        }
        if panel.dragging == Some(index)
            && let Some(pointer) = ui.ctx().pointer_interact_pos()
        {
            // Where it would land: before the row whose middle is below the pointer.
            let to = rows.iter().position(|other| pointer.y < other.center().y).unwrap_or(rows.len());
            let y = rows.get(to).map_or(rows[rows.len() - 1].bottom(), |other| other.top());
            ui.painter().rect_filled(Rect::from_min_max(pos2(row.left() + PAD, y - 1.0), pos2(row.right() - PAD, y + 1.0)), 1.0, crate::widgets::INK);
            if grip.drag_stopped() {
                drop_at = Some((index, if to > index { to - 1 } else { to }));
            }
        }
        let menu = more(ui, row, &level.id);
        let left = handle.right() + 12.0;
        let mut right = menu.rect.left() - 8.0;
        if panel.unlisted.levels.contains(&level.id) {
            right = warning(ui, menu.rect.left() - 16.0, row.center().y - 10.0, &not_on(source));
        }
        let name_right = row_text(ui, row, left, right, &level.name, &level_line(settings, &known, level));
        if settings.default_level.as_deref() == Some(level.id.as_str()) {
            tag(ui, name_right + 10.0, row.center().y - 10.0, "Default");
        }
        egui::Popup::menu(&menu).show(|ui| {
            match widgets::menu_rows(ui, &[menu_item("Edit", true), menu_item("Make Default", true), menu_item("Delete", !only)]) {
                Some(0) => panel.edit_level(level),
                Some(1) => choices.push(SettingsChoice::MakeDefault(level.id.clone())),
                Some(2) if !only => choices.push(SettingsChoice::DeleteLevel(level.id.clone())),
                _ => {}
            }
        });
    }
    if let Some((from, to)) = drop_at {
        panel.dragging = None;
        if from != to {
            choices.push(SettingsChoice::MoveLevel { from, to });
        }
    }
    if ui.input(|input| !input.pointer.any_down()) {
        panel.dragging = None;
    }
}

fn level_form(ui: &mut egui::Ui, panel: &mut SettingsPanel, env: &mut Env, choices: &mut Vec<SettingsChoice>) {
    if back(ui, panel, "Discussion") {
        panel.go(Page::Discussion);
        return;
    }
    let settings = env.settings;
    let editing = panel.level_form.editing.is_some();
    header(ui, panel, if editing { "Edit Level" } else { "Add Level" }, "A model at a reasoning effort.", None);
    // Levels follow the Discussion skill's endpoint, as the owner set on
    // 2026-10-29: the form has no endpoint of its own.
    let rows = card(ui, 3);
    let form = &mut panel.level_form;
    if let Some(endpoint) = settings.discussion_endpoint() {
        form.endpoint = endpoint.id.clone();
    }
    let name_rect = control(rows[0], CONTROL_W, 0.0);
    row_text(ui, rows[0], rows[0].left() + PAD, name_rect.left() - 8.0, "Name", "As the composer's picker shows it");
    field(ui, name_rect, "level-name", &mut form.name, false, false, "");
    let model_rect = control(rows[1], CONTROL_W, 0.0);
    let source = settings.endpoint(&form.endpoint).map_or("the endpoint", |endpoint| endpoint.name.as_str());
    let listed = format!("From {source}'s models");
    let known = env.probes.known_models();
    let models = match settings.endpoint(&form.endpoint) {
        Some(endpoint) => {
            let key = env.keys.get(&endpoint.id);
            env.probes.models(ui.ctx(), endpoint, key).cloned()
        }
        None => None,
    };
    let missing = matches!(&models, Some(Ok(list)) if !form.model.is_empty() && !list.iter().any(|model| model.id == form.model));
    model_row(ui, rows[1], model_rect.left() - 8.0, &listed, missing.then_some(source));
    if let Some(model) = model_select(ui, model_rect, "level-model", &form.model, &known, models.as_ref(), &mut panel.model_search) {
        panel.level_form.model = model;
    }
    let form = &mut panel.level_form;
    let effort_rect = control(rows[2], CONTROL_W, 0.0);
    row_text(ui, rows[2], rows[2].left() + PAD, effort_rect.left() - 8.0, "Effort", "How hard the model reasons");
    let efforts = [None, Some(Effort::Low), Some(Effort::Medium), Some(Effort::High)];
    let labels: Vec<String> = ["None", "Low", "Medium", "High"].iter().map(|label| label.to_string()).collect();
    let chosen = efforts.iter().position(|effort| *effort == form.effort);
    let shown = chosen.map_or("None", |index| labels[index].as_str()).to_string();
    if let Some(picked) = select(ui, effort_rect, "level-effort", &shown, &labels, chosen) {
        form.effort = efforts[picked];
    }
    if let Some(error) = &form.error {
        gap(ui, 10.0);
        ui.label(egui::RichText::new(error.as_str()).font(ts::small()).color(ts::FAILED));
    }
    gap(ui, 16.0);
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), CONTROL_H), Sense::hover());
    let save_label = if editing { "Save" } else { "Add Level" };
    let save_w = sessions::label_width(ui, save_label);
    let save = Rect::from_min_size(pos2(bar.right() - save_w, bar.top()), vec2(save_w, CONTROL_H));
    let cancel_w = sessions::label_width(ui, "Cancel");
    let cancel = Rect::from_min_size(pos2(save.left() - 8.0 - cancel_w, bar.top()), vec2(cancel_w, CONTROL_H));
    if plain(ui, panel, cancel, "Cancel") {
        panel.go(Page::Discussion);
        return;
    }
    if dark_button(ui, panel, save, None, save_label) {
        let form = panel.level_form.clone();
        if form.name.trim().is_empty() || form.model.trim().is_empty() || form.endpoint.is_empty() {
            panel.level_form.error = Some(String::from("A level needs a name, an endpoint, and a model."));
            return;
        }
        choices.push(SettingsChoice::SaveLevel(Level {
            id: form.editing.clone().unwrap_or_else(protocol::cell_id),
            name: form.name.trim().to_string(),
            endpoint: form.endpoint.clone(),
            model: form.model.trim().to_string(),
            effort: form.effort,
        }));
        panel.go(Page::Discussion);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Catches a search that hides the page a row lives on.
    #[test]
    fn the_search_keeps_pages_whose_rows_match() {
        let mut settings = Settings::default();
        settings.first_start("https://stand-in/v1");
        assert_eq!(matching_pages(&settings, "").len(), 4);
        assert_eq!(matching_pages(&settings, "sonnet"), vec![Page::Discussion]);
        assert_eq!(matching_pages(&settings, "openrouter"), vec![Page::Endpoints]);
        assert_eq!(matching_pages(&settings, "location"), vec![Page::General]);
        assert!(matching_pages(&settings, "nothing like this").is_empty());
    }

    /// Catches a switched endpoint that leaves levels and skills on models
    /// it does not have, with nothing said: each warns until a listed model
    /// is chosen, and nothing warns while the list is unknown.
    #[test]
    fn models_an_endpoint_lacks_are_warned_about() {
        let mut settings = Settings::default();
        settings.first_start("https://stand-in/v1");
        let local = EndpointSetting { id: "local".into(), name: "Ollama".into(), kind: EndpointKind::Openai, address: "http://local/v1".into() };
        settings.endpoints.push(local);
        settings.skills[0].endpoint = Some("local".into());
        settings.skills.push(SkillSetting {
            id: "story".into(),
            name: "Story notes".into(),
            description: String::new(),
            endpoint: Some("local".into()),
            model: Some("anthropic/claude-haiku-5.5".into()),
            prompt: String::new(),
        });
        let mut ollama = |endpoint: &EndpointSetting| (endpoint.id == "local").then(|| vec![String::from("llama3.2")]);
        let found = unlisted(&settings, &mut ollama);
        assert_eq!(found.levels.len(), 5, "every level on a Claude model should warn");
        assert_eq!(found.skills, ["story"]);
        settings.levels[0].model = "llama3.2".into();
        settings.skills[1].model = Some("llama3.2".into());
        let found = unlisted(&settings, &mut ollama);
        assert_eq!(found.levels.len(), 4);
        assert!(!found.levels.contains(&settings.levels[0].id));
        assert!(found.skills.is_empty());
        assert_eq!(unlisted(&settings, &mut |_| None), Unlisted::default());
    }

    /// Catches an icon name the Iconoir pack does not have, which would
    /// stop the window when the panel is drawn.
    #[test]
    fn the_panel_icons_exist() {
        let ctx = egui::Context::default();
        toolbar::install_fonts(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for name in ["settings", "ev-plug", "puzzle", "chat-bubble", "lock", "search", "plus", "more-horiz", HAZARD] {
                toolbar::paint_icon(ui.painter(), Pos2::ZERO, name, 16.0, TEXT);
            }
        });
        output.textures_delta.clear();
    }
}
