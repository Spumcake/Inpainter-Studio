//! The tool area. It draws the selected tab's declared toolbar, group by
//! group in the order declared, with the widgets of Simple Sketch 1.0.1's
//! `toolbar.rs`. A click returns the action its item declared.

use crate::{icons, widgets};
use eframe::egui::{
    self, Align, Align2, Color32, FontFamily, FontId, Layout, Rect, RichText, Sense, Stroke,
    UiBuilder, pos2, vec2,
};
use protocol::{Action, MenuEntry, Swatch, ToolItem, Toolbar};
use serde_json::json;
use std::sync::Arc;

/// The bar behind the tabs. The toolbar and the active tab are white.
pub(crate) const BAR: Color32 = Color32::from_rgb(249, 250, 251);
pub(crate) const ICON: Color32 = Color32::from_rgb(45, 47, 51);
pub(crate) const ICON_QUIET: Color32 = Color32::from_rgb(186, 188, 192);
/// The current tool's icon. It has no plate behind it.
pub(crate) const ICON_ACTIVE: Color32 = Color32::from_rgb(37, 99, 235);
const HAIRLINE: Color32 = Color32::from_rgb(236, 237, 240);
const SEPARATOR: Color32 = Color32::from_rgb(222, 225, 229);

/// Every toolbar item is this tall and paints its content centered in it,
/// so the whole row shares one vertical center. As the owner set on
/// 2026-10-28, the toolbar is a tenth smaller than the references show: its
/// row, buttons, separators, and swatches shrink with its icons.
const ROW: f32 = 36.0;
/// Space above and below the row.
const ROW_MARGIN: i8 = 5;
/// The toolbar's whole height.
pub(crate) const HEIGHT: f32 = ROW + 2.0 * ROW_MARGIN as f32;
/// A toolbar button's width. Its hover plate is a square two points narrower,
/// so neighboring plates keep a sliver of toolbar between them.
pub(crate) const BUTTON_W: f32 = 32.0;
/// A toolbar icon's size.
const ICON_POINTS: f32 = 18.0;
/// A swatch's width: room for the active swatch's ring, 24.5 points across.
pub(crate) const SWATCH_W: f32 = 26.0;
/// A divider between toolbar groups, with its space on either side.
pub(crate) const SEPARATOR_W: f32 = 11.0;
/// Space before the first swatch, so the active swatch's ring is not
/// clipped at the row's left edge.
pub(crate) const SWATCH_INSET: f32 = 2.0;

/// Inter's roman variable font. Weight only: this file has no italic axis.
const INTER: &[u8] = include_bytes!("../assets/fonts/Inter-Variable.ttf");
/// Inter's italic variable font. Italic and bold italic are this file at two weights.
const INTER_ITALIC_FONT: &[u8] = include_bytes!("../assets/fonts/Inter-Italic-Variable.ttf");
/// Google's monochrome Noto Emoji, a variable font by weight, for emoji
/// newer than egui's own emoji face knows.
const NOTO_EMOJI: &[u8] = include_bytes!("../assets/fonts/NotoEmoji-Variable.ttf");
const EMOJI: &str = "Noto Emoji";
const EMOJI_BOLD: &str = "Noto Emoji Bold";

fn at_weight(bytes: &'static [u8], weight: f32) -> egui::FontData {
    let mut data = egui::FontData::from_static(bytes);
    data.tweak.coords = egui::epaint::text::VariationCoords::new([(b"wght", weight)]);
    data
}

fn install_face(fonts: &mut egui::FontDefinitions, name: &str, data: egui::FontData) {
    fonts.font_data.insert(name.into(), Arc::new(data));
    fonts
        .families
        .insert(FontFamily::Name(name.into()), vec![name.into()]);
}

pub(crate) fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    install_face(&mut fonts, icons::INTER, at_weight(INTER, 400.0));
    install_face(&mut fonts, icons::INTER_BOLD, at_weight(INTER, 700.0));
    install_face(
        &mut fonts,
        icons::INTER_ITALIC,
        at_weight(INTER_ITALIC_FONT, 400.0),
    );
    install_face(
        &mut fonts,
        icons::INTER_BOLD_ITALIC,
        at_weight(INTER_ITALIC_FONT, 700.0),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, icons::INTER.into());
    fonts.font_data.insert(
        widgets::SEMIBOLD.into(),
        Arc::new(at_weight(INTER, 600.0)),
    );
    fonts.families.insert(
        FontFamily::Name(widgets::SEMIBOLD.into()),
        vec![widgets::SEMIBOLD.into(), icons::INTER.into()],
    );
    for font in iconflow::fonts() {
        fonts.font_data.insert(
            font.family.to_string(),
            Arc::new(egui::FontData::from_static(font.bytes)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Name(font.family.into()))
            .or_default()
            .insert(0, font.family.to_string());
    }
    // Every face falls back to Noto Emoji, at the face's weight, then to
    // egui's own emoji faces, or emoji draw as "?".
    fonts.font_data.insert(EMOJI.into(), Arc::new(at_weight(NOTO_EMOJI, 400.0)));
    fonts.font_data.insert(EMOJI_BOLD.into(), Arc::new(at_weight(NOTO_EMOJI, 700.0)));
    let older: Vec<String> = ["NotoEmoji-Regular", "emoji-icon-font"]
        .into_iter()
        .filter(|name| fonts.font_data.contains_key(*name))
        .map(String::from)
        .collect();
    for (family, emoji) in [
        (FontFamily::Proportional, EMOJI),
        (FontFamily::Monospace, EMOJI),
        (FontFamily::Name(icons::INTER.into()), EMOJI),
        (FontFamily::Name(icons::INTER_ITALIC.into()), EMOJI),
        (FontFamily::Name(icons::INTER_BOLD.into()), EMOJI_BOLD),
        (FontFamily::Name(icons::INTER_BOLD_ITALIC.into()), EMOJI_BOLD),
        (FontFamily::Name(widgets::SEMIBOLD.into()), EMOJI_BOLD),
    ] {
        if let Some(faces) = fonts.families.get_mut(&family) {
            faces.retain(|face| !older.contains(face));
            faces.push(emoji.to_string());
            faces.extend(older.iter().cloned());
        }
    }
    ctx.set_fonts(fonts);
}

/// The Iconoir glyph for `name`, in its filled style when the name ends in
/// `-solid`. Quarter turns are applied by [`paint_icon`].
fn icon_glyph(name: &str, points: f32) -> (String, FontId) {
    let spec = icons::icon_spec(name);
    let style = if spec.filled {
        iconflow::Style::Filled
    } else {
        iconflow::Style::Regular
    };
    let icon = iconflow::try_icon(
        iconflow::Pack::Iconoir,
        spec.glyph,
        style,
        iconflow::Size::Regular,
    )
    .unwrap_or_else(|error| panic!("iconoir `{name}`: {error}"));
    let glyph = char::from_u32(icon.codepoint).unwrap_or('?');
    (
        glyph.to_string(),
        FontId::new(points, FontFamily::Name(icon.family.into())),
    )
}

pub(crate) fn paint_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    name: &str,
    points: f32,
    color: Color32,
) {
    if paint_drawn_icon(painter, center, name, points, color) {
        return;
    }
    let (glyph, font) = icon_glyph(name, points);
    let turns = icons::icon_spec(name).quarter_turns;
    if turns == 0 {
        painter.text(center, Align2::CENTER_CENTER, glyph, font, color);
        return;
    }
    // A text shape turns about its top-left corner, so place that corner
    // where the turned glyph's center lands on `center`.
    let galley = painter.layout_no_wrap(glyph, font, color);
    let angle = f32::from(turns) * std::f32::consts::FRAC_PI_2;
    let half = galley.size() / 2.0;
    let (sin, cos) = angle.sin_cos();
    let turned_half = vec2(half.x * cos - half.y * sin, half.x * sin + half.y * cos);
    painter
        .add(egui::epaint::TextShape::new(center - turned_half, galley, color).with_angle(angle));
}

/// Draws one of [`icons::DRAWN_ICONS`] in Iconoir's style: a 1.5-unit line
/// with round ends on a 24-unit grid, sized like a glyph of `points`. Returns
/// false for any other name.
fn paint_drawn_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    name: &str,
    points: f32,
    color: Color32,
) -> bool {
    if !icons::DRAWN_ICONS.contains(&name) {
        return false;
    }
    let unit = points / 24.0;
    let at = |x: f32, y: f32| center + vec2((x - 12.0) * unit, (y - 12.0) * unit);
    let stroke = Stroke::new(1.5 * unit, color);
    match name {
        "brush" => {
            // The handle: a capsule along the diagonal, open at its lower end.
            let (top, low) = ((20.0, 4.0), (12.5, 11.5));
            let half = 1.5_f32;
            let side = vec2(1.0, 1.0).normalized() * half;
            let mut handle = vec![at(low.0 + side.x, low.1 + side.y)];
            let end = egui::Pos2::new(top.0, top.1);
            let start_angle = side.angle();
            for step in 0..=8 {
                let angle = start_angle - std::f32::consts::PI * step as f32 / 8.0;
                let point = end + vec2(angle.cos(), angle.sin()) * half;
                handle.push(at(point.x, point.y));
            }
            handle.push(at(low.0 - side.x, low.1 - side.y));
            painter.add(egui::Shape::line(handle, stroke));
            painter.add(egui::Shape::closed_line(
                teardrop(
                    egui::Pos2::new(10.0, 14.0),
                    3.0,
                    egui::Pos2::new(4.0, 20.0),
                    &at,
                ),
                stroke,
            ));
            true
        }
        _ => false,
    }
}

/// The brush's bristle tip: a circle drawn out to a point at `tail`.
fn teardrop(
    center: egui::Pos2,
    radius: f32,
    tail: egui::Pos2,
    at: &dyn Fn(f32, f32) -> egui::Pos2,
) -> Vec<egui::Pos2> {
    let reach = center - tail;
    let base = reach.angle();
    let offset = (radius / reach.length()).asin();
    // The tangent points seen from the tail sit a quarter turn plus `offset`
    // either side of `base`; the arc between them runs over the far side.
    let first = base + std::f32::consts::FRAC_PI_2 + offset;
    let sweep = std::f32::consts::PI + 2.0 * offset;
    let mut points = vec![at(tail.x, tail.y)];
    for step in 0..=16 {
        let angle = first - sweep * step as f32 / 16.0;
        let point = center + vec2(angle.cos(), angle.sin()) * radius;
        points.push(at(point.x, point.y));
    }
    points
}

pub(crate) fn icon_text(name: &str, points: f32, color: Color32) -> RichText {
    let (glyph, font) = icon_glyph(name, points);
    RichText::new(glyph).font(font).color(color)
}

/// A frameless icon button of a fixed size, its glyph painted at the center.
pub(crate) fn glyph_button(
    ui: &mut egui::Ui,
    name: &str,
    points: f32,
    size: egui::Vec2,
    tip: Option<&str>,
    enabled: bool,
    selected: bool,
) -> egui::Response {
    // A button that can be used covers the toolbar's row: dragging on it does
    // not scroll the row, and the pointer is not the row's open hand. One
    // that waits only shows its name, so the row can be dragged over it.
    let sense = if enabled {
        Sense::click_and_drag()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    // No plate behind the icon: the current tool, and a button under the
    // pointer, are marked by the icon's blue alone.
    let color = match (enabled, selected || response.hovered()) {
        (true, true) => ICON_ACTIVE,
        (true, false) => ICON,
        (false, _) => ICON_QUIET,
    };
    paint_icon(ui.painter(), rect.center(), name, points, color);
    let label = tip.unwrap_or(name).to_string();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &label));
    match tip {
        Some(tip) => response.on_hover_text(tip),
        None => response,
    }
}

pub(crate) fn icon_button(
    ui: &mut egui::Ui,
    name: &str,
    tip: Option<&str>,
    enabled: bool,
    selected: bool,
) -> egui::Response {
    glyph_button(
        ui,
        name,
        ICON_POINTS,
        vec2(BUTTON_W, ROW),
        tip,
        enabled,
        selected,
    )
}

/// Draws the selected tab's toolbar, or the empty bar when it declares none,
/// and under it the settings row when the toolbar declares one. The trailing
/// group is laid out first, from the right edge, so a narrow window scrolls
/// the other groups instead. The settings row's trailing group ends
/// `options_inset` short of the right edge, above the contents' own edge
/// when the panel is open. Returns the actions chosen.
pub(crate) fn show(ui: &mut egui::Ui, toolbar: Option<&Toolbar>, options_inset: f32) -> Vec<Action> {
    let mut chosen = Vec::new();
    let empty = Toolbar::default();
    let toolbar = toolbar.unwrap_or(&empty);
    let bar = egui::Frame::new()
        .fill(Color32::WHITE)
        .inner_margin(egui::Margin::symmetric(12, ROW_MARGIN))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(ROW);
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                for item in toolbar.trailing.iter().rev() {
                    item_ui(ui, item, &mut chosen);
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    // A row wider than the window scrolls: the wheel turns it
                    // sideways without Shift, and dragging it moves it. A
                    // swatch or a slider takes its own drag, so sorting and
                    // sizing do not scroll the row.
                    ui.style_mut().always_scroll_the_only_direction = true;
                    let scrolled = egui::ScrollArea::horizontal()
                        .id_salt("toolbar")
                        .scroll_source(egui::scroll_area::ScrollSource {
                            scroll_bar: false,
                            drag: egui::scroll_area::DragScroll::Always,
                            mouse_wheel: true,
                        })
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            // A full-height row: `horizontal` would start
                            // short and grow downward, below the trailing group.
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                ui.set_height(ROW);
                                for item in &toolbar.items {
                                    item_ui(ui, item, &mut chosen);
                                }
                            });
                        });
                    widgets::drag_scroll_cursor(ui, scrolled.id);
                });
            });
        });
    let rect = bar.response.rect;
    // The views draw no line between the toolbar and its settings row, only
    // under the row.
    if toolbar.options.is_empty() && toolbar.options_trailing.is_empty() {
        hairline(ui, rect.left(), rect.right(), rect.bottom());
    } else {
        options_row(ui, toolbar, options_inset, &mut chosen);
    }
    chosen
}

/// A settings-row control's height: the views draw them smaller than the
/// toolbar's buttons.
const OPTION: f32 = 28.0;
/// Space above and below the settings row's controls: the row is 40 tall.
const OPTION_MARGIN: i8 = 6;
/// Space between the settings row's controls, and between a caption and
/// its control.
const OPTION_GAP: f32 = 14.0;
const CAPTION_GAP: f32 = 10.0;
/// The settings row's words.
const OPTION_TEXT: f32 = 13.5;
/// A switch that is on, as brush.png draws Pen pressure.
const SWITCH_ON: Color32 = Color32::from_rgb(37, 99, 235);

fn option_font() -> FontId {
    FontId::proportional(OPTION_TEXT)
}

/// The chosen tool's settings, as a second white row under the toolbar.
fn options_row(ui: &mut egui::Ui, toolbar: &Toolbar, inset: f32, chosen: &mut Vec<Action>) {
    let row = egui::Frame::new()
        .fill(Color32::WHITE)
        .inner_margin(egui::Margin::symmetric(12, OPTION_MARGIN))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(OPTION);
            ui.spacing_mut().item_spacing.x = OPTION_GAP;
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if inset > 0.0 {
                    // The frame's own margin already stands in for the gap.
                    ui.add_space((inset - OPTION_GAP).max(0.0));
                }
                for item in toolbar.options_trailing.iter().rev() {
                    option_ui(ui, item, chosen);
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.set_height(OPTION);
                    for item in &toolbar.options {
                        option_ui(ui, item, chosen);
                    }
                });
            });
        });
    let rect = row.response.rect;
    hairline(ui, rect.left(), rect.right(), rect.bottom());
}

/// One settings-row item, drawn at the row's size. Items the views do not
/// show in the row are drawn as the toolbar draws them.
fn option_ui(ui: &mut egui::Ui, item: &ToolItem, chosen: &mut Vec<Action>) {
    match item {
        ToolItem::Separator => {
            let (rect, _) = ui.allocate_exact_size(vec2(1.0, OPTION), Sense::hover());
            ui.painter().vline(
                rect.center().x,
                (rect.center().y - 10.0)..=(rect.center().y + 10.0),
                Stroke::new(1.0, SEPARATOR),
            );
        }
        ToolItem::Label { text, strong } => {
            let color = if *strong { widgets::TEXT } else { widgets::MUTED };
            caption(ui, text, color);
        }
        ToolItem::Color { rgb, tip, set } => color_chip(ui, *rgb, tip, set, chosen),
        ToolItem::Slider {
            caption,
            width,
            value,
            min,
            max,
            tint,
            set,
            finish,
            suffix,
        } => {
            let tint = tint.map_or(widgets::INK, |rgb| Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
            let id = ui.id().with(("option-slider", caption.as_str()));
            let look = SliderLook {
                height: OPTION,
                caption: widgets::MUTED,
                suffix: spaced(suffix),
            };
            let edit = bar_slider(ui, id, caption, *width, *value, (*min, *max), tint, &look);
            slider_actions(edit, set, finish.as_ref(), chosen);
        }
        ToolItem::Switch { caption: words, on, action } => {
            let font = option_font();
            let text_w = ui.painter().layout_no_wrap(words.clone(), font.clone(), widgets::MUTED).size().x;
            let (rect, response) =
                ui.allocate_exact_size(vec2(text_w + CAPTION_GAP + 34.0, OPTION), Sense::click());
            ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, words, font, widgets::MUTED);
            let track = Rect::from_min_size(
                pos2(rect.right() - 34.0, rect.center().y - 10.0),
                vec2(34.0, 20.0),
            );
            ui.painter().rect_filled(track, 10.0, if *on { SWITCH_ON } else { widgets::RAIL });
            let knob = if *on { track.right() - 10.0 } else { track.left() + 10.0 };
            widgets::disc(ui.painter(), pos2(knob, track.center().y), 7.5, Color32::WHITE);
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, *on, words)
            });
            if response.clicked() {
                chosen.push(action.clone());
            }
        }
        ToolItem::Segmented { options, selected, set } => segmented_words(ui, options, *selected, set, chosen),
        ToolItem::Choice {
            caption: words,
            options,
            selected,
            set,
        } => {
            let font = option_font();
            let widest = options
                .iter()
                .map(|option| ui.painter().layout_no_wrap(option.clone(), font.clone(), widgets::TEXT).size().x)
                .fold(0.0, f32::max);
            let rect = captioned(ui, words, (widest + 48.0).max(92.0));
            let labels: Vec<&str> = options.iter().map(String::as_str).collect();
            let text = labels.get(*selected).copied().unwrap_or_default();
            let id = ui.id().with(("option-choice", words.as_str()));
            if let Some(index) =
                widgets::dropdown(ui, rect, id, text, &labels, Some(*selected), None, widgets::Scale(1.0))
                && index != *selected
            {
                chosen.push(set.with_value(json!(index)));
            }
        }
        ToolItem::Number {
            caption: words,
            value,
            min,
            max,
            suffix,
            set,
        } => {
            // Whole numbers unless the declared value or range has a fraction.
            let places: u8 = if [*value, *min, *max].iter().all(|number| number.fract() == 0.0) { 0 } else { 1 };
            let scale = 10f64.powi(i32::from(places));
            let rect = captioned(ui, words, 64.0);
            let id = ui.id().with(("option-number", words.as_str()));
            let shown = |number: f64| (number * scale).round() as i32;
            let edit = widgets::number_field(
                ui,
                rect,
                id,
                shown(*value),
                (shown(*min), shown(*max)),
                &spaced(suffix),
                false,
                places,
                widgets::Scale(1.0),
            );
            if let Some(next) = edit.value
                && next != shown(*value)
            {
                chosen.push(set.with_value(json!(f64::from(next) / scale)));
            }
        }
        ToolItem::Push {
            label,
            dark,
            enabled,
            action,
        } => {
            let font = option_font();
            let text_w = ui.painter().layout_no_wrap(label.clone(), font.clone(), widgets::TEXT).size().x;
            let sense = if *enabled { Sense::click() } else { Sense::hover() };
            let (rect, response) = ui.allocate_exact_size(vec2((text_w + 28.0).max(64.0), OPTION), sense);
            let hovered = *enabled && response.hovered();
            let (fill, edge, text) = match (*dark, *enabled) {
                (true, true) => (if hovered { ICON } else { widgets::INK }, widgets::INK, Color32::WHITE),
                (true, false) => (ICON_QUIET, ICON_QUIET, Color32::WHITE),
                (false, true) => (if hovered { widgets::SELECTED } else { Color32::WHITE }, widgets::BORDER, widgets::TEXT),
                (false, false) => (Color32::WHITE, widgets::BORDER, ICON_QUIET),
            };
            widgets::outlined(ui.painter(), rect, 6.0, fill, edge);
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font, text);
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, *enabled, label));
            if *enabled && response.clicked() {
                chosen.push(action.clone());
            }
        }
        ToolItem::Inset
        | ToolItem::Swatches { .. }
        | ToolItem::Button { .. }
        | ToolItem::Menu { .. }
        | ToolItem::Maximize { .. } => item_ui(ui, item, chosen),
    }
}

/// Words in the settings row, such as a control's caption.
fn caption(ui: &mut egui::Ui, text: &str, color: Color32) {
    let font = option_font();
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x, OPTION), Sense::hover());
    ui.painter().galley(pos2(rect.left(), rect.center().y - galley.size().y / 2.0), galley, color);
}

/// Room for a caption and a control `width` wide, taken as one piece so a
/// right-hand group keeps the caption first. Paints the caption and returns
/// the control's place.
fn captioned(ui: &mut egui::Ui, words: &str, width: f32) -> Rect {
    let font = option_font();
    let galley = ui.painter().layout_no_wrap(words.to_string(), font, widgets::MUTED);
    let lead = if words.is_empty() { 0.0 } else { galley.size().x + CAPTION_GAP };
    let (rect, _) = ui.allocate_exact_size(vec2(lead + width, OPTION), Sense::hover());
    if !words.is_empty() {
        ui.painter().galley(pos2(rect.left(), rect.center().y - galley.size().y / 2.0), galley, widgets::MUTED);
    }
    Rect::from_min_max(pos2(rect.left() + lead, rect.top()), rect.max)
}

/// A unit after a number, as the views write them: "24 px", but "80%".
fn spaced(suffix: &str) -> String {
    if suffix.chars().next().is_some_and(char::is_alphabetic) {
        format!(" {suffix}")
    } else {
        suffix.to_string()
    }
}

/// Studio's swatches, then white, which painting needs and the palette lacks.
fn chip_palette() -> Vec<[u8; 3]> {
    let mut colors: Vec<[u8; 3]> = draw::defaults::PALETTE.iter().map(|color| color.rgb).collect();
    colors.push([255, 255, 255]);
    colors
}

/// The current colour as a round chip. A click opens Studio's swatches and
/// a picker on the menus' white card; a choice sends `set` with the colour.
fn color_chip(ui: &mut egui::Ui, rgb: [u8; 3], tip: &str, set: &Action, chosen: &mut Vec<Action>) {
    let (rect, response) = ui.allocate_exact_size(vec2(24.0, OPTION), Sense::click());
    let color = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    widgets::disc(ui.painter(), rect.center(), 10.0, color);
    if u16::from(rgb[0]) + u16::from(rgb[1]) + u16::from(rgb[2]) > 3 * 236 {
        widgets::ring(ui.painter(), rect.center(), 10.0, 1.0, widgets::BORDER);
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ColorButton, true, tip));
    let response = response.on_hover_text(tip);
    egui::Popup::menu(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .gap(4.0)
        .show(|ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
            ui.set_max_width(9.0 * 26.0);
            ui.horizontal_wrapped(|ui| {
                for swatch in chip_palette() {
                    let (spot, picked) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                    let fill = Color32::from_rgb(swatch[0], swatch[1], swatch[2]);
                    if swatch == rgb {
                        widgets::ring(ui.painter(), spot.center(), 11.0, 1.5, fill);
                    }
                    widgets::disc(ui.painter(), spot.center(), 9.0, fill);
                    widgets::ring(ui.painter(), spot.center(), 9.0, 1.0, widgets::BORDER);
                    if picked.clicked() {
                        chosen.push(set.with_value(json!(swatch)));
                        ui.close();
                    }
                }
            });
            ui.separator();
            let mut picked = color;
            if egui::color_picker::color_picker_color32(ui, &mut picked, egui::color_picker::Alpha::Opaque)
                && picked != color
            {
                chosen.push(set.with_value(json!([picked.r(), picked.g(), picked.b()])));
            }
        });
}

/// Words side by side in one bordered field, the chosen one shaded, as
/// selection.png draws New, Add, Subtract, and Intersect. A click sends `set`
/// with the word's index.
fn segmented_words(ui: &mut egui::Ui, options: &[String], selected: usize, set: &Action, chosen: &mut Vec<Action>) {
    let font = option_font();
    let widths: Vec<f32> = options
        .iter()
        .map(|option| ui.painter().layout_no_wrap(option.clone(), font.clone(), widgets::TEXT).size().x + 22.0)
        .collect();
    let (rect, _) = ui.allocate_exact_size(vec2(widths.iter().sum::<f32>() + 4.0, OPTION), Sense::hover());
    widgets::outlined(ui.painter(), rect, 6.0, Color32::WHITE, widgets::BORDER);
    let mut x = rect.left() + 2.0;
    for (index, option) in options.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(x, rect.top()), vec2(widths[index], OPTION));
        x += widths[index];
        let response = ui.interact(cell, ui.id().with(("option-segment", option.as_str(), index)), Sense::click());
        if index == selected {
            ui.painter().rect_filled(cell.shrink2(vec2(0.0, 3.0)), 4.0, widgets::SELECTED);
        } else if response.hovered() {
            ui.painter().rect_filled(cell.shrink2(vec2(0.0, 3.0)), 4.0, widgets::SELECTED.gamma_multiply(0.6));
        }
        // A divider between two words that are both unshaded.
        if index > 0 && index != selected && index - 1 != selected {
            ui.painter().vline(
                cell.left(),
                (cell.top() + 7.0)..=(cell.bottom() - 7.0),
                Stroke::new(1.0, widgets::BORDER),
            );
        }
        ui.painter().text(cell.center(), Align2::CENTER_CENTER, option, font.clone(), widgets::TEXT);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, index == selected, option)
        });
        if response.clicked() && index != selected {
            chosen.push(set.with_value(json!(index)));
        }
    }
}

/// A slider's edit as its declared actions: `set` with the value and whether
/// it is still held, and `finish` on release.
fn slider_actions(edit: BarEdit, set: &Action, finish: Option<&Action>, chosen: &mut Vec<Action>) {
    if let Some((value, live)) = edit.value {
        let mut action = set.with_value(json!(value));
        if let Some(serde_json::Value::Object(arg)) = action.arg.as_mut() {
            arg.insert("live".to_string(), json!(live));
        }
        chosen.push(action);
    }
    if edit.finished
        && let Some(finish) = finish
    {
        chosen.push(finish.clone());
    }
}

/// One declared item, drawn with the reference's widget for its kind.
fn item_ui(ui: &mut egui::Ui, item: &ToolItem, chosen: &mut Vec<Action>) {
    match item {
        ToolItem::Separator => separator(ui),
        ToolItem::Inset => ui.add_space(SWATCH_INSET),
        ToolItem::Swatches { swatches, sort } => {
            self::swatches(ui, swatches, sort.as_ref(), chosen);
        }
        ToolItem::Button {
            icon,
            tip,
            enabled,
            selected,
            action,
            menu,
        } => {
            let response = icon_button(ui, icon, Some(tip), *enabled, *selected);
            if response.clicked()
                && *enabled
                && let Some(action) = action
            {
                chosen.push(action.clone());
            }
            if let Some(action) =
                widgets::action_menu(&response, menu).and_then(|row| row.action.clone())
            {
                chosen.push(action);
            }
        }
        ToolItem::Menu {
            icon,
            label,
            enabled,
            rows,
        } => context_menu(ui, icon, label, *enabled, rows, chosen),
        ToolItem::Slider {
            caption,
            width,
            value,
            min,
            max,
            tint,
            set,
            finish,
            suffix,
        } => {
            let tint = tint.map_or(widgets::INK, |rgb| {
                Color32::from_rgb(rgb[0], rgb[1], rgb[2])
            });
            let id = ui.id().with(("bar-slider", caption.as_str()));
            let look = SliderLook {
                height: ROW,
                caption: ICON,
                suffix: spaced(suffix),
            };
            let edit = bar_slider(ui, id, caption, *width, *value, (*min, *max), tint, &look);
            slider_actions(edit, set, finish.as_ref(), chosen);
        }
        // The settings row's own controls, should a tab declare one in the
        // toolbar itself.
        ToolItem::Label { .. }
        | ToolItem::Color { .. }
        | ToolItem::Switch { .. }
        | ToolItem::Segmented { .. }
        | ToolItem::Choice { .. }
        | ToolItem::Number { .. }
        | ToolItem::Push { .. } => {
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                option_ui(ui, item, chosen);
            });
        }
        ToolItem::Maximize {
            icon,
            restore_icon,
            tip,
        } => {
            let maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));
            let icon = if maximized { restore_icon } else { icon };
            if icon_button(ui, icon, Some(tip), true, false).clicked() {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
        }
    }
}

/// The current tool's palette. Clicking a swatch sends its action.
/// Right-clicking a swatch opens its menu. With `sort`, dragging a swatch
/// along the row sorts the palette: the swatch follows the pointer, a dark
/// bar marks where it lands, and the release is one move.
fn swatches(
    ui: &mut egui::Ui,
    swatches: &[Swatch],
    sort: Option<&Action>,
    chosen: &mut Vec<Action>,
) {
    let sortable = sort.is_some();
    let drag_id = ui.id().with("swatch-drag");
    let mut dragging: Option<u64> = ui.data(|data| data.get_temp(drag_id));
    let mut rects = Vec::with_capacity(swatches.len());
    for swatch in swatches {
        let lifted = dragging == Some(swatch.id);
        let response = swatch_dot(
            ui,
            swatch.rgb,
            swatch.active && !lifted,
            swatch.muted || lifted,
            &swatch.name,
        )
        .on_hover_text(swatch.name.clone());
        rects.push(response.rect);
        if response.drag_started() && sortable {
            dragging = Some(swatch.id);
        } else if response.clicked() {
            chosen.push(swatch.click.clone());
        }
        if let Some(action) =
            widgets::action_menu(&response, &swatch.menu).and_then(|row| row.action.clone())
        {
            chosen.push(action);
        }
    }

    if let Some(id) = dragging {
        let from = swatches.iter().position(|swatch| swatch.id == id);
        let pointer = ui.input(|input| input.pointer.interact_pos());
        match (from, pointer) {
            (Some(from), Some(pointer)) => {
                let others: Vec<Rect> = rects
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != from)
                    .map(|(_, rect)| *rect)
                    .collect();
                let target = others
                    .iter()
                    .filter(|rect| rect.center().x < pointer.x)
                    .count();
                if target != from {
                    let x = match others.get(target) {
                        Some(rect) => rect.left(),
                        None => others
                            .last()
                            .map_or(rects[from].right(), |rect| rect.right()),
                    };
                    let center_y = rects[from].center().y;
                    ui.painter().vline(
                        x,
                        (center_y - 12.0)..=(center_y + 12.0),
                        Stroke::new(2.0, widgets::INK),
                    );
                }
                let rgb = swatches[from].rgb;
                ui.painter().circle_filled(
                    pos2(pointer.x, rects[from].center().y),
                    9.0,
                    Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
                );
                if ui.input(|input| input.pointer.any_released()) {
                    if target != from
                        && let Some(sort) = sort
                    {
                        let mut action = sort.with_value(json!(target));
                        if let Some(serde_json::Value::Object(arg)) = action.arg.as_mut() {
                            arg.insert("color".to_string(), json!(id));
                        }
                        chosen.push(action);
                    }
                    dragging = None;
                }
            }
            _ => dragging = None,
        }
    }
    ui.data_mut(|data| match dragging {
        Some(id) => {
            data.insert_temp(drag_id, id);
        }
        None => {
            data.remove::<u64>(drag_id);
        }
    });
}

fn swatch_dot(
    ui: &mut egui::Ui,
    rgb: [u8; 3],
    selected: bool,
    muted: bool,
    label: &str,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(SWATCH_W, ROW), Sense::click_and_drag());
    let center = rect.center();
    let fill = if selected { 9.0 } else { 8.5 };
    let color = swatch_color(rgb, muted);
    if selected {
        // The ring takes the swatch's own color, with a sliver of toolbar
        // between it and the fill.
        ui.painter()
            .circle_stroke(center, 11.5, Stroke::new(1.5, color));
    }
    ui.painter().circle_filled(center, fill, color);
    // A near-white swatch, such as a text background, would vanish into the
    // toolbar, so it gets a faint edge.
    if u16::from(color.r()) + u16::from(color.g()) + u16::from(color.b()) > 3 * 236 {
        ui.painter()
            .circle_stroke(center, fill, Stroke::new(1.0, widgets::BORDER));
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, label)
    });
    response
}

/// How much wider a menu button is than a plain one, for its chevron.
const CHEVRON_W: f32 = 14.0;

/// A button that shows its icon and a small chevron, and opens its menu of
/// commands. Each row of the menu shows the command's icon and label.
fn context_menu(
    ui: &mut egui::Ui,
    icon: &str,
    label: &str,
    enabled: bool,
    rows: &[MenuEntry],
    chosen: &mut Vec<Action>,
) {
    // A menu with no icon shows its label as text in a bordered box, a
    // dropdown such as a notebook's cell kind.
    if icon.is_empty() {
        let font = egui::FontId::proportional(13.0);
        let text_w = ui.painter().layout_no_wrap(label.to_string(), font.clone(), widgets::TEXT).size().x;
        let (rect, response) = ui.allocate_exact_size(vec2(text_w + 46.0, ROW), Sense::click());
        let field = Rect::from_center_size(rect.center(), vec2(rect.width() - 4.0, ROW - 10.0));
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
        let color = if !enabled { ICON_QUIET } else if open || response.hovered() { ICON_ACTIVE } else { ICON };
        widgets::outlined(ui.painter(), field, 6.0, Color32::WHITE, widgets::BORDER);
        ui.painter().text(pos2(field.left() + 12.0, field.center().y), egui::Align2::LEFT_CENTER, label, font, widgets::TEXT);
        paint_icon(ui.painter(), pos2(field.right() - 14.0, field.center().y), CHEVRON_ICON, 12.0, color);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, enabled, label));
        if !enabled {
            return;
        }
        egui::Popup::menu(&response).show(|ui| {
            if let Some(action) = widgets::entry_rows(ui, rows).and_then(|row| row.action.clone()) {
                chosen.push(action);
            }
        });
        return;
    }
    let (rect, response) =
        ui.allocate_exact_size(vec2(BUTTON_W + CHEVRON_W, ROW), Sense::click_and_drag());
    let side = BUTTON_W - 2.0;
    let plate = Rect::from_center_size(rect.center(), vec2(rect.width() - 2.0, side));
    // An open menu's button is drawn like the current tool: in blue.
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    let color = match (enabled, open || response.hovered()) {
        (true, true) => ICON_ACTIVE,
        (true, false) => ICON,
        (false, _) => ICON_QUIET,
    };
    let icon_center = pos2(plate.left() + side / 2.0, rect.center().y);
    paint_icon(ui.painter(), icon_center, icon, ICON_POINTS, color);
    paint_icon(
        ui.painter(),
        pos2(plate.right() - CHEVRON_W / 2.0 - 2.0, rect.center().y),
        CHEVRON_ICON,
        12.0,
        color,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let response = response.on_hover_text(label);
    if !enabled {
        return;
    }
    egui::Popup::menu(&response).show(|ui| {
        if let Some(action) = widgets::entry_rows(ui, rows).and_then(|row| row.action.clone()) {
            chosen.push(action);
        }
    });
}

/// The chevron on a menu button.
const CHEVRON_ICON: &str = "nav-arrow-down";

/// What a toolbar slider did this frame: a new value, live while it is being
/// dragged, and whether a drag ended.
struct BarEdit {
    value: Option<(i32, bool)>,
    finished: bool,
}

/// How a slider is drawn where it sits: the toolbar's row or the settings
/// row, with its caption's colour and the unit after its value.
struct SliderLook {
    height: f32,
    caption: Color32,
    suffix: String,
}

/// A caption, a slider, and its value as a number that can be typed, on one
/// row. The Size control and the text tool's sliders are this one control.
#[allow(clippy::too_many_arguments)]
fn bar_slider(
    ui: &mut egui::Ui,
    id: egui::Id,
    caption: &str,
    width: f32,
    value: i32,
    (min, max): (i32, i32),
    tint: Color32,
    look: &SliderLook,
) -> BarEdit {
    let (rect, _) = ui.allocate_exact_size(vec2(width, look.height), Sense::hover());
    let caption_rect = ui.painter().text(
        rect.left_center(),
        Align2::LEFT_CENTER,
        caption,
        FontId::proportional(13.5),
        look.caption,
    );
    // The value takes room for its widest number and its unit.
    let unit_w = if look.suffix.is_empty() {
        0.0
    } else {
        ui.painter()
            .layout_no_wrap(look.suffix.clone(), FontId::proportional(13.5), ICON)
            .size()
            .x
    };
    let value_w = 32.0 + unit_w;
    let track = Rect::from_min_max(
        pos2(caption_rect.right() + 10.0, rect.top()),
        pos2(rect.right() - value_w, rect.bottom()),
    );
    let value_rect = Rect::from_min_max(pos2(track.right() + 6.0, rect.top()), rect.max);
    let span = (max - min).max(1) as f32;
    let t = (value - min) as f32 / span;
    let mut edit = BarEdit {
        value: None,
        finished: false,
    };
    let (response, asked) = widgets::slider_in(ui, track, id, t, tint);
    if let Some(asked) = asked {
        let next = (min as f32 + asked * span).round() as i32;
        if next != value {
            edit.value = Some((next, true));
        }
    }
    if response.drag_stopped() || response.clicked() {
        edit.finished = true;
    }
    response.widget_info(|| egui::WidgetInfo::slider(true, f64::from(value), caption));

    let mut field = ui.new_child(
        UiBuilder::new()
            .max_rect(value_rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    // A narrow field, so the number sits beside the slider's end.
    field.spacing_mut().interact_size.x = 22.0;
    let visuals = field.visuals_mut();
    visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.inactive.fg_stroke.color = ICON;
    visuals.widgets.hovered.weak_bg_fill = Color32::TRANSPARENT;
    visuals.widgets.hovered.bg_stroke = Stroke::NONE;
    let mut typed = value;
    let typed_response = field.add(
        egui::DragValue::new(&mut typed)
            .range(min..=max)
            .speed(0.08)
            .max_decimals(0)
            .suffix(look.suffix.as_str()),
    );
    if typed_response.changed() {
        edit.value = Some((typed.clamp(min, max), typed_response.dragged()));
    }
    if typed_response.drag_stopped() {
        edit.finished = true;
    }
    edit
}

fn swatch_color(rgb: [u8; 3], muted: bool) -> Color32 {
    if !muted {
        return Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    }
    Color32::from_rgb(mix_white(rgb[0]), mix_white(rgb[1]), mix_white(rgb[2]))
}

fn mix_white(channel: u8) -> u8 {
    (u16::from(channel) * 45 / 100 + 255 * 55 / 100) as u8
}

/// A 1-point line along the bottom edge of a bar, inside it, so the next bar
/// painted below does not cover it.
pub(crate) fn hairline(ui: &egui::Ui, from: f32, to: f32, bottom: f32) {
    if to <= from {
        return;
    }
    ui.painter()
        .hline(from..=to, bottom - 0.5, Stroke::new(1.0, HAIRLINE));
}

fn separator(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(SEPARATOR_W, ROW), Sense::hover());
    let center = rect.center();
    ui.painter().vline(
        center.x,
        (center.y - 12.0)..=(center.y + 12.0),
        Stroke::new(1.0, SEPARATOR),
    );
}
