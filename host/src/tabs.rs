//! Tab bar: the menu button, the tabs a sub-application reports, and the plus.

use crate::toolbar::{self, BAR, ICON, icon_text};
use eframe::egui::{self, Color32, RichText};

const INACTIVE: Color32 = Color32::from_rgb(243, 244, 247);
const TAB_EDGE: Color32 = Color32::from_rgb(229, 231, 235);
const TAB_HEIGHT: f32 = 34.0;

/// Draws the tab bar. The menu button, the plus, and each tab's close mark
/// are drawn, and a click on any of them does nothing.
pub(crate) fn show(ui: &mut egui::Ui, declaration: &protocol::Declaration) {
    let mut active_tab = None;
    let bar = egui::Frame::new()
        .fill(BAR)
        .inner_margin(egui::Margin {
            left: 8,
            right: 8,
            top: 6,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.set_height(TAB_HEIGHT);
                menu(ui);
                active_tab = tab_row(ui, declaration);
                let _plus = toolbar::glyph_button(
                    ui,
                    "plus",
                    16.0,
                    egui::vec2(32.0, TAB_HEIGHT),
                    None,
                    true,
                    false,
                );
            });
        });
    // The hairline stops under the active tab, which joins the white toolbar.
    let rect = bar.response.rect;
    match active_tab {
        Some(tab) => {
            toolbar::hairline(ui, rect.left(), tab.left(), rect.bottom());
            toolbar::hairline(ui, tab.right(), rect.right(), rect.bottom());
        }
        None => toolbar::hairline(ui, rect.left(), rect.right(), rect.bottom()),
    }
}

fn menu(ui: &mut egui::Ui) {
    let _response = toolbar::glyph_button(
        ui,
        "menu",
        18.0,
        egui::vec2(32.0, TAB_HEIGHT),
        Some("Menu"),
        true,
        false,
    );
}

/// Returns the visible part of the active tab.
fn tab_row(ui: &mut egui::Ui, declaration: &protocol::Declaration) -> Option<egui::Rect> {
    let tabs: Vec<_> = declaration
        .tabs
        .iter()
        .enumerate()
        .map(|(index, tab)| (tab.id.as_str(), tab.label.as_str(), index == 0))
        .collect();
    let width = (ui.available_width() - 36.0).max(48.0);
    let mut active_rect = None;
    let row = egui::Layout::left_to_right(egui::Align::Center);
    ui.allocate_ui_with_layout(egui::vec2(width, TAB_HEIGHT), row, |ui| {
        ui.set_max_width(width);
        egui::ScrollArea::horizontal()
            .id_salt("tabs")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                // A full-height row: `horizontal` would start short and grow
                // downward, which puts the tabs below the menu button.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_height(TAB_HEIGHT);
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let visible = ui.clip_rect();
                    for (id, name, active) in tabs {
                        let tab = Tab {
                            id,
                            name,
                            active,
                            close_waits: false,
                            unsaved: false,
                        };
                        let rect = tab_chip(ui, tab);
                        if active {
                            active_rect = Some(rect.intersect(visible));
                        }
                    }
                });
            });
    });
    active_rect.filter(|rect| rect.width() > 0.0)
}

struct Tab<'a> {
    id: &'a str,
    name: &'a str,
    active: bool,
    close_waits: bool,
    /// Unsaved changes: a dot stands in for the close mark until the tab is
    /// hovered.
    unsaved: bool,
}

/// One tab: its name, then its close control. Double-clicking does not edit
/// the title. A click on the tab or its close mark, including a middle click,
/// does nothing.
fn tab_chip(ui: &mut egui::Ui, tab: Tab<'_>) -> egui::Rect {
    const PAD_LEFT: f32 = 14.0;
    const PAD_RIGHT: f32 = 8.0;
    const CLOSE: f32 = 20.0;
    const MIN_WIDTH: f32 = 120.0;
    const MAX_LABEL: f32 = 180.0;

    let galley = egui::WidgetText::from(RichText::new(tab.name).size(13.5).color(ICON))
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            MAX_LABEL,
            egui::FontSelection::Default,
        );
    let label_width = galley.size().x;
    let width = (PAD_LEFT + label_width + 10.0 + CLOSE + PAD_RIGHT).max(MIN_WIDTH);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, TAB_HEIGHT), egui::Sense::click());
    paint_tab(ui.painter(), rect, tab.active);

    let label_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + PAD_LEFT, rect.top()),
        egui::vec2(label_width, TAB_HEIGHT),
    );
    let top = rect.center().y - galley.size().y / 2.0;
    ui.painter()
        .galley(egui::pos2(label_rect.left(), top), galley, ICON);

    let close_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - PAD_RIGHT - CLOSE / 2.0, rect.center().y),
        egui::vec2(CLOSE, CLOSE),
    );
    let close = close_control(ui, close_rect, tab.id, !tab.close_waits);
    let dot = tab.unsaved && !response.hovered() && !close.hovered();
    paint_close_mark(ui, close_rect, !tab.close_waits, dot);
    rect
}

/// Rounded top corners and an outline on the top and sides. The bottom edge
/// is the bar's hairline, or, for the active tab, the white toolbar.
fn paint_tab(painter: &egui::Painter, rect: egui::Rect, active: bool) {
    let fill = if active { Color32::WHITE } else { INACTIVE };
    let radius = egui::CornerRadius {
        nw: 8,
        ne: 8,
        sw: 0,
        se: 0,
    };
    painter.rect_filled(rect, radius, fill);

    let edge = rect.shrink2(egui::vec2(0.5, 0.0));
    let r = 8.0;
    let mut outline = vec![egui::pos2(edge.left(), edge.bottom())];
    let corner = |center: egui::Pos2, from: f32, outline: &mut Vec<egui::Pos2>| {
        for step in 0..=6 {
            let angle = from + std::f32::consts::FRAC_PI_2 * step as f32 / 6.0;
            outline.push(center + r * egui::vec2(angle.cos(), angle.sin()));
        }
    };
    let top = edge.top() + 0.5;
    corner(
        egui::pos2(edge.left() + r, top + r),
        std::f32::consts::PI,
        &mut outline,
    );
    corner(
        egui::pos2(edge.right() - r, top + r),
        -std::f32::consts::FRAC_PI_2,
        &mut outline,
    );
    outline.push(egui::pos2(edge.right(), edge.bottom()));
    painter.add(egui::Shape::line(outline, egui::Stroke::new(1.0, TAB_EDGE)));
}

fn close_control(ui: &mut egui::Ui, rect: egui::Rect, id: &str, enabled: bool) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    ui.interact(rect, egui::Id::new(("tab-close", id)), sense)
}

/// The close mark, or a dot while the sketch has unsaved changes and the
/// pointer is away from its tab.
fn paint_close_mark(ui: &egui::Ui, rect: egui::Rect, enabled: bool, dot: bool) {
    let color = if enabled { ICON } else { toolbar::ICON_QUIET };
    if dot {
        ui.painter().circle_filled(rect.center(), 4.0, color);
        return;
    }
    let glyph = icon_text("xmark", 11.0, color);
    let galley = egui::WidgetText::from(glyph).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::FontSelection::Default,
    );
    let top_left = rect.center() - galley.size() / 2.0;
    ui.painter().galley(top_left, galley, color);
}

pub(crate) mod live {
    //! Tab bar, sketch menu, and the keep-this-sketch confirmation.
    //! Widgets send session commands and keep no sketch state.

    use sketch::document::SketchId;
    use sketch::registry::{self, CommandId, SketchMenuRow};
    use sketch::session::{CloseEffect, CommandEffect, FileError, RecoveryOffer, SaveResult, Session};
    use crate::toolbar::{self, BAR, ICON, icon_text};
    use crate::widgets;
    use eframe::egui::{self, Color32, RichText};
    use std::path::PathBuf;

    const INACTIVE: Color32 = Color32::from_rgb(243, 244, 247);
    const TAB_EDGE: Color32 = Color32::from_rgb(229, 231, 235);
    const TAB_HEIGHT: f32 = 34.0;

    /// The one close confirmation, a replace question, and recovery offers.
    /// Marks, cameras, and history stay in the session.
    pub struct Chrome {
        confirm: Option<SketchId>,
        /// Window close is walking dirty sketches in tab order.
        quitting: bool,
        /// Dirty sketches still to ask about, after the one in `confirm`.
        quit_rest: Vec<SketchId>,
        /// Recovery files still to offer, one at a time.
        recoveries: Vec<RecoveryOffer>,
        replace: Option<ReplaceAsk>,
        /// A failed save or open being shown.
        error: Option<FileError>,
    }

    #[derive(Clone)]
    struct ReplaceAsk {
        id: SketchId,
        path: PathBuf,
        copy: bool,
        close_after: bool,
    }

    impl Chrome {
        pub fn new() -> Self {
            Self {
                confirm: None,
                quitting: false,
                quit_rest: Vec::new(),
                recoveries: Vec::new(),
                replace: None,
                error: None,
            }
        }

        pub fn set_recoveries(&mut self, offers: Vec<RecoveryOffer>) {
            self.recoveries = offers;
        }

        pub fn close(&mut self, session: &mut Session, id: SketchId) -> CloseEffect {
            let effect = session.request_close(id);
            match effect {
                CloseEffect::NeedsConfirm => self.confirm = Some(id),
                CloseEffect::Unchanged => {}
                CloseEffect::Closed { .. } => {
                    if self.confirm == Some(id) {
                        self.confirm = None;
                    }
                }
            }
            effect
        }

        pub fn ask_replace(&mut self, id: SketchId, path: PathBuf, copy: bool, close_after: bool) {
            self.replace = Some(ReplaceAsk {
                id,
                path,
                copy,
                close_after,
            });
        }

        /// Save As was cancelled, or the write failed. The sketch stays open.
        /// A quit in progress stops.
        pub fn cancel_save_as(&mut self) {
            self.stop_confirm();
            self.replace = None;
        }

        /// The sketch was saved. Close it, and continue a quit when one is walking
        /// the remaining dirty sketches. Returns whether a new blank should be centered.
        pub fn complete_save(
            &mut self,
            ctx: &egui::Context,
            session: &mut Session,
            id: SketchId,
        ) -> bool {
            let effect = session.request_close(id);
            let replacement = matches!(effect, CloseEffect::Closed { replacement: true });
            if !matches!(effect, CloseEffect::Closed { .. }) {
                return false;
            }
            if self.quitting {
                self.advance_quit(ctx, session);
            } else {
                self.confirm = None;
            }
            replacement
        }

        fn start_quit(&mut self, session: &mut Session, mut dirty: Vec<SketchId>) {
            let Some(first) = dirty.first().copied() else {
                return;
            };
            dirty.remove(0);
            self.quitting = true;
            self.quit_rest = dirty;
            self.confirm = Some(first);
            session.activate(first);
        }

        fn advance_quit(&mut self, ctx: &egui::Context, session: &mut Session) {
            while let Some(next) = self.quit_rest.first().copied() {
                self.quit_rest.remove(0);
                if session.name_of(next).is_some() {
                    self.confirm = Some(next);
                    session.activate(next);
                    return;
                }
            }
            self.quitting = false;
            self.quit_rest.clear();
            self.confirm = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        fn stop_confirm(&mut self) {
            self.confirm = None;
            self.quitting = false;
            self.quit_rest.clear();
        }
    }

    /// Draws the tab bar. Returns whether the active sketch is a new blank that
    /// should be centered in the current view.
    pub fn show(ui: &mut egui::Ui, session: &mut Session, chrome: &mut Chrome) -> bool {
        let mut center = false;
        note_window_close(ui.ctx(), session, chrome);
        let mut active_tab = None;
        let bar = egui::Frame::new()
            .fill(BAR)
            .inner_margin(egui::Margin {
                left: 8,
                right: 8,
                top: 6,
                bottom: 0,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.set_height(TAB_HEIGHT);
                    menu(ui, session, chrome, &mut center);
                    active_tab = tab_row(ui, session, chrome, &mut center);
                    let new_sketch = registry::command(CommandId::NewSketch);
                    let plus = toolbar::glyph_button(
                        ui,
                        "plus",
                        16.0,
                        egui::vec2(32.0, TAB_HEIGHT),
                        Some(&registry::hover(new_sketch.label, new_sketch.shortcuts)),
                        true,
                        false,
                    );
                    if plus.clicked() {
                        session.new_sketch();
                        center = true;
                    }
                });
            });
        // The hairline stops under the active tab, which joins the white toolbar.
        let rect = bar.response.rect;
        match active_tab {
            Some(tab) => {
                toolbar::hairline(ui, rect.left(), tab.left(), rect.bottom());
                toolbar::hairline(ui, tab.right(), rect.right(), rect.bottom());
            }
            None => toolbar::hairline(ui, rect.left(), rect.right(), rect.bottom()),
        }

        let pending = chrome.confirm;
        if chrome.replace.is_none()
            && let Some(id) = pending
            && confirm(ui.ctx(), session, chrome, id)
        {
            center = true;
        }
        if replace_dialog(ui.ctx(), session, chrome) {
            center = true;
        }
        recovery_dialog(ui.ctx(), session, chrome);
        error_dialog(ui.ctx(), session, chrome);
        center
    }

    /// Tells the person a save or open failed: which file, and why. A failed
    /// save leaves the sketch open with its changes.
    fn error_dialog(ctx: &egui::Context, session: &mut Session, chrome: &mut Chrome) {
        if chrome.error.is_none() {
            chrome.error = session.take_file_error();
        }
        let Some(error) = chrome.error.clone() else {
            return;
        };
        let heading = if error.saving {
            format!("Could not save “{}”", error.file)
        } else {
            format!("Could not open “{}”", error.file)
        };
        let mut ok = false;
        let modal = egui::Modal::new(egui::Id::new("file-error")).show(ctx, |ui| {
            ui.set_min_width(260.0);
            ui.set_max_width(360.0);
            ui.label(RichText::new(heading).strong().size(16.0));
            ui.add_space(4.0);
            ui.label(&error.reason);
            if error.saving {
                ui.label("Your changes are still open.");
            }
            ui.add_space(12.0);
            if ui.button("OK").clicked() {
                ok = true;
            }
        });
        if ok || modal.should_close() {
            chrome.error = None;
        }
    }

    /// Closing the window asks once per dirty sketch, in tab order. Cancel leaves
    /// the ones not yet closed still open. Save stores the sketch, then closes it.
    fn note_window_close(ctx: &egui::Context, session: &mut Session, chrome: &mut Chrome) {
        let close_requested = ctx.input(|input| input.viewport().close_requested());
        if !close_requested {
            return;
        }
        if chrome.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            return;
        }
        let _ = session.commit();
        let dirty = session.sketches_to_keep();
        if dirty.is_empty() {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        chrome.start_quit(session, dirty);
    }

    fn menu(ui: &mut egui::Ui, session: &mut Session, chrome: &mut Chrome, center: &mut bool) {
        let response = toolbar::glyph_button(
            ui,
            "menu",
            18.0,
            egui::vec2(32.0, TAB_HEIGHT),
            Some("Menu"),
            true,
            false,
        );
        egui::Popup::menu(&response).show(|ui| {
            let recent = session.recent_menu();
            let clear_enabled = session.command_enabled(CommandId::ClearRecent);
            let rows = registry::sketch_menu();
            let items: Vec<widgets::MenuItem> = rows
                .iter()
                .map(|row| match row {
                    SketchMenuRow::Command(id) => widgets::MenuItem {
                        label: registry::command(*id).label,
                        icon: None,
                        enabled: session.command_enabled(*id),
                        submenu: false,
                        divider: false,
                    },
                    SketchMenuRow::Recent => widgets::MenuItem {
                        label: registry::RECENT_MENU_LABEL,
                        icon: None,
                        enabled: !recent.is_empty(),
                        submenu: true,
                        divider: false,
                    },
                })
                .collect();
            let picked =
                widgets::menu_rows_with(ui, &items, |_, ui| recent_rows(ui, &recent, clear_enabled));
            match picked {
                Some(widgets::MenuPick::Row(index)) => match rows[index] {
                    SketchMenuRow::Command(CommandId::Close) => {
                        let sketch = session.active_id();
                        if matches!(
                            chrome.close(session, sketch),
                            CloseEffect::Closed { replacement: true }
                        ) {
                            *center = true;
                        }
                    }
                    SketchMenuRow::Command(id) => {
                        if matches!(session.perform(id, [0.0, 0.0]), CommandEffect::CenterView) {
                            *center = true;
                        }
                    }
                    SketchMenuRow::Recent => {}
                },
                Some(widgets::MenuPick::Sub(RecentPick::Path(index))) => {
                    session.open_path(&recent[index]);
                }
                Some(widgets::MenuPick::Sub(RecentPick::Clear)) => {
                    let _ = session.perform(CommandId::ClearRecent, [0.0, 0.0]);
                }
                None => {}
            }
        });
    }

    enum RecentPick {
        Path(usize),
        Clear,
    }

    /// The Open Recent submenu: each recent file by name, most recent first, then
    /// Clear Recent below a divider. Two files with one name also show their folder.
    fn recent_rows(ui: &mut egui::Ui, recent: &[PathBuf], clear_enabled: bool) -> Option<RecentPick> {
        let titles: Vec<String> = recent
            .iter()
            .map(|path| sketch::store::file_title(path))
            .collect();
        let labels: Vec<String> = recent
            .iter()
            .zip(&titles)
            .map(|(path, title)| {
                let shared = titles.iter().filter(|other| *other == title).count() > 1;
                let folder = path
                    .parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str());
                match folder {
                    Some(folder) if shared => format!("{title} — {folder}"),
                    _ => title.clone(),
                }
            })
            .collect();
        let clear = registry::command(CommandId::ClearRecent);
        let mut items: Vec<widgets::MenuItem> = labels
            .iter()
            .map(|label| widgets::MenuItem {
                label,
                icon: None,
                enabled: true,
                submenu: false,
                divider: false,
            })
            .collect();
        items.push(widgets::MenuItem {
            label: clear.label,
            icon: None,
            enabled: clear_enabled,
            submenu: false,
            divider: true,
        });
        let index = widgets::menu_rows(ui, &items)?;
        Some(if index < recent.len() {
            RecentPick::Path(index)
        } else {
            RecentPick::Clear
        })
    }

    /// Returns the visible part of the active tab.
    fn tab_row(
        ui: &mut egui::Ui,
        session: &mut Session,
        chrome: &mut Chrome,
        center: &mut bool,
    ) -> Option<egui::Rect> {
        let tabs: Vec<_> = session
            .tabs()
            .into_iter()
            .map(|tab| {
                (
                    tab.id,
                    tab.name.to_string(),
                    tab.active,
                    tab.close_waits,
                    tab.needs_keep,
                )
            })
            .collect();
        let width = (ui.available_width() - 36.0).max(48.0);
        let mut active_rect = None;
        let row = egui::Layout::left_to_right(egui::Align::Center);
        ui.allocate_ui_with_layout(egui::vec2(width, TAB_HEIGHT), row, |ui| {
            ui.set_max_width(width);
            egui::ScrollArea::horizontal()
                .id_salt("tabs")
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    // A full-height row: `horizontal` would start short and grow
                    // downward, which puts the tabs below the menu button.
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.set_height(TAB_HEIGHT);
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let visible = ui.clip_rect();
                        for (id, name, active, close_waits, unsaved) in tabs {
                            let tab = Tab {
                                id,
                                name: &name,
                                active,
                                close_waits,
                                unsaved,
                            };
                            let rect = tab_chip(ui, session, chrome, tab, center);
                            if active {
                                active_rect = Some(rect.intersect(visible));
                            }
                        }
                    });
                });
        });
        active_rect.filter(|rect| rect.width() > 0.0)
    }

    struct Tab<'a> {
        id: SketchId,
        name: &'a str,
        active: bool,
        close_waits: bool,
        /// Unsaved changes: a dot stands in for the close mark until the tab is
        /// hovered.
        unsaved: bool,
    }

    /// One tab: its name, then its close control. Clicking the tab shows it.
    /// Double-clicking does not edit the title. A middle click closes it.
    fn tab_chip(
        ui: &mut egui::Ui,
        session: &mut Session,
        chrome: &mut Chrome,
        tab: Tab<'_>,
        center: &mut bool,
    ) -> egui::Rect {
        const PAD_LEFT: f32 = 14.0;
        const PAD_RIGHT: f32 = 8.0;
        const CLOSE: f32 = 20.0;
        const MIN_WIDTH: f32 = 120.0;
        const MAX_LABEL: f32 = 180.0;

        let galley = egui::WidgetText::from(RichText::new(tab.name).size(13.5).color(ICON))
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                MAX_LABEL,
                egui::FontSelection::Default,
            );
        let label_width = galley.size().x;
        let width = (PAD_LEFT + label_width + 10.0 + CLOSE + PAD_RIGHT).max(MIN_WIDTH);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, TAB_HEIGHT), egui::Sense::click());
        paint_tab(ui.painter(), rect, tab.active);

        let label_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + PAD_LEFT, rect.top()),
            egui::vec2(label_width, TAB_HEIGHT),
        );
        let top = rect.center().y - galley.size().y / 2.0;
        ui.painter()
            .galley(egui::pos2(label_rect.left(), top), galley, ICON);

        let close_rect = egui::Rect::from_center_size(
            egui::pos2(rect.right() - PAD_RIGHT - CLOSE / 2.0, rect.center().y),
            egui::vec2(CLOSE, CLOSE),
        );
        let close = close_control(ui, close_rect, tab.id, !tab.close_waits);
        let dot = tab.unsaved && !response.hovered() && !close.hovered();
        paint_close_mark(ui, close_rect, !tab.close_waits, dot);
        if close.clicked() || response.middle_clicked() {
            finish_close(chrome.close(session, tab.id), center);
        } else if response.clicked() {
            session.activate(tab.id);
        }
        rect
    }

    /// Rounded top corners and an outline on the top and sides. The bottom edge
    /// is the bar's hairline, or, for the active tab, the white toolbar.
    fn paint_tab(painter: &egui::Painter, rect: egui::Rect, active: bool) {
        let fill = if active { Color32::WHITE } else { INACTIVE };
        let radius = egui::CornerRadius {
            nw: 8,
            ne: 8,
            sw: 0,
            se: 0,
        };
        painter.rect_filled(rect, radius, fill);

        let edge = rect.shrink2(egui::vec2(0.5, 0.0));
        let r = 8.0;
        let mut outline = vec![egui::pos2(edge.left(), edge.bottom())];
        let corner = |center: egui::Pos2, from: f32, outline: &mut Vec<egui::Pos2>| {
            for step in 0..=6 {
                let angle = from + std::f32::consts::FRAC_PI_2 * step as f32 / 6.0;
                outline.push(center + r * egui::vec2(angle.cos(), angle.sin()));
            }
        };
        let top = edge.top() + 0.5;
        corner(
            egui::pos2(edge.left() + r, top + r),
            std::f32::consts::PI,
            &mut outline,
        );
        corner(
            egui::pos2(edge.right() - r, top + r),
            -std::f32::consts::FRAC_PI_2,
            &mut outline,
        );
        outline.push(egui::pos2(edge.right(), edge.bottom()));
        painter.add(egui::Shape::line(outline, egui::Stroke::new(1.0, TAB_EDGE)));
    }

    fn finish_close(effect: CloseEffect, center: &mut bool) {
        if matches!(effect, CloseEffect::Closed { replacement: true }) {
            *center = true;
        }
    }

    /// Returns whether a replacement blank should be centered.
    fn confirm(ctx: &egui::Context, session: &mut Session, chrome: &mut Chrome, id: SketchId) -> bool {
        let Some(name) = session.name_of(id).map(str::to_string) else {
            if chrome.quitting {
                chrome.advance_quit(ctx, session);
            } else {
                chrome.confirm = None;
            }
            return false;
        };
        let mut save = false;
        let mut discard = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("keep-this-sketch")).show(ctx, |ui| {
            ui.set_min_width(260.0);
            ui.label(RichText::new("Keep this sketch?").strong().size(16.0));
            ui.add_space(4.0);
            ui.label(name);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    save = true;
                }
                if ui.button("Discard").clicked() {
                    discard = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        let dismiss = modal.should_close();
        if save {
            if let Some(path) = session.file_path(id) {
                match session.save_path(id, path, false, true) {
                    SaveResult::Wrote => return chrome.complete_save(ctx, session, id),
                    SaveResult::NeedsReplace(path) => {
                        chrome.ask_replace(id, path, false, true);
                        return false;
                    }
                    SaveResult::Failed => return false,
                }
            }
            session.request_save_dialog(id, true);
            return false;
        }
        if discard {
            let effect = session.discard(id);
            let replacement = matches!(effect, CloseEffect::Closed { replacement: true });
            if !matches!(effect, CloseEffect::Closed { .. }) {
                return false;
            }
            if chrome.quitting {
                chrome.advance_quit(ctx, session);
            } else {
                chrome.confirm = None;
            }
            return replacement;
        }
        if cancel || dismiss {
            chrome.stop_confirm();
        }
        false
    }

    /// Returns whether a replacement blank should be centered.
    fn replace_dialog(ctx: &egui::Context, session: &mut Session, chrome: &mut Chrome) -> bool {
        let Some(ask) = chrome.replace.clone() else {
            return false;
        };
        let name = sketch::store::file_title(&ask.path);
        let mut replace = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("replace-sketch-file")).show(ctx, |ui| {
            ui.set_min_width(260.0);
            ui.label(RichText::new("Replace this file?").strong().size(16.0));
            ui.add_space(4.0);
            ui.label(name);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Replace").clicked() {
                    replace = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if replace {
            chrome.replace = None;
            match session.save_path(ask.id, ask.path, ask.copy, true) {
                SaveResult::Wrote if ask.close_after => {
                    return chrome.complete_save(ctx, session, ask.id);
                }
                SaveResult::Wrote => return false,
                SaveResult::Failed | SaveResult::NeedsReplace(_) => {
                    if ask.close_after {
                        chrome.cancel_save_as();
                    }
                    return false;
                }
            }
        }
        if cancel || modal.should_close() {
            chrome.replace = None;
            if ask.close_after {
                chrome.cancel_save_as();
            }
        }
        false
    }

    /// One recovery at a time. Restore opens it still unsaved. Discard deletes
    /// the file and does not open it.
    fn recovery_dialog(ctx: &egui::Context, session: &mut Session, chrome: &mut Chrome) {
        if chrome.confirm.is_some() || chrome.replace.is_some() {
            return;
        }
        let Some(offer) = chrome.recoveries.first().cloned() else {
            return;
        };
        let mut restore = false;
        let mut discard = false;
        let _modal = egui::Modal::new(egui::Id::new("recovery-offer")).show(ctx, |ui| {
            ui.set_min_width(260.0);
            ui.label(RichText::new("Recovery").strong().size(16.0));
            ui.add_space(4.0);
            ui.label(&offer.name);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Restore").clicked() {
                    restore = true;
                }
                if ui.button("Discard").clicked() {
                    discard = true;
                }
            });
        });
        if restore {
            session.restore_recovery(offer.id);
            chrome.recoveries.remove(0);
        } else if discard {
            session.discard_recovery_file(offer.id);
            chrome.recoveries.remove(0);
        }
    }

    fn close_control(
        ui: &mut egui::Ui,
        rect: egui::Rect,
        id: SketchId,
        enabled: bool,
    ) -> egui::Response {
        let sense = if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        ui.interact(rect, egui::Id::new(("tab-close", id.get())), sense)
    }

    /// The close mark, or a dot while the sketch has unsaved changes and the
    /// pointer is away from its tab.
    fn paint_close_mark(ui: &egui::Ui, rect: egui::Rect, enabled: bool, dot: bool) {
        let color = if enabled { ICON } else { toolbar::ICON_QUIET };
        if dot {
            ui.painter().circle_filled(rect.center(), 4.0, color);
            return;
        }
        let glyph = icon_text("xmark", 11.0, color);
        let galley = egui::WidgetText::from(glyph).into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::FontSelection::Default,
        );
        let top_left = rect.center() - galley.size() / 2.0;
        ui.painter().galley(top_left, galley, color);
    }
}
