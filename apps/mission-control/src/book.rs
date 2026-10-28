//! A mission book: the cells, which one is selected, the file, and the undo.
//!
//! The host draws the cells and runs them. This process only keeps them.

use protocol::{
    Action, AppMessage, Ask, AskButton, Cell, CellPart, CellState, Chord, ContentsKind, FileFilter,
    FileKind, HostMessage, Keys, MenuEntry, TabEntry, Tabs, ToolItem, ToolResult,
    Toolbar,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const VERSION: u32 = 1;
const KIND: &str = "text";

#[derive(Clone)]
struct Snap {
    cells: Vec<Cell>,
    selected: Option<String>,
}

struct Book {
    id: String,
    label: String,
    path: Option<PathBuf>,
    cells: Vec<Cell>,
    selected: Option<String>,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    dirty: bool,
}

/// What a save during quit is waiting on.
struct Saving {
    book: usize,
    /// The quit continues after the write.
    then_quit: bool,
}

pub struct Session {
    books: Vec<Book>,
    active: usize,
    next: u64,
    ask: Option<Ask>,
    /// Book indices still to ask about, after the one `ask` names.
    quit_rest: Vec<usize>,
    quitting: bool,
    /// A close of one book is asking, rather than the window's quit.
    closing: bool,
    saving: Option<Saving>,
    /// The file request's id, when one is open.
    file_id: Option<String>,
    next_file: u64,
}

#[derive(Serialize, Deserialize)]
struct BookFile {
    version: u32,
    cells: Vec<Cell>,
}

impl Session {
    /// The book a person gets on launch: one empty book, already declared.
    pub fn launch() -> Self {
        let mut session = Self::empty();
        session.push_book(None);
        session
    }

    /// Started with `--restore`: no book until the host sends `open`.
    pub fn restored() -> Self {
        Self::empty()
    }

    fn empty() -> Self {
        Self {
            books: Vec::new(),
            active: 0,
            next: 1,
            ask: None,
            quit_rest: Vec::new(),
            quitting: false,
            closing: false,
            saving: None,
            file_id: None,
            next_file: 1,
        }
    }

    pub fn greeting(&self) -> Vec<AppMessage> {
        let mut messages = vec![AppMessage::Hello {
            protocol: protocol::PROTOCOL_VERSION,
        }];
        if let Some(book) = self.books.first() {
            messages.push(AppMessage::Tab {
                id: book.id.clone(),
                label: book.label.clone(),
            });
            messages.push(AppMessage::Contents {
                tab: book.id.clone(),
                kind: ContentsKind::Cells,
            });
        }
        // No skills yet: the assistant reading or changing a book comes later.
        messages.push(AppMessage::Skills { skills: Vec::new() });
        messages.push(AppMessage::End);
        if !self.books.is_empty() {
            messages.extend(self.declared());
        }
        messages
    }

    /// One message from the host, and every reply.
    pub fn handle(&mut self, message: HostMessage) -> Vec<AppMessage> {
        let mut out = Vec::new();
        match message {
            HostMessage::Shutdown => return Vec::new(),
            HostMessage::Ping => return vec![AppMessage::Pong],
            HostMessage::Quit => self.note_quit(&mut out),
            HostMessage::Open { path } => self.open_path(path, &mut out),
            HostMessage::Command { id, tab, .. } => self.command(&id, tab.as_deref(), &mut out),
            HostMessage::Answer { button, .. } => {
                self.ask = None;
                self.answer(&button, &mut out);
            }
            HostMessage::FileResult { id, path } => self.file_result(&id, path, &mut out),
            HostMessage::EditCell { tab, cell, input } => self.edit(&tab, &cell, input, &mut out),
            HostMessage::CellResult {
                tab,
                cell,
                endpoint,
                model,
                state,
                output,
                error,
            } => self.finish(&tab, &cell, endpoint, model, state, output, error, &mut out),
            HostMessage::InsertCell { tab, index, cell } => self.insert(&tab, index, cell, &mut out),
            HostMessage::SelectCell { tab, cell } => self.select(&tab, &cell, &mut out),
            HostMessage::Claim { id, path } => out.push(AppMessage::Claimed {
                id,
                yours: claims(Path::new(&path)),
            }),
            HostMessage::Invoke { call, .. } => out.extend(self.invoke(call)),
            HostMessage::Keep { tab } => out.push(AppMessage::Kept { tab, path: None }),
            _ => {}
        }
        out
    }

    fn command(&mut self, id: &str, tab: Option<&str>, out: &mut Vec<AppMessage>) {
        if let Some(tab) = tab {
            self.activate(tab);
        }
        match id {
            "new_book" => {
                self.push_book(None);
                out.extend(self.declared());
            }
            "open" => self.ask_file(FileKind::Open, None, false, out),
            "save" => self.save_active(false, out),
            "save_as" => self.save_active(true, out),
            "add_cell" => self.add_cell(out),
            "delete_cell" => self.delete_cell(out),
            "run" => self.request_run(out),
            "stop" => self.request_stop(out),
            "undo" => self.undo(out),
            "redo" => self.redo(out),
            "close_tab" => self.close_tab(tab, out),
            "select_tab" => {
                if let Some(tab) = tab {
                    self.activate(tab);
                    out.extend(self.declared());
                }
            }
            _ => {}
        }
    }

    fn push_book(&mut self, path: Option<PathBuf>) -> usize {
        let id = self.next.to_string();
        self.next += 1;
        let label = path
            .as_ref()
            .map(|path| file_title(path))
            .unwrap_or_else(|| String::from("Untitled Mission Book"));
        self.books.push(Book {
            id,
            label,
            path,
            cells: Vec::new(),
            selected: None,
            undo: Vec::new(),
            redo: Vec::new(),
            dirty: false,
        });
        self.active = self.books.len() - 1;
        self.active
    }

    fn book_mut(&mut self, tab: &str) -> Option<&mut Book> {
        self.books.iter_mut().find(|book| book.id == tab)
    }

    fn activate(&mut self, tab: &str) {
        if let Some(index) = self.books.iter().position(|book| book.id == tab) {
            self.active = index;
        }
    }

    fn add_cell(&mut self, out: &mut Vec<AppMessage>) {
        let Some(book) = self.books.get_mut(self.active) else {
            return;
        };
        book.snapshot();
        let cell = Cell::text(protocol::cell_id(), "");
        book.selected = Some(cell.id.clone());
        book.cells.push(cell);
        book.dirty = true;
        self.emit_cells(out);
    }

    fn delete_cell(&mut self, out: &mut Vec<AppMessage>) {
        let Some(book) = self.books.get_mut(self.active) else {
            return;
        };
        let Some(selected) = book.selected.clone() else {
            return;
        };
        let Some(index) = book.cells.iter().position(|cell| cell.id == selected) else {
            return;
        };
        book.snapshot();
        book.cells.remove(index);
        book.selected = book.cells.get(index).or_else(|| book.cells.last()).map(|cell| cell.id.clone());
        book.dirty = true;
        self.emit_cells(out);
    }

    fn edit(&mut self, tab: &str, id: &str, input: Vec<CellPart>, out: &mut Vec<AppMessage>) {
        let Some(book) = self.book_mut(tab) else {
            return;
        };
        let same = book.cells.iter().any(|cell| cell.id == id && cell.input == input);
        let known = book.cells.iter().any(|cell| cell.id == id);
        if !known || same {
            return;
        }
        book.snapshot();
        if let Some(cell) = book.cells.iter_mut().find(|cell| cell.id == id) {
            cell.input = input;
        }
        book.dirty = true;
        let tab = tab.to_string();
        self.emit_tab(&tab, out);
    }

    fn finish(
        &mut self,
        tab: &str,
        id: &str,
        endpoint: String,
        model: String,
        state: CellState,
        output: Vec<CellPart>,
        error: Option<String>,
        out: &mut Vec<AppMessage>,
    ) {
        let Some(book) = self.book_mut(tab) else {
            return;
        };
        if !book.cells.iter().any(|cell| cell.id == id) {
            return;
        }
        book.snapshot();
        if let Some(cell) = book.cells.iter_mut().find(|cell| cell.id == id) {
            cell.endpoint = Some(endpoint);
            cell.model = Some(model);
            cell.state = state;
            cell.output = output;
            cell.error = error;
        }
        book.dirty = true;
        let tab = tab.to_string();
        self.emit_tab(&tab, out);
    }

    fn insert(&mut self, tab: &str, index: usize, cell: Cell, out: &mut Vec<AppMessage>) {
        let Some(book) = self.book_mut(tab) else {
            return;
        };
        if cell.kind != KIND && cell.kind.is_empty() {
            return;
        }
        book.snapshot();
        let index = index.min(book.cells.len());
        book.selected = Some(cell.id.clone());
        book.cells.insert(index, cell);
        book.dirty = true;
        let tab = tab.to_string();
        self.emit_tab(&tab, out);
    }

    fn select(&mut self, tab: &str, id: &str, out: &mut Vec<AppMessage>) {
        let Some(book) = self.book_mut(tab) else {
            return;
        };
        if !book.cells.iter().any(|cell| cell.id == id) {
            return;
        }
        book.selected = Some(id.to_string());
        let tab = tab.to_string();
        self.emit_tab(&tab, out);
    }

    fn request_run(&mut self, out: &mut Vec<AppMessage>) {
        let Some(book) = self.books.get(self.active) else {
            return;
        };
        let Some(id) = book.selected.clone() else {
            return;
        };
        let Some(cell) = book.cells.iter().find(|cell| cell.id == id) else {
            return;
        };
        if cell.kind != KIND || cell.prompt().trim().is_empty() {
            return;
        }
        out.push(AppMessage::RunCell {
            tab: book.id.clone(),
            cell: id,
        });
    }

    fn request_stop(&mut self, out: &mut Vec<AppMessage>) {
        let Some(book) = self.books.get(self.active) else {
            return;
        };
        let Some(id) = book.selected.clone() else {
            return;
        };
        out.push(AppMessage::StopCell {
            tab: book.id.clone(),
            cell: id,
        });
    }

    fn undo(&mut self, out: &mut Vec<AppMessage>) {
        let Some(book) = self.books.get_mut(self.active) else {
            return;
        };
        let Some(snap) = book.undo.pop() else {
            return;
        };
        book.redo.push(Snap {
            cells: book.cells.clone(),
            selected: book.selected.clone(),
        });
        book.cells = snap.cells;
        book.selected = snap.selected;
        book.dirty = true;
        self.emit_cells(out);
    }

    fn redo(&mut self, out: &mut Vec<AppMessage>) {
        let Some(book) = self.books.get_mut(self.active) else {
            return;
        };
        let Some(snap) = book.redo.pop() else {
            return;
        };
        book.undo.push(Snap {
            cells: book.cells.clone(),
            selected: book.selected.clone(),
        });
        book.cells = snap.cells;
        book.selected = snap.selected;
        book.dirty = true;
        self.emit_cells(out);
    }

    fn close_tab(&mut self, tab: Option<&str>, out: &mut Vec<AppMessage>) {
        let Some(tab) = tab.or_else(|| self.books.get(self.active).map(|book| book.id.as_str())) else {
            return;
        };
        let Some(index) = self.books.iter().position(|book| book.id == tab) else {
            return;
        };
        if self.books[index].dirty {
            self.ask = Some(keep_ask());
            self.saving = None;
            self.quitting = false;
            self.closing = true;
            self.quit_rest.clear();
            self.active = index;
            out.push(AppMessage::Ask(keep_ask()));
            return;
        }
        self.remove_book(index);
        out.extend(self.declared());
    }

    fn remove_book(&mut self, index: usize) {
        self.books.remove(index);
        if self.books.is_empty() {
            self.push_book(None);
        } else if self.active >= self.books.len() {
            self.active = self.books.len() - 1;
        }
    }

    fn save_active(&mut self, copy: bool, out: &mut Vec<AppMessage>) {
        let Some(book) = self.books.get(self.active) else {
            return;
        };
        if !copy && let Some(path) = book.path.clone() {
            self.write_at(self.active, &path, false, out);
            return;
        }
        let name = book
            .path
            .as_ref()
            .map(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
            .unwrap_or_else(|| Some(String::from("Untitled Mission Book.smx")));
        self.ask_file(FileKind::Save, name, false, out);
    }

    fn ask_file(&mut self, kind: FileKind, name: Option<String>, then_quit: bool, out: &mut Vec<AppMessage>) {
        let id = format!("file-{}", self.next_file);
        self.next_file += 1;
        if kind == FileKind::Save {
            self.saving = Some(Saving {
                book: self.active,
                then_quit,
            });
        }
        self.file_id = Some(id.clone());
        out.push(AppMessage::File {
            id,
            kind,
            name,
            folder: None,
            filter: Some(FileFilter {
                name: "Mission book".into(),
                extensions: vec!["smx".into()],
            }),
        });
    }

    fn file_result(&mut self, id: &str, path: Option<String>, out: &mut Vec<AppMessage>) {
        if self.file_id.as_deref() != Some(id) {
            return;
        }
        self.file_id = None;
        let Some(path) = path else {
            if self.saving.take().is_some_and(|saving| saving.then_quit) || self.quitting {
                self.cancel_quit(out);
            }
            self.saving = None;
            return;
        };
        let path = PathBuf::from(path);
        if let Some(saving) = self.saving.take() {
            self.write_at(saving.book, &path, saving.then_quit, out);
            return;
        }
        // An Open.
        match read_book(&path) {
            Ok(cells) => {
                let index = self.push_book(Some(path));
                self.books[index].cells = cells;
                if let Some(first) = self.books[index].cells.first() {
                    self.books[index].selected = Some(first.id.clone());
                }
                out.push(AppMessage::Contents {
                    tab: self.books[index].id.clone(),
                    kind: ContentsKind::Cells,
                });
                self.emit_cells(out);
            }
            Err(reason) => out.push(AppMessage::Ask(Ask {
                id: "book-unreadable".into(),
                title: "This book could not be opened".into(),
                lines: vec![reason],
                buttons: vec![AskButton {
                    id: "ok".into(),
                    label: "OK".into(),
                }],
                dismiss: Some("ok".into()),
            })),
        }
    }

    fn write_at(&mut self, index: usize, path: &Path, then_quit: bool, out: &mut Vec<AppMessage>) {
        let Some(book) = self.books.get(index) else {
            return;
        };
        if let Err(reason) = write_book(path, &book.cells) {
            out.push(AppMessage::Ask(Ask {
                id: "book-unwritten".into(),
                title: "This book could not be saved".into(),
                lines: vec![reason],
                buttons: vec![AskButton {
                    id: "ok".into(),
                    label: "OK".into(),
                }],
                dismiss: Some("ok".into()),
            }));
            return;
        }
        if let Some(book) = self.books.get_mut(index) {
            book.path = Some(path.to_path_buf());
            book.label = file_title(path);
            book.dirty = false;
        }
        out.extend(self.declared());
        if self.closing {
            self.closing = false;
            self.remove_book(index);
            out.extend(self.declared());
        } else if then_quit {
            self.advance_quit(out);
        }
    }

    fn note_quit(&mut self, out: &mut Vec<AppMessage>) {
        if self.quitting {
            return;
        }
        let dirty: Vec<usize> = self
            .books
            .iter()
            .enumerate()
            .filter(|(_, book)| book.dirty)
            .map(|(index, _)| index)
            .collect();
        if dirty.is_empty() {
            out.push(AppMessage::QuitReady);
            return;
        }
        self.quitting = true;
        self.active = dirty[0];
        self.quit_rest = dirty.into_iter().skip(1).collect();
        self.ask = Some(keep_ask());
        out.push(AppMessage::Ask(keep_ask()));
    }

    fn answer(&mut self, button: &str, out: &mut Vec<AppMessage>) {
        match button {
            "save" => self.save_active_for_quit(out),
            "discard" => {
                if self.closing {
                    self.closing = false;
                    self.remove_book(self.active);
                    out.extend(self.declared());
                } else if self.quitting {
                    self.advance_quit(out);
                }
            }
            _ => self.cancel_quit(out),
        }
    }

    fn save_active_for_quit(&mut self, out: &mut Vec<AppMessage>) {
        let Some(book) = self.books.get(self.active) else {
            return;
        };
        if let Some(path) = book.path.clone() {
            self.write_at(self.active, &path, self.quitting, out);
            return;
        }
        let name = Some(String::from("Untitled Mission Book.smx"));
        self.ask_file(FileKind::Save, name, self.quitting, out);
    }

    fn advance_quit(&mut self, out: &mut Vec<AppMessage>) {
        if !self.quitting {
            if self.books.get(self.active).is_some_and(|book| book.dirty) {
                self.remove_book(self.active);
                out.extend(self.declared());
            }
            return;
        }
        let Some(next) = self.quit_rest.first().copied() else {
            self.quitting = false;
            out.push(AppMessage::QuitReady);
            return;
        };
        self.quit_rest.remove(0);
        self.active = next;
        self.ask = Some(keep_ask());
        out.push(AppMessage::Ask(keep_ask()));
    }

    fn cancel_quit(&mut self, out: &mut Vec<AppMessage>) {
        self.quitting = false;
        self.closing = false;
        self.quit_rest.clear();
        self.ask = None;
        out.extend(self.declared());
    }

    fn open_path(&mut self, path: Option<String>, out: &mut Vec<AppMessage>) {
        let Some(path) = path else {
            self.push_book(None);
            out.push(AppMessage::Contents {
                tab: self.books[self.active].id.clone(),
                kind: ContentsKind::Cells,
            });
            out.extend(self.declared());
            return;
        };
        let path = PathBuf::from(path);
        match read_book(&path) {
            Ok(cells) => {
                let index = self.push_book(Some(path));
                self.books[index].cells = cells;
                self.books[index].selected = self.books[index].cells.first().map(|cell| cell.id.clone());
                out.push(AppMessage::Contents {
                    tab: self.books[index].id.clone(),
                    kind: ContentsKind::Cells,
                });
                self.emit_cells(out);
            }
            Err(reason) => out.push(AppMessage::Ask(Ask {
                id: "book-unreadable".into(),
                title: "This book could not be opened".into(),
                lines: vec![reason],
                buttons: vec![AskButton {
                    id: "ok".into(),
                    label: "OK".into(),
                }],
                dismiss: Some("ok".into()),
            })),
        }
    }

    /// Mission Control declares no tools, so any call is refused.
    fn invoke(&mut self, call: String) -> Vec<AppMessage> {
        vec![AppMessage::ToolResult(ToolResult {
            call,
            ok: false,
            output: serde_json::json!({ "error": "Mission Control has no tools yet." }),
            images: Vec::new(),
        })]
    }

    fn emit_cells(&mut self, out: &mut Vec<AppMessage>) {
        if let Some(book) = self.books.get(self.active) {
            let tab = book.id.clone();
            self.emit_tab(&tab, out);
        }
    }

    fn emit_tab(&self, tab: &str, out: &mut Vec<AppMessage>) {
        let Some(book) = self.books.iter().find(|book| book.id == tab) else {
            return;
        };
        out.push(AppMessage::Cells {
            tab: book.id.clone(),
            cells: book.cells.clone(),
            selected: book.selected.clone(),
        });
        out.extend(self.declared());
    }

    fn declared(&self) -> Vec<AppMessage> {
        let mut messages = vec![
            AppMessage::Toolbar(toolbar()),
            AppMessage::Menu { rows: menu() },
            AppMessage::Keys(keys()),
        ];
        if let Some(book) = self.books.get(self.active) {
            messages.push(AppMessage::Tabs(Tabs {
                tabs: self
                    .books
                    .iter()
                    .map(|book| TabEntry {
                        id: book.id.clone(),
                        label: book.label.clone(),
                        dirty: book.dirty,
                        close_waits: false,
                        path: book.path.as_ref().map(|path| path.display().to_string()),
                        saves: true,
                    })
                    .collect(),
                active: book.id.clone(),
                new_tab: Some(Action::new("new_book")),
                new_tab_tip: "New book".into(),
            }));
        }
        messages
    }
}

impl Book {
    fn snapshot(&mut self) {
        self.undo.push(Snap {
            cells: self.cells.clone(),
            selected: self.selected.clone(),
        });
        self.redo.clear();
    }
}

fn keep_ask() -> Ask {
    Ask {
        id: "keep-book".into(),
        title: "Keep this book?".into(),
        lines: vec!["The book has changes that are not saved.".into()],
        buttons: vec![
            AskButton {
                id: "save".into(),
                label: "Save".into(),
            },
            AskButton {
                id: "discard".into(),
                label: "Discard".into(),
            },
            AskButton {
                id: "cancel".into(),
                label: "Cancel".into(),
            },
        ],
        dismiss: Some("cancel".into()),
    }
}

fn toolbar() -> Toolbar {
    Toolbar {
        items: vec![
            ToolItem::Button {
                icon: "plus".into(),
                tip: "Add cell".into(),
                enabled: true,
                selected: false,
                action: Some(Action::new("add_cell")),
                menu: Vec::new(),
            },
            ToolItem::Button {
                icon: "trash".into(),
                tip: "Delete cell".into(),
                enabled: true,
                selected: false,
                action: Some(Action::new("delete_cell")),
                menu: Vec::new(),
            },
            ToolItem::Button {
                icon: "play".into(),
                tip: "Run".into(),
                enabled: true,
                selected: false,
                action: Some(Action::new("run")),
                menu: Vec::new(),
            },
            ToolItem::Button {
                icon: "square".into(),
                tip: "Stop".into(),
                enabled: true,
                selected: false,
                action: Some(Action::new("stop")),
                menu: Vec::new(),
            },
        ],
        trailing: Vec::new(),
    }
}

fn menu() -> Vec<MenuEntry> {
    vec![
        row("New book", "new_book"),
        row("Open", "open"),
        row("Save", "save"),
        row("Save As", "save_as"),
    ]
}

fn row(label: &str, id: &str) -> MenuEntry {
    MenuEntry {
        label: label.into(),
        icon: None,
        enabled: true,
        divider: false,
        action: Some(Action::new(id)),
        submenu: Vec::new(),
        rename: false,
    }
}

fn keys() -> Keys {
    Keys {
        bindings: vec![
            protocol::Binding {
                chord: Chord {
                    key: "Z".into(),
                    command: true,
                    shift: Some(false),
                    alt: false,
                },
                action: Action::new("undo"),
            },
            protocol::Binding {
                chord: Chord {
                    key: "Z".into(),
                    command: true,
                    shift: Some(true),
                    alt: false,
                },
                action: Action::new("redo"),
            },
        ],
        copy: None,
        cut: None,
        paste: None,
    }
}

fn file_title(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| String::from("Untitled Mission Book"))
}

/// True when the file is a mission book this application can open.
pub fn claims(path: &Path) -> bool {
    read_book(path).is_ok()
}

pub fn read_book(path: &Path) -> Result<Vec<Cell>, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let file: BookFile = serde_json::from_str(&text).map_err(|_| {
        String::from("It is not a mission book, or it is damaged.")
    })?;
    if file.version != VERSION {
        return Err(format!(
            "It is version {} of a mission book, and this Mission Control reads version {VERSION}.",
            file.version
        ));
    }
    Ok(file.cells)
}

pub fn write_book(path: &Path, cells: &[Cell]) -> Result<(), String> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    }
    let file = BookFile {
        version: VERSION,
        cells: cells.to_vec(),
    };
    let text = serde_json::to_string_pretty(&file).map_err(|error| error.to_string())?;
    let mut partial = path.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    std::fs::write(&partial, text).map_err(|error| error.to_string())?;
    std::fs::rename(&partial, path).map_err(|error| {
        let _ = std::fs::remove_file(&partial);
        error.to_string()
    })
}

/// One line of the protocol, and the lines to write back.
pub fn respond(session: &mut Session, line: &str) -> Vec<String> {
    let Ok(message) = protocol::parse_host_message(line) else {
        return Vec::new();
    };
    if matches!(message, HostMessage::Shutdown) {
        return Vec::new();
    }
    session
        .handle(message)
        .iter()
        .map(protocol::encode_app_message)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_of(messages: &[AppMessage]) -> Vec<String> {
        messages.iter().map(protocol::encode_app_message).collect()
    }

    fn cells_of(message: &AppMessage) -> Option<&[Cell]> {
        match message {
            AppMessage::Cells { cells, .. } => Some(cells),
            _ => None,
        }
    }

    /// No host: create a book, add a cell, set its prompt, deliver a result,
    /// undo, save, and reopen. Every reply is the protocol's, and the file
    /// holds the cells.
    #[test]
    fn a_book_is_kept_with_no_host() {
        let dir = std::env::temp_dir().join(format!("inpainter-book-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keeper.smx");
        let mut session = Session::launch();
        let greeting = session.greeting();
        assert!(greeting.iter().any(|message| matches!(message, AppMessage::Contents { kind: ContentsKind::Cells, .. })));
        assert!(greeting.iter().any(|message| matches!(message, AppMessage::End)));

        let added = session.handle(HostMessage::Command {
            id: "add_cell".into(),
            tab: Some("1".into()),
            arg: None,
            node: None,
            viewport: [800.0, 600.0],
        });
        let cell = cells_of(added.iter().find(|message| cells_of(message).is_some()).unwrap())
            .unwrap()[0]
            .clone();
        assert_eq!(cell.kind, "text");
        assert_eq!(cell.state, CellState::Idle);
        assert!(cell.prompt().is_empty());

        let edited = session.handle(HostMessage::EditCell {
            tab: "1".into(),
            cell: cell.id.clone(),
            input: vec![CellPart::Text {
                text: "A lighthouse keeper.".into(),
            }],
        });
        let edited_cell = cells_of(edited.iter().find(|message| cells_of(message).is_some()).unwrap()).unwrap()[0].clone();
        assert_eq!(edited_cell.prompt(), "A lighthouse keeper.");

        let finished = session.handle(HostMessage::CellResult {
            tab: "1".into(),
            cell: cell.id.clone(),
            endpoint: "openrouter".into(),
            model: "stand-in/model".into(),
            state: CellState::Done,
            output: vec![CellPart::Text {
                text: "He kept the light.".into(),
            }],
            error: None,
        });
        let done = cells_of(finished.iter().find(|message| cells_of(message).is_some()).unwrap()).unwrap()[0].clone();
        assert_eq!(done.result_text(), "He kept the light.");
        assert_eq!(done.state, CellState::Done);

        let undone = session.handle(HostMessage::Command {
            id: "undo".into(),
            tab: Some("1".into()),
            arg: None,
            node: None,
            viewport: [800.0, 600.0],
        });
        let back = cells_of(undone.iter().find(|message| cells_of(message).is_some()).unwrap()).unwrap()[0].clone();
        assert_eq!(back.state, CellState::Idle);
        assert!(back.output.is_empty());
        assert_eq!(back.prompt(), "A lighthouse keeper.");

        let redone = session.handle(HostMessage::Command {
            id: "redo".into(),
            tab: Some("1".into()),
            arg: None,
            node: None,
            viewport: [800.0, 600.0],
        });
        let again = cells_of(redone.iter().find(|message| cells_of(message).is_some()).unwrap()).unwrap()[0].clone();
        assert_eq!(again.result_text(), "He kept the light.");

        let saved = session.handle(HostMessage::Command {
            id: "save".into(),
            tab: Some("1".into()),
            arg: None,
            node: None,
            viewport: [800.0, 600.0],
        });
        let AppMessage::File { id, kind: FileKind::Save, folder: None, .. } = &saved[0] else {
            panic!("save asked for a file, got {}", lines_of(&saved).join(""));
        };
        let wrote = session.handle(HostMessage::FileResult {
            id: id.clone(),
            path: Some(path.display().to_string()),
        });
        assert!(wrote.iter().any(|message| matches!(
            message,
            AppMessage::Tabs(tabs) if tabs.tabs[0].path.as_deref() == Some(path.display().to_string().as_str())
        )));
        let file = read_book(&path).unwrap();
        assert_eq!(file[0].result_text(), "He kept the light.");
        assert!(!path.with_extension("smx.partial").exists());

        let mut again = Session::restored();
        let opened = again.handle(HostMessage::Open {
            path: Some(path.display().to_string()),
        });
        let reopened = cells_of(opened.iter().find(|message| cells_of(message).is_some()).unwrap()).unwrap();
        assert_eq!(reopened[0].id, file[0].id);
        assert_eq!(reopened[0].prompt(), "A lighthouse keeper.");
        assert_eq!(reopened[0].result_text(), "He kept the light.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sketch_file_is_not_a_book() {
        let dir = std::env::temp_dir().join(format!("inpainter-book-claim-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("note.smx");
        std::fs::write(&path, r#"{"format_version":1,"nodes":[]}"#).unwrap();
        assert!(!claims(&path));
        std::fs::write(&path, r#"{"version":1,"cells":[]}"#).unwrap();
        assert!(claims(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
