//! The conversation guide's one type scale, its spacing, and its colors.
//! The conversation, cells, the composer, and chips use these and no other
//! sizes. The guide is the team repository's
//! `.project/projects/inpainter-studio/documents/guides/conversation-guide.md`.

use eframe::egui::{Color32, FontFamily, FontId};

use crate::icons::{INTER_BOLD, INTER_BOLD_ITALIC, INTER_ITALIC};

/// Prose, the person's messages, and a cell's result: Inter 14 on 21, a
/// step down from the research's 15 so the conversation sits with the rest
/// of Studio, as the owner set on 2026-10-28.
pub(crate) const BODY: f32 = 14.0;
pub(crate) const BODY_LINE: f32 = 21.0;
/// Tool rows, actions, the turn's time line, captions: Inter 12 on 16.
pub(crate) const SMALL: f32 = 12.0;
pub(crate) const SMALL_LINE: f32 = 16.0;
/// Inline code, code blocks, and a cell's prompt: Hack 13 on 20.
pub(crate) const CODE: f32 = 13.0;
pub(crate) const CODE_LINE: f32 = 20.0;
/// A cell's kind and the project's name: Inter SemiBold 14.
pub(crate) const TITLE: f32 = 14.0;

/// A heading's size and line: close to body, as the guide sets.
pub(crate) fn heading(level: usize) -> (f32, f32) {
    match level {
        1 => (16.5, 23.0),
        2 => (15.5, 21.0),
        _ => (14.0, 21.0),
    }
}

pub(crate) fn body() -> FontId {
    FontId::proportional(BODY)
}

pub(crate) fn small() -> FontId {
    FontId::proportional(SMALL)
}

pub(crate) fn code() -> FontId {
    FontId::monospace(CODE)
}

pub(crate) fn title() -> FontId {
    crate::widgets::semibold(TITLE)
}

/// Inter in the weight and slant asked for.
pub(crate) fn inter(size: f32, bold: bool, italic: bool) -> FontId {
    let family = match (bold, italic) {
        (true, true) => FontFamily::Name(INTER_BOLD_ITALIC.into()),
        (true, false) => FontFamily::Name(INTER_BOLD.into()),
        (false, true) => FontFamily::Name(INTER_ITALIC.into()),
        (false, false) => FontFamily::Proportional,
    };
    FontId::new(size, family)
}

// Spacing.
pub(crate) const PARAGRAPH: f32 = 12.0;
pub(crate) const HEADING_ABOVE: f32 = 22.0;
pub(crate) const HEADING_BELOW: f32 = 10.0;
pub(crate) const LIST_ITEM: f32 = 6.0;
pub(crate) const LIST_INDENT: f32 = 20.0;
pub(crate) const CODE_PAD: f32 = 8.0;
pub(crate) const CODE_ABOVE: f32 = 12.0;
pub(crate) const CODE_BELOW: f32 = 16.0;
/// One gap between everything in the conversation, as the owner set on
/// 2026-10-28: between a message and its reply, between the blocks of a
/// reply (prose, a tool row, a cell), and between a reply and the next
/// message.
pub(crate) const BLOCK: f32 = 22.0;
pub(crate) const TURN: f32 = BLOCK;
/// The turn's line sits close under the reply it belongs to.
pub(crate) const FOOTER: f32 = 8.0;
/// The conversation's column, at most this wide, with these gutters.
pub(crate) const COLUMN: f32 = 760.0;
pub(crate) const GUTTER: f32 = 20.0;

// Colors.
pub(crate) const TEXT: Color32 = Color32::from_rgb(0x2D, 0x2F, 0x33);
pub(crate) const MUTED: Color32 = Color32::from_rgb(0x6B, 0x72, 0x80);
pub(crate) const BORDER: Color32 = Color32::from_rgb(0xE2, 0xE5, 0xE9);
pub(crate) const HAIRLINE: Color32 = Color32::from_rgb(0xEC, 0xED, 0xF0);
pub(crate) const FAILED: Color32 = Color32::from_rgb(0xB9, 0x1C, 0x1C);
pub(crate) const CODE_FILL: Color32 = Color32::from_rgb(0xF5, 0xF6, 0xF8);
