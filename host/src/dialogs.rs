//! Native open, save, and folder dialogs, opened when a sub-application sends
//! a `file` request or the project menu asks for a folder. Frame tests answer
//! requests themselves and do not call this module, so they never open a dialog.

use protocol::FileFilter;
use std::path::{Path, PathBuf};

pub(crate) fn pick_open(
    parent: &eframe::Frame,
    directory: Option<&Path>,
    filter: Option<&FileFilter>,
) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new();
    if let Some(filter) = filter {
        dialog = dialog.add_filter(&filter.name, &filter.extensions);
    }
    if let Some(directory) = directory {
        dialog = dialog.set_directory(directory);
    }
    dialog.set_parent(parent).pick_file()
}

pub(crate) fn pick_save(
    parent: &eframe::Frame,
    directory: Option<&Path>,
    file_name: &str,
    filter: Option<&FileFilter>,
) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_file_name(file_name);
    if let Some(filter) = filter {
        dialog = dialog.add_filter(&filter.name, &filter.extensions);
    }
    if let Some(directory) = directory {
        dialog = dialog.set_directory(directory);
    }
    dialog.set_parent(parent).save_file()
}

/// Several files to open at once, for the assistant's attachments.
pub(crate) fn pick_files(parent: &eframe::Frame, filter: &FileFilter) -> Vec<PathBuf> {
    rfd::FileDialog::new()
        .add_filter(&filter.name, &filter.extensions)
        .set_parent(parent)
        .pick_files()
        .unwrap_or_default()
}

/// A folder to attach to the current project. It can be anywhere.
pub(crate) fn pick_folder(parent: &eframe::Frame) -> Option<PathBuf> {
    rfd::FileDialog::new().set_parent(parent).pick_folder()
}

/// The data folder cannot be used, so Studio shows the message and does not
/// open a window. The same portal as the file dialogs.
pub(crate) fn unavailable() {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("Inpainter")
        .set_description("The data folder is not available.")
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

/// The second start: Studio is already open, so this one shows the message
/// and does not open a window. The same portal as the file dialogs.
pub(crate) fn already_open() {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Info)
        .set_title("Inpainter")
        .set_description(crate::workspace::ALREADY_OPEN)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}
