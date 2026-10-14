//! The Layers tree. Rows, eyes, locks, and right-click menus come from the
//! registry. Dragging a row reorders it among its siblings. Moving a node into
//! or out of a group is not specified, so that drop stays unwired. Canvases,
//! groups, and palette colors are renamed in place, by double-clicking a name
//! or from the row's menu.

use sketch::document::{ColorId, LayerColor, LayerKind, LayerNode, NodeId};
use sketch::registry::{self, MenuAction};
use sketch::schema::PropId;
use sketch::session::Session;
use crate::toolbar;
use crate::widgets::{self, BORDER, MUTED, SELECTED, Scale, TEXT};
use eframe::egui::{self, Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use std::collections::HashSet;

/// The outliner sits in the panel but its rows act like buttons, not value
/// fields, so they are drawn larger than the panel's controls: a quarter
/// above full size, about the toolbar's icon size, with 15-point names.
const S: Scale = Scale(1.25);
const ROW: f32 = S.of(28.0);

fn name_font() -> egui::FontId {
    egui::FontId::proportional(15.0)
}

fn layer_icon(kind: LayerKind) -> &'static str {
    match kind {
        LayerKind::Canvas => registry::node_icon(false),
        LayerKind::Group => registry::node_icon(true),
        LayerKind::Rectangle => registry::tool(sketch::registry::Tool::Rectangle).icon,
        LayerKind::Ellipse => registry::tool(sketch::registry::Tool::Ellipse).icon,
        LayerKind::Line => registry::tool(sketch::registry::Tool::Line).icon,
        LayerKind::Text => registry::tool(sketch::registry::Tool::Text).icon,
    }
}

pub(crate) fn show(ui: &mut egui::Ui, session: &mut Session) {
    // The first row is centered on the same line as another page's first
    // heading, so the panel does not jump when the select tool swaps pages.
    ui.add_space(crate::panel::FIRST_LINE - ROW / 2.0);
    let layers = session.layers();
    let toggled = toggled_set(ui);
    let mut toggle: Option<u64> = None;
    let drag_id = ui.id().with("object-drag");
    let mut dragging: Option<NodeId> = ui.data(|data| data.get_temp(drag_id));
    let mut sibling_rects: Vec<(NodeId, Rect)> = Vec::new();
    let color_id = ui.id().with("color-drag");
    let mut color_drag: Option<(NodeId, ColorId)> = ui.data(|data| data.get_temp(color_id));
    let mut color_rects: Vec<(ColorId, Rect)> = Vec::new();
    let mut color_owner: Option<NodeId> = None;

    let mut hidden_at: Option<u32> = None;
    for node in &layers {
        if let Some(depth) = hidden_at {
            if node.depth > depth {
                continue;
            }
            hidden_at = None;
        }
        let expandable = node.kind == LayerKind::Group || !node.colors.is_empty();
        // A group starts open; a canvas's layers start collapsed. A click on
        // the disclosure arrow toggles either from where it started.
        let toggled_here = toggled.contains(&node.id.get());
        let open = !expandable
            || if node.kind == LayerKind::Group {
                !toggled_here
            } else {
                toggled_here
            };
        object_row(ui, session, node, open, &mut toggle, &mut dragging);
        if dragging.is_some_and(|id| {
            layers
                .iter()
                .find(|item| item.id == id)
                .is_some_and(|item| item.parent == node.parent)
        }) {
            if let Some(rect) = ui.data(|data| data.get_temp::<Rect>(row_rect_id(ui, node.id))) {
                sibling_rects.push((node.id, rect));
            }
        }
        if !open {
            hidden_at = Some(node.depth);
            continue;
        }
        if node.kind == LayerKind::Group {
            continue;
        }
        for color in &node.colors {
            let rect = color_row(
                ui,
                session,
                node.id,
                color,
                node.depth + 1,
                &mut color_drag,
                Place::Outliner,
            );
            if color_drag.is_some_and(|(owner, _)| owner == node.id) {
                color_owner = Some(node.id);
                color_rects.push((color.id, rect));
            }
        }
    }

    if finish_object_drag(ui, session, dragging, &sibling_rects) {
        dragging = None;
    }
    if finish_color_drag(ui, session, color_drag, color_owner, &color_rects) {
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
            data.remove::<NodeId>(drag_id);
        }
    });
    ui.data_mut(|data| {
        if let Some(drag) = color_drag {
            data.insert_temp(color_id, drag);
        } else {
            let _ = data.remove::<(NodeId, ColorId)>(color_id);
        }
    });
}

fn object_row(
    ui: &mut egui::Ui,
    session: &mut Session,
    node: &LayerNode,
    open: bool,
    toggle: &mut Option<u64>,
    dragging: &mut Option<NodeId>,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    ui.data_mut(|data| data.insert_temp(row_rect_id(ui, node.id), rect));
    if node.selected {
        ui.painter().rect_filled(rect, S.of(4.0), SELECTED);
    }
    let mut x = rect.left() + S.of(4.0) + node.depth as f32 * S.of(16.0);
    let expandable = node.kind == LayerKind::Group || !node.colors.is_empty();
    if expandable {
        let spot = Rect::from_min_size(pos2(x, rect.top()), vec2(S.of(16.0), ROW));
        let response = ui.interact(
            spot,
            ui.id().with(("disclose", node.id.get())),
            Sense::click_and_drag(),
        );
        disclosure(
            ui.painter(),
            spot.center(),
            open,
            if response.hovered() { TEXT } else { MUTED },
        );
        if response.clicked() {
            *toggle = Some(node.id.get());
        }
    }
    x += S.of(16.0);
    toolbar::paint_icon(
        ui.painter(),
        pos2(x + S.of(8.0), rect.center().y),
        layer_icon(node.kind),
        S.of(15.0),
        TEXT,
    );
    x += S.of(22.0);
    let lock_x = rect.right() - S.of(22.0);
    let eye_x = lock_x - S.of(24.0);
    let name_rect = Rect::from_min_max(pos2(x, rect.top()), pos2(eye_x - S.of(4.0), rect.bottom()));
    let target = Renaming::Node(node.id);
    let renamed = rename_field(ui, target, name_rect, &node.name, |name| {
        session.rename_node(node.id, name);
    });
    if !renamed {
        let response = ui.interact(
            name_rect,
            ui.id().with(("layer", node.id.get())),
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
        let menu = match node.kind {
            LayerKind::Group => registry::group_row_menu(),
            LayerKind::Canvas => registry::canvas_row_menu(),
            LayerKind::Rectangle | LayerKind::Ellipse | LayerKind::Line | LayerKind::Text => {
                registry::object_menu()
            }
        };
        let has_page = session.object_page_available(node.id);
        match widgets::action_menu(&response, menu, |action| {
            action != MenuAction::Properties || has_page
        }) {
            Some(MenuAction::Rename) => begin_rename(ui, target, &node.name),
            Some(MenuAction::MakePaletteDefault) => {
                session.make_palette_default(node.id);
            }
            Some(action) => session.object_action(node.id, action),
            None => {}
        }
        let named = matches!(node.kind, LayerKind::Canvas | LayerKind::Group);
        if named && response.double_clicked() {
            begin_rename(ui, target, &node.name);
        } else if response.clicked() {
            session.select_click(node.id, ui.input(|input| input.modifiers.shift));
        } else if response.drag_started() && !ui.input(|input| input.modifiers.shift) {
            *dragging = Some(node.id);
        }
    }
    flag_button(
        ui,
        session,
        Target::Node(node.id),
        PropId::Visible,
        node.visible,
        eye_x,
        rect,
    );
    flag_button(
        ui,
        session,
        Target::Node(node.id),
        PropId::Locked,
        node.locked,
        lock_x,
        rect,
    );
}

/// Where a palette row is drawn: under its canvas in the outliner, or in an
/// object's Palette section in the panel. The row is the same; a click in the
/// panel opens the color's page and keeps the way back to the object.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Outliner,
    Panel,
}

/// An object's colors in the panel, as the same rows the outliner draws
/// under a canvas: a dot, the name, an eye, and, for a canvas layer, a lock,
/// renaming, sorting, and its menu. A shape's or text's fixed roles have only
/// the eye and Properties.
pub(crate) fn palette(ui: &mut egui::Ui, session: &mut Session, owner: NodeId) {
    let drag_id = ui.id().with("palette-drag");
    let mut drag: Option<(NodeId, ColorId)> = ui.data(|data| data.get_temp(drag_id));
    let mut rects = Vec::new();
    for color in session.palette_entries(owner) {
        let rect = color_row(ui, session, owner, &color, 0, &mut drag, Place::Panel);
        rects.push((color.id, rect));
    }
    if finish_color_drag(ui, session, drag, Some(owner), &rects) {
        drag = None;
    }
    ui.data_mut(|data| match drag {
        Some(drag) => {
            data.insert_temp(drag_id, drag);
        }
        None => {
            data.remove::<(NodeId, ColorId)>(drag_id);
        }
    });
}

fn color_row(
    ui: &mut egui::Ui,
    session: &mut Session,
    owner: NodeId,
    color: &LayerColor,
    depth: u32,
    drag: &mut Option<(NodeId, ColorId)>,
    place: Place,
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
            session.rename_color(owner, color.id, name);
        });
    if !renamed {
        let sense = Sense::click_and_drag();
        let place_id = place == Place::Panel;
        let response = ui.interact(
            name_rect,
            ui.id().with(("color-row", color.id.get(), place_id)),
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
        let open = |session: &mut Session| match place {
            Place::Outliner => session.select_color(owner, color.id),
            Place::Panel => session.open_palette_color(color.id),
        };
        let menu = if color.fixed {
            registry::role_menu()
        } else {
            registry::color_row_menu()
        };
        let deletable = session.layer_count(owner) > 1;
        match widgets::action_menu(&response, menu, |action| {
            action != MenuAction::Delete || deletable
        }) {
            Some(MenuAction::Rename) => begin_rename(ui, target, &color.name),
            Some(MenuAction::Properties) => {
                open(session);
                session.show_panel();
            }
            Some(action) => {
                session.layer_action(owner, color.id, action);
            }
            None => {}
        }
        if response.double_clicked() && !color.fixed {
            begin_rename(ui, target, &color.name);
        } else if response.drag_started() && !color.fixed {
            *drag = Some((owner, color.id));
        } else if response.clicked() {
            open(session);
        }
    }
    flag_button(
        ui,
        session,
        Target::Color(owner, color.id),
        PropId::Visible,
        color.visible,
        eye_x,
        rect,
    );
    if !color.fixed {
        flag_button(
            ui,
            session,
            Target::Color(owner, color.id),
            PropId::Locked,
            color.locked,
            lock_x,
            rect,
        );
    }
    name_rect
}

fn finish_object_drag(
    ui: &mut egui::Ui,
    session: &mut Session,
    dragging: Option<NodeId>,
    rects: &[(NodeId, Rect)],
) -> bool {
    let Some(id) = dragging else {
        return false;
    };
    let Some(pointer) = ui.input(|input| input.pointer.interact_pos()) else {
        return false;
    };
    let Some(from) = rects.iter().position(|(node, _)| *node == id) else {
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
    let y = rects[from].1.center().y;
    if target != from {
        let mark = match others.get(target) {
            Some(rect) => rect.top(),
            None => others.last().map(|rect| rect.bottom()).unwrap_or(y),
        };
        ui.painter().hline(
            (rects[from].1.left() - S.of(8.0))..=(rects[from].1.right()),
            mark,
            Stroke::new(S.of(2.0), widgets::INK),
        );
    }
    if ui.input(|input| input.pointer.any_released()) {
        if target != from {
            session.reorder_node(id, target);
        }
        return true;
    }
    false
}

fn finish_color_drag(
    ui: &mut egui::Ui,
    session: &mut Session,
    drag: Option<(NodeId, ColorId)>,
    _owner: Option<NodeId>,
    rects: &[(ColorId, Rect)],
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
        if target != from {
            let _ = session.move_color(id, target);
        }
        return true;
    }
    false
}

enum Target {
    Node(NodeId),
    Color(NodeId, ColorId),
}

fn flag_button(
    ui: &mut egui::Ui,
    session: &mut Session,
    target: Target,
    prop: PropId,
    on: bool,
    x: f32,
    row: Rect,
) {
    let spot = Rect::from_min_size(pos2(x, row.top()), vec2(S.of(22.0), row.height()));
    let response = ui.interact(
        spot,
        ui.id().with((prop as u8, x.to_bits(), row.top().to_bits())),
        Sense::click_and_drag(),
    );
    let icon = match (prop, on) {
        (PropId::Visible, true) => registry::EYE_OPEN,
        (PropId::Visible, false) => registry::EYE_CLOSED,
        (_, true) => registry::LOCK_CLOSED,
        (_, false) => registry::LOCK_OPEN,
    };
    toolbar::paint_icon(
        ui.painter(),
        spot.center(),
        icon,
        S.of(15.0),
        if response.hovered() { TEXT } else { MUTED },
    );
    if response.clicked() {
        match target {
            Target::Node(id) => {
                session.toggle_node(id, prop);
            }
            Target::Color(canvas, color) => {
                session.toggle_color_flag(canvas, color, prop);
            }
        }
    }
}

/// What is being renamed in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Renaming {
    Node(NodeId),
    Color(ColorId),
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

fn row_rect_id(ui: &egui::Ui, id: NodeId) -> egui::Id {
    ui.id().with(("row-rect", id.get()))
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
