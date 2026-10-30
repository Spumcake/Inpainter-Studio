//! Every cell the host draws. A sub-application that keeps cells stores them
//! and never draws them. A run's growing text lives here until it finishes.

use crate::toolbar;
use crate::typescale as ts;
use crate::widgets::{self, BORDER, MUTED, TEXT};
use eframe::egui::{self, Align2, Color32, Rect, Sense, pos2, vec2};
use protocol::{Cell, CellPart, CellState};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};

const CARD_RADIUS: f32 = 12.0;

/// What a card asked for while it was drawn.
#[derive(Debug)]
pub(crate) enum CardAction {
    /// The prompt lost focus and differs from the stored one.
    Edit(String, Vec<CellPart>),
    Run(String),
    Stop(String),
    Select(String),
    /// The person started dragging this cell.
    Drag(Cell),
    /// The card's "…" was clicked, at this place.
    More(String, Rect),
    /// The card's chevron was clicked, at this place: choose its model.
    Models(String, Rect),
    /// A filmstrip thumbnail was clicked: show that version.
    ShowVersion(String, usize),
    /// A rendered Markdown cell was double-clicked: show its source.
    ShowSource(String),
}

pub(crate) enum Piece {
    Text(String),
    /// A picture's PNG, from an Image cell's run.
    Picture(Vec<u8>),
    End,
    Failed(String),
}

pub(crate) enum Place {
    Conversation,
    Book { app: usize, tab: String },
}

pub(crate) struct Run {
    pub place: Place,
    pub id: String,
    /// The assistant call this conversation cell answers, when it has one.
    pub call: Option<String>,
    /// The endpoint id and model it runs on, recorded on the cell.
    pub endpoint: String,
    pub model: String,
    pub effort: Option<String>,
    /// The picture an Image cell's run made: its path, as the cell names
    /// it, and its PNG, for the assistant to see.
    pub picture: Option<(String, Vec<u8>)>,
    /// The prompt as it was when the run began: the version the run makes
    /// holds this, whatever is typed meanwhile.
    pub prompt: String,
    pub text: String,
    pub rx: Receiver<Piece>,
    /// Set when the run has reported its end and the host has answered it.
    pub settled: bool,
}

/// Runs in progress, and the prompts being typed.
pub(crate) struct Board {
    pub drafts: HashMap<String, String>,
    /// The prompt last reported for a book cell, so a typed draft is not
    /// replaced by the copy the book has not echoed yet.
    pub committed: HashMap<String, String>,
    pub runs: Vec<Run>,
    /// The conversation card last drawn, so a check can drag from it.
    pub card_rect: Option<Rect>,
    /// The cells area last drawn, so a check can drop on it.
    pub drop_rect: Option<Rect>,
    /// Each notebook card last drawn, by cell id, so a check can press its
    /// buttons.
    pub cards: Vec<(String, Rect)>,
    /// The cell drawn with the blue border last frame, if any.
    pub bordered: Option<String>,
    /// The open cell menu's buttons last frame, by name, so a check can
    /// press them.
    pub menu_buttons: Vec<(&'static str, Rect)>,
    /// A notebook cell's level chosen from its chevron, by cell id: the
    /// endpoint id, model, and effort its next run uses. The run's result
    /// records them.
    pub chosen: HashMap<String, (String, String, Option<String>)>,
}

impl Default for Board {
    fn default() -> Self {
        Self {
            drafts: HashMap::new(),
            committed: HashMap::new(),
            runs: Vec::new(),
            card_rect: None,
            drop_rect: None,
            cards: Vec::new(),
            bordered: None,
            menu_buttons: Vec::new(),
            chosen: HashMap::new(),
        }
    }
}

impl Board {
    pub(crate) fn running(&self, id: &str) -> Option<&Run> {
        self.runs.iter().find(|run| run.id == id && !run.settled)
    }

    /// Takes one piece of one run. The window applies it and, on the last
    /// piece, answers the cell's owner.
    pub(crate) fn poll(&mut self) -> Option<(usize, Piece)> {
        for (index, run) in self.runs.iter_mut().enumerate() {
            if run.settled {
                continue;
            }
            if let Ok(piece) = run.rx.try_recv() {
                if let Piece::Text(text) = &piece {
                    run.text.push_str(text);
                }
                return Some((index, piece));
            }
        }
        None
    }
}

/// Starts a text run on `route`, or fails at once with the reason there is
/// none. Its pieces arrive on the run's channel as they are read.
pub(crate) fn start_run(route: Result<crate::endpoints::Route, String>, prompt: &str, picture: bool) -> Receiver<Piece> {
    let (tx, rx) = channel();
    let route = match route {
        Ok(route) => match route.blocked() {
            Some(reason) => Err(reason),
            None => Ok(route),
        },
        Err(reason) => Err(reason),
    };
    let route = match route {
        Ok(route) => route,
        Err(reason) => {
            let _ = tx.send(Piece::Failed(reason));
            return rx;
        }
    };
    let prompt = prompt.to_string();
    let _ = std::thread::Builder::new().name(String::from("cell-run")).spawn(move || {
        if picture {
            picture_into(route, prompt, tx);
        } else {
            stream_into(route, prompt, tx);
        }
    });
    rx
}

/// An Image cell's run: one picture, then the end, or why it failed.
fn picture_into(route: crate::endpoints::Route, prompt: String, tx: Sender<Piece>) {
    let key = route.key.clone().unwrap_or_default();
    let _ = match assistant::make_image(&route.base, &key, &route.model, &prompt) {
        Ok(png) => tx.send(Piece::Picture(png)).and_then(|_| tx.send(Piece::End)),
        Err(reason) => tx.send(Piece::Failed(reason)),
    };
}

/// Each piece goes to the window as soon as it is read. When the window
/// drops the run, as Stop does, the next send fails and the stream ends.
fn stream_into(route: crate::endpoints::Route, prompt: String, tx: Sender<Piece>) {
    let mut open = true;
    let key = route.key.clone().unwrap_or_default();
    let extra = route.extra();
    let result = assistant::stream_prompt(&route.base, &key, &route.model, &prompt, &extra, |text: &str| {
        open = tx.send(Piece::Text(text.to_string())).is_ok();
        open
    });
    if !open {
        return;
    }
    let _ = tx.send(match result {
        Ok(()) => Piece::End,
        Err(reason) => Piece::Failed(reason),
    });
}

/// The word a cell's kind is drawn as. A text cell is "LLM", as the owner
/// named it on 2026-10-27; anything else is the kind as stored, including a
/// kind this build does not run.
pub(crate) fn kind_label(kind: &str) -> &str {
    match kind {
        protocol::TEXT_KIND => "LLM",
        protocol::IMAGE_KIND => "Image",
        protocol::MARKDOWN_KIND => "Markdown",
        other => other,
    }
}

/// Kinds this build can run: an LLM cell calls its model; a Markdown cell
/// shows its source rendered, calling nothing.
pub(crate) fn runs(kind: &str) -> bool {
    kind == protocol::TEXT_KIND || kind == protocol::MARKDOWN_KIND || kind == protocol::IMAGE_KIND
}

/// An Image cell's picture: the height it is drawn, and the filmstrip's.
const PICTURE_H: f32 = 300.0;

/// How tall a cell's picture is drawn: at most `PICTURE_H`, and less when
/// the card's width limits it, so no room is left empty under it.
fn picture_height(ctx: &egui::Context, path: &str, width: f32) -> f32 {
    picture_texture(ctx, path).map_or(PICTURE_H, |texture| fit(texture.size_vec2(), width, PICTURE_H).y)
}
const FILM_H: f32 = 52.0;
const THUMB_GAP: f32 = 8.0;
/// The filmstrip's outline around the version shown.
const SHOWN: Color32 = Color32::from_rgb(37, 99, 235);

/// The picture a part names, if it is one.
fn picture_path(parts: &[CellPart]) -> Option<&str> {
    parts.iter().find_map(|part| match part {
        CellPart::Path { path, media_type } if media_type.starts_with("image/") => Some(path.as_str()),
        _ => None,
    })
}

/// The picture a cell shows, when it has one and is not running or failed.
fn shown_picture<'a>(cell: &'a Cell, running: bool) -> Option<&'a str> {
    if running || cell.state == CellState::Failed {
        return None;
    }
    picture_path(&cell.output)
}

/// The versions with a picture, by index, for the filmstrip, which shows
/// from the first version on, as the outcome says.
fn film(cell: &Cell) -> Option<Vec<(usize, &str)>> {
    let pictures: Vec<(usize, &str)> = cell
        .versions
        .iter()
        .enumerate()
        .filter_map(|(index, version)| picture_path(&version.output).map(|path| (index, path)))
        .collect();
    (!pictures.is_empty()).then_some(pictures)
}

/// Where pictures named by relative paths are: the project's folder, set
/// by the window each frame.
pub(crate) fn set_picture_root(ctx: &egui::Context, root: Option<std::path::PathBuf>) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("cell-picture-root"), root));
}

fn resolve(ctx: &egui::Context, path: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(path);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let root: Option<std::path::PathBuf> = ctx.data(|data| data.get_temp(egui::Id::new("cell-picture-root"))).flatten();
    root.map_or_else(|| path.to_path_buf(), |root| root.join(path))
}

/// A picture as a texture, decoded once per file. None when it cannot be
/// read, as when it was deleted.
fn picture_texture(ctx: &egui::Context, path: &str) -> Option<egui::TextureHandle> {
    type Cache = HashMap<std::path::PathBuf, Option<egui::TextureHandle>>;
    let file = resolve(ctx, path);
    let id = egui::Id::new("cell-pictures");
    if let Some(Some(known)) = ctx.data(|data| data.get_temp::<Cache>(id).and_then(|cache| cache.get(&file).cloned())) {
        return Some(known);
    }
    let texture = std::fs::read(&file)
        .ok()
        .and_then(|bytes| eframe::icon_data::from_png_bytes(&bytes).ok())
        .map(|icon| {
            let image = egui::ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);
            ctx.load_texture(file.display().to_string(), image, egui::TextureOptions::LINEAR)
        });
    // Only a picture read is kept: one not there yet is read again later.
    if let Some(texture) = &texture {
        ctx.data_mut(|data| {
            data.get_temp_mut_or_default::<Cache>(id).insert(file, Some(texture.clone()));
        });
    }
    texture
}

/// `size` scaled to fit `height` high and `width` wide.
fn fit(size: egui::Vec2, width: f32, height: f32) -> egui::Vec2 {
    let scale = (height / size.y).min(width / size.x).min(f32::MAX);
    size * scale
}

/// A Markdown cell that has run shows as rendered text, not a card.
pub(crate) fn rendered(cell: &Cell) -> bool {
    cell.kind == protocol::MARKDOWN_KIND && cell.state == CellState::Done
}

const PAD: f32 = 16.0;
const HEADER: f32 = 52.0;
const TILE: f32 = 28.0;
const BUTTON_H: f32 = 30.0;
const GUTTER: f32 = 30.0;
/// The prompt box is as tall as its text, as the owner set on 2026-10-28.
const MIN_LINES: usize = 1;
const OUTPUT_PAD: f32 = 14.0;
const TILE_FILL: Color32 = Color32::from_rgb(243, 244, 246);
const HAIRLINE: Color32 = ts::HAIRLINE;
const DISABLED: Color32 = Color32::from_rgb(170, 174, 180);

/// Every height a card needs, so the card and its list agree.
struct Layout {
    input: f32,
    output: Option<crate::markdown::Laid>,
    /// An Image cell's picture, and whether a filmstrip is below it.
    picture: bool,
    film: bool,
    total: f32,
}

/// A line number, laid out on the same line height as the prompt so it
/// shares its line's baseline, right-aligned to `right`, with its line's top
/// at `top`.
fn number_at(painter: &egui::Painter, right: f32, top: f32, number: usize) {
    let mut job = prompt_job(&number.to_string(), f32::INFINITY, false);
    job.sections[0].format.color = MUTED;
    let galley = painter.layout_job(job);
    painter.galley(pos2(right - galley.size().x, top), galley, MUTED);
}

/// The prompt laid out as the text box shows it: Hack 13 on 20, wrapped.
/// A Markdown cell's source is coloured as Jupyter colours it.
pub(crate) fn prompt_job(text: &str, width: f32, markdown: bool) -> egui::text::LayoutJob {
    let format = |token: crate::markdown::Token| {
        use crate::markdown::Token;
        let mut format = egui::TextFormat {
            font_id: ts::code(),
            color: TEXT,
            line_height: Some(ts::code_line()),
            ..Default::default()
        };
        match token {
            Token::Plain => {}
            // No face draws these; drawn tiny and clear, they take no room
            // and the cursor still counts them.
            Token::Hidden => {
                format.font_id.size = 0.01;
                format.color = Color32::TRANSPARENT;
            }
            Token::Link => {
                format.color = crate::markdown::token_color(token);
                format.underline = egui::Stroke::new(1.0, format.color);
            }
            _ => format.color = crate::markdown::token_color(token),
        }
        format
    };
    let mut job = egui::text::LayoutJob::default();
    let spans = if markdown {
        crate::markdown::source_tokens(text)
    } else {
        crate::markdown::source_tokens_plain(text)
    };
    for (range, token) in spans {
        job.append(&text[range], 0.0, format(token));
    }
    if job.sections.is_empty() {
        job.append("", 0.0, format(crate::markdown::Token::Plain));
    }
    job.wrap.max_width = width;
    job
}

fn layout(ui: &egui::Ui, cell: &Cell, draft: &str, live: Option<&str>, running: bool, inner_w: f32) -> Layout {
    let painter = ui.painter();
    let markdown = cell.kind == protocol::MARKDOWN_KIND;
    let text = painter.layout_job(prompt_job(draft, inner_w - GUTTER - 12.0, markdown)).size().y;
    let input = text.max(ts::code_line() * MIN_LINES as f32) + 24.0;
    let picture_h = shown_picture(cell, running).map(|path| picture_height(ui.ctx(), path, inner_w));
    let picture = picture_h.is_some();
    // The filmstrip stays while a picture is made, and after a run fails or
    // is stopped, so earlier versions are always in reach.
    let film = cell.kind == protocol::IMAGE_KIND && film(cell).is_some();
    let output = if picture {
        None
    } else {
        output_text(cell, live, running).map(|(text, color)| crate::markdown::lay_out(painter, &text, color, inner_w))
    };
    let shown = if picture {
        2.0 * OUTPUT_PAD + picture_h.unwrap_or(PICTURE_H)
    } else {
        output.as_ref().map_or(0.0, |output| output.height + 2.0 * OUTPUT_PAD)
    };
    // The filmstrip sits at the card's foot, one output pad below what is
    // shown above it.
    let below = shown + if !film { 0.0 } else if shown > 0.0 { FILM_H + OUTPUT_PAD } else { FILM_H + 2.0 * OUTPUT_PAD };
    Layout {
        input,
        total: HEADER + input + below,
        output,
        picture,
        film,
    }
}

/// The result as Markdown, the reason a run failed, or the words that say
/// a run has begun, with its color. None when the cell has not run.
fn output_text(cell: &Cell, live: Option<&str>, running: bool) -> Option<(String, Color32)> {
    if running {
        let text = live.unwrap_or("");
        if cell.kind == protocol::IMAGE_KIND {
            return Some((String::from("Making the picture…"), MUTED));
        }
        if text.is_empty() {
            return Some((String::from("Writing…"), MUTED));
        }
        return Some((text.to_string(), TEXT));
    }
    if cell.state == CellState::Failed {
        return Some((cell.error.clone().unwrap_or_else(|| String::from("The run failed.")), ts::FAILED));
    }
    let parts: Vec<String> = cell
        .output
        .iter()
        .map(|part| match part {
            CellPart::Text { text } => text.clone(),
            CellPart::Path { path, .. } => path.clone(),
            CellPart::Other(value) => value.to_string(),
        })
        .collect();
    let text = parts.join("\n");
    (!text.trim().is_empty()).then_some((text, TEXT))
}

/// Draws one cell into `row` and returns the card. `number` is the margin
/// mark a book shows, such as 1 for "[1]". The card follows the approved
/// reference, `llm-cell.png`: a header with the kind, the model, Generate
/// and its menu; the prompt as a text box with line numbers; and the result
/// below, as Markdown.
pub(crate) fn paint_card(
    ui: &mut egui::Ui,
    row: Rect,
    cell: &Cell,
    draft: &mut String,
    live: Option<&str>,
    running: bool,
    number: Option<usize>,
    actions: &mut Vec<CardAction>,
) -> Rect {
    // The card's own area: its prompt box and buttons are placed inside it,
    // so they never move the list's cursor back up into the card.
    let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(row));
    let painter = ui.painter().clone();
    let margin = if number.is_some() { 36.0 } else { 0.0 };
    if let Some(number) = number {
        painter.text(
            pos2(row.left(), row.top() + 18.0),
            Align2::LEFT_TOP,
            format!("[{number}]"),
            ts::code(),
            MUTED,
        );
    }
    let card = Rect::from_min_max(pos2(row.left() + margin, row.top()), row.max);
    let inner_w = (card.width() - 2.0 * PAD).max(60.0);
    let plan = layout(ui, cell, draft, live, running, inner_w);
    widgets::outlined(&painter, card, CARD_RADIUS, Color32::WHITE, BORDER);
    // A click anywhere on the card selects it. It is registered first, so
    // the buttons and the prompt drawn over it take their own clicks.
    let clicked = ui.interact(card, egui::Id::new(("cell-select", &cell.id)), Sense::click());
    if clicked.clicked() {
        actions.push(CardAction::Select(cell.id.clone()));
    }

    // Header: the kind's tile, its name, the model, Generate, its menu, more.
    let mid = card.top() + HEADER / 2.0;
    let tile = Rect::from_min_size(pos2(card.left() + PAD, mid - TILE / 2.0), vec2(TILE, TILE));
    painter.rect_filled(tile, 6.0, TILE_FILL);
    let tile_icon = if cell.kind == protocol::IMAGE_KIND { "media-image" } else { "align-left" };
    toolbar::paint_icon(&painter, tile.center(), tile_icon, 16.0, TEXT);
    let title = painter.text(
        pos2(tile.right() + 12.0, mid),
        Align2::LEFT_CENTER,
        kind_label(&cell.kind),
        ts::title(),
        TEXT,
    );
    if let Some(model) = cell.model.as_deref().filter(|model| !model.is_empty()) {
        let model = model_name(ui.ctx(), model);
        // The name stops short of the buttons, however narrow the card.
        let buttons = run_button(card).left() - 10.0;
        painter.with_clip_rect(Rect::from_min_max(pos2(title.right(), card.top()), pos2(buttons, card.top() + HEADER))).text(
            pos2(title.right() + 14.0, mid),
            Align2::LEFT_CENTER,
            model,
            ts::small(),
            MUTED,
        );
    }
    let more = more_button(card);
    toolbar::paint_icon(&painter, more.center(), "more-horiz", 16.0, TEXT);
    if ui.interact(more, egui::Id::new(("cell-more", &cell.id)), Sense::click()).clicked() {
        actions.push(CardAction::More(cell.id.clone(), more));
    }
    // The chevron chooses the model an LLM cell runs on, as the owner set
    // on 2026-10-29.
    let menu = Rect::from_min_size(pos2(more.left() - 10.0 - BUTTON_H, mid - BUTTON_H / 2.0), vec2(BUTTON_H, BUTTON_H));
    let chooses = cell.kind == protocol::TEXT_KIND && !running;
    let chevron = ui
        .interact(menu, egui::Id::new(("cell-models", &cell.id)), Sense::click())
        .on_hover_text("Model");
    let fill = if chooses && chevron.hovered() { widgets::SELECTED } else { Color32::WHITE };
    widgets::outlined(&painter, menu, 7.0, fill, BORDER);
    toolbar::paint_icon(&painter, menu.center(), "nav-arrow-down", 14.0, if chooses { TEXT } else { DISABLED });
    if chooses && chevron.clicked() {
        actions.push(CardAction::Models(cell.id.clone(), menu));
    }

    let can_run = runs(&cell.kind) && !draft.trim().is_empty() && !running;
    let enabled = running || can_run;
    // Every cell runs from play, as the owner set on 2026-10-28; while it
    // runs, play is stop.
    let (icon, word) = if running { ("square", "Stop") } else { ("play", "Run") };
    let ink = if enabled { TEXT } else { DISABLED };
    let button = run_button(card);
    widgets::outlined(&painter, button, 7.0, Color32::WHITE, BORDER);
    toolbar::paint_icon(&painter, button.center(), icon, 15.0, ink);
    let pressed = ui
        .interact(button, egui::Id::new(("cell-generate", &cell.id)), Sense::click())
        .on_hover_text(word);
    if pressed.clicked() && enabled {
        actions.push(if running {
            CardAction::Stop(cell.id.clone())
        } else {
            CardAction::Run(cell.id.clone())
        });
    }
    let header_line = card.top() + HEADER;
    painter.hline(card.left() + PAD..=card.right() - PAD, header_line, egui::Stroke::new(1.0, HAIRLINE));

    // The header's free part is the drag handle.
    let handle = Rect::from_min_max(pos2(card.left(), card.top()), pos2(button.left() - 8.0, header_line));
    let dragged = ui.interact(handle, egui::Id::new(("cell-drag", &cell.id)), Sense::drag());
    if dragged.drag_started() {
        actions.push(CardAction::Drag(shown(cell, live)));
    }

    // Input: the prompt, as a text box with line numbers.
    let input = Rect::from_min_size(pos2(card.left() + PAD, header_line + 12.0), vec2(inner_w, plan.input - 24.0));
    let text_left = input.left() + GUTTER + 12.0;
    let markdown = cell.kind == protocol::MARKDOWN_KIND;
    let numbers = painter.layout_job(prompt_job(draft, input.right() - text_left, markdown));
    let mut line = 1;
    let mut starts = true;
    for row in &numbers.rows {
        if starts {
            number_at(&painter, input.left() + GUTTER - 6.0, input.top() + row.min_y(), line);
            line += 1;
        }
        starts = row.ends_with_newline;
    }
    if numbers.rows.is_empty() || draft.is_empty() {
        number_at(&painter, input.left() + GUTTER - 6.0, input.top(), 1);
    }
    painter.vline(input.left() + GUTTER + 2.0, input.top()..=input.bottom(), egui::Stroke::new(1.0, HAIRLINE));
    let field = Rect::from_min_max(pos2(text_left, input.top()), input.max);
    let response = ui.put(
        field,
        egui::TextEdit::multiline(draft)
            .id(egui::Id::new(("cell-prompt", &cell.id)))
            // A kind this build cannot run is shown, never edited.
            .interactive(runs(&cell.kind))
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .desired_width(field.width())
            .layouter(&mut |ui: &egui::Ui, text: &dyn egui::TextBuffer, width: f32| {
                ui.fonts_mut(|fonts| fonts.layout_job(prompt_job(text.as_str(), width, markdown)))
            }),
    );
    if response.lost_focus() {
        let stored = cell.prompt();
        if draft != &stored {
            actions.push(CardAction::Edit(
                cell.id.clone(),
                vec![CellPart::Text { text: draft.clone() }],
            ));
        }
    }

    // Output: the result, on the card's white, below a hairline.
    let has_output = plan.output.is_some();
    if let Some(output) = plan.output {
        let top = header_line + plan.input;
        painter.hline(card.left() + PAD..=card.right() - PAD, top, egui::Stroke::new(1.0, HAIRLINE));
        output.paint(&painter, pos2(card.left() + PAD, top + OUTPUT_PAD));
    }
    // An Image cell's picture, centred, and under it the filmstrip of its
    // versions, as the approved image-cell mockup shows.
    if plan.picture
        && let Some(path) = shown_picture(cell, running)
    {
        let top = header_line + plan.input;
        painter.hline(card.left() + PAD..=card.right() - PAD, top, egui::Stroke::new(1.0, HAIRLINE));
        let area = Rect::from_min_size(pos2(card.left() + PAD, top + OUTPUT_PAD), vec2(inner_w, picture_height(ui.ctx(), path, inner_w)));
        match picture_texture(ui.ctx(), path) {
            Some(texture) => {
                let size = fit(texture.size_vec2(), area.width(), area.height());
                let rect = Rect::from_center_size(pos2(area.center().x, area.top() + size.y / 2.0), size);
                paint_rounded(&painter, rect, &texture, 8.0);
            }
            None => {
                painter.text(area.center(), Align2::CENTER_CENTER, "The picture could not be read.", ts::small(), MUTED);
            }
        }
    }
    if plan.film
        && let Some(film) = film(cell)
    {
        {
            // Under the picture, or under the words of a run that is making
            // one or failed.
            if !plan.picture && !has_output {
                let top = header_line + plan.input;
                painter.hline(card.left() + PAD..=card.right() - PAD, top, egui::Stroke::new(1.0, HAIRLINE));
            }
            let shown = cell.current.unwrap_or(cell.versions.len().saturating_sub(1));
            let thumbs: Vec<(usize, Option<egui::TextureHandle>)> =
                film.iter().map(|(index, path)| (*index, picture_texture(ui.ctx(), path))).collect();
            let width = |texture: &Option<egui::TextureHandle>| {
                texture.as_ref().map_or(FILM_H, |texture| fit(texture.size_vec2(), f32::MAX, FILM_H).x)
            };
            let total: f32 = thumbs.iter().map(|(_, texture)| width(texture)).sum::<f32>() + THUMB_GAP * (thumbs.len() - 1) as f32;
            let mut x = card.center().x - total / 2.0;
            let y = card.bottom() - OUTPUT_PAD - FILM_H;
            for (index, texture) in thumbs {
                let rect = Rect::from_min_size(pos2(x, y), vec2(width(&texture), FILM_H));
                x = rect.right() + THUMB_GAP;
                match &texture {
                    Some(texture) => paint_rounded(&painter, rect, texture, 5.0),
                    None => {
                        painter.rect_filled(rect, 5.0, TILE_FILL);
                    }
                }
                if index == shown {
                    widgets::outlined(&painter, rect.expand(3.0), 7.0, Color32::TRANSPARENT, SHOWN);
                    widgets::outlined(&painter, rect.expand(2.0), 7.0, Color32::TRANSPARENT, SHOWN);
                }
                let thumb = ui
                    .interact(rect, egui::Id::new(("cell-version", &cell.id, index)), Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(format!("Version {}", index + 1));
                if thumb.clicked() && index != shown && !running {
                    actions.push(CardAction::ShowVersion(cell.id.clone(), index));
                }
            }
        }
    }
    card
}

/// A picture in `rect`, its corners rounded by `radius`.
fn paint_rounded(painter: &egui::Painter, rect: Rect, texture: &egui::TextureHandle, radius: f32) {
    painter.add(
        egui::epaint::RectShape::filled(rect, radius, Color32::WHITE)
            .with_texture(texture.id(), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
    );
}

/// The names the model picker shows, by model id, set by the window each
/// frame so a card can show a model's name rather than its id.
pub(crate) fn set_model_names(ctx: &egui::Context, names: &[(String, String)]) {
    let names: HashMap<String, String> = names.iter().cloned().collect();
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("cell-model-names"), names));
}

/// A model's name from the picker's list, or its id without the provider
/// when the list does not hold it.
fn model_name(ctx: &egui::Context, id: &str) -> String {
    let known = ctx.data(|data| {
        data.get_temp::<HashMap<String, String>>(egui::Id::new("cell-model-names"))
            .and_then(|names| names.get(id).cloned())
    });
    known.unwrap_or_else(|| id.rsplit('/').next().unwrap_or(id).to_string())
}

/// Where a card's "…" is, from the card's rect.
pub(crate) fn more_button(card: Rect) -> Rect {
    Rect::from_center_size(pos2(card.right() - PAD - 8.0, card.top() + HEADER / 2.0), vec2(22.0, 22.0))
}

/// Where a card's play button is, from the card's rect.
pub(crate) fn run_button(card: Rect) -> Rect {
    let mid = card.top() + HEADER / 2.0;
    let more_left = card.right() - PAD - 8.0 - 11.0;
    let menu_left = more_left - 10.0 - BUTTON_H;
    Rect::from_min_size(pos2(menu_left - 6.0 - BUTTON_H, mid - BUTTON_H / 2.0), vec2(BUTTON_H, BUTTON_H))
}

/// A rendered Markdown cell: its source as rich text, with no card. A
/// double-click shows its source; a click selects it.
pub(crate) fn rendered_height(ui: &egui::Ui, cell: &Cell, width: f32) -> f32 {
    crate::markdown::lay_out(ui.painter(), &cell.prompt(), TEXT, (width - 2.0 * PAD).max(60.0)).height + 2.0 * RENDERED_PAD
}

pub(crate) fn paint_rendered(ui: &mut egui::Ui, row: Rect, cell: &Cell, actions: &mut Vec<CardAction>) -> Rect {
    let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(row));
    let laid = crate::markdown::lay_out(ui.painter(), &cell.prompt(), TEXT, (row.width() - 2.0 * PAD).max(60.0));
    laid.paint(ui.painter(), pos2(row.left() + PAD, row.top() + RENDERED_PAD));
    let response = ui.interact(row, egui::Id::new(("cell-rendered", &cell.id)), Sense::click());
    if response.double_clicked() {
        actions.push(CardAction::ShowSource(cell.id.clone()));
    } else if response.clicked() {
        actions.push(CardAction::Select(cell.id.clone()));
    }
    row
}

const RENDERED_PAD: f32 = 12.0;

/// The height a card needs at `width`, with its prompt and result laid out,
/// so a list gives each card its own height. `numbered` leaves the margin a
/// book's "[1]" takes.
pub(crate) fn card_height(
    ui: &egui::Ui,
    cell: &Cell,
    draft: &str,
    live: Option<&str>,
    running: bool,
    width: f32,
    numbered: bool,
) -> f32 {
    let margin = if numbered { 36.0 } else { 0.0 };
    let inner_w = (width - margin - 2.0 * PAD).max(60.0);
    layout(ui, cell, draft, live, running, inner_w).total
}

fn shown(cell: &Cell, live: Option<&str>) -> Cell {
    let mut copy = cell.clone();
    if let Some(live) = live {
        copy.output = vec![CellPart::Text { text: live.to_string() }];
    }
    copy
}


#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Cell {
        let mut cell = Cell::text("same-cell", "A lighthouse keeper.");
        cell.output = vec![CellPart::Text {
            text: "He kept the light.".into(),
        }];
        cell.state = CellState::Done;
        cell.endpoint = Some("openrouter".into());
        cell.model = Some("stand-in/model".into());
        cell
    }

    /// One frame painting `cell` in a card `width` wide, sized as a list
    /// sizes it. Returns the shapes, what the person did, and the card.
    fn paint(
        ctx: &egui::Context,
        cell: &Cell,
        width: f32,
        events: Vec<egui::Event>,
    ) -> (Vec<egui::epaint::ClippedShape>, Vec<CardAction>, Rect) {
        let mut draft = cell.prompt();
        let mut actions = Vec::new();
        let mut card = Rect::NOTHING;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 900.0))),
                events,
                ..Default::default()
            },
            |ui| {
                let height = card_height(ui, cell, &draft, None, false, width, true);
                let row = Rect::from_min_size(pos2(10.0, 10.0), vec2(width, height));
                card = paint_card(ui, row, cell, &mut draft, None, false, Some(1), &mut actions);
            },
        );
        output.textures_delta.clear();
        (output.shapes, actions, card)
    }

    fn texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, Rect)> {
        shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some((text.galley.text().to_string(), text.visual_bounding_rect())),
                _ => None,
            })
            .collect()
    }

    /// Catches a result drawn on one line, or a card too short for it: a
    /// long piece of writing wraps inside the card, and the card grows.
    #[test]
    fn a_long_result_wraps_inside_its_card() {
        let ctx = egui::Context::default();
        crate::toolbar::install_fonts(&ctx);
        let mut cell = sample();
        let long = "He kept the light through the storm, and the storm kept him. ".repeat(7);
        cell.output = vec![CellPart::Text { text: long.clone() }];
        let (_, _, short_card) = paint(&ctx, &sample(), 320.0, Vec::new());
        let (shapes, _, card) = paint(&ctx, &cell, 320.0, Vec::new());
        assert!(card.height() > short_card.height() + 60.0, "the card did not grow for a long result");
        let (_, bounds) = texts(&shapes)
            .into_iter()
            .find(|(text, _)| text.contains(long.trim()))
            .expect("the result was not drawn");
        assert!(card.contains_rect(bounds.shrink(0.5)), "the result runs outside its card: {bounds:?} in {card:?}");
    }

    /// Catches Run on a kind this build cannot run: clicking it starts
    /// nothing.
    #[test]
    fn an_unknown_kind_is_labeled_as_stored_and_cannot_run() {
        let mut cell = Cell::text("plot-1", "north");
        cell.kind = "plot".into();
        let ctx = egui::Context::default();
        crate::toolbar::install_fonts(&ctx);
        let (_, _, card) = paint(&ctx, &cell, 320.0, Vec::new());
        let at = run_button(card).center();
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        paint(&ctx, &cell, 320.0, vec![egui::Event::PointerMoved(at), press(true)]);
        let (_, actions, _) = paint(&ctx, &cell, 320.0, vec![press(false)]);
        assert!(actions.iter().all(|action| !matches!(action, CardAction::Run(_))), "an unknown kind ran");
        assert_eq!(kind_label("plot"), "plot");
        assert_eq!(kind_label("text"), "LLM");
    }
}
