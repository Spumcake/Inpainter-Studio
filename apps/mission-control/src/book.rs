//! A notebook: the cells, which one is active, every cell's runs, the file,
//! and the undo.
//!
//! The host draws the cells and runs them. This process only keeps them.
//! A run is kept for good: the runs live apart from the undo history, so no
//! undo or redo ever adds or removes one, as the owner set on 2026-10-28.
//!
//! Inside a project, which the host names with `project`, every notebook is
//! written into the project's folder by itself, a second after it changes,
//! and before its tab closes, a `keep` is answered, or the host quits. Save
//! and Save As are not offered there, and nothing asks to save.

use protocol::{
    Action, AppMessage, Ask, AskButton, Cell, CellPart, CellState, CellVersion, Chord,
    ContentsKind, FileFilter, FileKind, HostMessage, Keys, MARKDOWN_KIND, MenuEntry, TEXT_KIND,
    TabEntry, Tabs, ToolItem, ToolResult, Toolbar,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a change waits before it is written, so a burst of edits is
/// written once.
const WRITE_AFTER: Duration = Duration::from_secs(1);
/// How long a failed write waits before it is tried again by itself.
const RETRY_AFTER: Duration = Duration::from_secs(5);

const VERSION: u32 = 2;
const UNTITLED: &str = "Untitled Notebook";

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
    /// Every run of every cell, by cell id, including cells since deleted,
    /// so undoing a delete brings a cell back with all its runs.
    runs: HashMap<String, Vec<CellVersion>>,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    dirty: bool,
    /// Inside a project: when its change was first seen unwritten, and its
    /// last failed write, why and when.
    unwritten_since: Option<Instant>,
    write_failed: Option<(String, Instant)>,
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
    /// The kind + adds, which the toolbar's dropdown shows.
    adding: String,
    /// The cell cut or copied last, with its runs.
    clipboard: Option<Cell>,
    /// The project's folder, when the host runs this inside a project.
    project: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
struct BookFile {
    version: u32,
    cells: Vec<Cell>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected: Option<String>,
}

/// A notebook as read from its file: its cells and its active cell.
pub struct Read {
    pub cells: Vec<Cell>,
    pub selected: Option<String>,
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
            adding: String::from(TEXT_KIND),
            clipboard: None,
            project: None,
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
        if let Some(book) = self.books.first() {
            // A new notebook's first cell, so the host draws it at once.
            messages.push(AppMessage::Cells {
                tab: book.id.clone(),
                cells: book.cells.clone(),
                selected: book.selected.clone(),
            });
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
            HostMessage::Quit => {
                if self.project.is_some() {
                    // Everything is written first, so the quit asks nothing.
                    for index in 0..self.books.len() {
                        let _ = self.write_into_project(index);
                    }
                }
                self.note_quit(&mut out)
            }
            HostMessage::Project { folder } => {
                let folder = PathBuf::from(folder);
                self.project = Some(folder.canonicalize().unwrap_or(folder));
                out.extend(self.declared());
            }
            // A path outside the project opens nothing.
            HostMessage::Open { path } if path.as_deref().is_some_and(|path| !self.in_project(Path::new(path))) => {}
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
                input,
                endpoint,
                model,
                state,
                output,
                error,
            } => self.finish(&tab, &cell, Run { input, endpoint, model, state, output, error }, &mut out),
            HostMessage::InsertCell { tab, index, cell } => self.insert(&tab, index, cell, &mut out),
            HostMessage::SelectCell { tab, cell } => self.select(&tab, &cell, &mut out),
            HostMessage::SetVersion { tab, cell, index } => self.set_version(&tab, &cell, index, &mut out),
            HostMessage::MoveCell { tab, cell, to } => self.move_cell(&tab, &cell, to, &mut out),
            HostMessage::DeleteCell { tab, cell } => self.delete(&tab, &cell, &mut out),
            HostMessage::ShowSource { tab, cell } => self.show_source(&tab, &cell, &mut out),
            HostMessage::Claim { id, path } => out.push(AppMessage::Claimed {
                id,
                yours: claims(Path::new(&path)) && self.in_project(Path::new(&path)),
            }),
            HostMessage::Invoke { call, .. } => out.extend(self.invoke(call)),
            HostMessage::Keep { tab } => {
                let kept = match self.books.iter().position(|book| book.id == tab) {
                    Some(index) if self.project.is_some() => self.write_into_project(index),
                    Some(index) => Ok(self.books[index].path.clone()),
                    None => Ok(None),
                };
                out.extend(self.declared());
                out.push(match kept {
                    Ok(path) => AppMessage::Kept {
                        tab,
                        path: path.map(|path| path.display().to_string()),
                        error: None,
                    },
                    Err(error) => AppMessage::Kept { tab, path: None, error: Some(error) },
                });
            }
            _ => {}
        }
        if self.project.is_some() && self.write_due(Instant::now()) {
            out.extend(self.declared());
        }
        out
    }

    /// The loop's wake with no message: writes what is due inside a project,
    /// and sends what that changed, such as a tab's new name.
    pub fn tick(&mut self) -> Vec<AppMessage> {
        if self.project.is_some() && self.write_due(Instant::now()) {
            return self.declared();
        }
        Vec::new()
    }

    /// Inside a project, writes every notebook not yet written, as when the
    /// host has gone.
    pub fn write_all(&mut self) {
        if self.project.is_some() {
            for index in 0..self.books.len() {
                let _ = self.write_into_project(index);
            }
        }
    }

    /// Whether `path` is inside the project's folder. Always true outside a
    /// project.
    fn in_project(&self, path: &Path) -> bool {
        let Some(project) = &self.project else {
            return true;
        };
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        path.starts_with(project)
    }

    /// Writes every notebook whose change is a second old, and tries a failed
    /// one again after a while. Returns whether any was written.
    fn write_due(&mut self, now: Instant) -> bool {
        let mut wrote = false;
        for index in 0..self.books.len() {
            let book = &mut self.books[index];
            if !book.dirty {
                book.unwritten_since = None;
                continue;
            }
            let since = *book.unwritten_since.get_or_insert(now);
            if now.duration_since(since) < WRITE_AFTER {
                continue;
            }
            if book.write_failed.as_ref().is_some_and(|(_, failed)| now.duration_since(*failed) < RETRY_AFTER) {
                continue;
            }
            wrote |= self.write_into_project(index).is_ok();
        }
        wrote
    }

    /// Writes one notebook into the project's folder: to its own file, or,
    /// for one never written, to a new file named after it that replaces
    /// nothing, such as `Untitled Notebook.smx`, then `Untitled Notebook
    /// 2.smx`. Returns the file, or none for an untouched notebook.
    fn write_into_project(&mut self, index: usize) -> Result<Option<PathBuf>, String> {
        let Some(folder) = self.project.clone() else {
            return Err("No project folder.".into());
        };
        let Some(book) = self.books.get(index) else {
            return Ok(None);
        };
        if !book.dirty {
            return Ok(book.path.clone());
        }
        let path = match &book.path {
            Some(path) => path.clone(),
            None => match free_names(&book.label)
                .map(|name| folder.join(format!("{name}.smx")))
                .find(|path| !path.exists())
            {
                Some(path) => path,
                None => return Err("No free file name.".into()),
            },
        };
        let book = &mut self.books[index];
        match write_book(&path, &book.cells, book.selected.as_deref()) {
            Ok(()) => {
                book.label = file_title(&path);
                book.path = Some(path.clone());
                book.dirty = false;
                book.unwritten_since = None;
                book.write_failed = None;
                Ok(Some(path))
            }
            Err(reason) => {
                book.write_failed = Some((reason.clone(), Instant::now()));
                Err(reason)
            }
        }
    }

    fn command(&mut self, id: &str, tab: Option<&str>, out: &mut Vec<AppMessage>) {
        if let Some(tab) = tab {
            self.activate(tab);
        }
        match id {
            "new_book" => {
                self.push_book(None);
                out.push(AppMessage::Contents {
                    tab: self.books[self.active].id.clone(),
                    kind: ContentsKind::Cells,
                });
                // Its first cell, so the host draws it at once.
                self.emit_cells(out);
            }
            "add_above" => self.add_near(0, out),
            "add_below" => self.add_near(1, out),
            "kind_text" => {
                self.adding = String::from(TEXT_KIND);
                out.extend(self.declared());
            }
            "kind_markdown" => {
                self.adding = String::from(MARKDOWN_KIND);
                out.extend(self.declared());
            }
            "cut" => {
                self.copy_active();
                self.delete_cell(out);
            }
            "copy" => self.copy_active(),
            "paste" => self.paste(out),
            "open" => self.ask_file(FileKind::Open, None, false, out),
            "save" if self.project.is_some() => {
                let _ = self.write_into_project(self.active);
                out.extend(self.declared());
            }
            "save_as" if self.project.is_some() => {}
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
            .unwrap_or_else(|| String::from(UNTITLED));
        // A new notebook opens with one empty cell, which is active.
        let first = self.empty_cell();
        self.books.push(Book {
            id,
            label,
            path,
            selected: Some(first.id.clone()),
            cells: vec![first],
            runs: HashMap::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            dirty: false,
            unwritten_since: None,
            write_failed: None,
        });
        self.active = self.books.len() - 1;
        self.active
    }

    /// An empty cell of the kind + adds.
    fn empty_cell(&self) -> Cell {
        let mut cell = Cell::text(protocol::cell_id(), "");
        cell.kind = self.adding.clone();
        cell
    }

    fn book_mut(&mut self, tab: &str) -> Option<&mut Book> {
        self.books.iter_mut().find(|book| book.id == tab)
    }

    fn activate(&mut self, tab: &str) {
        if let Some(index) = self.books.iter().position(|book| book.id == tab) {
            self.active = index;
        }
    }

    /// + adds an empty cell of the dropdown's kind below the active cell.
    fn add_cell(&mut self, out: &mut Vec<AppMessage>) {
        self.add_near(1, out);
    }

    /// An empty cell above (`after` 0) or below (`after` 1) the active cell,
    /// which becomes active.
    fn add_near(&mut self, after: usize, out: &mut Vec<AppMessage>) {
        let cell = self.empty_cell();
        let Some(book) = self.books.get_mut(self.active) else {
            return;
        };
        book.snapshot();
        let at = book.active_index().map_or(book.cells.len(), |index| index + after);
        book.selected = Some(cell.id.clone());
        book.cells.insert(at.min(book.cells.len()), cell);
        book.dirty = true;
        self.emit_cells(out);
    }

    fn copy_active(&mut self) {
        let Some(book) = self.books.get(self.active) else {
            return;
        };
        if let Some(index) = book.active_index() {
            self.clipboard = Some(book.cells[index].clone());
        }
    }

    /// A copy of the clipboard's cell below the active cell, with a new id,
    /// `from` naming the cell it came from, and its runs under new ids.
    fn paste(&mut self, out: &mut Vec<AppMessage>) {
        let Some(source) = self.clipboard.clone() else {
            return;
        };
        let Some(book) = self.books.get_mut(self.active) else {
            return;
        };
        let mut cell = source.clone();
        cell.id = protocol::cell_id();
        cell.from = Some(source.id.clone());
        for version in &mut cell.versions {
            version.id = protocol::cell_id();
        }
        book.snapshot();
        book.runs.insert(cell.id.clone(), cell.versions.clone());
        let at = book.active_index().map_or(book.cells.len(), |index| index + 1);
        book.selected = Some(cell.id.clone());
        book.cells.insert(at.min(book.cells.len()), cell);
        book.dirty = true;
        self.emit_cells(out);
    }

    fn set_version(&mut self, tab: &str, id: &str, index: usize, out: &mut Vec<AppMessage>) {
        let Some(book) = self.book_mut(tab) else {
            return;
        };
        let Some(position) = book.cells.iter().position(|cell| cell.id == id) else {
            return;
        };
        if book.cells[position].current == Some(index) && book.cells[position].input == book.cells[position].versions.get(index).map(|v| v.input.clone()).unwrap_or_default() {
            return;
        }
        if index >= book.cells[position].versions.len() {
            return;
        }
        book.snapshot();
        book.cells[position].show_version(index);
        book.selected = Some(id.to_string());
        book.dirty = true;
        let tab = tab.to_string();
        self.emit_tab(&tab, out);
    }

    fn move_cell(&mut self, tab: &str, id: &str, to: usize, out: &mut Vec<AppMessage>) {
        let Some(book) = self.book_mut(tab) else {
            return;
        };
        let Some(from) = book.cells.iter().position(|cell| cell.id == id) else {
            return;
        };
        let to = to.min(book.cells.len() - 1);
        if from == to {
            return;
        }
        book.snapshot();
        let cell = book.cells.remove(from);
        book.cells.insert(to, cell);
        book.selected = Some(id.to_string());
        book.dirty = true;
        let tab = tab.to_string();
        self.emit_tab(&tab, out);
    }

    fn delete(&mut self, tab: &str, id: &str, out: &mut Vec<AppMessage>) {
        if let Some(index) = self.books.iter().position(|book| book.id == tab) {
            self.active = index;
        }
        let Some(book) = self.books.get_mut(self.active) else {
            return;
        };
        if !book.cells.iter().any(|cell| cell.id == id) {
            return;
        }
        book.selected = Some(id.to_string());
        self.delete_cell(out);
    }

    /// A rendered Markdown cell shows its source again. Not an undo step.
    fn show_source(&mut self, tab: &str, id: &str, out: &mut Vec<AppMessage>) {
        let Some(book) = self.book_mut(tab) else {
            return;
        };
        let Some(cell) = book.cells.iter_mut().find(|cell| cell.id == id && cell.kind == MARKDOWN_KIND) else {
            return;
        };
        if cell.state == CellState::Idle {
            return;
        }
        cell.state = CellState::Idle;
        let tab = tab.to_string();
        self.emit_tab(&tab, out);
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
        book.dirty = true;
        // There is always one active cell: the last one deleted leaves an
        // empty one.
        if book.cells.is_empty() {
            let mut cell = Cell::text(protocol::cell_id(), "");
            cell.kind = self.adding.clone();
            book.cells.push(cell);
        }
        book.selected = book.cells.get(index).or_else(|| book.cells.last()).map(|cell| cell.id.clone());
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

    /// A run ended. A Markdown cell shows rendered. Any other cell keeps the
    /// run for good as a new version, made from the prompt that actually ran,
    /// and shows it. A run is never an undo step.
    fn finish(&mut self, tab: &str, id: &str, run: Run, out: &mut Vec<AppMessage>) {
        let Some(book) = self.book_mut(tab) else {
            return;
        };
        let Some(cell) = book.cells.iter_mut().find(|cell| cell.id == id) else {
            // The cell was deleted while it ran. The run is still kept, so
            // undoing the delete brings it back with this run.
            if run.endpoint.is_empty() {
                return;
            }
            book.runs.entry(id.to_string()).or_default().push(CellVersion {
                id: protocol::cell_id(),
                input: run.input,
                endpoint: Some(run.endpoint),
                model: Some(run.model),
                output: run.output,
                state: run.state,
                error: run.error,
                time: now(),
            });
            book.dirty = true;
            return;
        };
        if cell.kind == MARKDOWN_KIND {
            cell.state = CellState::Done;
        } else {
            let runs = book.runs.entry(id.to_string()).or_default();
            runs.push(CellVersion {
                id: protocol::cell_id(),
                input: run.input,
                endpoint: Some(run.endpoint),
                model: Some(run.model),
                output: run.output,
                state: run.state,
                error: run.error,
                time: now(),
            });
            cell.versions = runs.clone();
            cell.current = Some(cell.versions.len() - 1);
            cell.agree();
            book.dirty = true;
        }
        let tab = tab.to_string();
        self.emit_tab(&tab, out);
    }

    /// A dragged cell, or, with none, an empty one of the dropdown's kind,
    /// as Shift+Enter at the last cell asks for.
    fn insert(&mut self, tab: &str, index: usize, cell: Option<Cell>, out: &mut Vec<AppMessage>) {
        let cell = cell.unwrap_or_else(|| self.empty_cell());
        let Some(book) = self.book_mut(tab) else {
            return;
        };
        if cell.kind.is_empty() {
            return;
        }
        book.snapshot();
        book.runs.insert(cell.id.clone(), cell.versions.clone());
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
        let runs = cell.kind == MARKDOWN_KIND || (cell.kind == TEXT_KIND && !cell.prompt().trim().is_empty());
        if !runs {
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
        book.attach_runs();
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
        book.attach_runs();
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
        if self.project.is_some() {
            // Written first, so closing asks nothing. A notebook that could
            // not be written still asks.
            let _ = self.write_into_project(index);
        }
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
            .unwrap_or_else(|| Some(format!("{UNTITLED}.smx")));
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
            folder: self.project.as_ref().map(|folder| folder.display().to_string()),
            filter: Some(FileFilter {
                name: "Notebook".into(),
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
        if !self.in_project(&path) {
            return;
        }
        // An Open.
        match read_book(&path) {
            Ok(read) => {
                let index = self.push_book(Some(path));
                self.books[index].load(read);
                out.push(AppMessage::Contents {
                    tab: self.books[index].id.clone(),
                    kind: ContentsKind::Cells,
                });
                self.emit_cells(out);
            }
            Err(reason) => out.push(AppMessage::Ask(Ask {
                id: "book-unreadable".into(),
                title: "This notebook could not be opened".into(),
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
        if let Err(reason) = write_book(path, &book.cells, book.selected.as_deref()) {
            out.push(AppMessage::Ask(Ask {
                id: "book-unwritten".into(),
                title: "This notebook could not be saved".into(),
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
        let name = Some(format!("{UNTITLED}.smx"));
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
            self.emit_cells(out);
            return;
        };
        let path = PathBuf::from(path);
        match read_book(&path) {
            Ok(read) => {
                let index = self.push_book(Some(path));
                self.books[index].load(read);
                out.push(AppMessage::Contents {
                    tab: self.books[index].id.clone(),
                    kind: ContentsKind::Cells,
                });
                self.emit_cells(out);
            }
            Err(reason) => out.push(AppMessage::Ask(Ask {
                id: "book-unreadable".into(),
                title: "This notebook could not be opened".into(),
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
            AppMessage::Toolbar(toolbar(&self.adding)),
            AppMessage::Menu { rows: menu(self.project.is_some()) },
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
                new_tab_tip: "New notebook".into(),
            }));
        }
        messages
    }
}

/// A run as the host reports it.
struct Run {
    input: Vec<CellPart>,
    endpoint: String,
    model: String,
    state: CellState,
    output: Vec<CellPart>,
    error: Option<String>,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

impl Book {
    fn active_index(&self) -> Option<usize> {
        let selected = self.selected.as_deref()?;
        self.cells.iter().position(|cell| cell.id == selected)
    }

    /// Every cell carries all its runs, whatever snapshot it came back
    /// from, and agrees with its current one.
    fn attach_runs(&mut self) {
        for cell in &mut self.cells {
            let runs = self.runs.get(&cell.id).cloned().unwrap_or_default();
            // A run made after this snapshot stays current: undo takes back
            // the steps around a run, never the run itself, so the newest
            // result keeps showing. Only a step that switched versions puts
            // an older one back.
            if runs.len() > cell.versions.len() {
                cell.current = Some(runs.len() - 1);
            }
            cell.versions = runs;
            cell.agree();
        }
        if self.active_index().is_none() {
            self.selected = self.cells.first().map(|cell| cell.id.clone());
        }
    }

    /// A notebook read from its file.
    fn load(&mut self, read: Read) {
        self.runs = read
            .cells
            .iter()
            .map(|cell| (cell.id.clone(), cell.versions.clone()))
            .collect();
        if !read.cells.is_empty() {
            self.cells = read.cells;
        }
        self.selected = read.selected;
        self.attach_runs();
    }

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
        title: "Keep this notebook?".into(),
        lines: vec!["The notebook has changes that are not saved.".into()],
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

/// The toolbar, as the owner's reference binds it from its start up to the
/// type dropdown: the pointer, +, cut, copy, paste, delete, run, stop, Run
/// All, and the dropdown, which shows the kind + adds.
fn toolbar(adding: &str) -> Toolbar {
    let button = |icon: &str, tip: &str, action: Option<&str>, enabled: bool, selected: bool| ToolItem::Button {
        icon: icon.into(),
        tip: tip.into(),
        enabled,
        selected,
        action: action.map(Action::new),
        menu: Vec::new(),
    };
    Toolbar {
        items: vec![
            // The only tool for now, always selected.
            button("cursor-pointer", "Select", None, true, true),
            button("plus", "Add cell", Some("add_cell"), true, false),
            ToolItem::Separator,
            button("scissor", "Cut cell", Some("cut"), true, false),
            button("copy", "Copy cell", Some("copy"), true, false),
            button("paste-clipboard", "Paste cell", Some("paste"), true, false),
            button("trash", "Delete cell", Some("delete_cell"), true, false),
            ToolItem::Separator,
            button("play", "Run", Some("run"), true, false),
            button("square", "Stop", Some("stop"), true, false),
            // Run All waits for a later version.
            button("forward", "Run all", None, false, false),
            ToolItem::Separator,
            ToolItem::Menu {
                icon: String::new(),
                label: kind_label(adding).into(),
                enabled: true,
                rows: vec![row("LLM", "kind_text"), row("Markdown", "kind_markdown")],
            },
        ],
        trailing: Vec::new(),
    }
}

fn kind_label(kind: &str) -> &'static str {
    if kind == MARKDOWN_KIND { "Markdown" } else { "LLM" }
}

/// The menu. In a project every change is written by itself, so Save and
/// Save As are not offered.
fn menu(in_project: bool) -> Vec<MenuEntry> {
    let mut rows = vec![row("New notebook", "new_book"), row("Open", "open")];
    if !in_project {
        rows.push(row("Save", "save"));
        rows.push(row("Save As", "save_as"));
    }
    rows.push(row("Undo", "undo"));
    rows.push(row("Redo", "redo"));
    rows
}

/// `name`, then the names after it: "Untitled Notebook" gives "Untitled
/// Notebook", "Untitled Notebook 2", and on; "Notes 3" gives "Notes 3",
/// "Notes 4".
fn free_names(name: &str) -> impl Iterator<Item = String> {
    let (stem, first) = match name.rsplit_once(' ') {
        Some((stem, number)) if !stem.is_empty() => match number.parse::<u32>() {
            Ok(number) => (stem.to_string(), Some(number)),
            Err(_) => (name.to_string(), None),
        },
        _ => (name.to_string(), None),
    };
    let start = first.unwrap_or(1);
    (start..start + 10_000).map(move |number| match first {
        None if number == 1 => stem.clone(),
        _ => format!("{stem} {number}"),
    })
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

/// The notebook's own keys, as Jupyter's command mode has them. The host
/// sends them only while no text is being edited.
fn keys() -> Keys {
    let chord = |key: &str, command: bool, shift: bool| Chord {
        key: key.into(),
        command,
        shift: Some(shift),
        alt: false,
    };
    let bind = |chord: Chord, then: Vec<Chord>, action: &str| protocol::Binding {
        chord,
        then,
        action: Action::new(action),
    };
    Keys {
        bindings: vec![
            bind(chord("Z", true, false), Vec::new(), "undo"),
            bind(chord("Z", true, true), Vec::new(), "redo"),
            bind(chord("Z", false, false), Vec::new(), "undo"),
            bind(chord("Z", false, true), Vec::new(), "redo"),
            bind(chord("A", false, false), Vec::new(), "add_above"),
            bind(chord("B", false, false), Vec::new(), "add_below"),
            bind(chord("M", false, false), Vec::new(), "kind_markdown"),
            bind(chord("L", false, false), Vec::new(), "kind_text"),
            bind(chord("D", false, false), vec![chord("D", false, false)], "delete_cell"),
        ],
        copy: Some(Action::new("copy")),
        cut: Some(Action::new("cut")),
        paste: Some(Action::new("paste")),
    }
}

fn file_title(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| String::from(UNTITLED))
}

/// True when the file is a notebook this application can open.
pub fn claims(path: &Path) -> bool {
    read_book(path).is_ok()
}

/// A notebook file: version 2, or a 0.5 mission book, version 1, whose
/// cells with a result each become one run.
pub fn read_book(path: &Path) -> Result<Read, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let file: BookFile = serde_json::from_str(&text).map_err(|_| {
        String::from("It is not a notebook, or it is damaged.")
    })?;
    let mut cells = file.cells;
    match file.version {
        1 => {
            for cell in &mut cells {
                if cell.versions.is_empty() && !cell.output.is_empty() {
                    cell.versions.push(CellVersion {
                        id: protocol::cell_id(),
                        input: cell.input.clone(),
                        endpoint: cell.endpoint.clone(),
                        model: cell.model.clone(),
                        output: cell.output.clone(),
                        state: cell.state,
                        error: cell.error.clone(),
                        time: 0,
                    });
                    cell.current = Some(0);
                }
            }
        }
        VERSION => {}
        other => {
            return Err(format!(
                "It is version {other} of a notebook, and this Mission Control reads version {VERSION}."
            ));
        }
    }
    for cell in &mut cells {
        cell.agree();
    }
    Ok(Read { cells, selected: file.selected })
}

pub fn write_book(path: &Path, cells: &[Cell], selected: Option<&str>) -> Result<(), String> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    }
    let file = BookFile {
        version: VERSION,
        cells: cells.to_vec(),
        selected: selected.map(String::from),
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

    fn command(session: &mut Session, id: &str) -> Vec<AppMessage> {
        session.handle(HostMessage::Command {
            id: id.into(),
            tab: Some("1".into()),
            arg: None,
            node: None,
            viewport: [800.0, 600.0],
        })
    }

    /// The last cells report, and the selected cell's id.
    fn last_cells(messages: &[AppMessage]) -> (Vec<Cell>, Option<String>) {
        messages
            .iter()
            .rev()
            .find_map(|message| match message {
                AppMessage::Cells { cells, selected, .. } => Some((cells.clone(), selected.clone())),
                _ => None,
            })
            .expect("no cells report")
    }

    fn text(words: &str) -> Vec<CellPart> {
        vec![CellPart::Text { text: words.into() }]
    }

    fn run(session: &mut Session, cell: &str, prompt: &str, result: &str) -> Vec<AppMessage> {
        session.handle(HostMessage::CellResult {
            tab: "1".into(),
            cell: cell.into(),
            input: text(prompt),
            endpoint: "openrouter".into(),
            model: "stand-in/model".into(),
            state: CellState::Done,
            output: text(result),
            error: None,
        })
    }

    /// No host: a notebook keeps every run as a version, made from the prompt
    /// that ran; back and forward switch versions; undo and redo never add or
    /// remove a run; a deleted cell comes back with its runs; cut and paste,
    /// Markdown, saving with the active cell, and reopening all hold.
    #[test]
    fn a_notebook_is_kept_with_no_host() {
        let dir = std::env::temp_dir().join(format!("inpainter-book-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keeper.smx");
        let mut session = Session::launch();
        let greeting = session.greeting();
        assert!(greeting.iter().any(|message| matches!(message, AppMessage::Contents { kind: ContentsKind::Cells, .. })));
        assert!(greeting.iter().any(|message| matches!(
            message,
            AppMessage::Tabs(tabs) if tabs.tabs[0].label == UNTITLED
        )));

        // A new notebook has one empty LLM cell, active.
        let (cells, selected) = last_cells(&session.handle(HostMessage::EditCell {
            tab: "1".into(),
            cell: session.books[0].cells[0].id.clone(),
            input: text("A lighthouse keeper."),
        }));
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].kind, TEXT_KIND);
        let first = cells[0].id.clone();
        assert_eq!(selected.as_deref(), Some(first.as_str()));

        // Three runs, three versions, the last current.
        run(&mut session, &first, "A lighthouse keeper.", "One.");
        run(&mut session, &first, "A lighthouse keeper.", "Two.");
        let (cells, _) = last_cells(&run(&mut session, &first, "A lighthouse keeper.", "Three."));
        assert_eq!(cells[0].versions.len(), 3);
        assert_eq!(cells[0].current, Some(2));
        assert_eq!(cells[0].result_text(), "Three.");

        // Back to version 2; an edit; a run from there adds a 4th at the end,
        // holding the prompt that ran, not a later edit.
        let (cells, _) = last_cells(&session.handle(HostMessage::SetVersion { tab: "1".into(), cell: first.clone(), index: 1 }));
        assert_eq!((cells[0].current, cells[0].result_text().as_str()), (Some(1), "Two."));
        session.handle(HostMessage::EditCell { tab: "1".into(), cell: first.clone(), input: text("A darker keeper.") });
        let (cells, _) = last_cells(&run(&mut session, &first, "A darker keeper.", "Four."));
        assert_eq!(cells[0].versions.len(), 4);
        assert_eq!(cells[0].current, Some(3));
        assert_eq!(cells[0].versions[3].input, text("A darker keeper."));
        let ids: std::collections::HashSet<_> = cells[0].versions.iter().map(|version| version.id.clone()).collect();
        assert_eq!(ids.len(), 4, "versions share an id");

        // Undo and redo, every step back to the start and forward again,
        // never change the number of runs.
        // The three steps: the first edit, the switch to version 2, and the
        // second edit.
        for _ in 0..3 {
            let (cells, _) = last_cells(&command(&mut session, "undo"));
            assert_eq!(cells[0].versions.len(), 4, "undo removed a run");
        }
        assert!(command(&mut session, "undo").is_empty(), "a run was an undo step");
        for _ in 0..3 {
            let (cells, _) = last_cells(&command(&mut session, "redo"));
            assert_eq!(cells[0].versions.len(), 4, "redo changed the runs");
        }

        // Add a Markdown cell below; running it shows it rendered, makes no
        // version, and showing its source again is not a version either.
        command(&mut session, "kind_markdown");
        let (cells, selected) = last_cells(&command(&mut session, "add_cell"));
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[1].kind, MARKDOWN_KIND);
        let notes = cells[1].id.clone();
        assert_eq!(selected.as_deref(), Some(notes.as_str()));
        session.handle(HostMessage::EditCell { tab: "1".into(), cell: notes.clone(), input: text("# The Last Signal") });
        let (cells, _) = last_cells(&session.handle(HostMessage::CellResult {
            tab: "1".into(),
            cell: notes.clone(),
            input: Vec::new(),
            endpoint: String::new(),
            model: String::new(),
            state: CellState::Done,
            output: Vec::new(),
            error: None,
        }));
        assert_eq!((cells[1].state, cells[1].versions.len()), (CellState::Done, 0));
        let (cells, _) = last_cells(&session.handle(HostMessage::ShowSource { tab: "1".into(), cell: notes.clone() }));
        assert_eq!(cells[1].state, CellState::Idle);

        // Move the LLM cell below the Markdown cell, then delete it and undo:
        // it comes back with all four runs.
        let (cells, _) = last_cells(&session.handle(HostMessage::MoveCell { tab: "1".into(), cell: first.clone(), to: 1 }));
        assert_eq!(cells[1].id, first);
        let (cells, _) = last_cells(&session.handle(HostMessage::DeleteCell { tab: "1".into(), cell: first.clone() }));
        assert_eq!(cells.len(), 1);
        let (cells, _) = last_cells(&command(&mut session, "undo"));
        let back = cells.iter().find(|cell| cell.id == first).expect("the deleted cell did not come back");
        assert_eq!(back.versions.len(), 4);

        // Copy and paste: a new id, `from` the source, runs under new ids.
        session.handle(HostMessage::SelectCell { tab: "1".into(), cell: first.clone() });
        command(&mut session, "copy");
        let (cells, selected) = last_cells(&command(&mut session, "paste"));
        let pasted = cells.iter().find(|cell| Some(&cell.id) == selected.as_ref()).unwrap();
        assert_ne!(pasted.id, first);
        assert_eq!(pasted.from.as_deref(), Some(first.as_str()));
        assert_eq!(pasted.versions.len(), 4);
        assert!(pasted.versions.iter().all(|version| !ids.contains(&version.id)));

        // Save with the active cell, and reopen as it was.
        let saved = command(&mut session, "save");
        let AppMessage::File { id, kind: FileKind::Save, .. } = &saved[0] else {
            panic!("save asked for a file, got {}", lines_of(&saved).join(""));
        };
        session.handle(HostMessage::FileResult { id: id.clone(), path: Some(path.display().to_string()) });
        let read = read_book(&path).unwrap();
        assert_eq!(read.selected, selected);
        let mut again = Session::restored();
        let (cells, reselected) = last_cells(&again.handle(HostMessage::Open { path: Some(path.display().to_string()) }));
        assert_eq!(reselected, selected);
        let reopened = cells.iter().find(|cell| cell.id == first).unwrap();
        assert_eq!((reopened.versions.len(), reopened.current), (4, Some(3)));
        assert_eq!(reopened.prompt(), "A darker keeper.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Undo after a run keeps that run's result showing, and a run that ends
    /// after its cell was deleted comes back with the cell.
    #[test]
    fn undo_never_hides_or_loses_a_run() {
        let mut session = Session::launch();
        let id = session.books[0].cells[0].id.clone();
        session.handle(HostMessage::EditCell { tab: "1".into(), cell: id.clone(), input: text("p1") });
        run(&mut session, &id, "p1", "R1");
        session.handle(HostMessage::EditCell { tab: "1".into(), cell: id.clone(), input: text("p2") });
        run(&mut session, &id, "p2", "R2");
        let (cells, _) = last_cells(&command(&mut session, "undo"));
        assert_eq!(cells[0].result_text(), "R2", "undo showed an older run");
        assert_eq!(cells[0].current, Some(1));

        command(&mut session, "add_cell");
        let doomed = session.books[0].cells[1].id.clone();
        session.handle(HostMessage::DeleteCell { tab: "1".into(), cell: doomed.clone() });
        run(&mut session, &doomed, "late", "Arrived after the delete.");
        let (cells, _) = last_cells(&command(&mut session, "undo"));
        let back = cells.iter().find(|cell| cell.id == doomed).expect("the cell did not come back");
        assert_eq!(back.versions.len(), 1, "the run that ended after the delete was lost");
    }

    /// A notebook made with + reports its one cell.
    #[test]
    fn a_new_notebook_reports_its_cell() {
        let mut session = Session::launch();
        let (cells, selected) = last_cells(&command(&mut session, "new_book"));
        assert_eq!(cells.len(), 1);
        assert_eq!(selected.as_deref(), Some(cells[0].id.as_str()));
        let (cells, _) = last_cells(&session.handle(HostMessage::Open { path: None }));
        assert_eq!(cells.len(), 1);
    }

    /// Deleting the last cell leaves one empty cell, active.
    #[test]
    fn the_last_cell_deleted_leaves_an_empty_one() {
        let mut session = Session::launch();
        let only = session.books[0].cells[0].id.clone();
        let (cells, selected) = last_cells(&session.handle(HostMessage::DeleteCell { tab: "1".into(), cell: only.clone() }));
        assert_eq!(cells.len(), 1);
        assert_ne!(cells[0].id, only);
        assert_eq!(selected.as_deref(), Some(cells[0].id.as_str()));
    }

    /// A mission book saved by 0.5 opens, each result as one version.
    #[test]
    fn a_version_one_book_opens() {
        let dir = std::env::temp_dir().join(format!("inpainter-book-v1-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.smx");
        let mut cell = Cell::text("old-1", "A keeper.");
        cell.output = text("He kept it.");
        cell.state = CellState::Done;
        std::fs::write(&path, serde_json::json!({"version": 1, "cells": [cell]}).to_string()).unwrap();
        let read = read_book(&path).unwrap();
        assert_eq!(read.cells[0].versions.len(), 1);
        assert_eq!(read.cells[0].current, Some(0));
        assert_eq!(read.cells[0].result_text(), "He kept it.");
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

    fn project_folder(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("inpainter-book-project-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    /// A launched session inside `folder`, its notebook changed.
    fn changed_in(folder: &Path) -> Session {
        let mut session = Session::launch();
        let _ = session.handle(HostMessage::Project { folder: folder.display().to_string() });
        let _ = command(&mut session, "add_cell");
        session
    }

    fn notebooks(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(folder)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".smx"))
            .collect();
        names.sort();
        names
    }

    /// Catches a notebook that waits for Save, or two sessions writing one
    /// file: a second after a change it is in the project's folder, and a
    /// second notebook of the same name gets the next free name.
    #[test]
    fn a_notebook_is_written_into_the_project_by_itself() {
        let folder = project_folder("written");
        let mut first = changed_in(&folder);
        first.write_due(Instant::now());
        assert!(notebooks(&folder).is_empty(), "written before a second passed");
        first.write_due(Instant::now() + WRITE_AFTER * 2);
        assert_eq!(notebooks(&folder), vec!["Untitled Notebook.smx".to_string()]);
        let written = std::fs::read(folder.join("Untitled Notebook.smx")).unwrap();
        let mut second = changed_in(&folder);
        second.write_due(Instant::now() + WRITE_AFTER * 2);
        assert_eq!(
            notebooks(&folder),
            vec!["Untitled Notebook 2.smx".to_string(), "Untitled Notebook.smx".to_string()]
        );
        assert_eq!(std::fs::read(folder.join("Untitled Notebook.smx")).unwrap(), written);
        let _ = std::fs::remove_dir_all(folder);
    }

    /// Catches a `keep` that never writes, a failed write reported as kept,
    /// Save offered in a project, and a path from outside it opened.
    #[test]
    fn a_project_notebook_keeps_and_refuses() {
        let folder = project_folder("keep");
        let mut session = changed_in(&folder);
        let replies = session.handle(HostMessage::Keep { tab: "1".into() });
        let expected = folder.join("Untitled Notebook.smx").display().to_string();
        assert!(replies.iter().any(|reply| matches!(reply, AppMessage::Kept { path: Some(path), error: None, .. } if *path == expected)));
        let labels: Vec<String> = menu(true).into_iter().map(|row| row.label).collect();
        assert!(!labels.iter().any(|label| label.starts_with("Save")), "{labels:?}");

        let outside = project_folder("outside").join("Elsewhere.smx");
        write_book(&outside, &[], None).unwrap();
        let before = session.books.len();
        let _ = session.handle(HostMessage::Open { path: Some(outside.display().to_string()) });
        assert_eq!(session.books.len(), before, "a notebook from outside the project opened");

        let locked = project_folder("locked");
        let mut failing = changed_in(&locked);
        let mut permissions = std::fs::metadata(&locked).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&locked, permissions.clone()).unwrap();
        let replies = failing.handle(HostMessage::Keep { tab: "1".into() });
        permissions.set_readonly(false);
        let _ = std::fs::set_permissions(&locked, permissions);
        assert!(replies.iter().any(|reply| matches!(reply, AppMessage::Kept { error: Some(_), .. })), "a failed write was reported as kept");
        assert!(failing.books[0].dirty, "the change was lost");
        for dir in [folder, locked, outside.parent().unwrap().to_path_buf()] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
