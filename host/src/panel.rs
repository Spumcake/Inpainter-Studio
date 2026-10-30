//! The properties panel, from Simple Sketch 1.0.1's `panel.rs`. It draws each
//! declared row from its kind and sends a property or an action. Every row
//! sits on one grid — a label column, then the control, then a value field —
//! so rows line up from section to section, and every control comes from
//! `widgets`, shared with the toolbar.

use crate::widgets::{self, BORDER, INK, MUTED, Scale, TEXT};
use draw::document::{Cap, CurvePoint, Dash, Sample};
use draw::ink;
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Layout, Painter, PopupCloseBehavior, Rect, Sense,
    Stroke, StrokeKind, UiBuilder, pos2, vec2,
};
use protocol::{Action, ChoicePreview, PresetSample, PropValue, Properties, Row, RowKind};
use serde_json::Value;

/// What the panel and the outliner send.
pub(crate) enum Sent {
    Run(Action),
    Property {
        prop: Value,
        value: PropValue,
        preview: bool,
    },
}

/// The panel draws its contents at this scale of the toolbar's size.
const S: Scale = Scale::PANEL;

/// The contents, 268 points wide at full size, at the panel's scale, plus
/// the side padding, rounded to a whole point so the edge stays crisp.
pub(crate) const WIDTH: f32 = 266.0;

const PAD: f32 = 16.0;
const ROW: f32 = S.of(30.0);
const ROW_GAP: f32 = S.of(8.0);
const LABEL_W: f32 = S.of(92.0);
const NUMBER_W: f32 = S.of(56.0);
const GAP: f32 = S.of(10.0);
const PRESET_SAMPLE_W: f32 = S.of(84.0);
const CURVE_H: f32 = S.of(140.0);
/// Space above the first heading. The panel has no title bar.
const TOP: f32 = S.of(22.0);
const HEADING: f32 = S.of(20.0);
/// How far below the panel's top a page's first line of text is centered:
/// the first section heading, or the outliner's header row.
pub(crate) const FIRST_LINE: f32 = TOP + HEADING / 2.0;
const TAB: egui::Vec2 = vec2(22.0, 40.0);
/// The tab's distance below the top of the canvas.
const TAB_TOP: f32 = 18.0;

const PLOT: Color32 = Color32::from_rgb(247, 248, 250);
const PLOT_GRID: Color32 = Color32::from_rgb(232, 234, 238);
const ERASER_SAMPLE: Color32 = Color32::from_rgb(160, 164, 170);

/// Draws the panel in `rect`: the declared properties, or an empty column
/// when the tab declares the panel but no properties.
pub(crate) fn show(ui: &mut egui::Ui, properties: Option<&Properties>, rect: Rect) -> Vec<Sent> {
    let mut sent = Vec::new();
    ui.painter().rect_filled(rect, 0.0, Color32::WHITE);
    ui.painter()
        .vline(rect.left() + 0.5, rect.y_range(), Stroke::new(1.0, BORDER));

    // Too narrow for the padding: nothing can be drawn there this frame.
    if rect.width() <= 2.0 * PAD {
        return sent;
    }
    let content = rect.shrink2(vec2(PAD, 0.0));
    let mut panel = ui.new_child(
        UiBuilder::new()
            .max_rect(content)
            .layout(Layout::top_down(Align::Min)),
    );
    panel.set_clip_rect(rect);
    panel.spacing_mut().item_spacing = vec2(0.0, 0.0);
    // No scroll bars: the page scrolls with the wheel or by dragging it.
    let scrolled = egui::ScrollArea::vertical()
        .id_salt("properties-panel")
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .scroll_source(egui::scroll_area::ScrollSource {
            scroll_bar: false,
            drag: egui::scroll_area::DragScroll::Always,
            mouse_wheel: true,
        })
        .show(&mut panel, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            ui.set_width(content.width());
            let Some(properties) = properties else {
                ui.add_space(PAD);
                return;
            };
            // With page tabs, as the asset-editor mockup shows: the tabs,
            // then the chosen layer's settings, then the list of layers.
            if !properties.pages.is_empty() {
                page_tabs(ui, &properties.pages, &mut sent);
                for (index, section) in properties.sections.iter().enumerate() {
                    draw_section(ui, &section.title, &section.rows, &properties.finish, index == 0, &mut sent);
                }
                if let Some(layers) = &properties.layers {
                    if !properties.sections.is_empty() {
                        ui.add_space(S.of(14.0));
                        let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                        ui.painter().hline(line.x_range(), line.center().y, Stroke::new(1.0, BORDER));
                        ui.add_space(S.of(4.0));
                    }
                    crate::outliner::show(ui, layers, true, &mut sent);
                }
                ui.add_space(PAD);
                return;
            }
            if let Some(layers) = &properties.layers {
                crate::outliner::show(ui, layers, false, &mut sent);
            }
            // A page opened from another, such as a color from an object's
            // palette or the outliner, starts with the way back to it.
            if let Some(back) = &properties.back {
                back_row(ui, &back.label, &back.action, &mut sent);
            }
            for (index, section) in properties.sections.iter().enumerate() {
                draw_section(
                    ui,
                    &section.title,
                    &section.rows,
                    &properties.finish,
                    index == 0 && properties.back.is_none(),
                    &mut sent,
                );
            }
            ui.add_space(PAD);
        });
    widgets::drag_scroll_cursor(ui, scrolled.id);
    sent
}

/// The tab that shows and hides the panel. It sits at the panel's left edge,
/// or at the canvas's right edge while the panel is closed, on a layer above
/// the canvas so a click on it never draws. Returns whether it was clicked.
pub(crate) fn toggle_tab(
    ctx: &egui::Context,
    open: bool,
    tip: &str,
    edge_x: f32,
    top: f32,
) -> bool {
    let origin = pos2(edge_x - TAB.x, top + TAB_TOP);
    let mut clicked = false;
    egui::Area::new(egui::Id::new("panel-toggle"))
        .order(egui::Order::Foreground)
        .fixed_pos(origin)
        .show(ctx, |ui| {
            // One point wider than the tab, so it covers the panel's hairline
            // and joins the panel instead of sitting beside it.
            let (rect, response) = ui.allocate_exact_size(TAB + vec2(1.0, 0.0), Sense::click());
            let response = response.on_hover_text(tip);
            let radius = CornerRadius {
                nw: 10,
                sw: 10,
                ne: 0,
                se: 0,
            };
            let fill = if response.hovered() {
                widgets::SELECTED
            } else {
                Color32::WHITE
            };
            ui.painter().rect(
                rect,
                radius,
                fill,
                Stroke::new(1.0, BORDER),
                StrokeKind::Inside,
            );
            if open {
                let seam = Rect::from_min_max(
                    pos2(rect.right() - 1.5, rect.top() + 1.0),
                    pos2(rect.right(), rect.bottom() - 1.0),
                );
                ui.painter().rect_filled(seam, 0.0, fill);
            }
            // Centered on the tab's visible inside: from the inner edge of
            // its border to the panel edge, which the tab overlaps by one
            // point. The rounded left side is part of that inside.
            let inside = Rect::from_min_max(pos2(rect.left() + 1.0, rect.top()), rect.max);
            let center = pos2(inside.center().x + 0.5, rect.center().y);
            widgets::chevron_side(ui.painter(), center, open, TEXT);
            clicked = response.clicked();
        });
    clicked
}

/// The page tabs' height, and the space above them.
const PAGE_H: f32 = S.of(40.0);
const PAGE_TOP: f32 = S.of(14.0);
/// A page tab's words, and the room on either side of them.
const PAGE_TEXT: f32 = S.of(15.5);
const PAGE_PAD: f32 = S.of(14.0);

/// Page tabs across the panel's top, as the asset-editor mockup shows: the
/// chosen page underlined, the others in gray,
/// and a page that waits lighter still. A click on another page that can be
/// used sends its action.
fn page_tabs(ui: &mut egui::Ui, pages: &[protocol::PanelPage], sent: &mut Vec<Sent>) {
    ui.add_space(PAGE_TOP);
    let width = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(width, PAGE_H), Sense::hover());
    let font = |active: bool| {
        if active {
            widgets::semibold(PAGE_TEXT)
        } else {
            egui::FontId::proportional(PAGE_TEXT)
        }
    };
    let galleys: Vec<_> = pages
        .iter()
        .map(|page| ui.painter().layout_no_wrap(page.label.clone(), font(page.active), TEXT))
        .collect();
    let words: f32 = galleys.iter().map(|galley| galley.size().x).sum();
    // Narrow panels squeeze the room around the words before the words.
    let pad = PAGE_PAD.min(((width - words) / (2.0 * pages.len().max(1) as f32)).max(S.of(4.0)));
    let mut x = row.left();
    for (index, (page, galley)) in pages.iter().zip(galleys).enumerate() {
        let tab = Rect::from_min_size(pos2(x, row.top()), vec2(galley.size().x + 2.0 * pad, PAGE_H));
        x = tab.right();
        let usable = page.enabled && !page.active && page.action.is_some();
        let sense = if usable { Sense::click() } else { Sense::hover() };
        let response = ui.interact(tab, ui.id().with(("panel-page", index)), sense);
        // The chosen page is marked by its underline alone, with no raised
        // tab or shadow, as the owner set on 2026-10-30.
        if page.active {
            ui.painter().rect_filled(
                Rect::from_min_max(pos2(tab.left() + pad, tab.bottom() - 2.0), pos2(tab.right() - pad, tab.bottom())),
                0.0,
                INK,
            );
        }
        let color = match (page.active, page.enabled, usable && response.hovered()) {
            (true, _, _) => TEXT,
            (false, false, _) => widgets::quiet(false),
            (false, true, true) => TEXT,
            (false, true, false) => MUTED,
        };
        let at = pos2(tab.center().x - galley.size().x / 2.0, tab.center().y - galley.size().y / 2.0);
        ui.painter().galley_with_override_text_color(at, galley, color);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, page.enabled, page.active, &page.label)
        });
        if usable
            && response.clicked()
            && let Some(action) = &page.action
        {
            sent.push(Sent::Run(action.clone()));
        }
    }
}

/// A caption and a row of icons that each turn on and off, as the mockup
/// draws "Lock:" and its four locks. A click sends the icon's index.
fn icons_row(ui: &mut egui::Ui, prop: &Value, row: Rect, label: &str, icons: &[protocol::IconToggle], sent: &mut Vec<Sent>) {
    let mut x = row.left();
    if !label.is_empty() {
        let shown = ui.painter().text(row.left_center(), Align2::LEFT_CENTER, label, widgets::body(S), MUTED);
        x = shown.right() + S.of(14.0);
    }
    let cell = S.of(34.0);
    for (index, icon) in icons.iter().enumerate() {
        let spot = Rect::from_min_size(pos2(x + index as f32 * (cell + S.of(6.0)), row.top()), vec2(cell, row.height()));
        let response = ui
            .interact(spot, ui.id().with(("panel-icon", prop.to_string(), index)), Sense::click_and_drag())
            .on_hover_text(&icon.tip);
        let plate = Rect::from_center_size(spot.center(), vec2(cell, cell).min(spot.size()));
        if icon.on {
            ui.painter().rect_filled(plate, S.of(6.0), ICON_ON_PLATE);
        } else if response.hovered() {
            ui.painter().rect_filled(plate, S.of(6.0), widgets::SELECTED);
        }
        let color = if icon.on { crate::toolbar::ICON_ACTIVE } else { TEXT };
        crate::toolbar::paint_icon(ui.painter(), spot.center(), &icon.icon, S.of(19.0), color);
        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, icon.on, &icon.tip));
        if response.clicked() {
            set(sent, prop, PropValue::Index(index as u8));
        }
    }
}

/// The plate behind an icon that is on.
const ICON_ON_PLATE: Color32 = Color32::from_rgb(226, 236, 252);

/// A left chevron and the name of the page this one was opened from, on the
/// panel's first line. Clicking it returns there.
fn back_row(ui: &mut egui::Ui, label: &str, action: &Action, sent: &mut Vec<Sent>) {
    ui.add_space(FIRST_LINE - ROW / 2.0);
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, ROW), Sense::click_and_drag());
    if response.hovered() {
        ui.painter().rect_filled(rect, S.of(6.0), widgets::SELECTED);
    }
    widgets::chevron_side(
        ui.painter(),
        pos2(rect.left() + S.of(10.0), rect.center().y),
        false,
        TEXT,
    );
    ui.painter().text(
        pos2(rect.left() + S.of(22.0), rect.center().y),
        Align2::LEFT_CENTER,
        label,
        widgets::body(S),
        TEXT,
    );
    let response = response.on_hover_text(format!("Back to {label}"));
    if response.clicked() {
        sent.push(Sent::Run(action.clone()));
    }
}

/// A section: a hairline above every section but the first, its heading,
/// then its rows.
fn draw_section(
    ui: &mut egui::Ui,
    title: &str,
    rows: &[Row],
    finish: &Action,
    first: bool,
    sent: &mut Vec<Sent>,
) {
    let width = ui.available_width();
    if first {
        ui.add_space(TOP);
    } else {
        ui.add_space(S.of(14.0));
        let (line, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
        ui.painter()
            .hline(line.x_range(), line.center().y, Stroke::new(1.0, BORDER));
        ui.add_space(S.of(14.0));
    }
    let heading = if title.is_empty() {
        ui.add_space(if first { S.of(4.0) } else { 0.0 });
        Rect::from_min_size(ui.cursor().min, vec2(width, 0.0))
    } else {
        let (heading, _) = ui.allocate_exact_size(vec2(width, HEADING), Sense::hover());
        ui.painter().text(
            heading.left_center(),
            Align2::LEFT_CENTER,
            title,
            widgets::semibold(S.of(15.0)),
            TEXT,
        );
        heading
    };
    let _ = heading;
    ui.add_space(S.of(10.0));
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            ui.add_space(ROW_GAP);
        }
        draw_row(ui, row, finish, sent);
    }
}

/// The label column and the space to its right. A row without a label uses
/// the whole width for its control.
fn split_row(ui: &egui::Ui, row: Rect, label: &str) -> Rect {
    if label.is_empty() {
        return row;
    }
    ui.painter().text(
        row.left_center(),
        Align2::LEFT_CENTER,
        label,
        widgets::body(S),
        MUTED,
    );
    Rect::from_min_max(pos2(row.left() + LABEL_W, row.top()), row.max)
}

fn row_id(ui: &egui::Ui, prop: &Value) -> egui::Id {
    ui.id().with(("panel-row", prop.to_string()))
}

fn set(sent: &mut Vec<Sent>, prop: &Value, value: PropValue) {
    sent.push(Sent::Property {
        prop: prop.clone(),
        value,
        preview: false,
    });
}

fn preview(sent: &mut Vec<Sent>, prop: &Value, value: PropValue) {
    sent.push(Sent::Property {
        prop: prop.clone(),
        value,
        preview: true,
    });
}

fn draw_row(ui: &mut egui::Ui, desc: &Row, finish: &Action, sent: &mut Vec<Sent>) {
    let value = desc.value.clone();
    let width = ui.available_width();
    if let RowKind::Palette {
        owner,
        colors,
        sort,
        flags,
    } = &desc.kind
    {
        crate::outliner::palette(ui, *owner, colors, sort.as_ref(), flags, sent);
        return;
    }
    let height = match desc.kind {
        RowKind::Curve => CURVE_H,
        _ => ROW,
    };
    let (row, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let id = row_id(ui, &desc.prop);
    let prop = &desc.prop;
    match (&desc.kind, value) {
        (RowKind::Icons { icons }, _) => icons_row(ui, prop, row, &desc.label, icons, sent),
        (RowKind::Toggle, PropValue::Bool(on)) => {
            ui.painter().text(
                row.left_center(),
                Align2::LEFT_CENTER,
                &desc.label,
                widgets::body(S),
                MUTED,
            );
            let spot = Rect::from_min_max(pos2(row.right() - S.of(40.0), row.top()), row.max);
            if widgets::toggle(ui, spot, id, on, S).clicked() {
                set(sent, prop, PropValue::Bool(!on));
            }
        }
        (
            RowKind::Number {
                min,
                max,
                percent,
                places,
            },
            PropValue::Number(number),
        ) => {
            let control = split_row(ui, row, &desc.label);
            number_row(
                ui,
                prop,
                control,
                id,
                (*min, *max),
                *percent,
                *places,
                number,
                false,
                finish,
                sent,
            );
        }
        (RowKind::Text, PropValue::Text(text)) => {
            let control = split_row(ui, row, &desc.label);
            if let Some(next) = widgets::text_field(ui, control, id, &text, "", S)
                && next != text
            {
                set(sent, prop, PropValue::Text(next));
            }
        }
        (RowKind::Text, PropValue::Mixed) => {
            let control = split_row(ui, row, &desc.label);
            if let Some(next) = widgets::text_field(ui, control, id, "", "Mixed", S)
                && !next.trim().is_empty()
            {
                set(sent, prop, PropValue::Text(next));
            }
        }
        (
            RowKind::Number {
                min,
                max,
                percent,
                places,
            },
            PropValue::Mixed,
        ) => {
            let control = split_row(ui, row, &desc.label);
            let mid = (min + max) / 2.0;
            number_row(
                ui,
                prop,
                control,
                id,
                (*min, *max),
                *percent,
                *places,
                mid,
                true,
                finish,
                sent,
            );
        }
        (
            RowKind::Choice {
                options,
                segmented,
                preview,
            },
            PropValue::Index(index),
        ) => {
            let control = split_row(ui, row, &desc.label);
            let options: Vec<&str> = options.iter().map(String::as_str).collect();
            let index = usize::from(index);
            let preview = *preview;
            let chosen = if *segmented {
                let paint = |painter: &Painter, cell: Rect, option: usize, selected: bool| {
                    paint_option(painter, cell, preview, options[option], selected);
                };
                widgets::segmented(ui, control, id, index, &options, &paint, S)
            } else {
                let sample = |painter: &Painter, area: Rect, option: usize| {
                    if preview == ChoicePreview::Dash {
                        widgets::dash_sample(painter, area, options[option] == "Dashed", S);
                    }
                };
                let text = options.get(index).copied().unwrap_or_default();
                let preview_fn: Option<widgets::OptionPreview> =
                    (preview == ChoicePreview::Dash).then_some(&sample);
                widgets::dropdown(ui, control, id, text, &options, Some(index), preview_fn, S)
            };
            if let Some(chosen) = chosen
                && chosen != index
            {
                set(sent, prop, PropValue::Index(chosen as u8));
            }
        }
        (
            RowKind::Preset {
                options,
                custom,
                sample,
            },
            PropValue::Index(index),
        ) => {
            preset_row(
                ui,
                prop,
                row,
                id,
                index,
                options,
                custom,
                sample.as_ref(),
                sent,
            );
        }
        (RowKind::Rgb { quick }, PropValue::Rgb(rgb)) => {
            let control = split_row(ui, row, &desc.label);
            color_well(ui, prop, control, id, Some(rgb), quick, finish, sent);
        }
        (RowKind::Rgb { quick }, PropValue::Mixed) => {
            let control = split_row(ui, row, &desc.label);
            color_well(ui, prop, control, id, None, quick, finish, sent);
        }
        (RowKind::Toggle, PropValue::Mixed) => {
            ui.painter().text(
                row.left_center(),
                Align2::LEFT_CENTER,
                &desc.label,
                widgets::body(S),
                MUTED,
            );
            let spot = Rect::from_min_max(pos2(row.right() - S.of(72.0), row.top()), row.max);
            let response = ui.interact(spot, id, Sense::click_and_drag());
            ui.painter().text(
                spot.right_center(),
                Align2::RIGHT_CENTER,
                "Mixed",
                widgets::body(S),
                MUTED,
            );
            if response.clicked() {
                set(sent, prop, PropValue::Bool(true));
            }
        }
        (
            RowKind::Choice {
                options,
                segmented: false,
                preview,
            },
            PropValue::Mixed,
        ) => {
            let control = split_row(ui, row, &desc.label);
            let options: Vec<&str> = options.iter().map(String::as_str).collect();
            let preview = *preview;
            let sample = |painter: &Painter, area: Rect, option: usize| {
                if preview == ChoicePreview::Dash {
                    widgets::dash_sample(painter, area, options[option] == "Dashed", S);
                }
            };
            let preview_fn: Option<widgets::OptionPreview> =
                (preview == ChoicePreview::Dash).then_some(&sample);
            if let Some(chosen) =
                widgets::dropdown(ui, control, id, "Mixed", &options, None, preview_fn, S)
            {
                set(sent, prop, PropValue::Index(chosen as u8));
            }
        }
        (RowKind::Curve, PropValue::Curve(points)) => {
            curve_editor(ui, prop, row, id, &points, finish, sent);
        }
        _ => {}
    }
}

/// A choice cell's content: a cap sample or a line sample.
fn paint_option(
    painter: &Painter,
    cell: Rect,
    preview: ChoicePreview,
    option: &str,
    selected: bool,
) {
    let color = if selected { INK } else { TEXT };
    match preview {
        ChoicePreview::Cap => {
            widgets::cap_sample(painter, cell.center(), option == "Round", color, S)
        }
        ChoicePreview::Label => {
            painter.text(
                cell.center(),
                Align2::CENTER_CENTER,
                option,
                widgets::body(S),
                color,
            );
        }
        ChoicePreview::Dash => widgets::dash_sample(
            painter,
            cell.shrink2(vec2(S.of(12.0), 0.0)),
            option == "Dashed",
            S,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn number_row(
    ui: &mut egui::Ui,
    prop: &Value,
    control: Rect,
    id: egui::Id,
    (min, max): (f64, f64),
    percent: bool,
    places: u8,
    value: f64,
    mixed: bool,
    finish: &Action,
    sent: &mut Vec<Sent>,
) {
    let scale = if percent {
        100.0
    } else {
        10f64.powi(i32::from(places))
    };
    let shown_min = (min * scale).round() as i32;
    let shown_max = (max * scale).round() as i32;
    let shown = (value * scale).round() as i32;
    let track = Rect::from_min_max(
        control.min,
        pos2(control.right() - NUMBER_W - GAP, control.bottom()),
    );
    let field = Rect::from_min_max(pos2(control.right() - NUMBER_W, control.top()), control.max);

    let span = (max - min).max(f64::EPSILON);
    let t = ((value - min) / span) as f32;
    let (response, asked) = widgets::slider(ui, track, id.with("slider"), t);
    if let Some(asked) = asked {
        let next = min + f64::from(asked) * span;
        let next = (next * scale).round() / scale;
        if (next - value).abs() > f64::EPSILON {
            preview(sent, prop, PropValue::Number(next));
        }
    }
    if response.drag_stopped() || response.clicked() {
        sent.push(Sent::Run(finish.clone()));
    }

    let suffix = if percent { "%" } else { "" };
    let edit = widgets::number_field(
        ui,
        field,
        id.with("field"),
        shown,
        (shown_min, shown_max),
        suffix,
        mixed,
        places,
        S,
    );
    match (edit.value, edit.finished) {
        (Some(next), true) => {
            set(sent, prop, PropValue::Number(f64::from(next) / scale));
        }
        (Some(next), false) => {
            preview(sent, prop, PropValue::Number(f64::from(next) / scale));
        }
        (None, true) => sent.push(Sent::Run(finish.clone())),
        (None, false) => {}
    }
}

/// A stroke sample drawn with the tool's own dynamics, then the preset list.
#[allow(clippy::too_many_arguments)]
fn preset_row(
    ui: &mut egui::Ui,
    prop: &Value,
    row: Rect,
    id: egui::Id,
    index: u8,
    options: &[String],
    custom: &str,
    sample: Option<&PresetSample>,
    sent: &mut Vec<Sent>,
) {
    let area = Rect::from_min_size(row.min, vec2(PRESET_SAMPLE_W, row.height()));
    widgets::field(ui.painter(), area, false, S);
    if let Some(sample) = sample
        && let Some(dynamics) = draw::document::snapshot::dynamics_from_value(&sample.dynamics)
    {
        let color = if sample.eraser { ERASER_SAMPLE } else { INK };
        let diameter = f64::from(S.of(if sample.eraser { 9.0 } else { 6.0 }));
        paint_stroke_sample(
            ui.painter(),
            area.shrink2(vec2(S.of(12.0), S.of(8.0))),
            &dynamics,
            diameter,
            color,
        );
    }
    let list = Rect::from_min_max(pos2(area.right() + S.of(8.0), row.top()), row.max);
    let options: Vec<&str> = options.iter().map(String::as_str).collect();
    let selected = (usize::from(index) < options.len()).then_some(usize::from(index));
    let text = selected
        .and_then(|index| options.get(index).copied())
        .unwrap_or(custom);
    if let Some(chosen) = widgets::dropdown(ui, list, id, text, &options, selected, None, S) {
        set(sent, prop, PropValue::Index(chosen as u8));
    }
}

/// A gentle S-curve with pressure rising and falling, outlined exactly as a
/// stroke would be with these dynamics.
fn paint_stroke_sample(
    painter: &Painter,
    area: Rect,
    dynamics: &draw::document::Dynamics,
    diameter: f64,
    color: Color32,
) {
    let steps = 24;
    let samples: Vec<Sample> = (0..=steps)
        .map(|step| {
            let t = f64::from(step) / f64::from(steps);
            let x = f64::from(area.left()) + t * f64::from(area.width());
            let wave = (t * std::f64::consts::TAU).sin() * 0.35;
            let y = f64::from(area.center().y) - wave * f64::from(area.height());
            let pressure = 0.25 + 0.75 * (t * std::f64::consts::PI).sin();
            Sample {
                x,
                y,
                pressure: Some(pressure),
            }
        })
        .collect();
    let Some(shapes) =
        ink::stroke_visible(&samples, dynamics, &[], diameter, Cap::Round, Dash::Solid)
    else {
        return;
    };
    let Ok(mesh) = ink::mesh_from_shapes(shapes) else {
        return;
    };
    let mut paint = egui::Mesh::default();
    for triangle in &mesh.triangles {
        let base = paint.vertices.len() as u32;
        for point in triangle {
            paint.colored_vertex(pos2(point[0] as f32, point[1] as f32), color);
        }
        paint.add_triangle(base, base + 1, base + 2);
    }
    painter.add(egui::Shape::mesh(paint));
}

/// A swatch and chevron in a field. Clicking opens a color picker; a drag in
/// the picker becomes one undo step when it is released.
#[allow(clippy::too_many_arguments)]
fn color_well(
    ui: &mut egui::Ui,
    prop: &Value,
    control: Rect,
    id: egui::Id,
    rgb: Option<[u8; 3]>,
    swatches: &[[u8; 3]],
    finish: &Action,
    sent: &mut Vec<Sent>,
) {
    // A chosen color is a swatch at the field's end. Mixed uses the whole
    // field for the word, so a placeholder gray is not a selected color.
    let well = match rgb {
        Some(_) => Rect::from_min_max(
            pos2(control.right() - S.of(64.0), control.top()),
            control.max,
        ),
        None => control,
    };
    let response = ui.interact(well, id, Sense::click_and_drag());
    widgets::field(ui.painter(), well, response.hovered(), S);
    let color = if let Some(rgb) = rgb {
        let color = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
        let dot = pos2(well.left() + S.of(20.0), well.center().y);
        ui.painter().circle_filled(dot, S.of(9.0), color);
        ui.painter()
            .circle_stroke(dot, S.of(9.0), Stroke::new(1.0, BORDER));
        color
    } else {
        ui.painter().text(
            pos2(well.left() + S.of(10.0), well.center().y),
            Align2::LEFT_CENTER,
            "Mixed",
            widgets::body(S),
            MUTED,
        );
        Color32::WHITE
    };
    widgets::chevron(
        ui.painter(),
        pos2(well.right() - S.of(14.0), well.center().y),
        TEXT,
        S,
    );
    let open = egui::Popup::menu(&response)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .gap(S.of(4.0))
        .show(|ui| {
            if !swatches.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for rgb in swatches {
                        let (spot, response) =
                            ui.allocate_exact_size(vec2(S.of(22.0), S.of(22.0)), Sense::click());
                        let fill = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                        ui.painter().circle_filled(spot.center(), S.of(8.0), fill);
                        ui.painter().circle_stroke(
                            spot.center(),
                            S.of(8.0),
                            Stroke::new(1.0, BORDER),
                        );
                        if response.clicked() {
                            set(sent, prop, PropValue::Rgb(*rgb));
                        }
                    }
                });
                ui.separator();
            }
            let mut picked = color;
            if egui::color_picker::color_picker_color32(
                ui,
                &mut picked,
                egui::color_picker::Alpha::Opaque,
            ) {
                preview(
                    sent,
                    prop,
                    PropValue::Rgb([picked.r(), picked.g(), picked.b()]),
                );
            }
        })
        .is_some();
    if open && ui.input(|input| input.pointer.any_released()) {
        sent.push(Sent::Run(finish.clone()));
    }
}

fn curve_editor(
    ui: &mut egui::Ui,
    prop: &Value,
    rect: Rect,
    id: egui::Id,
    points: &[(f64, f64)],
    finish: &Action,
    sent: &mut Vec<Sent>,
) {
    let response = ui.interact(rect, id, Sense::click_and_drag());
    ui.painter().rect(
        rect,
        S.of(8.0),
        PLOT,
        Stroke::new(1.0, BORDER),
        egui::StrokeKind::Inside,
    );
    let plot = rect.shrink(S.of(16.0));
    for step in 1..4 {
        let y = plot.bottom() - plot.height() * (step as f32 / 4.0);
        ui.painter()
            .hline(plot.left()..=plot.right(), y, Stroke::new(1.0, PLOT_GRID));
    }
    let curve: Vec<CurvePoint> = points
        .iter()
        .map(|(x, y)| CurvePoint { x: *x, y: *y })
        .collect();
    let line: Vec<egui::Pos2> = (0..=48)
        .map(|step| {
            let x = f64::from(step) / 48.0;
            plot_pos(plot, x, ink::curve_y(&curve, x))
        })
        .collect();
    ui.painter()
        .add(egui::Shape::line(line, Stroke::new(S.of(2.0), INK)));
    for point in points {
        let center = plot_pos(plot, point.0, point.1);
        ui.painter().circle_filled(center, S.of(5.0), INK);
        ui.painter()
            .circle_stroke(center, S.of(5.0), Stroke::new(1.5, Color32::WHITE));
    }

    let pointer = response.interact_pointer_pos().or(response.hover_pos());
    let hit = pointer.and_then(|pos| hit_point(plot, points, pos));
    if response.double_clicked()
        && let Some(index) = hit
        && points.len() > 2
    {
        let mut next = points.to_vec();
        next.remove(index);
        pin_curve_ends(&mut next);
        set(sent, prop, PropValue::Curve(next));
        return;
    }
    if response.drag_started()
        && let Some(index) = hit
    {
        ui.data_mut(|data| data.insert_temp(response.id, index));
    }
    if response.dragged()
        && let Some(index) = ui.data(|data| data.get_temp::<usize>(response.id))
        && let Some(pos) = pointer
        && index < points.len()
    {
        let (x, y) = plot_value(plot, pos);
        let mut next = points.to_vec();
        move_curve_point(&mut next, index, x, y);
        preview(sent, prop, PropValue::Curve(next));
    }
    if response.drag_stopped() {
        ui.data_mut(|data| data.remove::<usize>(response.id));
        sent.push(Sent::Run(finish.clone()));
    } else if response.clicked()
        && hit.is_none()
        && points.len() < 5
        && let Some(pos) = pointer
    {
        let (x, y) = plot_value(plot, pos);
        let mut next = points.to_vec();
        if insert_curve_point(&mut next, x, y) {
            set(sent, prop, PropValue::Curve(next));
        }
    }
}

fn plot_pos(plot: Rect, x: f64, y: f64) -> egui::Pos2 {
    pos2(
        plot.left() + (x as f32) * plot.width(),
        plot.bottom() - (y as f32) * plot.height(),
    )
}

fn plot_value(plot: Rect, pos: egui::Pos2) -> (f64, f64) {
    let x = f64::from((pos.x - plot.left()) / plot.width()).clamp(0.0, 1.0);
    let y = f64::from((plot.bottom() - pos.y) / plot.height()).clamp(0.0, 1.0);
    (x, y)
}

fn hit_point(plot: Rect, points: &[(f64, f64)], pos: egui::Pos2) -> Option<usize> {
    points.iter().enumerate().rev().find_map(|(index, point)| {
        let center = plot_pos(plot, point.0, point.1);
        ((center.x - pos.x).powi(2) + (center.y - pos.y).powi(2) < 12.0 * 12.0).then_some(index)
    })
}

fn move_curve_point(points: &mut [(f64, f64)], index: usize, x: f64, y: f64) {
    let last = points.len() - 1;
    let eps = 0.01;
    if index == 0 {
        points[0].0 = 0.0;
    } else if index == last {
        points[last].0 = 1.0;
    } else {
        let min_x = points[index - 1].0 + eps;
        let max_x = points[index + 1].0 - eps;
        points[index].0 = x.clamp(min_x, max_x.max(min_x));
    }
    let min_y = if index == 0 { 0.0 } else { points[index - 1].1 };
    let max_y = if index == last {
        1.0
    } else {
        points[index + 1].1
    };
    points[index].1 = y.clamp(min_y, max_y.max(min_y));
}

fn insert_curve_point(points: &mut Vec<(f64, f64)>, x: f64, y: f64) -> bool {
    if points.len() >= 5 || points.len() < 2 {
        return false;
    }
    let mut index = 1;
    while index < points.len() && points[index].0 < x {
        index += 1;
    }
    let left = points[index - 1].0;
    let right = points[index].0;
    if right - left < 0.04 {
        return false;
    }
    let x = x.clamp(left + 0.01, right - 0.01);
    let y = y.clamp(points[index - 1].1, points[index].1);
    points.insert(index, (x, y));
    true
}

fn pin_curve_ends(points: &mut [(f64, f64)]) {
    if points.is_empty() {
        return;
    }
    let last = points.len() - 1;
    points[0].0 = 0.0;
    points[last].0 = 1.0;
}
