//! The assistant's conversation: the logo and its menu, the project's name
//! and menu, the conversation, and the input box with its attachments, model
//! picker, microphone, and send. It fills the window, or sits in a column
//! when the workspace is shown. It is drawn with the shell's own widgets and
//! colors, and knows no sub-application: what the assistant can do comes
//! from what sub-applications declare.

use crate::toolbar::{self, ICON};
use crate::widgets::{self, BORDER, MUTED, SELECTED, TEXT};
use crate::workspace::ModelChoice;
use assistant::{Attachment, Entry, Model, Role};
use eframe::egui::{
    self, Align, Align2, Color32, Event, FontId, Galley, Key, Layout, Pos2, Rect, Sense,
    UiBuilder, pos2, vec2,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The logo's row, level with the tab strip beside it.
pub(crate) const LOGO_ROW: f32 = 40.0;
/// The strip's tabs sit 6 points down and are 34 tall; the logo is centered
/// on the same line as their labels.
const LOGO_CENTER: f32 = 6.0 + 17.0;
const LOGO: f32 = 26.0;
/// The space between the panel's edge and its contents.
const SIDE: f32 = 14.0;
const BODY: f32 = 13.5;
const SMALL: f32 = 11.5;
/// Space inside a message's bubble or card.
const BUBBLE_X: f32 = 12.0;
const BUBBLE_Y: f32 = 9.0;
const BUBBLE_RADIUS: f32 = 12.0;
const BULLET_INDENT: f32 = 14.0;
const ENTRY_GAP: f32 = 16.0;
const BOX_RADIUS: f32 = 16.0;
const BOX_PAD_X: f32 = 14.0;
const CONTROLS: f32 = 36.0;
/// The space above the text, and between the text and the controls.
const BOX_TOP: f32 = 16.0;
const BOX_GAP: f32 = 14.0;
const BOX_BOTTOM: f32 = 10.0;
const CHIP_H: f32 = 26.0;
const SEND_RADIUS: f32 = 18.0;
const MAX_LINES: f32 = 8.0;

/// The person's message.
const PERSON: Color32 = Color32::from_rgb(240, 241, 243);
/// The input's placeholder.
const HINT: Color32 = Color32::from_rgb(156, 160, 166);
/// The send button.
const SEND: Color32 = Color32::from_rgb(24, 24, 27);
/// The model picker's pill.
const PILL: Color32 = Color32::from_rgb(249, 250, 251);

/// The extensions the plus offers: pictures, then text files.
pub(crate) const PICTURES: &[&str] = &["png", "jpg", "jpeg", "webp", "gif"];
pub(crate) const TEXTS: &[&str] = &["txt", "md", "markdown", "csv", "json", "xml", "html", "svg"];

/// What the panel shows and holds between frames.
#[derive(Default)]
pub(crate) struct Chat {
    /// Every entry, in order, as the assistant sent them.
    pub conversation: Vec<Entry>,
    /// A request is running: send is stop, and the applications are locked.
    pub working: bool,
    pub input: String,
    /// Files chosen with the plus and not yet sent.
    pub chips: Vec<PathBuf>,
    /// The models that can use tools, once listed, or why not.
    pub models: Option<Result<Vec<Model>, String>>,
    pub model: Option<ModelChoice>,
    logo: Option<egui::TextureHandle>,
    /// The input sits in the middle: the conversation is empty and fills the window.
    pub input_centered: bool,
    /// Messages sit in a centred column: the conversation fills the window.
    pub centered_column: bool,
}

/// The logo menu has one row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum LogoChoice {
    Quit,
}

/// What the project menu offers, besides the name itself.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ProjectChoice {
    Rename,
    Switch(String),
    New,
    Remove,
    AddFolder,
    RemoveFolder(usize),
}

/// The project menu's contents for one frame.
pub(crate) struct ProjectMenu {
    pub name: String,
    pub folders: Vec<String>,
    /// Other projects: id, then the name the menu shows.
    pub others: Vec<(String, String)>,
    pub renaming: bool,
    pub draft: String,
}

/// What the person did in the panel.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ChatEvent {
    /// Send the input and the chips.
    Send,
    /// Stop the running request.
    Stop,
    /// The plus: choose files to attach.
    Attach,
    /// The model list is wanted.
    ListModels,
    Model(Model),
    Logo(LogoChoice),
    Project(ProjectChoice),
    /// The name was typed.
    Renamed(String),
    CancelRename,
}

/// The panel's width: a quarter of the window, but never so narrow that a
/// message cannot be read.
pub(crate) fn width(window: f32) -> f32 {
    (window * 0.25).round().max(260.0).min(window * 0.5)
}

/// Messages in the conversation that fills the window sit in a column no
/// wider than this, centred.
const TALK_COLUMN: f32 = 720.0;

/// The name the picker shows: the model's name without its maker's prefix.
pub(crate) fn short_name(name: &str) -> &str {
    name.split_once(": ").map_or(name, |(_, rest)| rest)
}

/// Draws the conversation in `rect` and returns what the person did.
/// `fills` is the conversation taking the whole window.
pub(crate) fn show(
    ui: &mut egui::Ui,
    rect: Rect,
    chat: &mut Chat,
    project: &mut ProjectMenu,
    fills: bool,
) -> Vec<ChatEvent> {
    let mut events = Vec::new();
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 0.0, Color32::WHITE);
    if !fills {
        widgets::vertical_rule(&painter, rect.right() - 0.5, rect.top(), rect.bottom(), BORDER);
    }
    let inner = if fills {
        rect
    } else {
        Rect::from_min_max(rect.min, pos2(rect.right() - 1.0, rect.bottom()))
    };

    let empty = !chat.conversation.iter().any(|entry| entry.role != Role::Tool);
    let centered = fills && empty;
    chat.input_centered = centered;
    chat.centered_column = fills && !empty;
    project_name(ui, inner, project, &mut events);

    let column = if fills { talk_column(inner) } else { inner };
    if centered {
        let frame = input_box(ui, column, chat, true, &mut events);
        let logo = Rect::from_center_size(
            pos2(frame.center().x - 10.0, frame.top() - 28.0),
            vec2(LOGO + 36.0, LOGO),
        );
        logo_button(ui, logo.center(), chat, &mut events);
    } else {
        logo_button(
            ui,
            pos2(inner.left() + 15.0 + LOGO / 2.0, inner.top() + LOGO_CENTER),
            chat,
            &mut events,
        );
        let frame = input_box(ui, column, chat, false, &mut events);
        let talk = Rect::from_min_max(
            pos2(column.left(), inner.top() + LOGO_ROW),
            pos2(column.right(), frame.top() - 8.0),
        );
        conversation(ui, talk, &chat.conversation);
    }
    events
}

/// The centred column messages use when the conversation fills the window.
fn talk_column(rect: Rect) -> Rect {
    let width = (rect.width() - 2.0 * SIDE).min(TALK_COLUMN).max(40.0);
    let left = rect.center().x - width / 2.0;
    Rect::from_min_size(pos2(left, rect.top()), vec2(width, rect.height()))
}

fn logo_texture(ui: &egui::Ui, chat: &mut Chat) -> Option<egui::TextureId> {
    if chat.logo.is_none() {
        let icon = eframe::icon_data::from_png_bytes(include_bytes!(
            "../assets/logo/logo-light.png"
        ))
        .ok()?;
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [icon.width as usize, icon.height as usize],
            &icon.rgba,
        );
        chat.logo = Some(
            ui.ctx()
                .load_texture("inpainter-logo", image, egui::TextureOptions::LINEAR),
        );
    }
    chat.logo.as_ref().map(egui::TextureHandle::id)
}

/// The logo and its chevron, which open a menu with Quit.
fn logo_button(ui: &mut egui::Ui, center: Pos2, chat: &mut Chat, events: &mut Vec<ChatEvent>) {
    let logo = Rect::from_center_size(center, vec2(LOGO, LOGO));
    let chevron = pos2(logo.right() + 16.0, center.y);
    let hit = Rect::from_min_max(
        pos2(logo.left() - 5.0, center.y - 17.0),
        pos2(chevron.x + 10.0, center.y + 17.0),
    );
    let response = ui
        .interact(hit, egui::Id::new("logo-menu"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    if response.hovered() || open {
        ui.painter().rect_filled(hit, 6.0, SELECTED);
    }
    if let Some(texture) = logo_texture(ui, chat) {
        ui.painter().image(
            texture,
            logo,
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    widgets::chevron(ui.painter(), chevron, MUTED, widgets::Scale(1.15));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Inpainter"));

    let waits = !chat.working;
    egui::Popup::menu(&response).show(|ui| {
        if widgets::menu_rows(ui, &[item("Quit", waits, false, false)]).is_some() {
            events.push(ChatEvent::Logo(LogoChoice::Quit));
        }
    });
}

/// The project's name, centred at the top, which opens its menu.
fn project_name(ui: &mut egui::Ui, rect: Rect, project: &mut ProjectMenu, events: &mut Vec<ChatEvent>) {
    if project.name.is_empty() && !project.renaming {
        return;
    }
    let center = pos2(rect.center().x, rect.top() + LOGO_CENTER);
    if project.renaming {
        let width = 240.0_f32.min(rect.width() - 24.0);
        let field = Rect::from_center_size(center, vec2(width, 28.0));
        let mut edit = ui.new_child(UiBuilder::new().max_rect(field));
        let response = edit.add(
            egui::TextEdit::singleline(&mut project.draft)
                .id(egui::Id::new("project-rename"))
                .hint_text("Project name")
                .desired_width(width - 12.0)
                .font(FontId::proportional(BODY)),
        );
        response.request_focus();
        let enter = ui.input(|input| input.key_pressed(Key::Enter));
        let escape = ui.input(|input| input.key_pressed(Key::Escape));
        if enter {
            events.push(ChatEvent::Renamed(project.draft.clone()));
        } else if escape {
            events.push(ChatEvent::CancelRename);
        }
        return;
    }

    let font = FontId::proportional(BODY);
    let galley = ui.painter().layout_no_wrap(project.name.clone(), font, TEXT);
    let chevron = pos2(center.x + galley.size().x / 2.0 + 12.0, center.y);
    let hit = Rect::from_min_max(
        pos2(center.x - galley.size().x / 2.0 - 8.0, center.y - 16.0),
        pos2(chevron.x + 10.0, center.y + 16.0),
    );
    let response = ui
        .interact(hit, egui::Id::new("project-menu"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    if response.hovered() || open {
        ui.painter().rect_filled(hit, 6.0, SELECTED);
    }
    ui.painter().galley(
        pos2(center.x - galley.size().x / 2.0, center.y - galley.size().y / 2.0),
        galley,
        TEXT,
    );
    widgets::chevron(ui.painter(), chevron, MUTED, widgets::Scale(1.0));
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, project.name.as_str())
    });

    egui::Popup::menu(&response).show(|ui| project_menu(ui, project, events));
}

/// Rows under the name: the folders, then rename, switch, and new, then remove.
fn project_menu(ui: &mut egui::Ui, project: &ProjectMenu, events: &mut Vec<ChatEvent>) {
    let width = 220.0_f32;
    for (index, folder) in project.folders.iter().enumerate() {
        let name = Path::new(folder)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| folder.clone());
        let (row, _) = ui.allocate_exact_size(vec2(width, 28.0), Sense::hover());
        if ui.rect_contains_pointer(row) {
            ui.painter().rect_filled(row, 4.0, SELECTED);
        }
        ui.painter().text(
            pos2(row.left() + 10.0, row.center().y),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(BODY),
            TEXT,
        );
        let mark = Rect::from_center_size(pos2(row.right() - 16.0, row.center().y), vec2(18.0, 18.0));
        let remove = ui
            .interact(mark, egui::Id::new(("folder-remove", index)), Sense::click())
            .on_hover_text("Remove");
        let color = if remove.hovered() { toolbar::ICON_ACTIVE } else { MUTED };
        toolbar::paint_icon(ui.painter(), mark.center(), "xmark", 12.0, color);
        if remove.clicked() {
            events.push(ChatEvent::Project(ProjectChoice::RemoveFolder(index)));
            ui.close();
        }
    }
    let rows = [
        item("Add folder…", true, false, false),
        item("Rename", true, false, true),
        item("Switch", !project.others.is_empty(), true, false),
        item("New Project", true, false, false),
        item("Remove Project", true, false, true),
    ];
    // Switch is the row that opens the other projects.
    let names: Vec<String> = project.others.iter().map(|(_, name)| name.clone()).collect();
    let picked = widgets::menu_rows_with(ui, &rows, |index, ui| {
        if index != 2 {
            return None;
        }
        let items: Vec<widgets::MenuItem> = names.iter().map(|name| item(name, true, false, false)).collect();
        widgets::menu_rows(ui, &items)
    });
    let choice = match picked {
        Some(widgets::MenuPick::Row(0)) => Some(ProjectChoice::AddFolder),
        Some(widgets::MenuPick::Row(1)) => Some(ProjectChoice::Rename),
        Some(widgets::MenuPick::Row(3)) => Some(ProjectChoice::New),
        Some(widgets::MenuPick::Row(4)) => Some(ProjectChoice::Remove),
        Some(widgets::MenuPick::Sub(index)) => project.others.get(index).map(|(id, _)| ProjectChoice::Switch(id.clone())),
        _ => None,
    };
    if let Some(choice) = choice {
        events.push(ChatEvent::Project(choice));
    }
}

fn item(label: &str, enabled: bool, submenu: bool, divider: bool) -> widgets::MenuItem<'_> {
    widgets::MenuItem {
        label,
        icon: None,
        enabled,
        submenu,
        divider,
    }
}

/// The input box, at the bottom or in the middle. Returns its frame.
fn input_box(
    ui: &mut egui::Ui,
    rect: Rect,
    chat: &mut Chat,
    centered: bool,
    events: &mut Vec<ChatEvent>,
) -> Rect {
    let font = FontId::proportional(BODY);
    let left = rect.left() + SIDE;
    let right = rect.right() - SIDE;
    let field_w = (right - left - 2.0 * BOX_PAD_X).max(40.0);
    let line = ui.painter().layout_no_wrap("Ag".to_string(), font.clone(), TEXT).size().y;
    let measured = ui
        .painter()
        .layout(chat.input.clone(), font.clone(), TEXT, field_w)
        .size()
        .y;
    let field_h = measured.clamp(line, line * MAX_LINES);

    // The chips, wrapped into rows above the text.
    let chip_font = FontId::proportional(12.5);
    let mut chips: Vec<(Rect, Arc<Galley>)> = Vec::new();
    let (mut x, mut row) = (0.0_f32, 0usize);
    for path in &chat.chips {
        let galley = ui.painter().layout_no_wrap(file_name(path), chip_font.clone(), TEXT);
        let w = (galley.size().x.min(field_w - 40.0) + 10.0 + 20.0).min(field_w);
        if x > 0.0 && x + w > field_w {
            x = 0.0;
            row += 1;
        }
        let at = Rect::from_min_size(pos2(x, row as f32 * (CHIP_H + 6.0)), vec2(w, CHIP_H));
        chips.push((at, galley));
        x += w + 6.0;
    }
    let chips_h = if chips.is_empty() {
        0.0
    } else {
        (row + 1) as f32 * (CHIP_H + 6.0) + 2.0
    };

    let height = BOX_TOP + chips_h + field_h + BOX_GAP + CONTROLS + BOX_BOTTOM;
    let bottom = if centered {
        rect.center().y + height / 2.0
    } else {
        rect.bottom() - SIDE
    };
    let frame = Rect::from_min_max(pos2(left, bottom - height), pos2(right, bottom));
    widgets::soft_shadow(ui.painter(), frame, BOX_RADIUS);
    widgets::outlined(ui.painter(), frame, BOX_RADIUS, Color32::WHITE, BORDER);
    let field_id = egui::Id::new("assistant-input");
    // A click anywhere in the box puts the caret in the text. The text field
    // lets go of the keys on a click outside itself, so the focus is asked
    // for after it has run.
    let box_clicked = ui
        .interact(frame, egui::Id::new("assistant-box"), Sense::click())
        .clicked();

    // The chips, each with its file's name and a close mark.
    let chips_top = frame.top() + BOX_TOP - 2.0;
    let mut removed = None;
    for (index, (at, galley)) in chips.into_iter().enumerate() {
        let chip = at.translate(vec2(frame.left() + BOX_PAD_X, chips_top));
        widgets::outlined(ui.painter(), chip, 6.0, PILL, BORDER);
        let text_w = chip.width() - 30.0;
        let clip = Rect::from_min_size(chip.min, vec2(10.0 + text_w, chip.height()));
        ui.painter().with_clip_rect(clip).galley(
            pos2(chip.left() + 10.0, chip.center().y - galley.size().y / 2.0),
            galley,
            TEXT,
        );
        let close = Rect::from_center_size(pos2(chip.right() - 12.0, chip.center().y), vec2(16.0, 16.0));
        let response = ui
            .interact(close, egui::Id::new(("chip-close", index)), Sense::click())
            .on_hover_text("Remove");
        let color = if response.hovered() { toolbar::ICON_ACTIVE } else { MUTED };
        toolbar::paint_icon(ui.painter(), close.center(), "xmark", 11.0, color);
        if response.clicked() {
            removed = Some(index);
        }
    }
    if let Some(index) = removed {
        chat.chips.remove(index);
    }

    // The text. Enter sends; Shift+Enter is a new line.
    let field = Rect::from_min_size(
        pos2(frame.left() + BOX_PAD_X, frame.top() + BOX_TOP + chips_h),
        vec2(field_w, field_h),
    );
    let focused = ui.memory(|memory| memory.has_focus(field_id));
    if focused {
        let enter = ui.input_mut(|input| {
            let pressed = input.events.iter().any(|event| is_plain_enter(event));
            if pressed {
                input.events.retain(|event| !is_plain_enter(event));
            }
            pressed
        });
        if enter && !chat.working && can_send(chat) {
            events.push(ChatEvent::Send);
        }
    }
    let mut field_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(field)
            .layout(Layout::top_down(Align::Min)),
    );
    egui::ScrollArea::vertical()
        .id_salt("assistant-input-scroll")
        .max_height(field_h)
        .stick_to_bottom(true)
        .show(&mut field_ui, |ui| {
            let text = ui.add(
                egui::TextEdit::multiline(&mut chat.input)
                    .id(field_id)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .hint_text(egui::RichText::new("Ask the assistant…").color(HINT))
                    .font(font.clone())
                    .text_color(TEXT)
                    .desired_width(field_w)
                    .desired_rows(1),
            );
            if box_clicked {
                text.request_focus();
            }
        });

    // The controls: plus, model picker, then microphone and send.
    let center_y = frame.bottom() - BOX_BOTTOM - CONTROLS / 2.0;
    let plus = Rect::from_center_size(pos2(frame.left() + 10.0 + 16.0, center_y), vec2(32.0, 32.0));
    let mut plus_ui = ui.new_child(UiBuilder::new().max_rect(plus));
    if toolbar::glyph_button(&mut plus_ui, "plus", 19.0, plus.size(), Some("Attach files"), true, false)
        .clicked()
    {
        events.push(ChatEvent::Attach);
    }

    let send_center = pos2(frame.right() - 9.0 - SEND_RADIUS, center_y);
    let mic_center = pos2(send_center.x - SEND_RADIUS - 10.0 - 14.0, center_y);
    model_picker(ui, chat, pos2(plus.right() + 6.0, center_y), mic_center.x - 18.0, events);

    // The microphone is shown in its place and does nothing yet.
    let mic = Rect::from_center_size(mic_center, vec2(28.0, 32.0));
    ui.interact(mic, egui::Id::new("assistant-mic"), Sense::hover());
    toolbar::paint_icon(ui.painter(), mic_center, "microphone", 19.0, ICON);

    let send = Rect::from_center_size(send_center, vec2(2.0 * SEND_RADIUS, 2.0 * SEND_RADIUS));
    let tip = if chat.working { "Stop" } else { "Send" };
    let response = ui
        .interact(send, egui::Id::new("assistant-send"), Sense::click())
        .on_hover_text(tip);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tip));
    widgets::disc(ui.painter(), send_center, SEND_RADIUS, SEND);
    if chat.working {
        let square = Rect::from_center_size(send_center, vec2(10.0, 10.0));
        ui.painter().rect_filled(square, 2.0, Color32::WHITE);
    } else {
        toolbar::paint_icon(ui.painter(), send_center, "arrow-up", 18.0, Color32::WHITE);
    }
    if response.clicked() {
        if chat.working {
            events.push(ChatEvent::Stop);
        } else if can_send(chat) {
            events.push(ChatEvent::Send);
        }
    }
    frame
}

fn is_plain_enter(event: &Event) -> bool {
    matches!(
        event,
        Event::Key {
            key: Key::Enter,
            pressed: true,
            modifiers,
            ..
        } if !modifiers.shift && !modifiers.ctrl && !modifiers.alt && !modifiers.command
    )
}

fn can_send(chat: &Chat) -> bool {
    !chat.input.trim().is_empty() || !chat.chips.is_empty()
}

/// The quiet pill: a lightning mark, the model's name, and a chevron. It
/// opens the list of models that can use tools.
fn model_picker(
    ui: &mut egui::Ui,
    chat: &mut Chat,
    left_center: Pos2,
    max_right: f32,
    events: &mut Vec<ChatEvent>,
) {
    let font = FontId::proportional(12.5);
    let name = chat
        .model
        .as_ref()
        .map_or("Choose a model", |model| short_name(&model.name))
        .to_string();
    let room = (max_right - left_center.x - 10.0 - 14.0 - 6.0 - 8.0 - 12.0 - 10.0).max(20.0);
    let galley = ui.painter().layout(
        name,
        font,
        TEXT,
        f32::INFINITY,
    );
    let text_w = galley.size().x.min(room);
    let width = 10.0 + 14.0 + 6.0 + text_w + 8.0 + 12.0 + 10.0;
    let pill = Rect::from_min_size(pos2(left_center.x, left_center.y - 16.0), vec2(width, 32.0));
    let response = ui
        .interact(pill, egui::Id::new("model-picker"), Sense::click())
        .on_hover_text("Model");
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    let fill = if response.hovered() || open { SELECTED } else { PILL };
    widgets::outlined(ui.painter(), pill, 16.0, fill, BORDER);
    toolbar::paint_icon(
        ui.painter(),
        pos2(pill.left() + 10.0 + 7.0, pill.center().y),
        "flash-solid",
        14.0,
        ICON,
    );
    let text_left = pill.left() + 10.0 + 14.0 + 6.0;
    let clip = Rect::from_min_size(pos2(text_left, pill.top()), vec2(text_w, pill.height()));
    ui.painter().with_clip_rect(clip).galley(
        pos2(text_left, pill.center().y - galley.size().y / 2.0),
        galley,
        TEXT,
    );
    widgets::chevron(
        ui.painter(),
        pos2(pill.right() - 10.0 - 6.0, pill.center().y),
        MUTED,
        widgets::Scale(1.0),
    );
    let listed = matches!(&chat.models, Some(Ok(models)) if !models.is_empty());
    if response.clicked() && !listed {
        events.push(ChatEvent::ListModels);
    }

    let chosen = chat.model.as_ref().map(|model| model.id.clone());
    egui::Popup::menu(&response)
        .width(300.0)
        .gap(6.0)
        .show(|ui| {
            ui.set_width(300.0);
            let note = |ui: &mut egui::Ui, text: &str| {
                ui.add(egui::Label::new(egui::RichText::new(text).color(MUTED).size(BODY)).wrap());
            };
            match &chat.models {
                None => note(ui, "Finding the models…"),
                Some(Err(reason)) => note(ui, reason),
                Some(Ok(models)) if models.is_empty() => note(ui, "No models that can use tools were found."),
                Some(Ok(models)) => {
                    egui::ScrollArea::vertical()
                        .id_salt("model-list")
                        .max_height(360.0)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            for model in models {
                                let (row, row_response) =
                                    ui.allocate_exact_size(vec2(288.0, 28.0), Sense::click());
                                if Some(&model.id) == chosen.as_ref() || row_response.hovered() {
                                    ui.painter().rect_filled(row, 4.0, SELECTED);
                                }
                                ui.painter().with_clip_rect(row.shrink(2.0)).text(
                                    pos2(row.left() + 10.0, row.center().y),
                                    Align2::LEFT_CENTER,
                                    &model.name,
                                    widgets::body(widgets::Scale(1.0)),
                                    TEXT,
                                );
                                if row_response.clicked() {
                                    events.push(ChatEvent::Model(model.clone()));
                                    ui.close();
                                }
                            }
                        });
                }
            }
        });
}

/// The conversation, from the top, scrolling. The person's messages are on
/// the right, the assistant's replies and notes on the left; the tools it
/// called are kept but not shown.
fn conversation(ui: &mut egui::Ui, rect: Rect, entries: &[Entry]) {
    if rect.height() <= 0.0 {
        return;
    }
    let mut talk = ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );
    egui::ScrollArea::vertical()
        .id_salt("conversation")
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(&mut talk, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let width = (ui.available_width() - 2.0 * SIDE).max(80.0);
            ui.add_space(12.0);
            for entry in entries {
                if entry.role == Role::Tool {
                    continue;
                }
                message(ui, entry, width);
                ui.add_space(ENTRY_GAP);
            }
        });
}

/// One part of a reply: a paragraph, or a bulleted line.
#[derive(Debug, PartialEq)]
enum Block {
    Paragraph(String),
    Bullet(String),
}

/// Paragraphs are separated by blank lines; a line starting with "- ",
/// "* ", or "• " is a bullet.
fn blocks(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let flush = |paragraph: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        if !paragraph.is_empty() {
            blocks.push(Block::Paragraph(paragraph.join("\n")));
            paragraph.clear();
        }
    };
    for line in text.lines() {
        let trimmed = line.trim_start();
        let bullet = ["- ", "* ", "• "]
            .iter()
            .find_map(|mark| trimmed.strip_prefix(mark));
        if let Some(rest) = bullet {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Bullet(rest.trim().to_string()));
        } else if trimmed.is_empty() {
            flush(&mut paragraph, &mut blocks);
        } else {
            paragraph.push(line.trim_end());
        }
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

fn message(ui: &mut egui::Ui, entry: &Entry, width: f32) {
    let person = entry.role == Role::Person;
    let font = FontId::proportional(BODY);
    let max = (width * if person { 0.82 } else { 0.9 }).floor();
    let wrap = max - 2.0 * BUBBLE_X;
    let painter = ui.painter().clone();

    // Each line of the bubble: its offset from the text's left, and its galley.
    let mut lines: Vec<(f32, Arc<Galley>, Color32)> = Vec::new();
    let mut gaps: Vec<f32> = Vec::new();
    if person {
        if !entry.text.trim().is_empty() {
            lines.push((0.0, painter.layout(entry.text.clone(), font.clone(), TEXT, wrap), TEXT));
            gaps.push(0.0);
        }
        for path in &entry.attachments {
            let name = file_name(Path::new(path));
            let galley = painter.layout(name, FontId::proportional(12.0), MUTED, wrap);
            gaps.push(if lines.is_empty() { 0.0 } else { 4.0 });
            lines.push((0.0, galley, MUTED));
        }
    } else {
        for block in blocks(&entry.text) {
            gaps.push(if lines.is_empty() { 0.0 } else { 6.0 });
            match block {
                Block::Paragraph(text) => lines.push((0.0, painter.layout(text, font.clone(), TEXT, wrap), TEXT)),
                Block::Bullet(text) => lines.push((
                    BULLET_INDENT,
                    painter.layout(text, font.clone(), TEXT, wrap - BULLET_INDENT),
                    TEXT,
                )),
            }
        }
    }
    if lines.is_empty() {
        lines.push((0.0, painter.layout(String::new(), font.clone(), TEXT, wrap), TEXT));
        gaps.push(0.0);
    }
    let text_w = lines
        .iter()
        .map(|(indent, galley, _)| indent + galley.size().x)
        .fold(0.0, f32::max);
    let text_h: f32 = lines.iter().map(|(_, galley, _)| galley.size().y).sum::<f32>()
        + gaps.iter().sum::<f32>();
    let bubble_size = vec2((text_w + 2.0 * BUBBLE_X).ceil(), (text_h + 2.0 * BUBBLE_Y).ceil());
    let time = painter.layout_no_wrap(clock(entry.time), FontId::proportional(SMALL), MUTED);
    let height = bubble_size.y + 5.0 + time.size().y;

    let (row, _) = ui.allocate_exact_size(vec2(width + 2.0 * SIDE, height), Sense::hover());
    let row = row.shrink2(vec2(SIDE, 0.0));
    let bubble = if person {
        Rect::from_min_size(pos2(row.right() - bubble_size.x, row.top()), bubble_size)
    } else {
        Rect::from_min_size(row.min, bubble_size)
    };
    if person {
        painter.rect_filled(bubble, BUBBLE_RADIUS, PERSON);
    } else {
        widgets::outlined(&painter, bubble, BUBBLE_RADIUS, Color32::WHITE, BORDER);
    }
    let mut y = bubble.top() + BUBBLE_Y;
    for ((indent, galley, color), gap) in lines.into_iter().zip(gaps) {
        y += gap;
        let x = bubble.left() + BUBBLE_X;
        if indent > 0.0 {
            let row_h = galley.rows.first().map_or(galley.size().y, |row| row.height());
            widgets::disc(&painter, pos2(x + 4.0, y + row_h / 2.0), 2.0, TEXT);
        }
        let h = galley.size().y;
        painter.galley(pos2(x + indent, y), galley, color);
        y += h;
    }
    let time_x = if person {
        row.right() - time.size().x - 2.0
    } else {
        row.left() + 2.0
    };
    painter.galley(pos2(time_x, bubble.bottom() + 5.0), time, MUTED);
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// A small local time, such as "10:24 AM".
pub(crate) fn clock(seconds: u64) -> String {
    let (hour, minute) = local_hour_minute(seconds);
    let (shown, half) = match hour {
        0 => (12, "AM"),
        1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        _ => (hour - 12, "PM"),
    };
    format!("{shown}:{minute:02} {half}")
}

#[cfg(unix)]
fn local_hour_minute(seconds: u64) -> (u32, u32) {
    let time = seconds as libc::time_t;
    // Safety: `localtime_r` writes only into `parts`, which lives on this
    // stack frame, and reads only `time`.
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    let done = unsafe { libc::localtime_r(&time, &mut parts) };
    if done.is_null() {
        return utc_hour_minute(seconds);
    }
    (parts.tm_hour as u32, parts.tm_min as u32)
}

#[cfg(not(unix))]
fn local_hour_minute(seconds: u64) -> (u32, u32) {
    utc_hour_minute(seconds)
}

fn utc_hour_minute(seconds: u64) -> (u32, u32) {
    let day = seconds % 86_400;
    ((day / 3600) as u32, ((day % 3600) / 60) as u32)
}

/// Reads a chosen file for the model: a picture by its extension, or text.
/// None when it cannot be read.
pub(crate) fn load_attachment(path: &Path) -> Option<Attachment> {
    let extension = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let mime = match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    };
    match mime {
        Some(mime) => Some(Attachment::Image {
            path: path.to_path_buf(),
            mime: mime.to_string(),
            data: std::fs::read(path).ok()?,
        }),
        None => Some(Attachment::Text {
            path: path.to_path_buf(),
            text: std::fs::read_to_string(path).ok()?,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_split_into_paragraphs_and_bullets() {
        let text = "Done. I drew two things:\n\n- one\n- two\n\nAnything else?";
        assert_eq!(
            blocks(text),
            vec![
                Block::Paragraph("Done. I drew two things:".into()),
                Block::Bullet("one".into()),
                Block::Bullet("two".into()),
                Block::Paragraph("Anything else?".into()),
            ]
        );
    }

    #[test]
    fn the_clock_reads_like_the_reference() {
        assert_eq!(utc_hour_minute(10 * 3600 + 24 * 60), (10, 24));
        let shown = clock(1_760_000_000);
        assert!(shown.ends_with(" AM") || shown.ends_with(" PM"), "{shown}");
    }

    #[test]
    fn the_panel_is_a_quarter_of_the_window() {
        assert_eq!(width(1600.0), 400.0);
        assert_eq!(width(800.0), 260.0);
        assert_eq!(short_name("Anthropic: Claude Sonnet 4.5"), "Claude Sonnet 4.5");
    }

    /// Catches an icon name the Iconoir pack does not have, which would
    /// stop the window when the panel is drawn.
    #[test]
    fn the_panel_icons_exist() {
        let ctx = egui::Context::default();
        crate::toolbar::install_fonts(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for name in ["plus", "flash-solid", "microphone", "arrow-up", "xmark"] {
                toolbar::paint_icon(ui.painter(), Pos2::ZERO, name, 16.0, ICON);
            }
        });
        output.textures_delta.clear();
    }
}
