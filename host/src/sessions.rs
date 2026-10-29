//! The session menu: the session's name at the top of the conversation, and
//! the panel it opens, as the approved session-menu, new-project, and trash
//! views show. The left column holds Recent, All Projects, and Trash, then
//! the projects; the right lists sessions, projects, or what is in Trash,
//! or the New Project form. What the person chooses is returned as a
//! `SessionChoice`; the host's flows carry it out.

use crate::toolbar;
use crate::typescale as ts;
use crate::widgets::{self, BORDER, MUTED, SELECTED, TEXT};
use eframe::egui::{self, Align2, Color32, Key, Pos2, Rect, Sense, UiBuilder, pos2, vec2};

/// The panel's size, and its parts.
const PANEL_W: f32 = 800.0;
const PANEL_H: f32 = 560.0;
const COLUMN_W: f32 = 236.0;
const PAD: f32 = 16.0;
const NAV_ROW: f32 = 36.0;
/// The space between the left column's rows.
const NAV_GAP: f32 = 2.0;
const LIST_ROW: f32 = 56.0;
const TILE: f32 = 32.0;
/// Every button and field in the panel shares the left rows' height, and
/// sets its text this far in from its edge, as the owner set on 2026-10-29.
const CONTROL_H: f32 = NAV_ROW;
const INSET: f32 = 12.0;
/// The dark button's fill.
const DARK: Color32 = Color32::from_rgb(24, 24, 27);
/// The faint dimming laid over everything behind the open panel, as the
/// owner set on 2026-10-29.
const OVERLAY: Color32 = Color32::from_black_alpha(20);
const HINT: Color32 = Color32::from_rgb(156, 160, 166);
/// The mark for a project or session kept on this computer.
const LOCAL: &str = "computer";

/// One session, as a list shows it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SessionRow {
    pub project: String,
    pub project_name: String,
    pub id: String,
    pub name: String,
    /// When it last changed, in seconds since the epoch.
    pub changed: u64,
    /// Its project's folder, as the list shows it.
    pub folder: String,
}

/// One project, as a list shows it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProjectRow {
    pub id: String,
    pub name: String,
    pub sessions: usize,
    pub used: u64,
    /// Kept at the top of the list, above the projects used most recently.
    pub pinned: bool,
    /// Its name, Trash, and archiving are held, and so is deleting its
    /// sessions.
    pub locked: bool,
    /// Off the Projects list and Recent, listed only in All Projects.
    pub archived: bool,
}

/// What a Trash row puts back.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Trashed {
    Session { project: String, id: String },
    Project(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TrashRow {
    pub item: Trashed,
    pub name: String,
    pub line: String,
}

/// Which page the panel shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) enum Page {
    #[default]
    Recent,
    AllProjects,
    Project(String),
    Trash,
    NewProject,
}

/// What is being renamed in a row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Renaming {
    Session { project: String, id: String },
    Project(String),
}

/// What the person chose in the menu.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SessionChoice {
    Open { project: String, session: String },
    NewSession { project: String },
    /// The New Project form's Change: choose a folder.
    ChooseLocation,
    CreateProject { name: String, location: String },
    RenameSession { project: String, session: String, name: String },
    RenameProject { project: String, name: String },
    DeleteSession { project: String, session: String },
    DeleteProject { project: String },
    PinProject { project: String, pinned: bool },
    LockProject { project: String, locked: bool },
    ArchiveProject { project: String, archived: bool },
    Restore(Trashed),
    EmptyTrash,
    /// The panel was opened: its lists should be read again.
    Opened,
}

/// The menu's state, kept by the host between frames.
#[derive(Default)]
pub(crate) struct SessionMenu {
    pub open: bool,
    pub page: Page,
    pub search: String,
    /// The current session's name, and its project's id.
    pub session: String,
    pub project: String,
    pub recent: Vec<SessionRow>,
    pub projects: Vec<ProjectRow>,
    pub trash: Vec<TrashRow>,
    pub renaming: Option<(Renaming, String)>,
    /// The New Project form: the name, where it goes, and why it was refused.
    pub form_name: String,
    pub form_location: String,
    /// The location follows the name until the person chooses a folder.
    pub form_root: String,
    pub form_error: Option<String>,
    /// While the host works on a choice, the panel takes no clicks.
    pub waiting: bool,
    /// The lists have been read since the panel opened.
    pub loaded: bool,
    /// The panel has just opened and is raised above everything this frame.
    pub raise: bool,
}

impl SessionMenu {
    /// Opens the New Project form with "Untitled Project" under `root`.
    pub(crate) fn start_new_project(&mut self, root: &str) {
        self.page = Page::NewProject;
        self.form_name = crate::workspace::UNTITLED.to_string();
        self.form_root = root.to_string();
        self.form_location = join(root, &self.form_name);
        self.form_error = None;
    }

    /// The folder the person chose with Change: the location becomes it,
    /// with the name, and keeps following the name.
    pub(crate) fn chose_root(&mut self, root: &str) {
        self.form_root = root.to_string();
        self.form_location = join(root, &self.form_name);
        self.form_error = None;
    }
}

fn join(root: &str, name: &str) -> String {
    let name = name.trim();
    let name = if name.is_empty() { crate::workspace::UNTITLED } else { name };
    std::path::Path::new(root).join(name).display().to_string()
}

/// "3s ago", "2h ago", "Yesterday", "3 days ago", "Last week", and on.
pub(crate) fn ago(now: u64, then: u64) -> String {
    let seconds = now.saturating_sub(then);
    match seconds {
        0..60 => format!("{seconds}s ago"),
        60..3_600 => format!("{}m ago", seconds / 60),
        3_600..86_400 => format!("{}h ago", seconds / 3_600),
        86_400..172_800 => "Yesterday".to_string(),
        172_800..604_800 => format!("{} days ago", seconds / 86_400),
        604_800..1_209_600 => "Last week".to_string(),
        _ => format!("{} weeks ago", seconds / 604_800),
    }
}

/// The session's name at the top centre, as a pill that opens the panel.
pub(crate) fn pill(ui: &mut egui::Ui, center: Pos2, menu: &mut SessionMenu, events: &mut Vec<SessionChoice>) {
    if menu.session.is_empty() {
        return;
    }
    let font = ts::title();
    let galley = ui.painter().layout_no_wrap(menu.session.clone(), font, TEXT);
    let chevron = pos2(center.x + galley.size().x / 2.0 + 12.0, center.y);
    let hit = Rect::from_min_max(
        pos2(center.x - galley.size().x / 2.0 - 12.0, center.y - 17.0),
        pos2(chevron.x + 14.0, center.y + 17.0),
    );
    let response = ui
        .interact(hit, egui::Id::new("session-pill"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.hovered() || menu.open {
        ui.painter().rect_filled(hit, 8.0, SELECTED);
    }
    ui.painter().galley(pos2(center.x - galley.size().x / 2.0, center.y - galley.size().y / 2.0), galley, TEXT);
    widgets::chevron(ui.painter(), chevron, MUTED, widgets::Scale(1.0));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, menu.session.as_str()));
    if response.clicked() {
        menu.open = !menu.open;
        if menu.open {
            menu.page = Page::Recent;
            menu.search.clear();
            menu.renaming = None;
            menu.raise = true;
            events.push(SessionChoice::Opened);
        }
    }
    if menu.open {
        panel(ui.ctx(), pos2(center.x, hit.bottom() + 10.0), hit, menu, events);
    }
}

/// The panel under the pill. A press outside it, or Escape, closes it.
fn panel(ctx: &egui::Context, top: Pos2, pill: Rect, menu: &mut SessionMenu, events: &mut Vec<SessionChoice>) {
    let screen = ctx.content_rect();
    let width = PANEL_W.min(screen.width() - 32.0);
    let height = PANEL_H.min(screen.height() - top.y - 16.0).max(240.0);
    let left = (top.x - width / 2.0).clamp(screen.left() + 16.0, (screen.right() - 16.0 - width).max(screen.left() + 16.0));
    let rect = Rect::from_min_size(pos2(left, top.y), vec2(width, height));
    // Under the panel and over everything else, the properties tab included,
    // as the owner set on 2026-10-29. It takes every press outside the panel,
    // which closes the panel below, so nothing underneath, such as a
    // brush, ever starts from it.
    let overlay = egui::Area::new(egui::Id::new("session-overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .fade_in(false)
        .show(ctx, |ui| {
            ui.painter().rect_filled(screen, 0.0, OVERLAY);
            ui.allocate_rect(screen, Sense::click_and_drag());
        })
        .response
        .layer_id;
    let area = egui::Area::new(egui::Id::new("session-panel"))
        .order(egui::Order::Foreground)
        // The panel shows at once, with no fade, as the owner set.
        .fade_in(false)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.set_min_size(rect.size());
            let painter = ui.painter().clone();
            widgets::outlined(&painter, rect, 12.0, Color32::WHITE, BORDER);
            // The left column is white like the rest, set apart by a line,
            // as the owner set on 2026-10-29.
            let column = Rect::from_min_size(rect.min, vec2(COLUMN_W, rect.height()));
            // Until every list is read, only the empty card shows, so the
            // click answers at once, as the owner set on 2026-10-29. The
            // lists are read ahead, so this is rare.
            if !menu.loaded {
                return;
            }
            painter.rect_filled(
                Rect::from_min_max(pos2(column.right(), rect.top() + 1.0), pos2(column.right() + 1.0, rect.bottom() - 1.0)),
                0.0,
                BORDER,
            );
            if menu.waiting {
                ui.disable();
            }
            nav(ui, column, menu, events);
            let right = Rect::from_min_max(pos2(column.right(), rect.top()), rect.max);
            match menu.page.clone() {
                Page::NewProject => new_project(ui, right, menu, events),
                page => list(ui, right, &page, menu, events),
            }
        });
    // The panel always sits directly above the overlay, whichever was last
    // pressed. The overlay is raised once, as it opens, above anything drawn
    // on top before, such as the properties tab; a row's "…" menu opened
    // later shows above both.
    ctx.set_sublayer(overlay, area.response.layer_id);
    if menu.raise {
        menu.raise = false;
        ctx.move_to_top(overlay);
    }
    let pressed_outside = ctx.input(|input| {
        input.pointer.any_pressed()
            && input
                .pointer
                .interact_pos()
                .is_some_and(|pos| !area.response.rect.contains(pos) && !pill.contains(pos))
    });
    let escape = ctx.input(|input| input.key_pressed(Key::Escape)) && menu.renaming.is_none();
    if pressed_outside || escape {
        menu.open = false;
        menu.renaming = None;
    }
}

/// One row of the left column: an icon, if any, and a label, on a plate
/// when chosen.
fn nav_row(ui: &mut egui::Ui, rect: Rect, icon: Option<&str>, label: &str, chosen: bool, id: &str) -> egui::Response {
    let response = ui.interact(rect, egui::Id::new(("session-nav", id)), Sense::click());
    if chosen || response.hovered() {
        ui.painter().rect_filled(rect, 8.0, if chosen { SELECTED } else { Color32::from_rgb(241, 242, 244) });
    }
    let text_left = match icon {
        Some(icon) => {
            toolbar::paint_icon(ui.painter(), pos2(rect.left() + 20.0, rect.center().y), icon, 18.0, TEXT);
            rect.left() + 42.0
        }
        None => rect.left() + 12.0,
    };
    let clip = ui.painter().with_clip_rect(Rect::from_min_max(rect.min, pos2(rect.right() - 60.0, rect.bottom())));
    clip.text(pos2(text_left, rect.center().y), Align2::LEFT_CENTER, label, ts::body(), TEXT);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    response
}

fn nav(ui: &mut egui::Ui, column: Rect, menu: &mut SessionMenu, events: &mut Vec<SessionChoice>) {
    let inner = column.shrink2(vec2(12.0, PAD));
    let mut y = inner.top();
    let mut next = |height: f32| {
        let rect = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), height));
        y += height + NAV_GAP;
        rect
    };
    let rows: [(&str, &str, Page); 4] = [
        ("clock", "Recent", Page::Recent),
        ("plus", "New Project", Page::NewProject),
        ("folder", "All Projects", Page::AllProjects),
        ("trash", "Trash", Page::Trash),
    ];
    for (icon, label, page) in rows {
        let rect = next(NAV_ROW);
        if nav_row(ui, rect, Some(icon), label, menu.page == page, label).clicked() {
            if page == Page::NewProject {
                let root = menu.form_root.clone();
                menu.start_new_project(&root);
            } else {
                menu.page = page;
            }
        }
    }
    let divider = next(14.0);
    ui.painter().rect_filled(
        Rect::from_min_max(pos2(divider.left() + 8.0, divider.center().y), pos2(divider.right() - 8.0, divider.center().y + 1.0)),
        0.0,
        BORDER,
    );
    let header = next(22.0);
    ui.painter().text(pos2(header.left() + 12.0, header.center().y), Align2::LEFT_CENTER, "Projects", ts::small(), MUTED);
    for project in menu.projects.clone().into_iter().filter(|project| !project.archived) {
        let rect = next(NAV_ROW);
        if rect.bottom() > inner.bottom() {
            break;
        }
        let target = Renaming::Project(project.id.clone());
        if let Some((what, draft)) = &mut menu.renaming
            && *what == target
        {
            let field = rect.shrink2(vec2(8.0, 4.0));
            let mut child = ui.new_child(UiBuilder::new().max_rect(field));
            let edit = child.add(
                egui::TextEdit::singleline(draft)
                    .id(egui::Id::new(("project-rename", &project.id)))
                    .font(ts::body())
                    .desired_width(field.width()),
            );
            edit.request_focus();
            if ui.input(|input| input.key_pressed(Key::Enter)) {
                let name = draft.clone();
                menu.renaming = None;
                events.push(SessionChoice::RenameProject { project: project.id.clone(), name });
            }
            continue;
        }
        let chosen = menu.page == Page::Project(project.id.clone());
        let row = nav_row(ui, rect, None, &project.name, chosen, &project.id);
        let more = Rect::from_center_size(pos2(rect.right() - 18.0, rect.center().y), vec2(26.0, 26.0));
        let pin = Rect::from_center_size(pos2(rect.right() - 46.0, rect.center().y), vec2(26.0, 26.0));
        let more_response = ui.interact(more, egui::Id::new(("project-more", &project.id)), Sense::click());
        let pin_response = ui.interact(pin, egui::Id::new(("project-pin", &project.id)), Sense::click());
        let hovered = row.hovered() || more_response.hovered() || pin_response.hovered()
            || egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&more_response));
        if hovered || project.pinned {
            if pin_response.hovered() {
                ui.painter().rect_filled(pin, 6.0, SELECTED);
            }
            toolbar::paint_icon(ui.painter(), pin.center(), "pin", 16.0, if project.pinned { TEXT } else { MUTED });
        }
        if hovered {
            if more_response.hovered() {
                ui.painter().rect_filled(more, 6.0, SELECTED);
            }
            toolbar::paint_icon(ui.painter(), more.center(), "more-horiz", 16.0, MUTED);
        }
        if project.locked {
            let mark = pos2(rect.right() - 74.0, rect.center().y);
            toolbar::paint_icon(ui.painter(), mark, "lock", 14.0, MUTED);
        }
        pin_response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, if project.pinned { "Unpin" } else { "Pin" }));
        if pin_response.clicked() {
            events.push(SessionChoice::PinProject { project: project.id.clone(), pinned: !project.pinned });
        } else if row.clicked() && !more_response.hovered() {
            menu.page = Page::Project(project.id.clone());
        }
        project_menu(&more_response, &project, menu, events);
    }
}

/// A project's "…" in the left column, as the owner's project-row-menu
/// picture shows: Rename, Pin, and Lock, then Share, then Archive. Share
/// waits for a later version and is drawn lighter; a locked project's
/// Rename and Archive are too.
fn project_menu(more: &egui::Response, project: &ProjectRow, menu: &mut SessionMenu, events: &mut Vec<SessionChoice>) {
    egui::Popup::menu(more).show(|ui| {
        let rows = [
            widgets::MenuItem { label: "Rename", icon: Some("edit-pencil"), enabled: !project.locked, submenu: false, divider: false },
            widgets::MenuItem { label: if project.pinned { "Unpin" } else { "Pin" }, icon: Some("pin"), enabled: true, submenu: false, divider: false },
            widgets::MenuItem { label: if project.locked { "Unlock" } else { "Lock" }, icon: Some("lock"), enabled: true, submenu: false, divider: false },
            widgets::MenuItem { label: "Share", icon: Some("share-ios"), enabled: false, submenu: false, divider: true },
            widgets::MenuItem { label: "Archive", icon: Some("archive"), enabled: !project.locked, submenu: false, divider: true },
        ];
        match widgets::menu_rows(ui, &rows) {
            Some(0) if !project.locked => menu.renaming = Some((Renaming::Project(project.id.clone()), project.name.clone())),
            Some(1) => events.push(SessionChoice::PinProject { project: project.id.clone(), pinned: !project.pinned }),
            Some(2) => events.push(SessionChoice::LockProject { project: project.id.clone(), locked: !project.locked }),
            Some(4) if !project.locked => events.push(SessionChoice::ArchiveProject { project: project.id.clone(), archived: true }),
            _ => {}
        }
    });
}

/// The search field and the button at the top of a list page.
fn list_top(ui: &mut egui::Ui, right: Rect, page: &Page, menu: &mut SessionMenu, events: &mut Vec<SessionChoice>) -> Rect {
    // The same height, and the same icon and text insets, as the left
    // column's rows, as the owner set on 2026-10-29.
    let row = Rect::from_min_size(right.min + vec2(PAD, PAD), vec2(right.width() - 2.0 * PAD, NAV_ROW));
    let trash = *page == Page::Trash;
    let label = if trash { "Empty Trash" } else { "New Session" };
    let font = ts::body();
    let label_w = ui.painter().layout_no_wrap(label.to_string(), font.clone(), TEXT).size().x;
    let button_w = label_w + if trash { 2.0 * INSET } else { 42.0 + INSET };
    let button = Rect::from_min_size(pos2(row.right() - button_w, row.top()), vec2(button_w, row.height()));
    let field = Rect::from_min_max(row.min, pos2(button.left() - 16.0, row.bottom()));
    widgets::outlined(ui.painter(), field, 8.0, Color32::WHITE, BORDER);
    toolbar::paint_icon(ui.painter(), pos2(field.left() + 20.0, field.center().y), "search", 18.0, MUTED);
    let hint = match page {
        Page::Trash => "Search trash…",
        Page::AllProjects => "Search projects…",
        _ => "Search sessions…",
    };
    let text = Rect::from_min_max(pos2(field.left() + 42.0, field.top() + 2.0), pos2(field.right() - 10.0, field.bottom() - 2.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(text).layout(egui::Layout::left_to_right(egui::Align::Center)));
    child.add(
        egui::TextEdit::singleline(&mut menu.search)
            .id(egui::Id::new("session-search"))
            .hint_text(egui::RichText::new(hint).color(HINT))
            .frame(egui::Frame::NONE)
            .font(font.clone())
            .desired_width(text.width()),
    );
    let response = ui
        .interact(button, egui::Id::new(("session-button", label)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if trash {
        widgets::outlined(ui.painter(), button, 8.0, Color32::WHITE, BORDER);
        ui.painter().text(button.center(), Align2::CENTER_CENTER, label, font, TEXT);
    } else {
        ui.painter().rect_filled(button, 8.0, DARK);
        toolbar::paint_icon(ui.painter(), pos2(button.left() + 20.0, button.center().y), "plus", 18.0, Color32::WHITE);
        ui.painter().text(pos2(button.left() + 42.0, button.center().y), Align2::LEFT_CENTER, label, font, Color32::WHITE);
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if response.clicked() {
        if trash {
            if !menu.trash.is_empty() {
                events.push(SessionChoice::EmptyTrash);
            }
        } else {
            let project = match page {
                Page::Project(id) => id.clone(),
                _ => menu.project.clone(),
            };
            events.push(SessionChoice::NewSession { project });
        }
    }
    Rect::from_min_max(pos2(right.left() + PAD, row.bottom() + 14.0), pos2(right.right() - PAD, right.bottom() - PAD))
}

/// One row of a list, as the session list draws it. Returns whether its body
/// was clicked, and whether its trailing control was.
struct Row<'a> {
    id: String,
    name: &'a str,
    line: String,
    selected: bool,
    /// "Restore" in place of the "…".
    restore: bool,
}

fn list_row(ui: &mut egui::Ui, rect: Rect, row: &Row, renaming: Option<&mut String>) -> (bool, Option<egui::Response>, bool) {
    let response = ui.interact(rect, egui::Id::new(("session-row", &row.id)), Sense::click());
    if row.selected || response.hovered() {
        ui.painter().rect_filled(rect, 10.0, if row.selected { SELECTED } else { Color32::from_rgb(246, 247, 249) });
    }
    let tile = Rect::from_center_size(pos2(rect.left() + INSET + TILE / 2.0, rect.center().y), vec2(TILE, TILE));
    widgets::outlined(ui.painter(), tile, 8.0, Color32::WHITE, BORDER);
    toolbar::paint_icon(ui.painter(), tile.center(), LOCAL, 18.0, TEXT);
    let text_left = tile.right() + INSET;
    let mut committed = false;
    match renaming {
        Some(draft) => {
            let field = Rect::from_min_size(pos2(text_left, rect.center().y - 20.0), vec2(rect.right() - text_left - 60.0, 22.0));
            let mut child = ui.new_child(UiBuilder::new().max_rect(field));
            let edit = child.add(
                egui::TextEdit::singleline(draft)
                    .id(egui::Id::new(("session-rename", &row.id)))
                    .font(ts::title())
                    .desired_width(field.width()),
            );
            edit.request_focus();
            committed = ui.input(|input| input.key_pressed(Key::Enter));
        }
        None => {
            ui.painter().text(pos2(text_left, rect.center().y - 9.0), Align2::LEFT_CENTER, row.name, ts::title(), TEXT);
        }
    }
    ui.painter().text(pos2(text_left, rect.center().y + 10.0), Align2::LEFT_CENTER, &row.line, ts::small(), MUTED);
    let trailing = if row.restore {
        let font = ts::body();
        let width = ui.painter().layout_no_wrap("Restore".into(), font.clone(), TEXT).size().x + 2.0 * INSET;
        let button = Rect::from_center_size(pos2(rect.right() - INSET - width / 2.0, rect.center().y), vec2(width, CONTROL_H));
        widgets::outlined(ui.painter(), button, 8.0, Color32::WHITE, BORDER);
        ui.painter().text(button.center(), Align2::CENTER_CENTER, "Restore", font, TEXT);
        ui.interact(button, egui::Id::new(("session-restore", &row.id)), Sense::click())
    } else {
        let more = Rect::from_center_size(pos2(rect.right() - INSET - 14.0, rect.center().y), vec2(28.0, 28.0));
        let response = ui.interact(more, egui::Id::new(("session-more", &row.id)), Sense::click());
        if response.hovered() {
            ui.painter().rect_filled(more, 6.0, SELECTED);
        }
        toolbar::paint_icon(ui.painter(), more.center(), "more-horiz", 18.0, MUTED);
        response
    };
    let body = response.clicked() && !trailing.hovered();
    (body, Some(trailing), committed)
}

fn list(ui: &mut egui::Ui, right: Rect, page: &Page, menu: &mut SessionMenu, events: &mut Vec<SessionChoice>) {
    let below = list_top(ui, right, page, menu, events);
    let now = crate::workspace::now_seconds();
    let search = menu.search.trim().to_lowercase();
    let matches = |name: &str| search.is_empty() || name.to_lowercase().contains(&search);
    let archived: Vec<String> = menu.projects.iter().filter(|row| row.archived).map(|row| row.id.clone()).collect();
    let locked: Vec<String> = menu.projects.iter().filter(|row| row.locked).map(|row| row.id.clone()).collect();
    let title = match page {
        Page::Recent => "Recent".to_string(),
        Page::AllProjects => "All Projects".to_string(),
        Page::Trash => "Trash".to_string(),
        Page::Project(id) => menu.projects.iter().find(|row| &row.id == id).map(|row| row.name.clone()).unwrap_or_default(),
        Page::NewProject => String::new(),
    };
    ui.painter().text(pos2(below.left() + 2.0, below.top() + 8.0), Align2::LEFT_CENTER, &title, ts::small(), MUTED);
    let rows_rect = Rect::from_min_max(pos2(below.left(), below.top() + 24.0), below.max);
    let mut child = ui.new_child(UiBuilder::new().max_rect(rows_rect));
    egui::ScrollArea::vertical()
        .id_salt(("session-list", title.as_str()))
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(&mut child, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            match page {
                Page::Recent | Page::Project(_) => {
                    let rows: Vec<SessionRow> = menu
                        .recent
                        .iter()
                        .filter(|row| match page {
                            Page::Project(id) => &row.project == id,
                            _ => !archived.contains(&row.project),
                        })
                        .filter(|row| matches(&row.name))
                        .cloned()
                        .collect();
                    for session in rows {
                        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), LIST_ROW), Sense::hover());
                        let current = session.name == menu.session && session.project == menu.project;
                        let line = format!(
                            "Edited {} · {} · {}",
                            ago(now, session.changed).to_lowercase(),
                            session.project_name,
                            shorten_home(&session.folder),
                        );
                        let target = Renaming::Session { project: session.project.clone(), id: session.id.clone() };
                        let row = Row { id: format!("s-{}", session.id), name: &session.name, line, selected: current, restore: false };
                        let renaming = match &mut menu.renaming {
                            Some((what, draft)) if *what == target => Some(draft),
                            _ => None,
                        };
                        let editing = renaming.is_some();
                        let (body, more, committed) = list_row(ui, rect, &row, renaming);
                        if committed && let Some((_, draft)) = menu.renaming.take() {
                            events.push(SessionChoice::RenameSession { project: session.project.clone(), session: session.id.clone(), name: draft });
                        }
                        if body && !editing {
                            events.push(SessionChoice::Open { project: session.project.clone(), session: session.id.clone() });
                        }
                        if let Some(more) = more {
                            let held = locked.contains(&session.project);
                            row_menu(&more, &session.name, target, held, None, menu, events);
                        }
                    }
                }
                Page::AllProjects => {
                    let rows: Vec<ProjectRow> = menu.projects.iter().filter(|row| matches(&row.name)).cloned().collect();
                    for project in rows {
                        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), LIST_ROW), Sense::hover());
                        let sessions = if project.sessions == 1 { "1 session".to_string() } else { format!("{} sessions", project.sessions) };
                        let target = Renaming::Project(project.id.clone());
                        let row = Row {
                            id: format!("p-{}", project.id),
                            name: &project.name,
                            line: if project.archived {
                                format!("Archived · {sessions} · {}", ago(now, project.used))
                            } else {
                                format!("{sessions} · {}", ago(now, project.used))
                            },
                            selected: project.id == menu.project,
                            restore: false,
                        };
                        let renaming = match &mut menu.renaming {
                            Some((what, draft)) if *what == target => Some(draft),
                            _ => None,
                        };
                        let editing = renaming.is_some();
                        let (body, more, committed) = list_row(ui, rect, &row, renaming);
                        if committed && let Some((_, draft)) = menu.renaming.take() {
                            events.push(SessionChoice::RenameProject { project: project.id.clone(), name: draft });
                        }
                        if body && !editing {
                            menu.page = Page::Project(project.id.clone());
                        }
                        if let Some(more) = more {
                            row_menu(&more, &project.name, target, project.locked, Some(project.archived), menu, events);
                        }
                    }
                }
                Page::Trash => {
                    let rows: Vec<TrashRow> = menu.trash.iter().filter(|row| matches(&row.name)).cloned().collect();
                    if rows.is_empty() {
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new("Nothing is in Trash.").font(ts::body()).color(MUTED));
                    }
                    for trashed in rows {
                        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), LIST_ROW), Sense::hover());
                        let id = match &trashed.item {
                            Trashed::Session { id, .. } => format!("ts-{id}"),
                            Trashed::Project(id) => format!("tp-{id}"),
                        };
                        let row = Row { id, name: &trashed.name, line: trashed.line.clone(), selected: false, restore: true };
                        let (_, restore, _) = list_row(ui, rect, &row, None);
                        if restore.is_some_and(|restore| restore.clicked()) {
                            events.push(SessionChoice::Restore(trashed.item.clone()));
                        }
                    }
                }
                Page::NewProject => {}
            }
        });
}

/// A row's "…": Rename and Delete, and, for an archived project,
/// Unarchive first. A locked project holds its name and its Delete, and
/// those of its sessions.
fn row_menu(
    more: &egui::Response,
    name: &str,
    target: Renaming,
    locked: bool,
    archived: Option<bool>,
    menu: &mut SessionMenu,
    events: &mut Vec<SessionChoice>,
) {
    egui::Popup::menu(more).show(|ui| {
        let unarchive = archived == Some(true);
        let mut rows = Vec::new();
        if unarchive {
            rows.push(widgets::MenuItem { label: "Unarchive", icon: None, enabled: true, submenu: false, divider: false });
        }
        rows.push(widgets::MenuItem { label: "Rename", icon: None, enabled: !locked, submenu: false, divider: false });
        rows.push(widgets::MenuItem { label: "Delete", icon: None, enabled: !locked, submenu: false, divider: false });
        let picked = widgets::menu_rows(ui, &rows).map(|index| if unarchive { index } else { index + 1 });
        match picked {
            Some(0) => {
                if let Renaming::Project(project) = &target {
                    events.push(SessionChoice::ArchiveProject { project: project.clone(), archived: false });
                }
            }
            Some(1) if !locked => menu.renaming = Some((target.clone(), name.to_string())),
            Some(2) if !locked => events.push(match &target {
                Renaming::Session { project, id } => SessionChoice::DeleteSession { project: project.clone(), session: id.clone() },
                Renaming::Project(project) => SessionChoice::DeleteProject { project: project.clone() },
            }),
            _ => {}
        }
    });
}

/// A plain bordered button, `CONTROL_H` tall, its label `INSET` in from
/// each side. Returns whether it was clicked.
fn plain_button(ui: &mut egui::Ui, rect: Rect, label: &str, id: &str) -> bool {
    widgets::outlined(ui.painter(), rect, 8.0, Color32::WHITE, BORDER);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, ts::body(), TEXT);
    ui.interact(rect, egui::Id::new(id), Sense::click()).clicked()
}

fn label_width(ui: &egui::Ui, label: &str) -> f32 {
    ui.painter().layout_no_wrap(label.to_string(), ts::body(), TEXT).size().x + 2.0 * INSET
}

/// The New Project form, as the new-project view shows it, with no title,
/// as the owner set on 2026-10-29: the left column already says where it is.
/// Its fields and buttons are the panel's one height.
fn new_project(ui: &mut egui::Ui, right: Rect, menu: &mut SessionMenu, events: &mut Vec<SessionChoice>) {
    let inner = right.shrink(PAD);
    let mut y = inner.top();
    let label = |ui: &egui::Ui, y: f32, text: &str| {
        ui.painter().text(pos2(inner.left() + 2.0, y + 8.0), Align2::LEFT_CENTER, text, ts::small(), MUTED);
    };
    label(ui, y, "Name");
    y += 22.0;
    let name_box = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), CONTROL_H));
    widgets::outlined(ui.painter(), name_box, 8.0, Color32::WHITE, BORDER);
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(name_box.shrink2(vec2(INSET, 2.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let name = child.add(
        egui::TextEdit::singleline(&mut menu.form_name)
            .id(egui::Id::new("new-project-name"))
            .frame(egui::Frame::NONE)
            .font(ts::body())
            .desired_width(name_box.width() - 2.0 * INSET),
    );
    if name.changed() {
        menu.form_location = join(&menu.form_root, &menu.form_name);
        menu.form_error = None;
    }
    y += CONTROL_H + 16.0;
    label(ui, y, "Location");
    y += 22.0;
    let change_w = label_width(ui, "Change");
    let location = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width() - change_w - 8.0, CONTROL_H));
    widgets::outlined(ui.painter(), location, 8.0, Color32::from_rgb(246, 247, 249), BORDER);
    let shown = shorten_home(&menu.form_location);
    let clip = ui.painter().with_clip_rect(location.shrink(2.0));
    clip.text(pos2(location.left() + INSET, location.center().y), Align2::LEFT_CENTER, shown, ts::code(), MUTED);
    let change = Rect::from_min_size(pos2(location.right() + 8.0, y), vec2(change_w, CONTROL_H));
    if plain_button(ui, change, "Change", "new-project-change") {
        events.push(SessionChoice::ChooseLocation);
    }
    y += CONTROL_H + 12.0;
    ui.painter().text(
        pos2(inner.left() + 2.0, y + 8.0),
        Align2::LEFT_CENTER,
        "Studio creates this folder. Everything the project makes is kept in it.",
        ts::small(),
        MUTED,
    );
    if let Some(error) = &menu.form_error {
        y += 22.0;
        ui.painter().text(pos2(inner.left() + 2.0, y + 8.0), Align2::LEFT_CENTER, error, ts::small(), ts::FAILED);
    }
    let create_w = label_width(ui, "Create Project");
    let create = Rect::from_min_size(pos2(inner.right() - create_w, inner.bottom() - CONTROL_H), vec2(create_w, CONTROL_H));
    ui.painter().rect_filled(create, 8.0, DARK);
    ui.painter().text(create.center(), Align2::CENTER_CENTER, "Create Project", ts::body(), Color32::WHITE);
    let cancel_w = label_width(ui, "Cancel");
    let cancel = Rect::from_min_size(pos2(create.left() - 8.0 - cancel_w, create.top()), vec2(cancel_w, CONTROL_H));
    if plain_button(ui, cancel, "Cancel", "new-project-cancel") {
        menu.page = Page::Recent;
        menu.form_error = None;
    }
    let enter = name.has_focus() && ui.input(|input| input.key_pressed(Key::Enter));
    if ui.interact(create, egui::Id::new("new-project-create"), Sense::click()).clicked() || enter {
        events.push(SessionChoice::CreateProject { name: menu.form_name.clone(), location: menu.form_location.clone() });
    }
}

/// `/home/someone/Inpainter/Lighthouse` as `~/Inpainter/Lighthouse`.
fn shorten_home(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Catches a time line that says the wrong thing at its edges.
    #[test]
    fn ages_read_as_the_views_show() {
        assert_eq!(ago(100, 97), "3s ago");
        assert_eq!(ago(10_000, 10_000 - 2 * 3_600), "2h ago");
        assert_eq!(ago(200_000, 200_000 - 90_000), "Yesterday");
        assert_eq!(ago(1_000_000, 1_000_000 - 3 * 86_400), "3 days ago");
        assert_eq!(ago(2_000_000, 2_000_000 - 8 * 86_400), "Last week");
    }
}
