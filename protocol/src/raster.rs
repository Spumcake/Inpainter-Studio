//! A tab of pixels: the picture a sub-application composites, what the
//! host's pointer does on it, and the gestures the host sends back. The host
//! draws the picture, its selection, crop box, and layer bounds, previews a
//! brush stroke while it is drawn, and owns the camera. The sub-application
//! owns every pixel, layer, and step of history.

use crate::DocumentCamera;
use serde::{Deserialize, Serialize};

/// What the host draws in a tab whose contents are `raster`. Sent after
/// every change; the host reads the picture again when `revision` changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RasterView {
    pub tab: String,
    /// The picture's size, in pixels.
    pub width: u32,
    pub height: u32,
    /// The composited picture: a file of `width × height × 4` bytes, RGBA
    /// with straight (unpremultiplied) alpha, rows top to bottom. It is
    /// replaced whole, by a write to a scratch file and a rename, so the
    /// host never reads half of one.
    pub picture: String,
    /// Rises with every change to the picture.
    pub revision: u64,
    pub tool: RasterTool,
    /// The selection, as closed outlines in picture pixels, drawn with a
    /// marching edge. Empty when nothing is selected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selection: Vec<Vec<[f64; 2]>>,
    /// The crop box being set, `[x, y, width, height]` in picture pixels:
    /// drawn with thirds and its outside dimmed. Its edges and corners drag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<[f64; 4]>,
    /// The active layer's box, `[x, y, width, height]`, drawn with handles
    /// while the move tool is chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<[f64; 4]>,
    /// Where the host's camera goes, when a picture is opened or made. The
    /// host owns the camera after that and reports it with `camera`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<DocumentCamera>,
}

/// What the host's pointer does on a raster tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RasterTool {
    /// A drag moves the active layer: `drag`.
    Move,
    /// A drag makes a rectangle: `rect`.
    Marquee,
    /// A drag draws a freehand outline: `lasso`.
    Lasso,
    /// A drag makes or changes the crop box: `crop`.
    Crop,
    /// A click picks a colour: `pick`.
    Eyedropper,
    /// A pressure stroke, previewed as it is drawn: `stroke`. Sizes are in
    /// picture pixels; hardness and opacity run from 0 to 1.
    Brush {
        rgb: [u8; 3],
        size: f64,
        hardness: f64,
        opacity: f64,
        /// Pen pressure scales the size.
        pressure: bool,
    },
    /// As the brush, taking colour away: `stroke`.
    Erase {
        size: f64,
        hardness: f64,
        opacity: f64,
        pressure: bool,
    },
    /// A drag pans the camera. Nothing is sent.
    Hand,
    /// A click zooms in at the pointer, with Alt out. Nothing is sent.
    Zoom,
}

/// One finished gesture on a raster tab, in picture pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RasterGesture {
    /// A brush or eraser stroke: `[x, y, pressure]`, pressure from 0 to 1,
    /// and 1 with a mouse.
    Stroke { points: Vec<[f64; 3]> },
    /// A rectangle, corner to corner.
    Rect { from: [f64; 2], to: [f64; 2] },
    /// A freehand outline, closed by its last point.
    Lasso { points: Vec<[f64; 2]> },
    /// A move, from where the press was to where it was released.
    Drag { from: [f64; 2], to: [f64; 2] },
    /// A click, for the eyedropper.
    Pick { at: [f64; 2] },
    /// The crop box after a drag made or changed it: `[x, y, width, height]`.
    Crop { rect: [f64; 4] },
}

/// A small picture of one layer, for its row: a file of `width × height × 4`
/// bytes, as `RasterView::picture` is. The host reads it again when
/// `revision` changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Thumbnail {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub revision: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every raster message survives a round trip, so a field cannot be
    /// dropped quietly.
    #[test]
    fn raster_messages_round_trip() {
        let view = RasterView {
            tab: "1".into(),
            width: 1920,
            height: 1080,
            picture: "/tmp/1.rgba".into(),
            revision: 3,
            tool: RasterTool::Brush { rgb: [20, 20, 22], size: 24.0, hardness: 0.8, opacity: 1.0, pressure: true },
            selection: vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]],
            crop: Some([1.0, 2.0, 3.0, 4.0]),
            bounds: Some([0.0, 0.0, 1920.0, 1080.0]),
            camera: Some(DocumentCamera { origin: [0.0, 0.0], zoom: 0.5 }),
        };
        let text = serde_json::to_string(&crate::AppMessage::Raster(view.clone())).unwrap();
        assert_eq!(crate::parse_app_message(&text).unwrap(), crate::AppMessage::Raster(view));
        for gesture in [
            RasterGesture::Stroke { points: vec![[1.0, 2.0, 0.5]] },
            RasterGesture::Rect { from: [0.0, 0.0], to: [5.0, 5.0] },
            RasterGesture::Lasso { points: vec![[0.0, 0.0], [5.0, 0.0], [5.0, 5.0]] },
            RasterGesture::Drag { from: [0.0, 0.0], to: [5.0, 5.0] },
            RasterGesture::Pick { at: [3.0, 4.0] },
            RasterGesture::Crop { rect: [0.0, 0.0, 5.0, 5.0] },
        ] {
            let message = crate::HostMessage::RasterGesture { tab: "1".into(), gesture };
            let text = serde_json::to_string(&message).unwrap();
            assert_eq!(serde_json::from_str::<crate::HostMessage>(&text).unwrap(), message);
        }
    }
}
