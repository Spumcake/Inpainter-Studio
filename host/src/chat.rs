//! The assistant's conversation: the logo and its menu, the project's name
//! and menu, the conversation, and the input box with its attachments, model
//! picker, microphone, and send. It fills the window, or sits in a column
//! when the workspace is shown. It is drawn with the shell's own widgets and
//! colors, and knows no sub-application: what the assistant can do comes
//! from what sub-applications declare.

use crate::toolbar::{self, ICON};
use crate::typescale as ts;
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
/// Space inside a message's bubble or card.
const BUBBLE_X: f32 = 12.0;
const BUBBLE_Y: f32 = 9.0;
const BUBBLE_RADIUS: f32 = 12.0;
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
    /// The reply arriving now, piece by piece, until its entry comes.
    pub live: String,
    /// When the running request started, in seconds since the epoch.
    pub started: Option<u64>,
    /// Messages sent while a request ran, each waiting its turn.
    pub queued: Vec<(String, Vec<PathBuf>)>,
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
    /// Hidden from the menu. A project has its own folder; attaching another waits.
    #[allow(dead_code)]
    AddFolder,
    /// Hidden with the folder rows, for the same reason.
    #[allow(dead_code)]
    RemoveFolder(usize),
}

/// The project menu's contents for one frame.
pub(crate) struct ProjectMenu {
    pub name: String,
    /// Kept for the later project system; not listed now.
    #[allow(dead_code)]
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
    /// Send this person's message again.
    Retry(String),
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

/// The panel's width beside the workspace. It does not follow the window's
/// width, as the owner set on 2026-10-28; only a window too narrow for it
/// takes some back, so the workspace keeps half. The person can drag its
/// edge a little either way, as the owner set the same day.
pub(crate) const PANEL_WIDTH: f32 = 420.0;
pub(crate) const PANEL_MIN: f32 = 360.0;
pub(crate) const PANEL_MAX: f32 = 520.0;

/// The panel's width in a window `window` wide, for the width the person
/// chose, if any.
pub(crate) fn width(window: f32, chosen: Option<f32>) -> f32 {
    chosen
        .unwrap_or(PANEL_WIDTH)
        .clamp(PANEL_MIN, PANEL_MAX)
        .round()
        .min((window * 0.5).round())
}


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
    board: &mut crate::cells::Board,
    cell_actions: &mut Vec<crate::cells::CardAction>,
) -> Vec<ChatEvent> {
    let mut events = Vec::new();
    // As the panel, the conversation's text is a step smaller.
    let _sizes = ts::panel_sizes(!fills);
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

    let empty = !chat.conversation.iter().any(|entry| entry.role != Role::Tool || entry.cell.is_some());
    let centered = fills && empty;
    chat.input_centered = centered;
    chat.centered_column = fills && !empty;
    project_name(ui, inner, project, &mut events);

    // One column for the conversation and the input, with the guide's gutters.
    let column = if fills { talk_column(inner) } else { inner.shrink2(vec2(ts::GUTTER, 0.0)) };
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
        conversation(ui, talk, chat, board, cell_actions, &mut events);
    }
    events
}

/// The centred column messages use when the conversation fills the window.
fn talk_column(rect: Rect) -> Rect {
    let width = (rect.width() - 2.0 * ts::GUTTER).min(ts::COLUMN).max(40.0);
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
                .font(ts::title()),
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

    let font = ts::title();
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
    // Attached folders wait for the Unity-style project system, so even an
    // old project's folders, kept in its file, are not listed.
    // "Add folder…" stays in the choice list and the project file. It is
    // not shown: a project has its own folder until a later project system.
    let rows = [
        item("Rename", true, false, true),
        item("Switch", !project.others.is_empty(), true, false),
        item("New Project", true, false, false),
        item("Remove Project", true, false, true),
    ];
    // Switch is the row that opens the other projects.
    let names: Vec<String> = project.others.iter().map(|(_, name)| name.clone()).collect();
    let picked = widgets::menu_rows_with(ui, &rows, |index, ui| {
        if index != 1 {
            return None;
        }
        let items: Vec<widgets::MenuItem> = names.iter().map(|name| item(name, true, false, false)).collect();
        widgets::menu_rows(ui, &items)
    });
    let choice = match picked {
        Some(widgets::MenuPick::Row(0)) => Some(ProjectChoice::Rename),
        Some(widgets::MenuPick::Row(2)) => Some(ProjectChoice::New),
        Some(widgets::MenuPick::Row(3)) => Some(ProjectChoice::Remove),
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
    let font = ts::body();
    // The box spans the conversation's column, edge to edge with the messages.
    let left = rect.left();
    let right = rect.right();
    let field_w = (right - left - 2.0 * BOX_PAD_X).max(40.0);
    let line = ui.painter().layout_no_wrap("Ag".to_string(), font.clone(), TEXT).size().y;
    let measured = ui
        .painter()
        .layout(chat.input.clone(), font.clone(), TEXT, field_w)
        .size()
        .y;
    let field_h = measured.clamp(line, line * MAX_LINES);

    // The chips, wrapped into rows above the text.
    let chip_font = ts::small();
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
        // While a request runs, Enter queues the message; the button is Stop.
        if enter && can_send(chat) {
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
    let font = ts::small();
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
                ui.add(egui::Label::new(egui::RichText::new(text).color(MUTED).size(ts::body_size())).wrap());
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
                                    ts::body(),
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
/// the right, the assistant's replies and notes on the left. A tool it called
/// is shown only when that call carries a cell.
fn conversation(
    ui: &mut egui::Ui,
    rect: Rect,
    chat: &Chat,
    board: &mut crate::cells::Board,
    actions: &mut Vec<crate::cells::CardAction>,
    events: &mut Vec<ChatEvent>,
) {
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
        // No scrollbar, as the owner set on 2026-10-28; the wheel and a drag
        // still scroll.
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .stick_to_bottom(true)
        .show(&mut talk, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let width = ui.available_width().max(80.0);
            ui.add_space(12.0);
            let turns = turns(&chat.conversation);
            let count = turns.len();
            for (index, turn) in turns.iter().enumerate() {
                if index > 0 {
                    ui.add_space(ts::TURN);
                }
                let running = index + 1 == count && chat.working;
                draw_turn(ui, index, turn, running, chat, width, board, actions, events);
            }
            if chat.working && turns.is_empty() {
                working_row(ui, chat.started, width);
            }
            for (text, chips) in &chat.queued {
                ui.add_space(ts::TURN);
                let names: Vec<String> = chips.iter().map(|path| file_name(path)).collect();
                person_bubble(ui, text, &names, width);
                caption(ui, "Queued", width, true);
            }
            ui.add_space(12.0);
        });
}

/// One message of the person's and everything the assistant did for it.
struct Turn<'a> {
    person: Option<&'a Entry>,
    rest: Vec<&'a Entry>,
}

fn turns(entries: &[Entry]) -> Vec<Turn<'_>> {
    let mut turns: Vec<Turn> = Vec::new();
    for entry in entries {
        if entry.role == Role::Person {
            turns.push(Turn {
                person: Some(entry),
                rest: Vec::new(),
            });
        } else {
            // A notice the host makes after a reply has begun, such as a file
            // left out when a project reopens, stands on its own below it.
            let replied = turns.last().is_some_and(|turn| turn.rest.iter().any(|seen| !seen.aside));
            if entry.aside && replied {
                turns.push(Turn {
                    person: None,
                    rest: vec![entry],
                });
                continue;
            }
            match turns.last_mut() {
                Some(turn) => turn.rest.push(entry),
                None => turns.push(Turn {
                    person: None,
                    rest: vec![entry],
                }),
            }
        }
    }
    turns
}

/// The words a stopped request leaves, which its time line shows instead.
use assistant::STOPPED;

#[allow(clippy::too_many_arguments)]
fn draw_turn(
    ui: &mut egui::Ui,
    index: usize,
    turn: &Turn,
    running: bool,
    chat: &Chat,
    width: f32,
    board: &mut crate::cells::Board,
    actions: &mut Vec<crate::cells::CardAction>,
    events: &mut Vec<ChatEvent>,
) {
    let mut first = true;
    let gap = |ui: &mut egui::Ui, first: &mut bool| {
        if !*first {
            ui.add_space(ts::BLOCK);
        }
        *first = false;
    };
    if let Some(person) = turn.person {
        let names: Vec<String> = person.attachments.iter().map(|path| file_name(Path::new(path))).collect();
        person_bubble(ui, &person.text, &names, width);
        first = false;
    }
    let steps = turn
        .rest
        .iter()
        .filter(|entry| entry.role == Role::Tool && entry.cell.is_none())
        .count();
    let open_id = egui::Id::new(("turn-steps", index));
    let open = running || ui.ctx().data(|data| data.get_temp::<bool>(open_id).unwrap_or(false));
    let stopped = turn.rest.iter().any(|entry| entry.role == Role::Note && entry.text == STOPPED);
    // Only the note that ended a request is a failure; an aside never is.
    let failure = (!running)
        .then(|| {
            turn.rest
                .iter()
                .rev()
                .find(|entry| !entry.aside)
                .filter(|entry| entry.role == Role::Note && entry.text != STOPPED)
        })
        .flatten();
    for entry in &turn.rest {
        match entry.role {
            Role::Assistant if !entry.text.trim().is_empty() => {
                gap(ui, &mut first);
                prose(ui, &entry.text, width, false);
            }
            Role::Tool => match &entry.cell {
                Some(cell) => {
                    gap(ui, &mut first);
                    cell_card(ui, cell, width, board, actions);
                }
                None if open => {
                    gap(ui, &mut first);
                    tool_row(ui, entry, width);
                }
                None => {}
            },
            Role::Note if entry.text != STOPPED => {
                gap(ui, &mut first);
                let failed = failure.is_some_and(|failed| std::ptr::eq(*failed, *entry));
                note(ui, &entry.text, width, failed);
            }
            _ => {}
        }
    }
    if running {
        gap(ui, &mut first);
        if chat.live.trim().is_empty() {
            working_row(ui, chat.started, width);
        } else {
            prose(ui, &chat.live, width, true);
        }
        return;
    }
    // A turn of asides alone has no reply, so no line.
    let Some(last_reply) = turn.rest.iter().rev().find(|entry| !entry.aside) else {
        return;
    };
    ui.add_space(ts::FOOTER);
    let start = turn.person.map_or(turn.rest[0].time, |person| person.time);
    let end = last_reply.time;
    footer(ui, width, Footer {
        seconds: end.saturating_sub(start),
        clock: clock(end),
        stopped,
        failed: failure.is_some(),
        steps,
        open,
        open_id,
        // Everything the reply says: its prose, its cells' results, its notices.
        copy: turn
            .rest
            .iter()
            .filter_map(|entry| match (&entry.cell, entry.role) {
                (Some(cell), _) => Some(cell.result_text()),
                (None, Role::Assistant | Role::Note) => Some(entry.text.clone()),
                _ => None,
            })
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        retry: turn.person.map(|person| person.text.clone()),
    }, events);
}

/// The person's message: a pale bubble at the right, its files as chips under it.
fn person_bubble(ui: &mut egui::Ui, text: &str, files: &[String], width: f32) {
    let painter = ui.painter().clone();
    let max = (width * 0.82).floor();
    let wrap = max - 2.0 * BUBBLE_X;
    let galley = painter.layout_job(body_job(text, ts::TEXT, wrap));
    let size = vec2((galley.size().x + 2.0 * BUBBLE_X).ceil(), (galley.size().y + 2.0 * BUBBLE_Y).ceil());
    let (row, _) = ui.allocate_exact_size(vec2(width, size.y), Sense::hover());
    let bubble = Rect::from_min_size(pos2(row.right() - size.x, row.top()), size);
    painter.rect_filled(bubble, BUBBLE_RADIUS, PERSON);
    painter.galley(pos2(bubble.left() + BUBBLE_X, bubble.top() + BUBBLE_Y), galley, ts::TEXT);
    if files.is_empty() {
        return;
    }
    ui.add_space(6.0);
    let chips: Vec<Arc<Galley>> = files
        .iter()
        .map(|name| painter.layout_no_wrap(name.clone(), ts::small(), ts::MUTED))
        .collect();
    let (row, _) = ui.allocate_exact_size(vec2(width, CHIP_ROW), Sense::hover());
    let mut right = row.right();
    for chip in chips.into_iter().rev() {
        let w = chip.size().x + 20.0;
        let rect = Rect::from_min_size(pos2(right - w, row.top()), vec2(w, CHIP_ROW));
        widgets::outlined(&painter, rect, 8.0, Color32::WHITE, ts::BORDER);
        painter.galley(pos2(rect.left() + 10.0, rect.center().y - chip.size().y / 2.0), chip, ts::MUTED);
        right = rect.left() - 6.0;
    }
}

const CHIP_ROW: f32 = 24.0;

/// One body-size text, wrapped, on the guide's line height.
fn body_job(text: &str, color: Color32, wrap: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat {
            font_id: ts::body(),
            color,
            line_height: Some(ts::body_line()),
            ..Default::default()
        },
    );
    job.wrap.max_width = wrap;
    job
}

/// The assistant's words, bare on the background, as Markdown. `caret`
/// marks the end of text still arriving.
fn prose(ui: &mut egui::Ui, text: &str, width: f32, caret: bool) {
    let painter = ui.painter().clone();
    let laid = crate::markdown::lay_out(&painter, text, ts::TEXT, width);
    let (row, _) = ui.allocate_exact_size(vec2(width, laid.height), Sense::hover());
    laid.paint(&painter, row.min);
    if caret && let Some(end) = laid.end {
        let at = row.min + end.to_vec2();
        painter.rect_filled(Rect::from_min_size(pos2(at.x + 2.0, at.y + 3.0), vec2(2.0, ts::body_line() - 6.0)), 0.0, ts::TEXT);
    }
}

/// A cell in the turn, at the column's full width.
fn cell_card(
    ui: &mut egui::Ui,
    cell: &protocol::Cell,
    width: f32,
    board: &mut crate::cells::Board,
    actions: &mut Vec<crate::cells::CardAction>,
) {
    let mut draft = board.drafts.get(&cell.id).cloned().unwrap_or_else(|| cell.prompt());
    let live = board.running(&cell.id).map(|run| run.text.clone());
    let height = crate::cells::card_height(ui, cell, &draft, live.as_deref(), live.is_some(), width, false);
    let (row, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let card = crate::cells::paint_card(ui, row, cell, &mut draft, live.as_deref(), live.is_some(), None, actions);
    board.drafts.insert(cell.id.clone(), draft);
    board.card_rect = Some(card);
}

/// One tool call: a Small row with its mark and what it did.
fn tool_row(ui: &mut egui::Ui, entry: &Entry, width: f32) {
    let painter = ui.painter().clone();
    let (row, _) = ui.allocate_exact_size(vec2(width, ts::small_line()), Sense::hover());
    let (icon, color) = match entry.ok {
        Some(true) => ("check", ts::MUTED),
        Some(false) => ("xmark", ts::FAILED),
        None => ("more-horiz", ts::MUTED),
    };
    toolbar::paint_icon(&painter, pos2(row.left() + 7.0, row.center().y), icon, 13.0, color);
    painter.text(
        pos2(row.left() + 20.0, row.center().y),
        Align2::LEFT_CENTER,
        step_words(entry.tool.as_deref().unwrap_or("")),
        ts::small(),
        color,
    );
}

/// A tool's key, `<application>__<skill>__<tool>`, as words.
fn step_words(key: &str) -> String {
    let mut parts = key.split("__");
    let app = parts.next().unwrap_or(key).replace('-', " ");
    let tool = parts.last().unwrap_or("").replace('_', " ");
    if tool.is_empty() { app } else { format!("{tool} · {app}") }
}

/// A notice in the turn. A failure that ended it is in the failure color;
/// its Retry is in the turn's line, with Copy, as for every reply.
fn note(ui: &mut egui::Ui, text: &str, width: f32, failed: bool) {
    let painter = ui.painter().clone();
    let color = if failed { ts::FAILED } else { ts::MUTED };
    let galley = painter.layout_job(small_job(text, color, width));
    let (row, _) = ui.allocate_exact_size(vec2(width, galley.size().y), Sense::hover());
    painter.galley(row.min, galley, color);
}

fn small_job(text: &str, color: Color32, wrap: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat {
            font_id: ts::small(),
            color,
            line_height: Some(ts::small_line()),
            ..Default::default()
        },
    );
    job.wrap.max_width = wrap;
    job
}

/// A muted Small word that acts when clicked.
fn action_word(ui: &mut egui::Ui, rect: Rect, word: &str, salt: &str) -> egui::Response {
    let response = ui.interact(rect, egui::Id::new((salt, rect.min.x as i32, rect.min.y as i32)), Sense::click());
    let color = if response.hovered() { ts::TEXT } else { ts::MUTED };
    ui.painter().text(pos2(rect.left(), rect.center().y), Align2::LEFT_CENTER, word, ts::small(), color);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Where the reply will appear: three dots and "Working", and after thirty
/// seconds the time so far.
fn working_row(ui: &mut egui::Ui, started: Option<u64>, width: f32) {
    let painter = ui.painter().clone();
    let (row, _) = ui.allocate_exact_size(vec2(width, ts::small_line()), Sense::hover());
    let time = ui.input(|input| input.time);
    for dot in 0..3 {
        let phase = ((time * 3.0 - dot as f64 * 0.5).sin() * 0.5 + 0.5) as f32;
        let color = ts::MUTED.gamma_multiply(0.35 + 0.65 * phase);
        widgets::disc(&painter, pos2(row.left() + 4.0 + dot as f32 * 8.0, row.center().y), 2.5, color);
    }
    let elapsed = started.map(|started| crate::workspace::now_seconds().saturating_sub(started)).unwrap_or(0);
    let words = if elapsed >= 30 { format!("Working · {}", duration(elapsed)) } else { String::from("Working") };
    painter.text(pos2(row.left() + 30.0, row.center().y), Align2::LEFT_CENTER, words, ts::small(), ts::MUTED);
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
}

/// A muted Small caption, such as "Queued", under a message.
fn caption(ui: &mut egui::Ui, words: &str, width: f32, right: bool) {
    ui.add_space(4.0);
    let (row, _) = ui.allocate_exact_size(vec2(width, ts::small_line()), Sense::hover());
    let (x, align) = if right { (row.right(), Align2::RIGHT_CENTER) } else { (row.left(), Align2::LEFT_CENTER) };
    ui.painter().text(pos2(x, row.center().y), align, words, ts::small(), ts::MUTED);
}

struct Footer {
    seconds: u64,
    clock: String,
    stopped: bool,
    failed: bool,
    steps: usize,
    open: bool,
    open_id: egui::Id,
    copy: String,
    retry: Option<String>,
}

/// The finished turn's line: how long it worked, or that it stopped, and
/// when; its steps, folded; then Copy and Retry.
fn footer(ui: &mut egui::Ui, width: f32, footer: Footer, events: &mut Vec<ChatEvent>) {
    let painter = ui.painter().clone();
    let (row, _) = ui.allocate_exact_size(vec2(width, ts::small_line()), Sense::hover());
    let lead = if footer.stopped {
        String::from("Stopped")
    } else if footer.failed {
        String::from("Failed")
    } else {
        format!("Worked for {}", duration(footer.seconds))
    };
    let mut words = format!("{lead} · {}", footer.clock);
    if footer.steps > 0 {
        words.push_str(&format!(" · {} step{}", footer.steps, if footer.steps == 1 { "" } else { "s" }));
    }
    let galley = painter.layout_no_wrap(words, ts::small(), ts::MUTED);
    let line = Rect::from_min_size(row.min, vec2(galley.size().x, ts::small_line()));
    painter.galley(pos2(row.left(), row.center().y - galley.size().y / 2.0), galley, ts::MUTED);
    let mut x = line.right() + 6.0;
    if footer.steps > 0 {
        let chevron = Rect::from_min_size(pos2(x, row.top()), vec2(16.0, ts::small_line()));
        let icon = if footer.open { "nav-arrow-up" } else { "nav-arrow-down" };
        toolbar::paint_icon(&painter, chevron.center(), icon, 12.0, ts::MUTED);
        let toggle = ui.interact(line.union(chevron), footer.open_id.with("toggle"), Sense::click());
        if toggle.clicked() {
            ui.ctx().data_mut(|data| data.insert_temp(footer.open_id, !footer.open));
        }
        x = chevron.right() + 6.0;
    }
    // Copy and Retry show under every finished reply, as the owner set on
    // 2026-10-28, so they are always where the person looks for them.
    x += 10.0;
    if !footer.copy.trim().is_empty() {
        let copy = Rect::from_min_size(pos2(x, row.top()), vec2(34.0, ts::small_line()));
        if action_word(ui, copy, "Copy", "turn-copy").clicked() {
            ui.ctx().copy_text(footer.copy.clone());
        }
        x = copy.right() + 10.0;
    }
    if let Some(retry) = footer.retry {
        let again = Rect::from_min_size(pos2(x, row.top()), vec2(36.0, ts::small_line()));
        if action_word(ui, again, "Retry", "turn-retry").clicked() {
            events.push(ChatEvent::Retry(retry));
        }
    }
}

/// Seconds as "12s" or "1m 5s".
fn duration(seconds: u64) -> String {
    if seconds < 60 { format!("{seconds}s") } else { format!("{}m {}s", seconds / 60, seconds % 60) }
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

    /// Catches a turn split or merged wrongly: everything the assistant did
    /// for a message belongs to that message's turn, in order.
    #[test]
    fn entries_group_into_turns() {
        let entry = |role: Role, text: &str| Entry {
            role,
            text: text.into(),
            time: 0,
            attachments: Vec::new(),
            tool: None,
            ok: None,
            cell: None,
            aside: false,
        };
        let entries = vec![
            entry(Role::Note, "Restored."),
            entry(Role::Person, "Draw it."),
            entry(Role::Assistant, "Drawing."),
            entry(Role::Tool, ""),
            entry(Role::Assistant, "Done."),
            entry(Role::Person, "Again."),
            entry(Role::Note, "Stopped."),
        ];
        let turns = turns(&entries);
        assert_eq!(turns.len(), 3);
        assert!(turns[0].person.is_none() && turns[0].rest.len() == 1);
        assert_eq!(turns[1].person.unwrap().text, "Draw it.");
        assert_eq!(turns[1].rest.iter().map(|e| e.role).collect::<Vec<_>>(), [Role::Assistant, Role::Tool, Role::Assistant]);
        assert_eq!(turns[2].rest[0].text, STOPPED);

        // A notice the host makes later stands on its own, so the reply
        // above it is not marked failed.
        let mut later = entry(Role::Note, "“kept.smx” was left out: it no longer exists.");
        later.aside = true;
        let entries = vec![entry(Role::Person, "Draw it."), entry(Role::Assistant, "Done."), later];
        let split = super::turns(&entries);
        assert_eq!(split.len(), 2);
        assert_eq!(split[0].rest.len(), 1);
        assert!(split[1].person.is_none() && split[1].rest[0].aside);
    }

    #[test]
    fn the_clock_reads_like_the_reference() {
        assert_eq!(utc_hour_minute(10 * 3600 + 24 * 60), (10, 24));
        let shown = clock(1_760_000_000);
        assert!(shown.ends_with(" AM") || shown.ends_with(" PM"), "{shown}");
    }

    #[test]
    /// Catches a panel that grows and shrinks with the window: it keeps one
    /// width, and gives some back only when the window is too narrow.
    fn the_panel_keeps_its_width() {
        assert_eq!(width(1600.0, None), PANEL_WIDTH);
        assert_eq!(width(2400.0, None), PANEL_WIDTH);
        assert_eq!(width(700.0, None), 350.0);
        // Dragged only a little either way.
        assert_eq!(width(1600.0, Some(480.0)), 480.0);
        assert_eq!(width(1600.0, Some(900.0)), PANEL_MAX);
        assert_eq!(width(1600.0, Some(100.0)), PANEL_MIN);
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
