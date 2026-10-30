//! The Layers tree, from Simple Sketch 1.0.1's `outliner.rs`. Rows, eyes,
//! locks, and right-click menus come from the declared layers. Dragging a row
//! reorders it among its siblings. When the layers declare `nest`, a row can
//! also be dragged into and out of groups, as in Photoshop's Layers panel:
//! dropped on the middle of a group's row it goes to the top of that group,
//! and dropped between rows it goes there, under the parent of the row below
//! the drop, or at the bottom of the top level below the last row. Canvases,
//! groups, and palette colors are renamed in place, by double-clicking a name
//! or from the row's menu.

use crate::panel::Sent;
use crate::toolbar;
use crate::widgets::{self, BORDER, MUTED, SELECTED, Scale, TEXT};
use eframe::egui::{self, Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use protocol::{Action, ColorRow, FlagIcons, HeadingAction, LayerRow, Layers};
use serde_json::json;
use std::collections::HashSet;

/// The outliner sits in the panel but its rows act like buttons, not value
/// fields, so they are drawn larger than the panel's controls: a quarter
/// above full size, about the toolbar's icon size, with 15-point names.
const S: Scale = Scale(1.25);
const ROW: f32 = S.of(28.0);

fn name_font() -> egui::FontId {
    egui::FontId::proportional(15.0)
}

/// The layers. `paged` is a panel with page tabs, where the list follows the
/// sections rather than starting the page.
pub(crate) fn show(ui: &mut egui::Ui, layers: &Layers, paged: bool, sent: &mut Vec<Sent>) {
    let flags = &layers.flags;
    if !paged {
        // The first row is centered on the same line as another page's first
        // heading, so the panel does not jump when the select tool swaps pages.
        ui.add_space(crate::panel::FIRST_LINE - ROW / 2.0);
    }
    if !layers.actions.is_empty() {
        heading(ui, &layers.actions, sent);
    }
    let rows = &layers.rows;
    // Rows with pictures are drawn as the asset-editor mockup draws them, and
    // then every row is, so a group sits among its layers' pictures.
    let pictures = rows.iter().any(|row| row.thumbnail.is_some());
    let toggled = toggled_set(ui);
    let mut toggle: Option<u64> = None;
    let drag_id = ui.id().with("object-drag");
    let mut dragging: Option<u64> = ui.data(|data| data.get_temp(drag_id));
    let mut spots: Vec<Spot> = Vec::new();
    let color_id = ui.id().with("color-drag");
    let mut color_drag: Option<(u64, u64)> = ui.data(|data| data.get_temp(color_id));
    let mut color_rects: Vec<(u64, Rect)> = Vec::new();

    let mut hidden_at: Option<u32> = None;
    for node in rows {
        if let Some(depth) = hidden_at {
            if node.depth > depth {
                continue;
            }
            hidden_at = None;
        }
        let expandable = node.group || !node.colors.is_empty();
        // A group starts open; a canvas's layers start collapsed. A click on
        // the disclosure arrow toggles either from where it started.
        let toggled_here = toggled.contains(&node.id);
        let open = !expandable
            || if node.group {
                !toggled_here
            } else {
                toggled_here
            };
        if pictures {
            picture_row(ui, node, flags, open, &mut toggle, &mut dragging, sent);
        } else {
            object_row(ui, node, flags, open, &mut toggle, &mut dragging, sent);
        }
        if dragging.is_some()
            && let Some(rect) = ui.data(|data| data.get_temp::<Rect>(row_rect_id(ui, node.id)))
        {
            spots.push(Spot { id: node.id, parent: node.parent, depth: node.depth, group: node.group, rect });
        }
        if !open {
            hidden_at = Some(node.depth);
            continue;
        }
        if node.group || pictures {
            continue;
        }
        for color in &node.colors {
            let rect = color_row(
                ui,
                node.id,
                color,
                flags,
                node.depth + 1,
                &mut color_drag,
                Place::Outliner,
                sent,
            );
            if color_drag.is_some_and(|(owner, _)| owner == node.id) {
                color_rects.push((color.id, rect));
            }
        }
    }

    if finish_object_drag(ui, dragging, &spots, layers, sent) {
        dragging = None;
    }
    if finish_color_drag(ui, color_drag, &color_rects, Some(&layers.sort_color), sent) {
        color_drag = None;
    }
    if let Some(id) = toggle {
        let mut set = toggled;
        if !set.remove(&id) {
            set.insert(id);
        }
        ui.data_mut(|data| data.insert_temp(ui.id().with("toggled"), set));
    }
    ui.data_mut(|data| {
        if let Some(id) = dragging {
            data.insert_temp(drag_id, id);
        } else {
            data.remove::<u64>(drag_id);
        }
    });
    ui.data_mut(|data| {
        if let Some(drag) = color_drag {
            data.insert_temp(color_id, drag);
        } else {
            let _ = data.remove::<(u64, u64)>(color_id);
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn object_row(
    ui: &mut egui::Ui,
    node: &LayerRow,
    flags: &FlagIcons,
    open: bool,
    toggle: &mut Option<u64>,
    dragging: &mut Option<u64>,
    sent: &mut Vec<Sent>,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    ui.data_mut(|data| data.insert_temp(row_rect_id(ui, node.id), rect));
    if node.selected {
        ui.painter().rect_filled(rect, S.of(4.0), SELECTED);
    }
    let mut x = rect.left() + S.of(4.0) + node.depth as f32 * S.of(16.0);
    let expandable = node.group || !node.colors.is_empty();
    if expandable {
        let spot = Rect::from_min_size(pos2(x, rect.top()), vec2(S.of(16.0), ROW));
        let response = ui.interact(
            spot,
            ui.id().with(("disclose", node.id)),
            Sense::click_and_drag(),
        );
        disclosure(
            ui.painter(),
            spot.center(),
            open,
            if response.hovered() { TEXT } else { MUTED },
        );
        if response.clicked() {
            *toggle = Some(node.id);
        }
    }
    x += S.of(16.0);
    toolbar::paint_icon(
        ui.painter(),
        pos2(x + S.of(8.0), rect.center().y),
        &node.icon,
        S.of(15.0),
        TEXT,
    );
    x += S.of(22.0);
    let lock_x = rect.right() - S.of(22.0);
    let eye_x = lock_x - S.of(24.0);
    let name_rect = Rect::from_min_max(pos2(x, rect.top()), pos2(eye_x - S.of(4.0), rect.bottom()));
    name_area(ui, node, name_rect, name_rect, dragging, sent);
    flag_button(
        ui,
        Flag::Eye,
        flags,
        node.visible,
        &node.toggle_visible,
        eye_x,
        rect,
        sent,
    );
    flag_button(
        ui,
        Flag::Lock,
        flags,
        node.locked,
        &node.toggle_locked,
        lock_x,
        rect,
        sent,
    );
}

/// A row's name: renamed in place by a double-click or its menu, clicked to
/// select, dragged to reorder, and right-clicked for its menu. `area` takes
/// the pointer; the name is drawn in `name_rect`.
fn name_area(
    ui: &mut egui::Ui,
    node: &LayerRow,
    name_rect: Rect,
    area: Rect,
    dragging: &mut Option<u64>,
    sent: &mut Vec<Sent>,
) {
    let target = Renaming::Node(node.id);
    let renamed = rename_field(ui, target, name_rect, &node.name, |name| {
        sent.push(Sent::Run(node.rename.with_value(json!(name))));
    });
    if !renamed {
        let response = ui.interact(
            area,
            ui.id().with(("layer", node.id)),
            Sense::click_and_drag(),
        );
        let lifted = dragging.is_some_and(|id| id == node.id) && response.dragged();
        if !lifted {
            ui.painter().text(
                pos2(name_rect.left(), name_rect.center().y),
                Align2::LEFT_CENTER,
                &node.name,
                name_font(),
                TEXT,
            );
        }
        match widgets::action_menu(&response, &node.menu) {
            Some(row) if row.rename => begin_rename(ui, target, &node.name),
            Some(row) => {
                if let Some(action) = &row.action {
                    sent.push(Sent::Run(action.clone()));
                }
            }
            None => {}
        }
        if node.renamable && response.double_clicked() {
            begin_rename(ui, target, &node.name);
        } else if response.clicked() {
            let shift = ui.input(|input| input.modifiers.shift);
            sent.push(Sent::Run(node.click.with_value(json!(shift))));
        } else if response.drag_started() && !ui.input(|input| input.modifiers.shift) {
            *dragging = Some(node.id);
        }
    }
}

/// The "Layers" heading over the list, with its icon buttons at the right,
/// as the asset-editor mockup shows add, group, and delete.
fn heading(ui: &mut egui::Ui, actions: &[HeadingAction], sent: &mut Vec<Sent>) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, HEADING_H), Sense::hover());
    ui.painter().text(
        pos2(rect.left(), rect.center().y),
        Align2::LEFT_CENTER,
        "Layers",
        widgets::semibold(14.5),
        TEXT,
    );
    let mut right = rect.right();
    for (index, action) in actions.iter().enumerate().rev() {
        let spot = Rect::from_min_max(pos2(right - HEADING_BUTTON, rect.top()), pos2(right, rect.bottom()));
        right = spot.left();
        let sense = if action.enabled { Sense::click() } else { Sense::hover() };
        let response = ui
            .interact(spot, ui.id().with(("layers-heading", index)), sense)
            .on_hover_text(&action.tip);
        let color = match (action.enabled, response.hovered()) {
            (true, true) => toolbar::ICON_ACTIVE,
            (true, false) => toolbar::ICON,
            (false, _) => toolbar::ICON_QUIET,
        };
        toolbar::paint_icon(ui.painter(), spot.center(), &action.icon, 18.0, color);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, action.enabled, &action.tip));
        if action.enabled && response.clicked() {
            sent.push(Sent::Run(action.action.clone()));
        }
    }
    let (line, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
    ui.painter().hline(line.x_range(), line.center().y, Stroke::new(1.0, ROW_RULE));
}

/// The heading's height and each of its buttons' width.
const HEADING_H: f32 = 40.0;
const HEADING_BUTTON: f32 = 34.0;
/// A row with a picture, as tall as the mockup's.
const PICTURE_ROW: f32 = 52.0;
/// The picture's box in a row.
const THUMB: egui::Vec2 = vec2(56.0, 38.0);
/// The hairline between rows.
const ROW_RULE: Color32 = Color32::from_rgb(238, 240, 243);
/// A chosen row with a picture, pale blue as the mockup's Background Photo.
const PICKED: Color32 = Color32::from_rgb(226, 236, 252);
const PICKED_EDGE: Color32 = Color32::from_rgb(37, 99, 235);

/// A row as the asset-editor mockup draws it: the eye, the layer's picture
/// in a small box, its name, and its lock when it is locked or the pointer is
/// over the row. A group shows its icon in the box and a disclosure arrow.
#[allow(clippy::too_many_arguments)]
fn picture_row(
    ui: &mut egui::Ui,
    node: &LayerRow,
    flags: &FlagIcons,
    open: bool,
    toggle: &mut Option<u64>,
    dragging: &mut Option<u64>,
    sent: &mut Vec<Sent>,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), PICTURE_ROW), Sense::hover());
    ui.data_mut(|data| data.insert_temp(row_rect_id(ui, node.id), rect));
    let hovered = ui.rect_contains_pointer(rect);
    if node.selected {
        ui.painter().rect_filled(rect.shrink2(vec2(0.0, 1.0)), 8.0, PICKED);
    } else {
        ui.painter().hline(
            (rect.left() + 4.0)..=(rect.right() - 4.0),
            rect.bottom() - 0.5,
            Stroke::new(1.0, ROW_RULE),
        );
    }
    flag_button(ui, Flag::Eye, flags, node.visible, &node.toggle_visible, rect.left() + 2.0, rect, sent);
    // Every row keeps room for a group's disclosure triangle, and each level
    // indents further than that room, so a group's layers sit clearly
    // inside it rather than level with its picture.
    let x0 = rect.left() + 34.0 + node.depth as f32 * 20.0;
    if node.group {
        let spot = Rect::from_min_size(pos2(x0 - 2.0, rect.top()), vec2(14.0, PICTURE_ROW));
        let response = ui.interact(spot, ui.id().with(("disclose", node.id)), Sense::click_and_drag());
        disclosure(ui.painter(), spot.center(), open, if response.hovered() { TEXT } else { MUTED });
        if response.clicked() {
            *toggle = Some(node.id);
        }
    }
    let x = x0 + 14.0;
    let frame = Rect::from_min_size(pos2(x, rect.center().y - THUMB.y / 2.0), THUMB);
    let edge = if node.selected { PICKED_EDGE } else { BORDER };
    ui.painter().rect_filled(frame, 4.0, Color32::WHITE);
    match &node.thumbnail {
        Some(thumbnail) => {
            let inside = frame.shrink(1.0);
            let size = vec2(thumbnail.width.max(1) as f32, thumbnail.height.max(1) as f32);
            let scale = (inside.width() / size.x).min(inside.height() / size.y);
            let picture = Rect::from_center_size(inside.center(), size * scale);
            let painter = ui.painter().with_clip_rect(inside);
            match crate::raster::raw_picture(
                ui.ctx(),
                &thumbnail.path,
                thumbnail.width,
                thumbnail.height,
                thumbnail.revision,
            ) {
                Some((texture, opaque)) => {
                    if !opaque {
                        crate::raster::checker(ui.ctx(), &painter, picture);
                    }
                    painter.image(texture, picture, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                None => crate::raster::checker(ui.ctx(), &painter, picture),
            }
        }
        None => toolbar::paint_icon(ui.painter(), frame.center(), &node.icon, 18.0, TEXT),
    }
    ui.painter().rect_stroke(frame, 4.0, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
    let lock_x = rect.right() - 30.0;
    let name_rect = Rect::from_min_max(pos2(frame.right() + 12.0, rect.top()), pos2(lock_x - 4.0, rect.bottom()));
    // The whole row between the eye and the lock takes the pointer, so the
    // picture is clicked and dragged as the name is.
    let area = Rect::from_min_max(pos2(frame.left(), rect.top()), name_rect.max);
    name_area(ui, node, name_rect, area, dragging, sent);
    if node.locked || hovered {
        flag_button(ui, Flag::Lock, flags, node.locked, &node.toggle_locked, lock_x, rect, sent);
    }
}

/// Where a palette row is drawn: under its canvas in the outliner, or in an
/// object's Palette section in the panel. The row is the same; its declared
/// actions say what a click does in each place.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Outliner,
    Panel,
}

/// An object's colors in the panel, as the same rows the outliner draws
/// under a canvas: a dot, the name, an eye, and, for a canvas layer, a lock,
/// renaming, sorting, and its menu. A shape's or text's fixed roles have only
/// the eye and Properties.
pub(crate) fn palette(
    ui: &mut egui::Ui,
    owner: u64,
    colors: &[ColorRow],
    sort: Option<&Action>,
    flags: &FlagIcons,
    sent: &mut Vec<Sent>,
) {
    let drag_id = ui.id().with("palette-drag");
    let mut drag: Option<(u64, u64)> = ui.data(|data| data.get_temp(drag_id));
    let mut rects = Vec::new();
    for color in colors {
        let rect = color_row(ui, owner, color, flags, 0, &mut drag, Place::Panel, sent);
        rects.push((color.id, rect));
    }
    if finish_color_drag(ui, drag, &rects, sort, sent) {
        drag = None;
    }
    ui.data_mut(|data| match drag {
        Some(drag) => {
            data.insert_temp(drag_id, drag);
        }
        None => {
            data.remove::<(u64, u64)>(drag_id);
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn color_row(
    ui: &mut egui::Ui,
    owner: u64,
    color: &ColorRow,
    flags: &FlagIcons,
    depth: u32,
    drag: &mut Option<(u64, u64)>,
    place: Place,
    sent: &mut Vec<Sent>,
) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    let x = rect.left() + S.of(4.0) + depth as f32 * S.of(16.0) + S.of(16.0);
    let dot = pos2(x + S.of(8.0), rect.center().y);
    ui.painter().circle_filled(
        dot,
        S.of(6.0),
        Color32::from_rgb(color.rgb[0], color.rgb[1], color.rgb[2]),
    );
    ui.painter()
        .circle_stroke(dot, S.of(6.0), Stroke::new(1.0, BORDER));
    let lock_x = rect.right() - S.of(22.0);
    let eye_x = lock_x - S.of(24.0);
    let name_rect = Rect::from_min_max(
        pos2(x + S.of(22.0), rect.top()),
        pos2(eye_x - S.of(4.0), rect.bottom()),
    );
    let target = Renaming::Color(color.id);
    let renamed = !color.fixed
        && rename_field(ui, target, name_rect, &color.name, |name| {
            if let Some(rename) = &color.rename {
                sent.push(Sent::Run(rename.with_value(json!(name))));
            }
        });
    if !renamed {
        let sense = Sense::click_and_drag();
        let place_id = place == Place::Panel;
        let response = ui.interact(
            name_rect,
            ui.id().with(("color-row", color.id, place_id)),
            sense,
        );
        let lifted = drag.is_some_and(|(_, id)| id == color.id) && response.dragged();
        if !lifted {
            ui.painter().text(
                pos2(name_rect.left(), name_rect.center().y),
                Align2::LEFT_CENTER,
                &color.name,
                name_font(),
                TEXT,
            );
        }
        match widgets::action_menu(&response, &color.menu) {
            Some(row) if row.rename => begin_rename(ui, target, &color.name),
            Some(row) => {
                if let Some(action) = &row.action {
                    sent.push(Sent::Run(action.clone()));
                }
            }
            None => {}
        }
        if response.double_clicked() && !color.fixed {
            begin_rename(ui, target, &color.name);
        } else if response.drag_started() && !color.fixed {
            *drag = Some((owner, color.id));
        } else if response.clicked() {
            sent.push(Sent::Run(color.open.clone()));
        }
    }
    flag_button(
        ui,
        Flag::Eye,
        flags,
        color.visible,
        &color.toggle_visible,
        eye_x,
        rect,
        sent,
    );
    if let Some(toggle_locked) = &color.toggle_locked {
        flag_button(
            ui,
            Flag::Lock,
            flags,
            color.locked,
            toggle_locked,
            lock_x,
            rect,
            sent,
        );
    }
    name_rect
}

/// A row drawn this frame, as a drag sees it.
#[derive(Clone, Copy, Debug)]
struct Spot {
    id: u64,
    parent: Option<u64>,
    depth: u32,
    group: bool,
    rect: Rect,
}

/// Where a dragged row would go: under `parent`, `value` rows from the top
/// of its children, as `reorder` says it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drop {
    parent: Option<u64>,
    value: usize,
    /// Dropped on the middle of a group's row, which is then marked whole.
    into: Option<Rect>,
    /// Where the line between rows is drawn, when it is not `into`.
    line: f32,
}

/// Where `dragged` goes when let go at height `y`, or nothing when it would
/// stay where it is. Without `nest`, only among its siblings.
fn drop_place(spots: &[Spot], dragged: u64, y: f32, nest: bool) -> Option<Drop> {
    let at = spots.iter().position(|spot| spot.id == dragged)?;
    let me = spots[at];
    // The dragged row's own rows, below it and deeper, are no place for it.
    let inside = spots[at + 1..].iter().take_while(|spot| spot.depth > me.depth).count();
    let others: Vec<Spot> = spots
        .iter()
        .enumerate()
        .filter(|(index, spot)| {
            *index != at && (if nest { *index <= at || *index > at + inside } else { spot.parent == me.parent })
        })
        .map(|(_, spot)| *spot)
        .collect();
    let now = spots[..at].iter().filter(|spot| spot.parent == me.parent).count();
    let stays = |parent: Option<u64>, value: usize| parent == me.parent && value == now;
    if nest
        && let Some(group) = others.iter().find(|spot| {
            spot.group && y > spot.rect.top() + spot.rect.height() / 4.0 && y < spot.rect.bottom() - spot.rect.height() / 4.0
        })
    {
        let parent = Some(group.id);
        return (!stays(parent, 0)).then_some(Drop { parent, value: 0, into: Some(group.rect), line: y });
    }
    let gap = others.iter().filter(|spot| spot.rect.center().y < y).count();
    let (parent, value) = match others.get(gap) {
        _ if !nest => (me.parent, gap),
        Some(below) => (below.parent, others[..gap].iter().filter(|spot| spot.parent == below.parent).count()),
        None => (None, others.iter().filter(|spot| spot.parent.is_none()).count()),
    };
    if stays(parent, value) {
        return None;
    }
    let line = match others.get(gap) {
        Some(spot) => spot.rect.top(),
        None => others.last().map(|spot| spot.rect.bottom()).unwrap_or(me.rect.center().y),
    };
    Some(Drop { parent, value, into: None, line })
}

fn finish_object_drag(ui: &mut egui::Ui, dragging: Option<u64>, spots: &[Spot], layers: &Layers, sent: &mut Vec<Sent>) -> bool {
    let Some(id) = dragging else {
        return false;
    };
    let Some(pointer) = ui.input(|input| input.pointer.interact_pos()) else {
        return false;
    };
    let Some(me) = spots.iter().find(|spot| spot.id == id) else {
        return false;
    };
    let drop = drop_place(spots, id, pointer.y, layers.nest);
    match drop.and_then(|drop| drop.into) {
        Some(group) => {
            ui.painter().rect_stroke(group.shrink(1.0), S.of(4.0), Stroke::new(S.of(2.0), widgets::INK), egui::StrokeKind::Inside);
        }
        None => {
            if let Some(drop) = drop {
                ui.painter().hline(
                    (me.rect.left() - S.of(8.0))..=(me.rect.right()),
                    drop.line,
                    Stroke::new(S.of(2.0), widgets::INK),
                );
            }
        }
    }
    if ui.input(|input| input.pointer.any_released()) {
        if let Some(drop) = drop {
            let mut action = layers.reorder.with_value(json!(drop.value));
            if let Some(serde_json::Value::Object(arg)) = action.arg.as_mut() {
                arg.insert("node".to_string(), json!(id));
                if layers.nest {
                    arg.insert("parent".to_string(), json!(drop.parent));
                }
            }
            sent.push(Sent::Run(action));
        }
        return true;
    }
    false
}

fn finish_color_drag(
    ui: &mut egui::Ui,
    drag: Option<(u64, u64)>,
    rects: &[(u64, Rect)],
    sort: Option<&Action>,
    sent: &mut Vec<Sent>,
) -> bool {
    let Some((_, id)) = drag else {
        return false;
    };
    let Some(pointer) = ui.input(|input| input.pointer.interact_pos()) else {
        return false;
    };
    let Some(from) = rects.iter().position(|(color, _)| *color == id) else {
        return false;
    };
    let others: Vec<Rect> = rects
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != from)
        .map(|(_, (_, rect))| *rect)
        .collect();
    let target = others
        .iter()
        .filter(|rect| rect.center().y < pointer.y)
        .count();
    if ui.input(|input| input.pointer.any_released()) {
        if target != from
            && let Some(sort) = sort
        {
            let mut action = sort.with_value(json!(target));
            if let Some(serde_json::Value::Object(arg)) = action.arg.as_mut() {
                arg.insert("color".to_string(), json!(id));
            }
            sent.push(Sent::Run(action));
        }
        return true;
    }
    false
}

#[derive(Clone, Copy)]
enum Flag {
    Eye,
    Lock,
}

#[allow(clippy::too_many_arguments)]
fn flag_button(
    ui: &mut egui::Ui,
    flag: Flag,
    flags: &FlagIcons,
    on: bool,
    action: &Action,
    x: f32,
    row: Rect,
    sent: &mut Vec<Sent>,
) {
    let spot = Rect::from_min_size(pos2(x, row.top()), vec2(S.of(22.0), row.height()));
    let response = ui.interact(
        spot,
        ui.id().with((flag as u8, x.to_bits(), row.top().to_bits())),
        Sense::click_and_drag(),
    );
    let icon = match (flag, on) {
        (Flag::Eye, true) => &flags.eye_open,
        (Flag::Eye, false) => &flags.eye_closed,
        (Flag::Lock, true) => &flags.lock_closed,
        (Flag::Lock, false) => &flags.lock_open,
    };
    toolbar::paint_icon(
        ui.painter(),
        spot.center(),
        icon,
        S.of(15.0),
        if response.hovered() { TEXT } else { MUTED },
    );
    if response.clicked() {
        sent.push(Sent::Run(action.clone()));
    }
}

/// What is being renamed in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Renaming {
    Node(u64),
    Color(u64),
}

fn rename_key(ui: &egui::Ui) -> egui::Id {
    ui.id().with("rename")
}

/// While `target` is being renamed, draws its name field in `rect` and returns
/// true. The field sits where the name is drawn, so the text does not move.
/// Enter or clicking away passes the new name to `commit`; Escape keeps the
/// old one.
fn rename_field(
    ui: &mut egui::Ui,
    target: Renaming,
    rect: Rect,
    current: &str,
    commit: impl FnOnce(&str),
) -> bool {
    let editing: Option<Renaming> = ui.data(|data| data.get_temp(rename_key(ui)));
    if editing != Some(target) {
        return false;
    }
    let text_id = ui.id().with("rename-text");
    let mut text = ui
        .data(|data| data.get_temp::<String>(text_id))
        .unwrap_or_else(|| current.to_string());
    let mut editor = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let response = editor.add(
        egui::TextEdit::singleline(&mut text)
            .id(ui.id().with(("rename-edit", format!("{target:?}"))))
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .font(name_font())
            .text_color(TEXT)
            .desired_width(rect.width()),
    );
    // Focus is asked for once. Asking every frame would take it back from
    // Escape or a click elsewhere, and the field would never close.
    if ui
        .data_mut(|data| data.remove_temp::<bool>(rename_focus_key(ui)))
        .is_some()
    {
        response.request_focus();
    }
    let cancel = ui.input(|input| input.key_pressed(egui::Key::Escape));
    let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
    if cancel || enter || response.lost_focus() {
        ui.data_mut(|data| {
            data.remove::<Renaming>(rename_key(ui));
            data.remove::<String>(text_id);
        });
        response.surrender_focus();
        if !cancel {
            commit(&text);
        }
    } else {
        ui.data_mut(|data| data.insert_temp(text_id, text));
    }
    true
}

fn rename_focus_key(ui: &egui::Ui) -> egui::Id {
    ui.id().with("rename-focus")
}

fn begin_rename(ui: &egui::Ui, target: Renaming, name: &str) {
    ui.data_mut(|data| {
        data.insert_temp(rename_key(ui), target);
        data.insert_temp(ui.id().with("rename-text"), name.to_string());
        data.insert_temp(rename_focus_key(ui), true);
    });
}

/// Rows whose disclosure arrow has been clicked an odd number of times.
fn toggled_set(ui: &egui::Ui) -> HashSet<u64> {
    ui.data(|data| data.get_temp(ui.id().with("toggled")))
        .unwrap_or_default()
}

fn row_rect_id(ui: &egui::Ui, id: u64) -> egui::Id {
    ui.id().with(("row-rect", id))
}

fn disclosure(painter: &egui::Painter, center: egui::Pos2, open: bool, color: Color32) {
    let tip = if open {
        [
            pos2(center.x - S.of(4.0), center.y - S.of(2.0)),
            pos2(center.x + S.of(4.0), center.y - S.of(2.0)),
            pos2(center.x, center.y + S.of(3.0)),
        ]
    } else {
        [
            pos2(center.x - S.of(2.0), center.y - S.of(4.0)),
            pos2(center.x - S.of(2.0), center.y + S.of(4.0)),
            pos2(center.x + S.of(3.0), center.y),
        ]
    };
    painter.add(egui::Shape::convex_polygon(
        tip.to_vec(),
        color,
        Stroke::NONE,
    ));
}
