//! Property ids, descriptors, and sections. One schema drives the panel.
//! Setting a value clamps in the `Inspect` implementation of that value.

/// Shown when a tool's settings match none of its presets.
pub const CUSTOM_PRESET: u8 = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropId {
    Preset,
    PressureSize,
    Curve,
    Smoothing,
    Stabilization,
    Streamline,
    EraserSize,
    Name,
    Hex,
    Rgb,
    Opacity,
    ColorSize,
    Dash,
    Cap,
    /// Whether a color is drawn: on every color page, and a layer's eye.
    Visible,
    Locked,
    /// Canvas padding, 0 through 128.
    Padding,
    /// Under Stroke, on a tool's color that has a size: the next mark follows
    /// the color's size until the Size slider is moved.
    LockSize,
    /// A palette as rows of its colors. Each row opens that color.
    Palette,
    CornerRadius,
    /// A shape tool's setting. Not stored on the object.
    Constrain,
    /// A shape tool's setting. Not stored on the object.
    Snap,
    Family,
    FontSize,
    Bold,
    Italic,
    Underline,
    Align,
    VAlign,
    /// Every line as a bullet, number, or checkbox item, or none.
    List,
    /// Space between a text box's edge and its words.
    TextPadding,
    /// A text box's corner radius.
    TextRadius,
    LineHeight,
    LetterSpacing,
    AllCaps,
    AutoSize,
    WrapText,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PropValue {
    Bool(bool),
    Number(f64),
    Text(String),
    Index(u8),
    Rgb([u8; 3]),
    Curve(Vec<(f64, f64)>),
    Idle,
    /// The selected objects do not share one value.
    Mixed,
}

/// How a choice's options are drawn beside or instead of their labels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoicePreview {
    /// A short line drawn solid or dashed, before the option's label.
    Dash,
    /// A short thick bar with round or flat ends, in place of the label.
    Cap,
    /// The option's words, with no sample.
    Label,
}

#[derive(Clone, Copy, Debug)]
pub enum PropKind {
    Toggle,
    /// `percent` shows `value * 100` with a percent sign. The stored range is `min..=max`.
    Number {
        min: f64,
        max: f64,
        percent: bool,
        /// Digits after the decimal point. Zero shows a whole number.
        places: u8,
    },
    Text,
    Choice {
        options: &'static [&'static str],
        segmented: bool,
        preview: ChoicePreview,
    },
    /// Options depend on which tool is the subject.
    Preset,
    /// A color well that opens a picker.
    Rgb,
    Curve,
    /// The owner's colors, one row each.
    Palette,
}

#[derive(Clone, Copy, Debug)]
pub struct PropDesc {
    pub id: PropId,
    pub label: &'static str,
    pub kind: PropKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionId {
    Preset,
    Size,
    Dynamics,
    PressureCurve,
    Behavior,
    Color,
    Stroke,
    StrokeStyle,
    /// The tool's or object's colors. Every tool with colors lists it first.
    Palette,
    /// A rectangle's corner radius.
    ShapeStyle,
    /// A shape tool's Constrain proportions and Snap to grid, a switch each.
    ShapeBehavior,
    Text,
    TextLayout,
    TextBehavior,
}

#[derive(Clone, Copy, Debug)]
pub struct SectionDesc {
    pub title: &'static str,
    pub props: &'static [PropId],
}

pub trait Inspect {
    fn read(&self, id: PropId) -> Option<PropValue>;
    /// Clamps numbers into range. Returns false when the value cannot be
    /// applied, and leaves the previous value in place.
    fn write(&mut self, id: PropId, value: &PropValue) -> bool;
}

const PRESET: &[PropId] = &[PropId::Preset];
const SIZE: &[PropId] = &[PropId::EraserSize];
const DYNAMICS: &[PropId] = &[PropId::PressureSize];
const CURVE: &[PropId] = &[PropId::Curve];
const BEHAVIOR: &[PropId] = &[PropId::Smoothing, PropId::Stabilization, PropId::Streamline];
// The color page follows color-property-ui.png and nothing else.
const COLOR: &[PropId] = &[PropId::Rgb, PropId::Name, PropId::Hex, PropId::Visible];
const STROKE: &[PropId] = &[PropId::ColorSize, PropId::Opacity, PropId::LockSize];
const STROKE_STYLE: &[PropId] = &[PropId::Dash, PropId::Cap];

pub const NO_SECTIONS: &[SectionId] = &[];
// A tool's colors are its swatches; their pages open from a swatch's menu,
// so a tool's page has no Palette section.
pub const BRUSH_SECTIONS: &[SectionId] = &[
    SectionId::Preset,
    SectionId::Dynamics,
    SectionId::PressureCurve,
    SectionId::Behavior,
];
pub const ERASER_SECTIONS: &[SectionId] = &[
    SectionId::Preset,
    SectionId::Size,
    SectionId::Dynamics,
    SectionId::PressureCurve,
    SectionId::Behavior,
];
/// One palette color, from any palette. Rows a color's role does not use
/// are left out.
pub const COLOR_SECTIONS: &[SectionId] =
    &[SectionId::Color, SectionId::Stroke, SectionId::StrokeStyle];
const PALETTE_PROPS: &[PropId] = &[PropId::Palette];
const SHAPE_STYLE_PROPS: &[PropId] = &[PropId::CornerRadius];
const SHAPE_BEHAVIOR_PROPS: &[PropId] = &[PropId::Constrain, PropId::Snap];
pub const RECTANGLE_SECTIONS: &[SectionId] = &[SectionId::ShapeStyle, SectionId::ShapeBehavior];
pub const ELLIPSE_SECTIONS: &[SectionId] = &[SectionId::ShapeBehavior];
pub const LINE_SECTIONS: &[SectionId] = &[SectionId::ShapeBehavior];
/// A selected rectangle, under Layers. Constrain and snap stay on the tool.
pub const RECTANGLE_OBJECT_SECTIONS: &[SectionId] = &[SectionId::Palette, SectionId::ShapeStyle];
pub const ELLIPSE_OBJECT_SECTIONS: &[SectionId] = &[SectionId::Palette];
pub const LINE_OBJECT_SECTIONS: &[SectionId] = &[SectionId::Palette];
/// A selected canvas: its layers, each opening that layer's color.
pub const CANVAS_OBJECT_SECTIONS: &[SectionId] = &[SectionId::Palette];
const TEXT_FACE: &[PropId] = &[
    PropId::Family,
    PropId::FontSize,
    PropId::Bold,
    PropId::Italic,
    PropId::Underline,
];
const TEXT_LAYOUT: &[PropId] = &[
    PropId::Align,
    PropId::VAlign,
    PropId::List,
    PropId::LineHeight,
    PropId::LetterSpacing,
    PropId::TextPadding,
    PropId::TextRadius,
];
const TEXT_BEHAVIOR: &[PropId] = &[PropId::AllCaps, PropId::AutoSize, PropId::WrapText];
pub const TEXT_SECTIONS: &[SectionId] = &[
    SectionId::Text,
    SectionId::TextLayout,
    SectionId::TextBehavior,
];
/// A selected text object: its own colors, then the text tool's sections.
pub const TEXT_OBJECT_SECTIONS: &[SectionId] = &[
    SectionId::Palette,
    SectionId::Text,
    SectionId::TextLayout,
    SectionId::TextBehavior,
];

const DASH_OPTIONS: &[&str] = &["Solid", "Dashed"];
const CAP_OPTIONS: &[&str] = &["Round", "Flat"];
const FAMILY_OPTIONS: &[&str] = &["Inter", "Monospace"];
const ALIGN_OPTIONS: &[&str] = &["Left", "Center", "Right", "Justify"];
const VALIGN_OPTIONS: &[&str] = &["Top", "Middle", "Bottom"];
const LIST_OPTIONS: &[&str] = &["None", "Bullets", "Numbers", "Checklist"];

pub fn section(id: SectionId) -> SectionDesc {
    match id {
        SectionId::Preset => SectionDesc {
            title: "Preset",
            props: PRESET,
        },
        SectionId::Size => SectionDesc {
            title: "Size",
            props: SIZE,
        },
        SectionId::Dynamics => SectionDesc {
            title: "Dynamics",
            props: DYNAMICS,
        },
        SectionId::PressureCurve => SectionDesc {
            title: "Pressure Curve",
            props: CURVE,
        },
        SectionId::Behavior => SectionDesc {
            title: "Behavior",
            props: BEHAVIOR,
        },
        SectionId::Color => SectionDesc {
            title: "Color",
            props: COLOR,
        },
        SectionId::Stroke => SectionDesc {
            title: "Stroke",
            props: STROKE,
        },
        SectionId::StrokeStyle => SectionDesc {
            title: "Stroke Style",
            props: STROKE_STYLE,
        },
        SectionId::Palette => SectionDesc {
            title: "Palette",
            props: PALETTE_PROPS,
        },
        SectionId::ShapeStyle => SectionDesc {
            title: "Shape style",
            props: SHAPE_STYLE_PROPS,
        },
        SectionId::ShapeBehavior => SectionDesc {
            title: "Behavior",
            props: SHAPE_BEHAVIOR_PROPS,
        },
        SectionId::Text => SectionDesc {
            title: "Text",
            props: TEXT_FACE,
        },
        SectionId::TextLayout => SectionDesc {
            title: "Layout",
            props: TEXT_LAYOUT,
        },
        SectionId::TextBehavior => SectionDesc {
            title: "Behavior",
            props: TEXT_BEHAVIOR,
        },
    }
}

pub fn prop(id: PropId) -> PropDesc {
    match id {
        PropId::Preset => PropDesc {
            id,
            label: "",
            kind: PropKind::Preset,
        },
        PropId::PressureSize => PropDesc {
            id,
            label: "Pressure affects size",
            kind: PropKind::Toggle,
        },
        PropId::Curve => PropDesc {
            id,
            label: "",
            kind: PropKind::Curve,
        },
        PropId::Smoothing => number(id, "Smoothing", 0.0, 100.0, false),
        PropId::Stabilization => number(id, "Stabilization", 0.0, 100.0, false),
        PropId::Streamline => number(id, "Streamline", 0.0, 100.0, false),
        PropId::EraserSize => number(id, "", 1.0, 32.0, false),
        PropId::Name => PropDesc {
            id,
            label: "Name",
            kind: PropKind::Text,
        },
        PropId::Hex => PropDesc {
            id,
            label: "Hex",
            kind: PropKind::Text,
        },
        PropId::Rgb => PropDesc {
            id,
            label: "Color",
            kind: PropKind::Rgb,
        },
        PropId::Opacity => number(id, "Opacity", 0.0, 1.0, true),
        PropId::ColorSize => number(id, "Size", 1.0, 32.0, false),
        PropId::Dash => PropDesc {
            id,
            label: "Style",
            kind: PropKind::Choice {
                options: DASH_OPTIONS,
                segmented: false,
                preview: ChoicePreview::Dash,
            },
        },
        PropId::Cap => PropDesc {
            id,
            label: "Cap",
            kind: PropKind::Choice {
                options: CAP_OPTIONS,
                segmented: true,
                preview: ChoicePreview::Cap,
            },
        },
        PropId::Visible => PropDesc {
            id,
            label: "Visible",
            kind: PropKind::Toggle,
        },
        PropId::Locked => PropDesc {
            id,
            label: "Locked",
            kind: PropKind::Toggle,
        },
        PropId::Padding => number(id, "Padding", 0.0, 128.0, false),
        PropId::LockSize => PropDesc {
            id,
            label: "Lock size",
            kind: PropKind::Toggle,
        },
        PropId::Palette => PropDesc {
            id,
            label: "",
            kind: PropKind::Palette,
        },
        PropId::CornerRadius => number(id, "Corner radius", 0.0, 64.0, false),
        PropId::Constrain => PropDesc {
            id,
            label: "Constrain proportions",
            kind: PropKind::Toggle,
        },
        PropId::Snap => PropDesc {
            id,
            label: "Snap to grid",
            kind: PropKind::Toggle,
        },
        PropId::Family => PropDesc {
            id,
            label: "Family",
            kind: PropKind::Choice {
                options: FAMILY_OPTIONS,
                segmented: false,
                preview: ChoicePreview::Label,
            },
        },
        PropId::FontSize => number(id, "Size", 8.0, 144.0, false),
        PropId::Bold => PropDesc {
            id,
            label: "Bold",
            kind: PropKind::Toggle,
        },
        PropId::Italic => PropDesc {
            id,
            label: "Italic",
            kind: PropKind::Toggle,
        },
        PropId::Underline => PropDesc {
            id,
            label: "Underline",
            kind: PropKind::Toggle,
        },
        PropId::Align => PropDesc {
            id,
            label: "Alignment",
            kind: PropKind::Choice {
                options: ALIGN_OPTIONS,
                segmented: false,
                preview: ChoicePreview::Label,
            },
        },
        PropId::VAlign => PropDesc {
            id,
            label: "Vertical",
            kind: PropKind::Choice {
                options: VALIGN_OPTIONS,
                segmented: false,
                preview: ChoicePreview::Label,
            },
        },
        PropId::List => PropDesc {
            id,
            label: "List",
            kind: PropKind::Choice {
                options: LIST_OPTIONS,
                segmented: false,
                preview: ChoicePreview::Label,
            },
        },
        PropId::TextPadding => number(id, "Padding", 0.0, 48.0, false),
        PropId::TextRadius => number(id, "Radius", 0.0, 48.0, false),
        PropId::LineHeight => number_places(id, "Line height", 0.8, 3.0, false, 1),
        PropId::LetterSpacing => number(id, "Letter spacing", -5.0, 20.0, false),
        PropId::AllCaps => PropDesc {
            id,
            label: "All caps",
            kind: PropKind::Toggle,
        },
        PropId::AutoSize => PropDesc {
            id,
            label: "Auto-size",
            kind: PropKind::Toggle,
        },
        PropId::WrapText => PropDesc {
            id,
            label: "Wrap text",
            kind: PropKind::Toggle,
        },
    }
}

fn number(id: PropId, label: &'static str, min: f64, max: f64, percent: bool) -> PropDesc {
    number_places(id, label, min, max, percent, 0)
}

fn number_places(
    id: PropId,
    label: &'static str,
    min: f64,
    max: f64,
    percent: bool,
    places: u8,
) -> PropDesc {
    PropDesc {
        id,
        label,
        kind: PropKind::Number {
            min,
            max,
            percent,
            places,
        },
    }
}

/// A whole number clamped into `min..=max`. Non-finite input is refused.
pub fn clamp_u8(number: f64, min: u8, max: u8) -> Option<u8> {
    if !number.is_finite() {
        return None;
    }
    Some(number.round().clamp(f64::from(min), f64::from(max)) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The color page is color-property-ui.png: Color, Stroke, and Stroke
    /// Style, with a Visible switch every color has. Its actions are in a
    /// swatch's or a layer row's menu, not on the page.
    #[test]
    fn the_color_page_follows_its_render() {
        let page: Vec<(&str, Vec<PropId>)> = COLOR_SECTIONS
            .iter()
            .map(|id| {
                let section = section(*id);
                (section.title, section.props.to_vec())
            })
            .collect();
        assert_eq!(
            page,
            vec![
                (
                    "Color",
                    vec![PropId::Rgb, PropId::Name, PropId::Hex, PropId::Visible]
                ),
                (
                    "Stroke",
                    vec![PropId::ColorSize, PropId::Opacity, PropId::LockSize]
                ),
                ("Stroke Style", vec![PropId::Dash, PropId::Cap]),
            ]
        );
        assert!(matches!(
            prop(PropId::Cap).kind,
            PropKind::Choice {
                options: &["Round", "Flat"],
                segmented: true,
                ..
            }
        ));
    }
}
