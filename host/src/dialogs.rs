//! Native open and save dialogs. Tests pass a path into the session and do
//! not call this module, so they never open a dialog.

use std::path::{Path, PathBuf};

pub(crate) fn pick_open(parent: &eframe::Frame, directory: Option<&Path>) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().add_filter("Sketch", &["smx"]);
    if let Some(directory) = directory {
        dialog = dialog.set_directory(directory);
    }
    dialog.set_parent(parent).pick_file()
}

pub(crate) fn pick_save(
    parent: &eframe::Frame,
    directory: Option<&Path>,
    file_name: &str,
) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new()
        .add_filter("Sketch", &["smx"])
        .set_file_name(file_name);
    if let Some(directory) = directory {
        dialog = dialog.set_directory(directory);
    }
    dialog.set_parent(parent).save_file()
}
