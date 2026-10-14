//! Built-in palette and tool defaults. Preferences override these later.

pub struct PaletteColor {
    pub name: &'static str,
    pub rgb: [u8; 3],
}

/// Black through light gray. Cyan and gray are not in the default palette.
pub const PALETTE: &[PaletteColor] = &[
    PaletteColor {
        name: "Black",
        rgb: [20, 20, 20],
    },
    PaletteColor {
        name: "Red",
        rgb: [214, 48, 49],
    },
    PaletteColor {
        name: "Orange",
        rgb: [238, 122, 40],
    },
    PaletteColor {
        name: "Yellow",
        rgb: [241, 196, 15],
    },
    PaletteColor {
        name: "Green",
        rgb: [39, 174, 96],
    },
    PaletteColor {
        name: "Blue",
        rgb: [52, 120, 246],
    },
    PaletteColor {
        name: "Purple",
        rgb: [142, 68, 173],
    },
    PaletteColor {
        name: "Pink",
        rgb: [232, 67, 147],
    },
    PaletteColor {
        name: "Light gray",
        rgb: [189, 195, 199],
    },
];

pub const COLOR_SIZE: u8 = 3;
pub const COLOR_OPACITY: f64 = 1.0;
pub const CANVAS_PADDING: f64 = 16.0;
pub const ERASER_SIZE: u8 = 8;
pub const SHAPE_STROKE: u8 = 3;
/// The same black as the default palette.
pub const SHAPE_STROKE_RGB: [u8; 3] = [20, 20, 20];
pub const SHAPE_FILL_RGB: [u8; 3] = [0xFC, 0xA5, 0xA5];
pub const SHAPE_FILL_OPACITY: f64 = 0.4;
pub const CORNER_MIN: f64 = 0.0;
pub const CORNER_MAX: f64 = 64.0;
pub const FONT_SIZE: u8 = 24;
pub const SIZE_MIN: u8 = 1;
pub const SIZE_MAX: u8 = 32;
pub const FONT_MIN: u8 = 8;
pub const FONT_MAX: u8 = 144;
pub const LINE_HEIGHT: f64 = 1.2;
pub const LINE_HEIGHT_MIN: f64 = 0.8;
pub const LINE_HEIGHT_MAX: f64 = 3.0;
pub const LETTER_SPACING_MIN: f64 = -5.0;
pub const LETTER_SPACING_MAX: f64 = 20.0;
pub const TEXT_PADDING_MIN: f64 = 0.0;
pub const TEXT_PADDING_MAX: f64 = 48.0;
pub const TEXT_RADIUS_MIN: f64 = 0.0;
pub const TEXT_RADIUS_MAX: f64 = 48.0;
/// The width of a text box placed by a click while the text-box button is on.
pub const TEXT_BOX_WIDTH: f64 = 240.0;
/// The same black as the default palette.
pub const TEXT_RGB: [u8; 3] = [20, 20, 20];
pub const TEXT_BACKGROUND_RGB: [u8; 3] = [255, 255, 255];
/// The line around a text box, when its Border is on.
pub const TEXT_BORDER_RGB: [u8; 3] = [20, 20, 20];
pub const TEXT_BORDER_SIZE: u8 = 2;

pub const PRESSURE_SIZE: bool = true;
pub const SMOOTHING: u8 = 50;
pub const STABILIZATION: u8 = 0;
pub const STREAMLINE: u8 = 50;

pub const BRUSH_PRESETS: &[&str] = &["Round Brush", "Monoline"];
pub const ERASER_PRESETS: &[&str] = &["Round Eraser"];
pub const CUSTOM_PRESET: &str = "Custom";
