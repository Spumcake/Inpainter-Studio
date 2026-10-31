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
    /// The part of the picture that differs from revision `since` of the
    /// same file. A host that still shows `since` reads only these rows;
    /// any other host reads the whole picture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<RasterPatch>,
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
    /// The tab's pictures, as a filmstrip along the bottom of the working
    /// area, as the owner's mockup and 0.11's gallery views show. None draws
    /// no strip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub film: Option<Film>,
}

/// What changed since an earlier revision: `[x, y, width, height]` in
/// picture pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RasterPatch {
    pub since: u64,
    pub rect: [u32; 4],
}

/// A gallery's filmstrip: every picture in the tab, the one shown outlined
/// in blue, and a bordered + tile at the end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Film {
    pub items: Vec<FilmItem>,
    /// The index of the picture shown.
    pub shown: usize,
    /// A click on a thumbnail sends `select` with `{"value": index}`.
    pub select: crate::Action,
    /// A thumbnail dragged along the strip sends `reorder` with
    /// `{"from": index, "to": index}` on release, `to` counted after the
    /// dragged one is taken out.
    pub reorder: crate::Action,
    /// The + tile's menu: a click opens these rows above it.
    pub add: Vec<crate::MenuEntry>,
    /// A press anywhere on the strip sends `focus` first, so the
    /// application knows the strip, not the picture, has the keyboard.
    pub focus: crate::Action,
}

/// One picture on the filmstrip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilmItem {
    /// Shown when the pointer rests on the thumbnail.
    pub name: String,
    pub thumbnail: Thumbnail,
    /// The thumbnail's right-click menu.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub menu: Vec<crate::MenuEntry>,
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
    /// The newest points of a brush or eraser stroke still being drawn,
    /// sent as they come, so the sub-application paints them at once:
    /// `from` is how many points were sent before. The host draws no
    /// preview of its own; the picture shows the stroke.
    StrokePart { from: usize, points: Vec<[f64; 3]> },
    /// The open stroke was let go with Escape: it is taken off the picture.
    StrokeCancel,
    /// A finished brush or eraser stroke, every point: `[x, y, pressure]`,
    /// pressure from 0 to 1, and 1 with a mouse.
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
            patch: Some(RasterPatch { since: 2, rect: [1, 2, 3, 4] }),
            tool: RasterTool::Brush { rgb: [20, 20, 22], size: 24.0, hardness: 0.8, opacity: 1.0, pressure: true },
            selection: vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]],
            crop: Some([1.0, 2.0, 3.0, 4.0]),
            bounds: Some([0.0, 0.0, 1920.0, 1080.0]),
            camera: Some(DocumentCamera { origin: [0.0, 0.0], zoom: 0.5 }),
            film: Some(Film {
                items: vec![FilmItem {
                    name: "Beach".into(),
                    thumbnail: Thumbnail { path: "/tmp/t.rgba".into(), width: 68, height: 44, revision: 1 },
                    menu: Vec::new(),
                }],
                shown: 0,
                select: crate::Action::new("select_picture"),
                reorder: crate::Action::new("move_picture"),
                add: Vec::new(),
                focus: crate::Action::new("film_focus"),
            }),
        };
        let text = serde_json::to_string(&crate::AppMessage::Raster(view.clone())).unwrap();
        assert_eq!(crate::parse_app_message(&text).unwrap(), crate::AppMessage::Raster(view));
        for gesture in [
            RasterGesture::StrokePart { from: 3, points: vec![[1.0, 2.0, 0.5]] },
            RasterGesture::StrokeCancel,
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
