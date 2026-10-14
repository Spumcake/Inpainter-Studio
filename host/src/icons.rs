//! Font names and the panel shortcut. Tool chords and icon specs come from
//! the sketch registry.

use eframe::egui;

pub(crate) use sketch::registry::{icon_spec, hover, Chord, Key};

pub(crate) const INTER: &str = "Inter";
pub(crate) const INTER_BOLD: &str = "Inter Bold";
pub(crate) const INTER_ITALIC: &str = "Inter Italic";
pub(crate) const INTER_BOLD_ITALIC: &str = "Inter Bold Italic";

/// The panel shortcut. The edge tab's hover is "Properties (F4)".
const PANEL_LABEL: &str = "Properties";
const PANEL: &[Chord] = &[Chord {
    command: false,
    shift: Some(false),
    alt: false,
    key: Key::F4,
}];

pub(crate) fn panel_tip() -> String {
    hover(PANEL_LABEL, PANEL)
}

/// The moved panel chord: plain F4, so Shift, Ctrl, and Alt do not count.
pub(crate) fn panel_shortcut_pressed(input: &egui::InputState) -> bool {
    PANEL.iter().any(|chord| {
        if input.modifiers.command != chord.command || input.modifiers.alt != chord.alt {
            return false;
        }
        if let Some(shift) = chord.shift
            && input.modifiers.shift != shift
        {
            return false;
        }
        matches!(chord.key, Key::F4) && input.key_pressed(egui::Key::F4)
    })
}
