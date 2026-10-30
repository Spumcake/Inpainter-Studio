//! Controls shared by the toolbar and the properties panel, so both read as one
//! window: a near-black accent, light rails, and white fields with a faint
//! border, as in the renders. Nothing here takes egui's default blue.
//!
//! Sizes below are written at full size, the toolbar's buttons' size. Panel
//! controls take a [`Scale`] and are drawn a size smaller; the slider is drawn
//! at that size in the toolbar too, so every slider matches.

use eframe::egui::{
    self, Align, Align2, Color32, FontFamily, FontId, Layout, Painter, Pos2, Rect, Response, Sense,
    Stroke, StrokeKind, UiBuilder, pos2, vec2,
};

/// The one accent: slider fill and knob, a toggle that is on, the curve.
pub(crate) const INK: Color32 = Color32::from_rgb(40, 42, 46);
/// Values and headings.
pub(crate) const TEXT: Color32 = Color32::from_rgb(45, 47, 51);
/// Row labels and secondary lines.
pub(crate) const MUTED: Color32 = Color32::from_rgb(107, 114, 128);
pub(crate) const RAIL: Color32 = Color32::from_rgb(212, 214, 218);
pub(crate) const BORDER: Color32 = Color32::from_rgb(226, 229, 233);
const BORDER_HOVER: Color32 = Color32::from_rgb(196, 200, 206);
/// A chosen segment or menu row.
pub(crate) const SELECTED: Color32 = Color32::from_rgb(236, 238, 241);
const DISABLED: Color32 = Color32::from_rgb(186, 188, 192);

const KNOB_RADIUS: f32 = 7.0;
const FIELD_RADIUS: f32 = 6.0;
const FIELD_PAD: f32 = 10.0;
const BODY: f32 = 13.5;

/// The semibold face used for headings. Body text is Inter at regular weight.
pub(crate) const SEMIBOLD: &str = "Inter SemiBold";

/// How large the shared controls are drawn, as a factor of full size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Scale(pub f32);

impl Scale {
    /// The properties panel: seven eighths of full size, which the owner chose
    /// by shrinking a screenshot of the panel until it read right.
    pub(crate) const PANEL: Scale = Scale(0.875);

    /// A full-size length at this scale.
    pub(crate) const fn of(self, length: f32) -> f32 {
        length * self.0
    }
}

pub(crate) fn body(s: Scale) -> FontId {
    FontId::proportional(s.of(BODY))
}

pub(crate) fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

/// A white field with a faint border, the frame of every value control.
pub(crate) fn field(painter: &Painter, rect: Rect, hovered: bool, s: Scale) {
    painter.rect(
        rect,
        s.of(FIELD_RADIUS),
        Color32::WHITE,
        Stroke::new(1.0, if hovered { BORDER_HOVER } else { BORDER }),
        StrokeKind::Inside,
    );
}

/// A rail with a dark fill and knob. `t` is the position in `0..=1`. While the
/// pointer is down on it, the second value is the position the pointer asks
/// for. The rail and knob are one size everywhere: the toolbar's Size slider
/// matches the panel's sliders, so it is always drawn at the panel's scale.
pub(crate) fn slider(
    ui: &mut egui::Ui,
    rect: Rect,
    id: egui::Id,
    t: f32,
) -> (Response, Option<f32>) {
    slider_in(ui, rect, id, t, INK)
}

/// The slider with its fill and knob in `color`: the toolbar's Size slider
/// takes a locked palette color's color.
pub(crate) fn slider_in(
    ui: &mut egui::Ui,
    rect: Rect,
    id: egui::Id,
    t: f32,
    color: Color32,
) -> (Response, Option<f32>) {
    let s = Scale::PANEL;
    let knob_radius = s.of(KNOB_RADIUS);
    let half = s.of(2.0);
    let left = rect.left() + knob_radius;
    let right = (rect.right() - knob_radius).max(left + 1.0);
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let asked = if response.is_pointer_button_down_on() {
        response
            .interact_pointer_pos()
            .map(|pos| ((pos.x - left) / (right - left)).clamp(0.0, 1.0))
    } else {
        None
    };
    let shown = asked.unwrap_or(t).clamp(0.0, 1.0);
    // The rail is snapped to whole device pixels: a rail that straddles a pixel
    // boundary is smeared over an extra row and reads thicker, so rows at
    // fractional heights would otherwise each show a different weight.
    let pixels = ui.ctx().pixels_per_point();
    let thickness = (2.0 * half * pixels).round().max(1.0) / pixels;
    let rail_top = ((rect.center().y - thickness / 2.0) * pixels).round() / pixels;
    let knob = pos2(left + shown * (right - left), rail_top + thickness / 2.0);
    let rail =
        |to: f32| Rect::from_min_max(pos2(left - half, rail_top), pos2(to, rail_top + thickness));
    let painter = ui.painter();
    painter.rect_filled(rail(right + half), thickness / 2.0, RAIL);
    painter.rect_filled(rail(knob.x), thickness / 2.0, color);
    painter.circle_filled(knob, knob_radius, color);
    (response, asked)
}

/// A pill switch: dark when on, a light rail when off.
pub(crate) fn toggle(ui: &mut egui::Ui, rect: Rect, id: egui::Id, on: bool, s: Scale) -> Response {
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), vec2(s.of(36.0), s.of(20.0)));
    let end = s.of(10.0);
    ui.painter()
        .rect_filled(track, end, if on { INK } else { RAIL });
    let knob_x = if on {
        track.right() - end
    } else {
        track.left() + end
    };
    ui.painter()
        .circle_filled(pos2(knob_x, track.center().y), s.of(7.5), Color32::WHITE);
    response
}

fn show_number(value: i32, places: u8) -> String {
    if places == 0 {
        return value.to_string();
    }
    let scale = 10f64.powi(i32::from(places));
    format!("{:.*}", usize::from(places), f64::from(value) / scale)
}

/// What a number field did this frame.
#[derive(Default)]
pub(crate) struct NumberEdit {
    /// A new value, while dragging or once typed.
    pub value: Option<i32>,
    /// The edit is over: a drag was released or typing was committed.
    pub finished: bool,
}

/// A number in a field, left-aligned as in the renders. Dragging sideways
/// scrubs the value; clicking types a replacement, which is clamped. A
/// `blank` field reads "Mixed" until it is set.
#[allow(clippy::too_many_arguments)]
pub(crate) fn number_field(
    ui: &mut egui::Ui,
    rect: Rect,
    id: egui::Id,
    value: i32,
    range: (i32, i32),
    suffix: &str,
    blank: bool,
    places: u8,
    s: Scale,
) -> NumberEdit {
    let (min, max) = range;
    let pad = s.of(FIELD_PAD);
    let typing_id = id.with("typing");
    let focus_id = id.with("focus");
    let start_id = id.with("start");
    let mut edit = NumberEdit::default();

    if let Some(mut text) = ui.data(|data| data.get_temp::<String>(typing_id)) {
        field(ui.painter(), rect, true, s);
        let mut inner = ui.new_child(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(pad, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
        );
        let response = inner.add(
            egui::TextEdit::singleline(&mut text)
                .id(id.with("text"))
                .frame(egui::Frame::NONE)
                .font(body(s))
                .text_color(TEXT)
                .desired_width(rect.width() - 2.0 * pad),
        );
        if ui
            .data_mut(|data| data.remove_temp::<bool>(focus_id))
            .is_some()
        {
            response.request_focus();
        }
        let cancel = ui.input(|input| input.key_pressed(egui::Key::Escape));
        if response.lost_focus() {
            ui.data_mut(|data| data.remove::<String>(typing_id));
            if !cancel {
                if places == 0 {
                    let digits: String = text
                        .chars()
                        .filter(|c| c.is_ascii_digit() || *c == '-')
                        .collect();
                    if let Ok(typed) = digits.parse::<i32>() {
                        edit.value = Some(typed.clamp(min, max));
                        edit.finished = true;
                    }
                } else if let Ok(typed) = text.trim().parse::<f64>() {
                    let scale = 10f64.powi(i32::from(places));
                    let typed = (typed * scale).round() as i32;
                    edit.value = Some(typed.clamp(min, max));
                    edit.finished = true;
                }
            }
        } else {
            ui.data_mut(|data| data.insert_temp(typing_id, text));
        }
        return edit;
    }

    let response = ui
        .interact(rect, id, Sense::click_and_drag())
        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    field(
        ui.painter(),
        rect,
        response.hovered() || response.dragged(),
        s,
    );
    ui.painter().text(
        pos2(rect.left() + pad, rect.center().y),
        Align2::LEFT_CENTER,
        if blank {
            "Mixed".to_string()
        } else {
            format!("{}{suffix}", show_number(value, places))
        },
        body(s),
        if blank { MUTED } else { TEXT },
    );
    if response.drag_started() {
        ui.data_mut(|data| data.insert_temp(start_id, value));
    }
    if response.dragged()
        && let (Some(start), Some(origin), Some(pos)) = (
            ui.data(|data| data.get_temp::<i32>(start_id)),
            ui.input(|input| input.pointer.press_origin()),
            response.interact_pointer_pos(),
        )
    {
        let per_point = ((max - min) as f32 / 160.0).max(0.1);
        let next = start + ((pos.x - origin.x) * per_point).round() as i32;
        edit.value = Some(next.clamp(min, max));
    }
    if response.drag_stopped() {
        ui.data_mut(|data| data.remove::<i32>(start_id));
        edit.finished = true;
    }
    if response.clicked() {
        ui.data_mut(|data| {
            data.insert_temp(typing_id, show_number(value, places));
            data.insert_temp(focus_id, true);
        });
    }
    edit
}

/// A text value in a field. Returns the new text when an edit is committed by
/// Enter or by leaving the field. `hint` shows while the field is empty.
pub(crate) fn text_field(
    ui: &mut egui::Ui,
    rect: Rect,
    id: egui::Id,
    current: &str,
    hint: &str,
    s: Scale,
) -> Option<String> {
    let pad = s.of(FIELD_PAD);
    let focused = ui.memory(|memory| memory.has_focus(id));
    let mut text = if focused {
        ui.data(|data| data.get_temp::<String>(id.with("buffer")))
            .unwrap_or_else(|| current.to_string())
    } else {
        current.to_string()
    };
    let hovered = ui.rect_contains_pointer(rect);
    field(ui.painter(), rect, hovered || focused, s);
    let mut inner = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink2(vec2(pad, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    let response = inner.add(
        egui::TextEdit::singleline(&mut text)
            .id(id)
            .hint_text(hint)
            .frame(egui::Frame::NONE)
            .font(body(s))
            .text_color(TEXT)
            .desired_width(rect.width() - 2.0 * pad),
    );
    if response.changed() {
        ui.data_mut(|data| data.insert_temp(id.with("buffer"), text.clone()));
    }
    if response.lost_focus() {
        ui.data_mut(|data| data.remove::<String>(id.with("buffer")));
        return Some(text);
    }
    None
}

/// Draws a sample of option `usize` in a rectangle, before its label.
pub(crate) type OptionPreview<'a> = &'a dyn Fn(&Painter, Rect, usize);

/// A field that opens a list. `preview` draws a sample before an option's
/// label, in the field and in the list. Returns the option chosen this frame.
#[allow(clippy::too_many_arguments)]
pub(crate) fn dropdown(
    ui: &mut egui::Ui,
    rect: Rect,
    id: egui::Id,
    text: &str,
    options: &[&str],
    selected: Option<usize>,
    preview: Option<OptionPreview>,
    s: Scale,
) -> Option<usize> {
    let sample_w = s.of(40.0);
    let sample_gap = s.of(12.0);
    let response = ui.interact(rect, id, Sense::click_and_drag());
    field(ui.painter(), rect, response.hovered(), s);
    let mut text_x = rect.left() + s.of(FIELD_PAD);
    if let (Some(preview), Some(index)) = (preview, selected) {
        let sample = Rect::from_min_size(pos2(text_x, rect.top()), vec2(sample_w, rect.height()));
        preview(ui.painter(), sample, index);
        text_x = sample.right() + sample_gap;
    }
    ui.painter().text(
        pos2(text_x, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        body(s),
        TEXT,
    );
    chevron(
        ui.painter(),
        pos2(rect.right() - s.of(14.0), rect.center().y),
        TEXT,
        s,
    );

    let mut chosen = None;
    egui::Popup::menu(&response)
        .width(rect.width())
        .gap(s.of(4.0))
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = s.of(2.0);
            for (index, option) in options.iter().enumerate() {
                let (row, row_response) = ui.allocate_exact_size(
                    vec2(rect.width() - s.of(12.0), s.of(28.0)),
                    Sense::click(),
                );
                if Some(index) == selected || row_response.hovered() {
                    ui.painter().rect_filled(row, s.of(4.0), SELECTED);
                }
                let mut label_x = row.left() + s.of(8.0);
                if let Some(preview) = preview {
                    let sample =
                        Rect::from_min_size(pos2(label_x, row.top()), vec2(sample_w, row.height()));
                    preview(ui.painter(), sample, index);
                    label_x = sample.right() + sample_gap;
                }
                ui.painter().text(
                    pos2(label_x, row.center().y),
                    Align2::LEFT_CENTER,
                    *option,
                    body(s),
                    TEXT,
                );
                if row_response.clicked() {
                    chosen = Some(index);
                    ui.close();
                }
            }
        });
    chosen
}

/// Equal cells in one field, the chosen one shaded. `paint` draws a cell's
/// content; `tips` names each cell on hover. Returns the cell clicked.
pub(crate) fn segmented(
    ui: &mut egui::Ui,
    rect: Rect,
    id: egui::Id,
    selected: usize,
    tips: &[&str],
    paint: &dyn Fn(&Painter, Rect, usize, bool),
    s: Scale,
) -> Option<usize> {
    let count = tips.len().max(1);
    field(ui.painter(), rect, false, s);
    let width = rect.width() / count as f32;
    let inset = s.of(2.0);
    let radius = s.of(4.0);
    let mut clicked = None;
    for (index, tip) in tips.iter().enumerate() {
        let cell = Rect::from_min_size(
            pos2(rect.left() + width * index as f32, rect.top()),
            vec2(width, rect.height()),
        );
        let response = ui
            .interact(cell, id.with(index), Sense::click_and_drag())
            .on_hover_text(*tip);
        if index == selected {
            ui.painter()
                .rect_filled(cell.shrink(inset), radius, SELECTED);
        } else if response.hovered() {
            ui.painter()
                .rect_filled(cell.shrink(inset), radius, SELECTED.gamma_multiply(0.6));
        }
        if index > 0 {
            ui.painter().vline(
                cell.left(),
                (cell.top() + s.of(6.0))..=(cell.bottom() - s.of(6.0)),
                Stroke::new(1.0, BORDER),
            );
        }
        paint(ui.painter(), cell, index, index == selected);
        if response.clicked() {
            clicked = Some(index);
        }
    }
    clicked
}

/// A small downward chevron, drawn rather than taken from a font so it sits
/// exactly on the field's center line.
pub(crate) fn chevron(painter: &Painter, center: Pos2, color: Color32, s: Scale) {
    let stroke = Stroke::new(1.5, color);
    let (x, y) = (s.of(4.0), s.of(2.0));
    painter.line_segment([center + vec2(-x, -y), center + vec2(0.0, y)], stroke);
    painter.line_segment([center + vec2(0.0, y), center + vec2(x, -y)], stroke);
}

/// A small sideways chevron: pointing right when `right`, left otherwise.
pub(crate) fn chevron_side(painter: &Painter, center: Pos2, right: bool, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let x = if right { 2.0 } else { -2.0 };
    painter.line_segment([center + vec2(-x, -4.0), center + vec2(x, 0.0)], stroke);
    painter.line_segment([center + vec2(x, 0.0), center + vec2(-x, 4.0)], stroke);
}

/// A horizontal line sample, solid or dashed, for the Style choice.
pub(crate) fn dash_sample(painter: &Painter, rect: Rect, dashed: bool, s: Scale) {
    let y = rect.center().y;
    let stroke = Stroke::new(s.of(2.0), TEXT);
    if dashed {
        let mut x = rect.left();
        while x < rect.right() {
            let end = (x + s.of(7.0)).min(rect.right());
            painter.line_segment([pos2(x, y), pos2(end, y)], stroke);
            x += s.of(11.0);
        }
    } else {
        painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], stroke);
    }
}

/// A short thick bar with round or flat ends, for the Cap choice.
pub(crate) fn cap_sample(painter: &Painter, center: Pos2, round: bool, color: Color32, s: Scale) {
    let bar = Rect::from_center_size(center, vec2(s.of(18.0), s.of(7.0)));
    painter.rect_filled(bar, if round { s.of(3.5) } else { 0.0 }, color);
}

/// One row of a dropdown or right-click menu.
pub(crate) struct MenuItem<'a> {
    pub label: &'a str,
    /// An icon before the label. When any row has one, every label starts
    /// after the icon column.
    pub icon: Option<&'a str>,
    pub enabled: bool,
    /// The row opens a submenu to its right instead of acting itself.
    pub submenu: bool,
    /// A hairline separates this row from the rows above it.
    pub divider: bool,
}

/// What a menu chose: one of its own rows, or what one of its submenus chose.
pub(crate) enum MenuPick<T> {
    Row(usize),
    Sub(T),
}

/// The narrowest a menu row is drawn.
const MENU_MIN_W: f32 = 148.0;
/// The space between a menu row's edge and its label, on both sides.
const MENU_INSET: f32 = 10.0;
/// A menu row's icon, and the gap from it to the label.
const MENU_ICON: f32 = 18.0;
const MENU_ICON_GAP: f32 = 8.0;
/// The chevron at the end of a row that opens a submenu.
const MENU_CHEVRON: f32 = 14.0;
const MENU_CHEVRON_ICON: &str = "nav-arrow-right";

/// The rows of every small menu: the sketch menu, the select tool's action
/// menus, the panel heading's menu, and the outliner's right-click menus. Menus
/// are drawn at full size wherever they open, because their rows act like
/// buttons. The rows are as wide as the longest label plus the same space on
/// either side. A row is highlighted under the pointer and drawn lighter when
/// it waits. Returns the row clicked, and closes the menu.
#[allow(dead_code)]
pub(crate) fn menu_rows(ui: &mut egui::Ui, items: &[MenuItem]) -> Option<usize> {
    match menu_rows_with(ui, items, |_, _| None::<()>) {
        Some(MenuPick::Row(index)) => Some(index),
        _ => None,
    }
}

/// `menu_rows` with submenus. A row marked `submenu` shows a chevron and,
/// while it is hovered or open, shows `submenu`'s contents for that row's
/// index to its right. Choosing in a submenu closes the whole menu.
pub(crate) fn menu_rows_with<T>(
    ui: &mut egui::Ui,
    items: &[MenuItem],
    mut submenu: impl FnMut(usize, &mut egui::Ui) -> Option<T>,
) -> Option<MenuPick<T>> {
    ui.spacing_mut().item_spacing.y = 2.0;
    let font = body(Scale(1.0));
    let widest = items
        .iter()
        .map(|item| {
            ui.painter()
                .layout_no_wrap(item.label.to_string(), font.clone(), TEXT)
                .size()
                .x
        })
        .fold(0.0, f32::max);
    let icons = items.iter().any(|item| item.icon.is_some());
    let label_x = MENU_INSET
        + if icons {
            MENU_ICON + MENU_ICON_GAP
        } else {
            0.0
        };
    let chevron = if items.iter().any(|item| item.submenu) {
        MENU_ICON_GAP + MENU_CHEVRON
    } else {
        0.0
    };
    let width = (widest + label_x + chevron + MENU_INSET)
        .max(MENU_MIN_W)
        .ceil();
    let mut chosen = None;
    for (index, item) in items.iter().enumerate() {
        if item.divider {
            let (line, _) = ui.allocate_exact_size(vec2(width, 5.0), Sense::hover());
            ui.painter().hline(
                line.x_range(),
                line.center().y.round() + 0.5,
                Stroke::new(1.0, BORDER),
            );
        }
        let sense = if item.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (row, response) = ui.allocate_exact_size(vec2(width, 28.0), sense);
        // Filled in once it is known whether this row's submenu is open.
        let highlight = ui.painter().add(egui::Shape::Noop);
        let mut open = false;
        if item.submenu {
            crate::toolbar::paint_icon(
                ui.painter(),
                pos2(row.right() - MENU_INSET - MENU_CHEVRON / 2.0, row.center().y),
                MENU_CHEVRON_ICON,
                MENU_CHEVRON,
                quiet(item.enabled),
            );
            if item.enabled
                && let Some(inner) =
                    egui::containers::menu::SubMenu::new().show(ui, &response, |ui| {
                        submenu(index, ui)
                    })
            {
                open = true;
                if let Some(value) = inner.inner {
                    chosen = Some(MenuPick::Sub(value));
                }
            }
        }
        if item.enabled && (response.hovered() || open) {
            ui.painter().set(
                highlight,
                egui::Shape::rect_filled(row, 4.0, SELECTED),
            );
        }
        if let Some(icon) = item.icon {
            crate::toolbar::paint_icon(
                ui.painter(),
                pos2(row.left() + MENU_INSET + MENU_ICON / 2.0, row.center().y),
                icon,
                MENU_ICON,
                quiet(item.enabled),
            );
        }
        ui.painter().text(
            pos2(row.left() + label_x, row.center().y),
            Align2::LEFT_CENTER,
            item.label,
            font.clone(),
            quiet(item.enabled),
        );
        if item.enabled && !item.submenu && response.clicked() {
            chosen = Some(MenuPick::Row(index));
            ui.close();
        }
    }
    chosen
}

/// A right-click menu of declared rows on `response`, drawn with the shared
/// menu rows. Returns the row chosen this frame, or a row of its submenu.
pub(crate) fn action_menu<'a>(
    response: &Response,
    rows: &'a [protocol::MenuEntry],
) -> Option<&'a protocol::MenuEntry> {
    if rows.is_empty() {
        return None;
    }
    let mut chosen = None;
    egui::Popup::context_menu(response).show(|ui| {
        chosen = entry_rows(ui, rows);
    });
    chosen
}

/// Declared menu rows, with their submenus, drawn as the shared menu rows.
/// Returns the row chosen, at any depth.
pub(crate) fn entry_rows<'a>(
    ui: &mut egui::Ui,
    rows: &'a [protocol::MenuEntry],
) -> Option<&'a protocol::MenuEntry> {
    let items: Vec<MenuItem> = rows
        .iter()
        .map(|row| MenuItem {
            label: &row.label,
            icon: row.icon.as_deref(),
            enabled: row.enabled,
            submenu: !row.submenu.is_empty(),
            divider: row.divider,
        })
        .collect();
    match menu_rows_with(ui, &items, |index, ui| entry_rows(ui, &rows[index].submenu)) {
        Some(MenuPick::Row(index)) => Some(&rows[index]),
        Some(MenuPick::Sub(row)) => Some(row),
        None => None,
    }
}

/// An area that can be dragged to scroll, the panel or the toolbar's row,
/// shows an open hand over its background and a closed hand while it is
/// dragged. Every control on such an area senses drags as well as clicks, so
/// it covers the background: no hand over it, and no scroll from it. A
/// control that asked for its own pointer, such as a text field's I-beam,
/// keeps it. `scroll` is the scroll area's id.
pub(crate) fn drag_scroll_cursor(ui: &egui::Ui, scroll: egui::Id) {
    let Some(background) = ui.ctx().read_response(scroll.with("area")) else {
        return;
    };
    if background.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if background.hovered()
        && ui.ctx().output(|output| output.cursor_icon) == egui::CursorIcon::Default
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
}

/// Text drawn lighter, for a control that waits.
pub(crate) fn quiet(enabled: bool) -> Color32 {
    if enabled { TEXT } else { DISABLED }
}

/// A filled rounded rectangle with a 1-point edge inside it.
pub(crate) fn outlined(painter: &Painter, rect: Rect, radius: f32, fill: Color32, edge: Color32) {
    painter.rect(
        rect,
        radius,
        fill,
        Stroke::new(1.0, edge),
        StrokeKind::Inside,
    );
}

/// A filled disc.
pub(crate) fn disc(painter: &Painter, center: Pos2, radius: f32, color: Color32) {
    painter.circle_filled(center, radius, color);
}

/// A ring of `width` around `center`, such as a brush's outline at the pointer.
pub(crate) fn ring(painter: &Painter, center: Pos2, radius: f32, width: f32, color: Color32) {
    painter.circle_stroke(center, radius, Stroke::new(width, color));
}

/// A soft shadow under a raised rounded box, as under the menus.
pub(crate) fn soft_shadow(painter: &Painter, rect: Rect, radius: f32) {
    let shadow = egui::epaint::Shadow {
        offset: [0, 2],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(22),
    };
    painter.add(shadow.as_shape(rect, radius));
}

/// A thin vertical rule from `top` to `bottom` at `x`.
pub(crate) fn vertical_rule(painter: &Painter, x: f32, top: f32, bottom: f32, color: Color32) {
    painter.vline(x, top..=bottom, Stroke::new(1.0, color));
}

/// Every menu floats on one card: white, a faint hairline, well-rounded
/// corners, and a soft, wide shadow, as the owner set on 2026-10-29.
const MENU_EDGE: Color32 = Color32::from_rgb(236, 237, 240);
const MENU_RADIUS: u8 = 16;
const MENU_SHADOW: egui::epaint::Shadow = egui::epaint::Shadow {
    offset: [0, 8],
    blur: 32,
    spread: 0,
    color: Color32::from_black_alpha(18),
};

/// Gives every menu, list, and popup the card.
pub(crate) fn menu_card(style: &mut egui::Style) {
    style.visuals.window_fill = Color32::WHITE;
    style.visuals.window_stroke = Stroke::new(1.0, MENU_EDGE);
    style.visuals.menu_corner_radius = MENU_RADIUS.into();
    style.visuals.popup_shadow = MENU_SHADOW;
    style.spacing.menu_margin = egui::Margin::same(8);
}

/// The card, with the room the level picker's contents want.
pub(crate) fn floating_card() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::WHITE)
        .stroke(Stroke::new(1.0, MENU_EDGE))
        .corner_radius(MENU_RADIUS)
        .inner_margin(12)
        .shadow(MENU_SHADOW)
}
