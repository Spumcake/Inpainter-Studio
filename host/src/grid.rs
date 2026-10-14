//! The shared grid, drawn in a tab's contents area when that tab declares it.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke};

const GRID_SPACING: f64 = 24.0;

pub(crate) fn screen_to_document(rect: Rect, origin: [f64; 2], zoom: f64, pos: Pos2) -> [f64; 2] {
    [
        origin[0] + f64::from(pos.x - rect.min.x) / zoom,
        origin[1] + f64::from(pos.y - rect.min.y) / zoom,
    ]
}

pub(crate) fn document_to_screen(rect: Rect, origin: [f64; 2], zoom: f64, point: [f64; 2]) -> Pos2 {
    Pos2::new(
        ((point[0] - origin[0]) * zoom) as f32 + rect.min.x,
        ((point[1] - origin[1]) * zoom) as f32 + rect.min.y,
    )
}

pub(crate) fn paint_grid(painter: &egui::Painter, rect: Rect, origin: [f64; 2], zoom: f64) {
    // As faint as the picture's grid: a guide, not a feature of the drawing.
    let stroke = Stroke::new(1.0, Color32::from_rgb(234, 235, 238));
    let top_left = screen_to_document(rect, origin, zoom, rect.min);
    let bottom_right = screen_to_document(rect, origin, zoom, rect.max);
    let mut x = (top_left[0] / GRID_SPACING).floor() * GRID_SPACING;
    while x <= bottom_right[0] {
        let start = document_to_screen(rect, origin, zoom, [x, top_left[1]]);
        let end = document_to_screen(rect, origin, zoom, [x, bottom_right[1]]);
        painter.line_segment([start, end], stroke);
        x += GRID_SPACING;
    }
    let mut y = (top_left[1] / GRID_SPACING).floor() * GRID_SPACING;
    while y <= bottom_right[1] {
        let start = document_to_screen(rect, origin, zoom, [top_left[0], y]);
        let end = document_to_screen(rect, origin, zoom, [bottom_right[0], y]);
        painter.line_segment([start, end], stroke);
        y += GRID_SPACING;
    }
}
