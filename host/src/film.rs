//! A raster tab's filmstrip: the pictures a sub-application lists, along the
//! bottom of the working area, as the owner's mockup and 0.11's gallery views
//! show. The host draws the thumbnails and the + tile, and sends what the
//! pointer does on them: `focus` at any press, `select` for a click, a
//! thumbnail's own rows for a right-click, the + tile's rows, and `reorder`
//! for a drag. What any of it means is the sub-application's.

use crate::widgets;
use eframe::egui::{
    self, Color32, CursorIcon, Event, Key, MouseWheelUnit, PointerButton, Popup, Rect, RectAlign, Sense,
    SetOpenCommand, pos2, vec2,
};
use protocol::{Action, Film, FilmItem};
use serde_json::{Value, json};

/// The strip's height, its hairline included, in points.
pub(crate) const HEIGHT: f32 = 68.0;
/// A thumbnail's height. Its width follows its picture's proportions, within
/// these, so a very long or very tall picture still makes a tile to click.
const THUMB_H: f32 = 44.0;
const THUMB_MIN_W: f32 = 24.0;
const THUMB_MAX_W: f32 = 3.0 * THUMB_H;
/// The + tile, as wide as a landscape thumbnail, as gallery.png shows.
const ADD_W: f32 = 68.0;
/// The space between tiles, and before the first.
const GAP: f32 = 10.0;
const INSET: f32 = 18.0;
const RADIUS: f32 = 4.0;
/// The shown picture's outline, as an Image cell's filmstrip draws it.
const SHOWN: Color32 = Color32::from_rgb(37, 99, 235);
/// The + tile's sign, lighter than text, as gallery.png shows it.
const SIGN: Color32 = Color32::from_rgb(160, 164, 172);
/// The room between a menu and the tile it opens from.
const MENU_GAP: f32 = 8.0;
/// A drag this near an end of the strip scrolls it, this far each frame.
const EDGE: f32 = 28.0;
const EDGE_STEP: f32 = 8.0;
/// A wheel notch, as on the picture.
const NOTCH_POINTS: f32 = 50.0;

/// The area a raster tab gives its picture, and the strip under it when the
/// view has a filmstrip. An area too short for both keeps the picture.
pub(crate) fn split(rect: Rect, film: Option<&Film>) -> (Rect, Option<Rect>) {
    match film {
        Some(_) if rect.height() > 2.0 * HEIGHT => {
            let top = rect.bottom() - HEIGHT;
            (Rect::from_min_max(rect.min, pos2(rect.right(), top)), Some(Rect::from_min_max(pos2(rect.left(), top), rect.max)))
        }
        _ => (rect, None),
    }
}

/// The strip's state between frames: how far it is scrolled, and the
/// thumbnail being dragged along it.
#[derive(Default)]
pub(crate) struct Strip {
    scroll: f32,
    moving: Option<Moving>,
    /// A thumbnail being renamed from its menu: its index, the name typed so
    /// far, the row's action, and whether its field still wants the keyboard.
    renaming: Option<(usize, String, Action, bool)>,
}

struct Moving {
    from: usize,
    /// Where the pointer took the thumbnail, from its left edge.
    grab: f32,
}

impl Strip {
    /// The name field over a thumbnail being renamed, on the menus' card
    /// above it. Enter sends the row's action with the name as `value`;
    /// Escape, or a press elsewhere, keeps the old name.
    fn rename_field(&mut self, ui: &mut egui::Ui, layout: &Layout, film: &Film, start: f32, rect: Rect, out: &mut Vec<Action>) {
        let Some((index, text, action, focus)) = self.renaming.as_mut() else {
            return;
        };
        if *index >= film.items.len() {
            self.renaming = None;
            return;
        }
        let left = start + layout.lefts[*index];
        let width = 200.0;
        let at = pos2(left.min(rect.right() - width - 8.0).max(rect.left() + 8.0), rect.top() - 46.0);
        let id = egui::Id::new(("film-rename", *index));
        let mut done = None;
        let shown = egui::Area::new(egui::Id::new("film-rename-card"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .fade_in(false)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    let edit = ui.add(egui::TextEdit::singleline(text).id(id).desired_width(width - 24.0));
                    if *focus {
                        edit.request_focus();
                        *focus = false;
                    }
                    if ui.input(|input| input.key_pressed(Key::Enter)) {
                        done = Some(true);
                    } else if ui.input(|input| input.key_pressed(Key::Escape)) {
                        done = Some(false);
                    }
                });
            });
        let pressed_away = ui.input(|input| {
            input.pointer.any_pressed() && input.pointer.interact_pos().is_some_and(|pos| !shown.response.rect.contains(pos))
        });
        if done.is_none() && pressed_away {
            done = Some(false);
        }
        if let Some(keep) = done {
            if keep && !text.trim().is_empty() {
                out.push(action.with_value(json!(text.trim())));
            }
            self.renaming = None;
        }
    }
}

/// Where each tile lies along the strip, from its start: the thumbnails,
/// then the + tile.
struct Layout {
    lefts: Vec<f32>,
    widths: Vec<f32>,
    add: f32,
    total: f32,
}

fn layout(film: &Film) -> Layout {
    let mut x = INSET;
    let mut lefts = Vec::with_capacity(film.items.len());
    let mut widths = Vec::with_capacity(film.items.len());
    for item in &film.items {
        let width = thumb_width(item);
        lefts.push(x);
        widths.push(width);
        x += width + GAP;
    }
    Layout { lefts, widths, add: x, total: x + ADD_W + INSET }
}

/// A thumbnail's width at the strip's height, at its picture's proportions.
fn thumb_width(item: &FilmItem) -> f32 {
    let (width, height) = (item.thumbnail.width.max(1) as f32, item.thumbnail.height.max(1) as f32);
    (THUMB_H * width / height).clamp(THUMB_MIN_W, THUMB_MAX_W)
}

/// Where a dragged thumbnail lands with the pointer at `x` along the strip:
/// before every other thumbnail whose middle is past it, counted after the
/// dragged one is taken out, as `reorder` wants.
fn landing(layout: &Layout, from: usize, x: f32) -> usize {
    (0..layout.lefts.len())
        .filter(|index| *index != from && layout.lefts[*index] + layout.widths[*index] / 2.0 < x)
        .count()
}

/// `action` with these fields added to its argument, as `Action::with_value`
/// adds a value.
fn with_fields(action: &Action, fields: &[(&str, Value)]) -> Action {
    let mut object = match action.arg.clone() {
        Some(Value::Object(object)) => object,
        Some(other) => {
            let mut object = serde_json::Map::new();
            object.insert(String::from("arg"), other);
            object
        }
        None => serde_json::Map::new(),
    };
    for (key, value) in fields {
        object.insert((*key).to_string(), value.clone());
    }
    Action { id: action.id.clone(), arg: Some(Value::Object(object)) }
}

impl Strip {
    /// Draws the strip in `rect` and handles the pointer on it. The actions
    /// it sends go to `out`, in order. While the assistant works the strip is
    /// shown and scrolls, but sends nothing and opens no menu.
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        film: &Film,
        rect: Rect,
        locked: bool,
        dismissing: bool,
        out: &mut Vec<Action>,
    ) {
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect_filled(rect, 0.0, Color32::WHITE);
        painter.hline(rect.x_range(), rect.top() + 0.5, (1.0, widgets::BORDER));
        let layout = layout(film);
        if self.moving.as_ref().is_some_and(|moving| moving.from >= film.items.len()) {
            self.moving = None;
        }

        let over = ui.rect_contains_pointer(rect);
        let (pos, pressed, primary_down, escape) = ui.input(|input| {
            (input.pointer.latest_pos(), input.pointer.any_pressed(), input.pointer.primary_down(), input.key_pressed(Key::Escape))
        });
        // Any press on the strip tells the sub-application the strip has the
        // keyboard, before whatever else the press does. A press that closes
        // a menu does nothing else.
        if over && pressed {
            if dismissing {
                Popup::close_all(ui.ctx());
            } else if !locked {
                out.push(film.focus.clone());
            }
        }
        let acts = !dismissing && !locked;

        // The wheel scrolls the strip sideways, as does a drag held near
        // either end.
        if over || self.moving.is_some() {
            let events = ui.input(|input| input.events.clone());
            for event in events {
                if let Event::MouseWheel { unit, delta, .. } = event {
                    let scale = match unit {
                        MouseWheelUnit::Point => 1.0,
                        MouseWheelUnit::Line => NOTCH_POINTS,
                        MouseWheelUnit::Page => rect.width(),
                    };
                    self.scroll -= (delta.x + delta.y) * scale;
                }
            }
        }
        if self.moving.is_some()
            && let Some(pos) = pos
        {
            if pos.x < rect.left() + EDGE {
                self.scroll -= EDGE_STEP;
                ui.ctx().request_repaint();
            } else if pos.x > rect.right() - EDGE {
                self.scroll += EDGE_STEP;
                ui.ctx().request_repaint();
            }
        }
        self.scroll = self.scroll.clamp(0.0, (layout.total - rect.width()).max(0.0));
        let start = rect.left() - self.scroll;
        let top = rect.center().y + 0.5 - THUMB_H / 2.0;
        let tile = |left: f32, width: f32| Rect::from_min_size(pos2(start + left, top), vec2(width, THUMB_H));

        // The open drag lands or is let go.
        if let Some(moving) = &self.moving {
            let along = pos.map_or(0.0, |pos| pos.x - start);
            if escape || locked {
                self.moving = None;
            } else if !primary_down {
                let to = landing(&layout, moving.from, along);
                if to != moving.from {
                    out.push(with_fields(&film.reorder, &[("from", json!(moving.from)), ("to", json!(to))]));
                }
                self.moving = None;
            }
        }

        let mut strip = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt("film"));
        strip.set_clip_rect(rect.intersect(ui.clip_rect()));
        let mut chosen: Option<Action> = None;
        for (index, item) in film.items.iter().enumerate() {
            let place = tile(layout.lefts[index], layout.widths[index]);
            let mut response = strip.interact(place, strip.id().with(("thumb", index)), Sense::click_and_drag());
            if self.moving.is_none() {
                response = response.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(&item.name);
            }
            if acts && response.clicked() {
                out.push(film.select.with_value(json!(index)));
            }
            if acts
                && self.moving.is_none()
                && film.items.len() > 1
                && response.drag_started_by(PointerButton::Primary)
            {
                let grab = response.interact_pointer_pos().map_or(place.width() / 2.0, |at| at.x - place.left());
                self.moving = Some(Moving { from: index, grab });
            }
            if self.moving.as_ref().is_some_and(|moving| moving.from == index) {
                continue;
            }
            paint_thumb(&strip, &painter, place, item, index == film.shown);
            if !item.menu.is_empty() {
                let open = (acts && response.secondary_clicked()).then_some(SetOpenCommand::Bool(true));
                let mut renaming = None;
                Popup::menu(&response).open_memory(open).align(RectAlign::TOP_START).gap(MENU_GAP).show(|ui| {
                    match widgets::entry_rows(ui, &item.menu) {
                        // A rename row asks for the new name in place, as a
                        // layer's row does, and sends it as `value`.
                        Some(row) if row.rename => {
                            renaming = row.action.clone().map(|action| (index, item.name.clone(), action, true));
                        }
                        Some(row) => chosen = row.action.clone(),
                        None => {}
                    }
                });
                if renaming.is_some() {
                    self.renaming = renaming;
                }
            }
        }

        // The + tile, with its rows above it.
        let add = tile(layout.add, ADD_W);
        let response = strip.interact(add, strip.id().with("add"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        let edge = if response.hovered() { widgets::RAIL } else { widgets::BORDER };
        widgets::outlined(&painter, add, RADIUS + 2.0, Color32::WHITE, edge);
        crate::toolbar::paint_icon(&painter, add.center(), "plus", 18.0, SIGN);
        if !film.add.is_empty() {
            let open = (acts && response.clicked()).then_some(SetOpenCommand::Toggle);
            Popup::menu(&response).open_memory(open).align(RectAlign::TOP_START).gap(MENU_GAP).show(|ui| {
                if let Some(action) = widgets::entry_rows(ui, &film.add).and_then(|row| row.action.clone()) {
                    chosen = Some(action);
                }
            });
        }
        out.extend(chosen);
        self.rename_field(ui, &layout, film, start, rect, out);

        // The dragged thumbnail follows the pointer, over a line where it
        // will land.
        if let Some(moving) = &self.moving
            && let Some(pos) = pos
        {
            let to = landing(&layout, moving.from, pos.x - start);
            let others: Vec<usize> = (0..film.items.len()).filter(|index| *index != moving.from).collect();
            let before = if to == 0 { INSET - GAP } else { layout.lefts[others[to - 1]] + layout.widths[others[to - 1]] };
            let after = others.get(to).map_or(layout.add, |index| layout.lefts[*index]);
            let x = start + (before + after) / 2.0;
            painter.vline(x, (top - 4.0)..=(top + THUMB_H + 4.0), (2.0, SHOWN));
            let width = layout.widths[moving.from];
            let place = Rect::from_min_size(pos2(pos.x - moving.grab, top), vec2(width, THUMB_H));
            let floating = painter.with_layer_id(egui::LayerId::new(egui::Order::Tooltip, strip.id().with("moving")));
            widgets::soft_shadow(&floating, place, RADIUS);
            paint_thumb(&strip, &floating, place, &film.items[moving.from], moving.from == film.shown);
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }
    }
}

/// One thumbnail: its picture at its proportions, with rounded corners, over
/// squares where it is clear, outlined in blue when it is the one shown and
/// with a hairline otherwise.
fn paint_thumb(ui: &egui::Ui, painter: &egui::Painter, place: Rect, item: &FilmItem, shown: bool) {
    let thumbnail = &item.thumbnail;
    match crate::raster::raw_picture(ui.ctx(), &thumbnail.path, thumbnail.width, thumbnail.height, thumbnail.revision, None) {
        Some((texture, opaque)) => {
            if !opaque {
                crate::raster::checker(ui.ctx(), painter, place);
            }
            let options = egui::ImageOptions { corner_radius: RADIUS.into(), ..Default::default() };
            egui::widgets::paint_texture_at(painter, place, &options, &egui::load::SizedTexture::new(texture, place.size()));
        }
        None => crate::raster::checker(ui.ctx(), painter, place),
    }
    if shown {
        widgets::outlined(painter, place.expand(3.0), RADIUS + 3.0, Color32::TRANSPARENT, SHOWN);
        widgets::outlined(painter, place.expand(2.0), RADIUS + 2.0, Color32::TRANSPARENT, SHOWN);
    } else {
        widgets::outlined(painter, place, RADIUS, Color32::TRANSPARENT, widgets::BORDER);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::Thumbnail;

    fn film(sizes: &[(u32, u32)]) -> Film {
        Film {
            items: sizes
                .iter()
                .map(|(width, height)| FilmItem {
                    name: String::new(),
                    thumbnail: Thumbnail { path: String::new(), width: *width, height: *height, revision: 1 },
                    menu: Vec::new(),
                })
                .collect(),
            shown: 0,
            select: Action::new("select"),
            reorder: Action::new("reorder"),
            add: Vec::new(),
            focus: Action::new("focus"),
        }
    }

    /// Catches a drag that lands one place off: `to` is counted after the
    /// dragged thumbnail is taken out, so a drag past every other one lands
    /// last, and one held over its own place lands where it was.
    #[test]
    fn a_dragged_thumbnail_lands_between_the_others() {
        let layout = layout(&film(&[(16, 9), (9, 16), (1, 1)]));
        assert_eq!(layout.widths, vec![THUMB_H * 16.0 / 9.0, THUMB_MIN_W.max(THUMB_H * 9.0 / 16.0), THUMB_H]);
        let middle = |index: usize| layout.lefts[index] + layout.widths[index] / 2.0;
        assert_eq!(landing(&layout, 0, middle(0)), 0);
        assert_eq!(landing(&layout, 0, middle(2) + 1.0), 2);
        assert_eq!(landing(&layout, 2, 0.0), 0);
        assert_eq!(landing(&layout, 2, middle(0) + 1.0), 1);
        assert_eq!(landing(&layout, 1, middle(1)), 1);
        let reorder = with_fields(&Action::with_arg("reorder", json!({ "tab": "1" })), &[("from", json!(0)), ("to", json!(2))]);
        assert_eq!(reorder.arg, Some(json!({ "tab": "1", "from": 0, "to": 2 })));
    }
}
