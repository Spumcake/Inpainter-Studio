//! The tool area. This slice draws the empty bar; declared groups come later.

use crate::{icons, widgets};
use sketch::document::ColorId;
use sketch::registry::{self, CommandId, Tool};
use sketch::schema::PropValue;
use sketch::session::Session;
use eframe::egui::{
    self, Align, Align2, Color32, FontFamily, FontId, Layout, Rect, RichText, Sense, Stroke,
    UiBuilder, pos2, vec2,
};
use std::sync::Arc;

/// The bar behind the tabs. The toolbar and the active tab are white.
pub(crate) const BAR: Color32 = Color32::from_rgb(249, 250, 251);
pub(crate) const ICON: Color32 = Color32::from_rgb(45, 47, 51);
pub(crate) const ICON_QUIET: Color32 = Color32::from_rgb(186, 188, 192);
/// The current tool's icon. It has no plate behind it.
pub(crate) const ICON_ACTIVE: Color32 = Color32::from_rgb(37, 99, 235);
const HAIRLINE: Color32 = Color32::from_rgb(236, 237, 240);
#[allow(dead_code)]
const SEPARATOR: Color32 = Color32::from_rgb(222, 225, 229);

/// Every toolbar item is this tall and paints its content centered in it,
/// so the whole row shares one vertical center.
const ROW: f32 = 40.0;
/// A toolbar button's width. Its hover plate is a square two points narrower,
/// so neighboring plates keep a sliver of toolbar between them.
#[allow(dead_code)]
pub(crate) const BUTTON_W: f32 = 36.0;
#[allow(dead_code)]
const ICON_POINTS: f32 = 20.0;
/// A divider between toolbar groups, with its space on either side.
#[allow(dead_code)]
pub(crate) const SEPARATOR_W: f32 = 13.0;
/// A swatch's width: room for the active swatch's ring, 28 points across.
pub(crate) const SWATCH_W: f32 = 28.0;
/// Space before the first swatch, so the active swatch's ring is not clipped.
pub(crate) const SWATCH_INSET: f32 = 2.0;
const SIZE_W: f32 = 186.0;

/// Inter's roman variable font. Weight only: this file has no italic axis.
const INTER: &[u8] = include_bytes!("../assets/fonts/Inter-Variable.ttf");
/// Inter's italic variable font. Italic and bold italic are this file at two weights.
const INTER_ITALIC_FONT: &[u8] = include_bytes!("../assets/fonts/Inter-Italic-Variable.ttf");

fn inter_weight(bytes: &'static [u8], weight: f32) -> egui::FontData {
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
    install_face(&mut fonts, icons::INTER, inter_weight(INTER, 400.0));
    install_face(&mut fonts, icons::INTER_BOLD, inter_weight(INTER, 700.0));
    install_face(
        &mut fonts,
        icons::INTER_ITALIC,
        inter_weight(INTER_ITALIC_FONT, 400.0),
    );
    install_face(
        &mut fonts,
        icons::INTER_BOLD_ITALIC,
        inter_weight(INTER_ITALIC_FONT, 700.0),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, icons::INTER.into());
    fonts.font_data.insert(
        widgets::SEMIBOLD.into(),
        Arc::new(inter_weight(INTER, 600.0)),
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

#[allow(dead_code)]
fn icon_button(
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

/// Draws the empty toolbar. The trailing side is laid out first, from the
/// right, so a later declaration can pin that group while the rest scrolls.
pub(crate) fn show(ui: &mut egui::Ui) {
    let bar = egui::Frame::new()
        .fill(Color32::WHITE)
        .inner_margin(egui::Margin::symmetric(12, 5))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(ROW);
            ui.spacing_mut().item_spacing.x = 0.0;
            // The trailing group is laid out first, from the right edge, so a
            // narrow window scrolls the other groups instead.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    // A row wider than the window scrolls: the wheel turns it
                    // sideways without Shift, and dragging it moves it.
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
                            });
                        });
                    widgets::drag_scroll_cursor(ui, scrolled.id);
                });
            });
        });
    let rect = bar.response.rect;
    hairline(ui, rect.left(), rect.right(), rect.bottom());
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

#[allow(dead_code)]
fn separator(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(SEPARATOR_W, ROW), Sense::hover());
    let center = rect.center();
    ui.painter().vline(
        center.x,
        (center.y - 12.0)..=(center.y + 12.0),
        Stroke::new(1.0, SEPARATOR),
    );
}

pub(crate) fn show_session(ui: &mut egui::Ui, session: &mut Session, viewport: [f64; 2]) -> bool {
    let mut center = false;
    let bar = egui::Frame::new()
        .fill(Color32::WHITE)
        .inner_margin(egui::Margin::symmetric(12, 5))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(ROW);
            ui.spacing_mut().item_spacing.x = 0.0;
            // Zoom and the maximize toggle are laid out first, from the right
            // edge, so a narrow window scrolls the other groups instead.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                zoom(ui, session, viewport);
                separator(ui);
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
                            // short and grow downward, below the zoom group.
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                ui.set_height(ROW);
                                // The eraser and select have no palette, so
                                // there is nothing for a divider to set apart.
                                if !session.swatches().is_empty() {
                                    ui.add_space(SWATCH_INSET);
                                    swatches(ui, session);
                                    separator(ui);
                                }
                                for id in registry::file_commands() {
                                    if command_button(ui, session, *id, viewport) {
                                        center = true;
                                    }
                                }
                                separator(ui);
                                for id in registry::history_commands() {
                                    let _ = command_button(ui, session, *id, viewport);
                                }
                                separator(ui);
                                for id in registry::canvas_commands() {
                                    let _ = command_button(ui, session, *id, viewport);
                                }
                                tools(ui, session);
                                if session.tool() == Tool::Select {
                                    separator(ui);
                                    context_actions(ui, session, viewport);
                                } else {
                                    let bar = registry::tool(session.tool()).bar;
                                    if !bar.is_empty() {
                                        separator(ui);
                                        tool_bar(ui, session, bar);
                                    }
                                }
                            });
                        });
                    widgets::drag_scroll_cursor(ui, scrolled.id);
                });
            });
        });
    let rect = bar.response.rect;
    hairline(ui, rect.left(), rect.right(), rect.bottom());
    center
}

fn paint_drawn_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    name: &str,
    points: f32,
    color: Color32,
) -> bool {
    if !registry::DRAWN_ICONS.contains(&name) {
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


fn swatches(ui: &mut egui::Ui, session: &mut Session) {
    let swatches = session.swatches();
    let sortable = session.tool() == Tool::Brush;
    let drag_id = ui.id().with("swatch-drag");
    let mut dragging: Option<ColorId> = ui.data(|data| data.get_temp(drag_id));
    let mut rects = Vec::with_capacity(swatches.len());
    for swatch in &swatches {
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
            session.swatch_click(swatch.id);
        }
        let menu = session.swatch_menu(swatch.id);
        let chosen = widgets::action_menu(&response, menu, |action| {
            session.swatch_action_enabled(action)
        });
        if let Some(action) = chosen {
            session.swatch_action(swatch.id, action);
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
                    if target != from {
                        let _ = session.move_brush_color(id, target);
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
            data.remove::<ColorId>(drag_id);
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
    let fill = if selected { 10.5 } else { 9.0 };
    let color = swatch_color(rgb, muted);
    if selected {
        // The ring takes the swatch's own color, with a sliver of toolbar
        // between it and the fill.
        ui.painter()
            .circle_stroke(center, 13.5, Stroke::new(1.5, color));
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

/// Returns whether the command created a sketch that should be centered.
fn command_button(
    ui: &mut egui::Ui,
    session: &mut Session,
    id: CommandId,
    viewport: [f64; 2],
) -> bool {
    let definition = registry::command(id);
    let enabled = session.command_enabled(id);
    let tip = registry::hover(definition.label, definition.shortcuts);
    if icon_button(ui, definition.icon, Some(&tip), enabled, false).clicked() && enabled {
        let _ = session.perform(id, viewport);
        return id == CommandId::NewSketch;
    }
    false
}

fn tools(ui: &mut egui::Ui, session: &mut Session) {
    let current = session.tool();
    for tool in registry::tools() {
        // Every tool can always be chosen. The brush with no canvas
        // highlighted draws a new one; the eraser then erases nothing.
        let tip = registry::hover(tool.label, tool.shortcuts);
        let response = icon_button(ui, tool.icon, Some(&tip), true, tool.id == current);
        if response.clicked() {
            session.set_tool(tool.id);
        }
        if let Some(action) = widgets::action_menu(&response, registry::tool_menu(), |_| true) {
            session.tool_action(tool.id, action);
        }
    }
}

/// Context actions take the Size slot while Select is current: one run of
/// buttons with no dividers, since they all act on the selection. They are
/// the same buttons as the tools and commands, at the same size and spacing.
/// A command with nothing to do is drawn lighter and does not run; a menu
/// waits when none of its commands can run.
fn context_actions(ui: &mut egui::Ui, session: &mut Session, viewport: [f64; 2]) {
    for item in registry::select_actions() {
        match item {
            registry::SelectItem::Button(id) if session.command_shown(*id) => {
                let _ = command_button(ui, session, *id, viewport);
            }
            registry::SelectItem::Menu(menu)
                if menu.commands.iter().any(|id| session.command_shown(*id)) =>
            {
                context_menu(ui, session, menu, viewport);
            }
            registry::SelectItem::Button(_) | registry::SelectItem::Menu(_) => {}
        }
    }
}

/// How much wider a menu button is than a plain one, for its chevron.
const CHEVRON_W: f32 = 14.0;

/// A button that shows its icon and a small chevron, and opens its menu of
/// commands. Each row of the menu shows the command's icon and label.
fn context_menu(
    ui: &mut egui::Ui,
    session: &mut Session,
    menu: &registry::ActionMenu,
    viewport: [f64; 2],
) {
    let enabled = menu.commands.iter().any(|id| session.command_enabled(*id));
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
    paint_icon(ui.painter(), icon_center, menu.icon, ICON_POINTS, color);
    paint_icon(
        ui.painter(),
        pos2(plate.right() - CHEVRON_W / 2.0 - 2.0, rect.center().y),
        CHEVRON_ICON,
        12.0,
        color,
    );
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, menu.label));
    let response = response.on_hover_text(menu.label);
    if !enabled {
        return;
    }
    egui::Popup::menu(&response).show(|ui| {
        let items: Vec<widgets::MenuItem> = menu
            .commands
            .iter()
            .map(|id| {
                let definition = registry::command(*id);
                widgets::MenuItem {
                    label: definition.label,
                    icon: Some(definition.icon),
                    enabled: session.command_enabled(*id),
                    submenu: false,
                    divider: false,
                }
            })
            .collect();
        if let Some(index) = widgets::menu_rows(ui, &items) {
            let _ = session.perform(menu.commands[index], viewport);
        }
    });
}

/// The chevron on a menu button.
const CHEVRON_ICON: &str = "nav-arrow-down";

/// The caption, the slider, and the value share one row and one center. The
/// slider is painted here: egui's own fills its knob with the rail color, and
/// the picture has a dark knob on a light rail.
/// The current tool's settings after the tools, from its registry entry.
fn tool_bar(ui: &mut egui::Ui, session: &mut Session, bar: &[registry::BarControl]) {
    for control in bar {
        match control {
            registry::BarControl::Size => size_control(ui, session),
            registry::BarControl::Divider => separator(ui),
            registry::BarControl::Slider {
                prop,
                label,
                min,
                max,
            } => {
                let Some(PropValue::Number(value)) = session.bar_value(*prop) else {
                    continue;
                };
                let id = ui.id().with(("bar-slider", *prop as u8));
                let edit = bar_slider(
                    ui,
                    id,
                    label,
                    BAR_SLIDER_W,
                    value.round() as i32,
                    (*min, *max),
                    widgets::INK,
                );
                if let Some((value, live)) = edit.value {
                    session.set_bar_value(*prop, PropValue::Number(f64::from(value)), !live);
                }
                if edit.finished {
                    session.finish_size_edit();
                }
            }
            registry::BarControl::Choice { prop, options } => {
                let chosen = match session.bar_value(*prop) {
                    Some(PropValue::Index(index)) => Some(usize::from(index)),
                    _ => None,
                };
                for (index, (icon, label)) in options.iter().enumerate() {
                    let response = icon_button(ui, icon, Some(label), true, chosen == Some(index));
                    if response.clicked() {
                        session.set_bar_value(*prop, PropValue::Index(index as u8), true);
                    }
                }
            }
            registry::BarControl::List => list_button(ui, session),
        }
    }
}

/// The list button: its icon is the list's kind, blue while the list is on.
/// A click turns the list on or off; a right-click chooses the kind.
fn list_button(ui: &mut egui::Ui, session: &mut Session) {
    let (on, kind) = session.text_list();
    let (icon, label) = registry::LIST_KINDS[kind.min(registry::LIST_KINDS.len() - 1)];
    let response = icon_button(ui, icon, Some(label), true, on);
    if response.clicked() {
        session.toggle_text_list();
    }
    egui::Popup::context_menu(&response).show(|ui| {
        let items: Vec<widgets::MenuItem> = registry::LIST_KINDS
            .iter()
            .map(|(icon, label)| widgets::MenuItem {
                label,
                icon: Some(icon),
                enabled: true,
                submenu: false,
                divider: false,
            })
            .collect();
        if let Some(index) = widgets::menu_rows(ui, &items) {
            session.choose_text_list(index);
        }
    });
}

/// A labelled slider's width in the tool settings, other than Size.
const BAR_SLIDER_W: f32 = 150.0;

/// What a toolbar slider did this frame: a new value, live while it is being
/// dragged, and whether a drag ended.
struct BarEdit {
    value: Option<(i32, bool)>,
    finished: bool,
}

/// A caption, a slider, and its value as a number that can be typed, on one
/// row. The Size control and the text tool's sliders are this one control.
fn bar_slider(
    ui: &mut egui::Ui,
    id: egui::Id,
    caption: &str,
    width: f32,
    value: i32,
    (min, max): (i32, i32),
    tint: Color32,
) -> BarEdit {
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW), Sense::hover());
    let caption_rect = ui.painter().text(
        rect.left_center(),
        Align2::LEFT_CENTER,
        caption,
        FontId::proportional(13.5),
        ICON,
    );
    let track = Rect::from_min_max(
        pos2(caption_rect.right() + 10.0, rect.top()),
        pos2(rect.right() - 32.0, rect.bottom()),
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
            .max_decimals(0),
    );
    if typed_response.changed() {
        edit.value = Some((typed.clamp(min, max), typed_response.dragged()));
    }
    if typed_response.drag_stopped() {
        edit.finished = true;
    }
    edit
}

/// The Size control: the size of the current tool's next mark, drawn in a
/// locked palette color's color.
fn size_control(ui: &mut egui::Ui, session: &mut Session) {
    let Some(control) = session.size_control() else {
        return;
    };
    let tint = control.tint.map_or(widgets::INK, |rgb| {
        Color32::from_rgb(rgb[0], rgb[1], rgb[2])
    });
    let edit = bar_slider(
        ui,
        ui.id().with("size-slider"),
        registry::SIZE_LABEL,
        SIZE_W,
        i32::from(control.value),
        (i32::from(control.min), i32::from(control.max)),
        tint,
    );
    if let Some((value, live)) = edit.value {
        let value = value.clamp(0, 255) as u8;
        if live {
            let _ = session.preview_size(value);
        } else {
            let _ = session.set_size(value);
        }
    }
    if edit.finished {
        session.finish_size_edit();
    }
}

/// Laid out right to left: zoom out, zoom in, fit, then the maximize toggle.
fn zoom(ui: &mut egui::Ui, session: &mut Session, viewport: [f64; 2]) {
    let maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));
    for id in registry::view_commands().iter().rev() {
        if *id == CommandId::Maximize {
            let definition = registry::command(*id);
            let icon = if maximized {
                definition.toggle_icon.unwrap_or(definition.icon)
            } else {
                definition.icon
            };
            let tip = registry::hover(definition.label, definition.shortcuts);
            if icon_button(ui, icon, Some(&tip), true, false).clicked() {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
        } else {
            let _ = command_button(ui, session, *id, viewport);
        }
    }
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
