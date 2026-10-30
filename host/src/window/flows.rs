//! The project's and session's flows: keeping the session, switching
//! sessions and projects, new ones, Trash, and quitting.
//!
//! The host owns projects and sessions. A session's file, inside its
//! project's `.inpainter/sessions`, holds each tab's sub-application, file
//! (relative to the project's folder), label, and camera, the selected tab,
//! whether the workspace panel is shown, and the conversation, never a
//! sub-application's work. Switching closes every tab through the quit flow;
//! each sub-application writes its own work into the project as it goes, so
//! nothing is asked. The session is written on its own, at most once a
//! second, and always before switching or closing.

use super::{AppSpec, HostWindow, Project, Started};
use crate::chat::LogoChoice;
use crate::sessions::{Page, ProjectRow, SessionChoice, SessionRow, TrashRow, Trashed};
use crate::process::Running;
use crate::workspace::{self, ProjectFile, ProjectRecord, ProjectTab, SessionFile, View};
use assistant::{Command, Entry, Role};
use eframe::egui;
use protocol::{Ask, AskButton, HostMessage};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// After the sub-applications have declared themselves, what they report
/// for this long is still the project's starting state, not a change.
#[cfg(not(test))]
const SETTLE_GRACE: Duration = Duration::from_millis(400);
#[cfg(test)]
const SETTLE_GRACE: Duration = Duration::ZERO;
/// How long a restore waits for the tabs it asked to reopen.
const RESTORE_WAIT: Duration = Duration::from_secs(10);
/// How often a changed project is written, apart from a switch, a new one,
/// or the window closing, which write at once.
pub(super) const WRITE_EVERY: Duration = Duration::from_secs(1);
/// The host's own question's id, apart from every sub-application's.
const QUESTION: usize = usize::MAX;
/// What Open asks about a file from outside the project.
const IMPORT_TITLE: &str = "Import a copy into this project?";

fn import_line(path: &Path) -> String {
    format!(
        "{} is outside the project. A copy is added to the project's folder, and the original stays as it is.",
        path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
    )
}

/// What Empty Trash asks before it acts.
const EMPTY_TRASH_LINE: &str =
    "Projects in Trash go to the system's trash with their folders. Sessions in Trash are deleted; their projects keep their files.";

/// The quit flow is closing every tab, and then this happens.
enum Closing {
    Quit { closed: bool },
    Open(Opening),
}

/// Before a switch or quit: every tab was asked to `keep`, and these
/// answers are still awaited. A failure stops it all.
struct Keeping {
    closing: Closing,
    /// The sub-application and tab of each awaited answer.
    waiting: Vec<(usize, String)>,
    started: Instant,
}

/// How long a sub-application may take to answer `keep`. One that never
/// answers saves no files, so the switch goes on without it.
const KEEP_WAIT: Duration = Duration::from_secs(5);

/// A project and one of its sessions, read and ready to open.
pub(super) struct Opening {
    id: String,
    name: String,
    folder: PathBuf,
    session: SessionFile,
}

/// A question the host asks itself, in the existing dialog look.
enum Question {
    EmptyTrash,
    /// A file from outside the project, picked in an application's Open:
    /// the application, its request, and the file.
    Import { app: usize, request: String, path: PathBuf },
}

struct RestoreTab {
    path: Option<String>,
    camera: Option<View>,
}

/// One restarted sub-application's tabs to reopen, in order.
struct RestoreApp {
    tabs: Vec<RestoreTab>,
    opened: bool,
    /// The new tab id of each, once matched.
    ids: Option<Vec<Option<String>>>,
}

struct Restore {
    apps: Vec<RestoreApp>,
    /// The selected tab: its sub-application, and its place among that one's tabs.
    selected: Option<(usize, usize)>,
    started: Instant,
    mapped: bool,
    cameras: Vec<(usize, String, View)>,
}

/// A tab as the project's changes are judged: its sub-application, its id,
/// and its file.
#[derive(Debug, Clone, PartialEq)]
struct TabKey {
    app: String,
    tab: String,
    path: Option<String>,
}

#[derive(Default)]
struct Baseline {
    tabs: Vec<TabKey>,
    selected: Option<TabKey>,
    conversation: usize,
    name: String,
    folders: Vec<String>,
    workspace_open: bool,
}

#[derive(Default)]
enum Settle {
    /// The sub-applications are still declaring themselves.
    #[default]
    Waiting,
    Grace(Instant),
    Done,
}

/// A tab as closing began. An application may close a tab after the person
/// answers, and the project still keeps the tab from before that.
struct KeptTab {
    app: usize,
    tab: String,
    record: ProjectTab,
    /// A never-saved tab the person discarded. It leaves the project.
    dropped: bool,
    /// The application has stopped reporting it.
    gone: bool,
}

struct Kept {
    tabs: Vec<KeptTab>,
    /// Which `tabs` entry was selected, before any were dropped.
    selected: Option<usize>,
}

#[derive(Default)]
pub(crate) struct Flows {
    /// A tab's camera was moved by hand since the project was written.
    pub moved: bool,
    baseline: Baseline,
    settle: Settle,
    question: Option<Question>,
    keeping: Option<Keeping>,
    closing: Option<Closing>,
    restore: Option<Restore>,
    /// The tabs at the start of a quit, updated with a path a save produces.
    kept: Option<Kept>,
    /// Paths the file dialog returned during a quit: the application, how
    /// many tab reports it had sent by then, and the path.
    save_paths: Vec<(usize, usize, String)>,
}

impl Flows {
    /// The title of the host's own question, while one shows.
    #[cfg(test)]
    pub(crate) fn question_shown(&self) -> Option<String> {
        match self.question.as_ref()? {
            Question::EmptyTrash => Some("Empty Trash?".to_string()),
            Question::Import { .. } => Some(IMPORT_TITLE.to_string()),
        }
    }

    /// The lines of the host's own question, while one shows.
    #[cfg(test)]
    pub(crate) fn question_lines(&self) -> Vec<String> {
        match &self.question {
            Some(Question::EmptyTrash) => vec![EMPTY_TRASH_LINE.to_string()],
            Some(Question::Import { path, .. }) => vec![import_line(path)],
            None => Vec::new(),
        }
    }

    /// The quit flow is closing every tab.
    #[cfg(test)]
    pub(crate) fn closing_started(&self) -> bool {
        self.keeping.is_some() || self.closing.is_some()
    }

    /// No question, quit, or restore is under way.
    #[cfg(test)]
    pub(crate) fn idle(&self) -> bool {
        !self.busy()
    }

    fn busy(&self) -> bool {
        self.question.is_some() || self.keeping.is_some() || self.closing.is_some() || self.restore.is_some()
    }
}

fn button(id: &str, label: &str) -> AskButton {
    AskButton {
        id: id.to_string(),
        label: label.to_string(),
    }
}

fn note(text: String) -> Entry {
    Entry {
        role: Role::Note,
        text,
        time: assistant::now(),
        attachments: Vec::new(),
        tool: None,
        ok: None,
        cell: None,
        aside: true,
    }
}

/// Matches the tabs to reopen with the tabs a restarted sub-application
/// reports: by file first, then the rest in order.
fn match_tabs(wanted: &[RestoreTab], reported: &[protocol::TabEntry]) -> Vec<Option<String>> {
    let same = |a: &str, b: &str| {
        a == b
            || matches!(
                (std::fs::canonicalize(a), std::fs::canonicalize(b)),
                (Ok(a), Ok(b)) if a == b
            )
    };
    let mut used = vec![false; reported.len()];
    let mut ids = vec![None; wanted.len()];
    for (index, tab) in wanted.iter().enumerate() {
        let Some(path) = &tab.path else {
            continue;
        };
        if let Some(found) = reported.iter().enumerate().position(|(at, entry)| {
            !used[at] && entry.path.as_deref().is_some_and(|known| same(known, path))
        }) {
            used[found] = true;
            ids[index] = Some(reported[found].id.clone());
        }
    }
    for id in ids.iter_mut().filter(|id| id.is_none()) {
        if let Some(found) = used.iter().position(|taken| !taken) {
            used[found] = true;
            *id = Some(reported[found].id.clone());
        }
    }
    ids
}

impl HostWindow {
    /// Every running sub-application's tabs, as the project's changes are judged.
    fn tab_keys(&self) -> Vec<TabKey> {
        let mut keys = Vec::new();
        for started in self.apps.iter().filter(|started| !started.running.exited()) {
            for tab in started.running.tabs().0 {
                keys.push(TabKey {
                    app: started.id.clone(),
                    tab: tab.id,
                    path: tab.path,
                });
            }
        }
        keys
    }

    fn selected_key(&self) -> Option<TabKey> {
        let started = self.apps.get(self.selected)?;
        let (tabs, active) = started.running.tabs();
        let active = active?;
        let tab = tabs.into_iter().find(|tab| tab.id == active)?;
        Some(TabKey {
            app: started.id.clone(),
            tab: tab.id,
            path: tab.path,
        })
    }

    fn rebaseline(&mut self) {
        let (name, folders, workspace_open) = match &self.project {
            Some(project) => (
                project.name.clone(),
                Vec::<String>::new(),
                project.workspace_open,
            ),
            None => (String::new(), Vec::new(), true),
        };
        self.flows.baseline = Baseline {
            tabs: self.tab_keys(),
            selected: self.selected_key(),
            conversation: self.chat.conversation.len(),
            name,
            folders,
            workspace_open,
        };
        self.flows.moved = false;
    }

    /// The project has changes it has not written: a tab opened or closed, a
    /// tab gained a file, the selection or a camera changed, the conversation
    /// grew, or the name, the folders, or the workspace changed. A project
    /// still starting has none.
    pub(super) fn project_dirty(&self) -> bool {
        if !matches!(self.flows.settle, Settle::Done) {
            return false;
        }
        let (name, folders, workspace_open) = match &self.project {
            Some(project) => (
                project.name.as_str(),
                &[] as &[String],
                project.workspace_open,
            ),
            None => ("", &[][..], true),
        };
        let base = &self.flows.baseline;
        self.flows.moved
            || self.tab_keys() != base.tabs
            || self.selected_key() != base.selected
            || self.chat.conversation.len() != base.conversation
            || name != base.name
            || folders != base.folders
            || workspace_open != base.workspace_open
    }

    /// The project as its file holds it.
    fn project_file(&self) -> ProjectFile {
        let mut tabs = Vec::new();
        let mut selected = None;
        for (index, started) in self.apps.iter().enumerate() {
            if started.running.exited() {
                continue;
            }
            let (entries, active) = started.running.tabs();
            for entry in entries {
                if index == self.selected && active.as_deref() == Some(entry.id.as_str()) {
                    selected = Some(tabs.len());
                }
                let camera = started
                    .running
                    .declared
                    .documents
                    .get(&entry.id)
                    .map(|display| View {
                        origin: display.camera.origin,
                        zoom: display.camera.zoom,
                    });
                tabs.push(ProjectTab {
                    app: started.id.clone(),
                    path: entry.path,
                    label: entry.label,
                    camera,
                });
            }
        }
        let (name, folders, workspace_open) = match &self.project {
            Some(project) => (
                project.name.clone(),
                Vec::<String>::new(),
                project.workspace_open,
            ),
            None => (workspace::UNTITLED.to_string(), Vec::new(), false),
        };
        ProjectFile {
            version: workspace::VERSION,
            name,
            folders,
            tabs,
            selected,
            workspace_open,
            conversation: self.chat.conversation.clone(),
        }
    }

    /// What changed, so a failed write is tried again only after a new change.
    fn change_signature(&self) -> String {
        let tabs = self.tab_keys();
        let selected = self.selected_key();
        let (name, folders, open) = match &self.project {
            Some(project) => (
                project.name.clone(),
                Vec::<String>::new(),
                project.workspace_open,
            ),
            None => (String::new(), Vec::new(), true),
        };
        format!(
            "{tabs:?}|{selected:?}|{}|{name}|{folders:?}|{open}|{}|{:?}",
            self.chat.conversation.len(),
            self.flows.moved,
            self.level
        )
    }

    fn tell(&mut self, text: String) {
        self.assistant.send(Command::Note(text));
    }

    /// Writes the project when it has changed. `force` writes even during the
    /// once-a-second wait, and even when the last attempt failed on this same
    /// change: switching, creating, and closing use it.
    pub(super) fn write_project(&mut self, force: bool) {
        if self.project.is_none() || self.data.is_none() {
            return;
        }
        if !force && (self.flows.closing.is_some() || self.flows.restore.is_some()) {
            return;
        }
        if !force && !matches!(self.flows.settle, Settle::Done) {
            return;
        }
        let signature = self.change_signature();
        if !force && !self.project_dirty() {
            return;
        }
        if !force && self.failed_signature.as_ref() == Some(&signature) {
            return;
        }
        if !force
            && self
                .last_project_write
                .is_some_and(|last| last.elapsed() < WRITE_EVERY)
        {
            return;
        }
        let file = self.project_file();
        self.write_file(file);
    }

    /// Writes the open session into its project, its paths relative to the
    /// project's folder.
    fn write_file(&mut self, file: ProjectFile) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let signature = self.change_signature();
        // A session deleted while open keeps its place in Trash when the
        // switch away from it writes it a last time.
        let trashed_at = SessionFile::read(&project.folder, &project.session)
            .ok()
            .and_then(|file| file.trashed_at);
        let base = SessionFile {
            version: workspace::VERSION,
            id: project.session.clone(),
            name: project.session_name.clone(),
            created_at: project.session_created,
            updated_at: workspace::now_seconds(),
            trashed_at,
            tabs: Vec::new(),
            selected: None,
            workspace_open: false,
            conversation: Vec::new(),
            level: self.level.clone(),
        };
        let session = workspace::file_as_session(&project.folder, &file, base);
        match session.write(&project.folder) {
            Ok(()) => {
                self.rebaseline();
                self.failed_signature = None;
                self.last_project_write = Some(Instant::now());
            }
            Err(reason) => {
                self.last_project_write = Some(Instant::now());
                if self.failed_signature.as_ref() != Some(&signature) {
                    self.tell(format!("The session could not be kept: {reason}"));
                }
                self.failed_signature = Some(signature);
            }
        }
    }

    /// The open tabs, taken when a quit starts. An application may close a tab
    /// it has asked about, so the tabs after the questions are not the project's.
    fn snapshot_tabs(&self) -> Kept {
        let mut tabs = Vec::new();
        let mut selected = None;
        for (index, started) in self.apps.iter().enumerate() {
            if started.running.exited() {
                continue;
            }
            // The same tabs the project file records, including one an
            // application reported only in its greeting.
            let (entries, active) = started.running.tabs();
            for entry in entries {
                if index == self.selected && active.as_deref() == Some(entry.id.as_str()) {
                    selected = Some(tabs.len());
                }
                let camera = started
                    .running
                    .declared
                    .documents
                    .get(&entry.id)
                    .map(|display| View {
                        origin: display.camera.origin,
                        zoom: display.camera.zoom,
                    });
                tabs.push(KeptTab {
                    app: index,
                    tab: entry.id,
                    record: ProjectTab {
                        app: started.id.clone(),
                        path: entry.path,
                        label: entry.label,
                        camera,
                    },
                    dropped: false,
                    gone: false,
                });
            }
        }
        Kept { tabs, selected }
    }

    /// A file dialog answered with a path during a quit. Which tab it saved
    /// shows in the application's later tab reports, so it is only noted here.
    pub(super) fn note_save_path(&mut self, app: usize, path: String) {
        if self.flows.closing.is_none() {
            return;
        }
        let reports = self
            .apps
            .get(app)
            .and_then(|started| started.running.declared.tab_log.as_ref())
            .map_or(0, Vec::len);
        self.flows.save_paths.push((app, reports, path));
    }

    /// Follows each application's tab reports since the quit started, using
    /// only what the protocol carries. A tab keeps the last file it reported.
    /// A tab with no file that leaves a report after a file dialog returned a
    /// path, alone, was saved there. Any other tab with no file that leaves
    /// was discarded, and leaves the project.
    fn absorb_kept(&mut self) {
        let logs: Vec<Vec<Vec<(String, Option<String>, String)>>> = self
            .apps
            .iter_mut()
            .map(|started| started.running.declared.tab_log.take().unwrap_or_default())
            .collect();
        let mut paths = std::mem::take(&mut self.flows.save_paths);
        let Some(kept) = &mut self.flows.kept else {
            return;
        };
        for (app, log) in logs.iter().enumerate() {
            for (index, report) in log.iter().enumerate() {
                let mut left = Vec::new();
                for (row_index, row) in kept.tabs.iter_mut().enumerate() {
                    if row.app != app || row.gone {
                        continue;
                    }
                    match report.iter().find(|tab| tab.0 == row.tab) {
                        Some((_, path, label)) => {
                            if let Some(path) = path {
                                // A dialog's path a tab now reports is that
                                // tab's, and is no longer waiting for one.
                                paths.retain(|(which, _, waiting)| *which != app || waiting != path);
                                row.record.path = Some(path.clone());
                            }
                            if !label.is_empty() {
                                row.record.label = label.clone();
                            }
                        }
                        None => {
                            row.gone = true;
                            if row.record.path.is_none() {
                                left.push(row_index);
                            }
                        }
                    }
                }
                let waiting = paths
                    .iter()
                    .position(|(which, reports, _)| *which == app && *reports <= index);
                match (left.as_slice(), waiting) {
                    ([saved], Some(at)) => {
                        kept.tabs[*saved].record.path = Some(paths.remove(at).2);
                    }
                    _ => {
                        for row in left {
                            kept.tabs[row].dropped = true;
                        }
                    }
                }
            }
        }
    }

    /// Writes the tabs from when the quit started, not the ones left after
    /// each question closed its tab.
    fn write_kept(&mut self) {
        self.absorb_kept();
        let mut file = self.project_file();
        if let Some(kept) = self.flows.kept.take() {
            let mut tabs = Vec::new();
            let mut selected = None;
            for (index, tab) in kept.tabs.into_iter().enumerate() {
                if tab.dropped {
                    continue;
                }
                if kept.selected == Some(index) {
                    selected = Some(tabs.len());
                }
                tabs.push(tab.record);
            }
            file.tabs = tabs;
            file.selected = selected;
        }
        self.write_file(file);
    }

    /// The logo menu. It only quits, and quitting closes the window as its
    /// close button does.
    pub(super) fn logo(&mut self, choice: LogoChoice, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if choice == LogoChoice::Settings {
            self.settings_panel.open_at(crate::settings_panel::Page::General);
            return;
        }
        if self.locked() || self.flows.busy() {
            return;
        }
        if choice == LogoChoice::Quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Where Studio makes a project by itself and where New Project starts:
    /// `~/Inpainter`, or beside a frame test's data folder.
    pub(super) fn projects_root(&self) -> Option<PathBuf> {
        self.projects_root
            .clone()
            .or_else(|| self.data.as_ref().map(|data| data.join("Inpainter")))
    }

    /// Asks whether to import a file from outside the project that an
    /// application's Open picked.
    pub(super) fn ask_import(&mut self, app: usize, request: String, path: PathBuf) {
        self.flows.question = Some(Question::Import { app, request, path });
    }

    /// A choice in the session menu.
    pub(super) fn session_choice(&mut self, choice: SessionChoice, frame: &mut eframe::Frame) {
        if choice == SessionChoice::Opened {
            self.refresh_sessions();
            return;
        }
        if self.locked() || self.flows.busy() || self.project.is_none() {
            return;
        }
        let Some(data) = self.data.clone() else {
            return;
        };
        // A locked project holds its name, Trash, and archiving, and its
        // sessions' Delete, whatever asks.
        let held = match &choice {
            SessionChoice::RenameProject { project, .. }
            | SessionChoice::DeleteProject { project }
            | SessionChoice::DeleteSession { project, .. }
            | SessionChoice::ArchiveProject { project, archived: true } => workspace::known_projects(&data)
                .into_iter()
                .any(|known| known.entry.id == *project && known.entry.locked),
            _ => false,
        };
        if held {
            self.tell("That project is locked. Unlock it first.".to_string());
            return;
        }
        match choice {
            SessionChoice::Opened => {}
            SessionChoice::Open { project, session } => {
                self.sessions.open = false;
                self.switch_to(&project, Some(&session));
            }
            SessionChoice::NewSession { project } => {
                let Some(known) = workspace::known_projects(&data).into_iter().find(|known| known.entry.id == project) else {
                    return;
                };
                let taken: Vec<String> = workspace::list_sessions(&known.folder)
                    .into_iter()
                    .filter(|session| session.trashed_at.is_none())
                    .map(|session| session.name)
                    .collect();
                match workspace::new_session(&known.folder, workspace::untitled_session_name(&taken)) {
                    Ok(session) => {
                        self.sessions.open = false;
                        self.switch_to(&project, Some(&session.id));
                    }
                    Err(reason) => self.tell(format!("A session could not be started: {reason}")),
                }
            }
            SessionChoice::ChooseLocation => {
                let picked = if self.native_dialogs {
                    crate::dialogs::pick_folder(frame)
                } else {
                    self.scripted_path()
                };
                if let Some(path) = picked {
                    self.sessions.chose_root(&path.display().to_string());
                }
            }
            SessionChoice::CreateProject { name, location } => {
                let name = name.trim();
                let name = if name.is_empty() { workspace::UNTITLED } else { name };
                match workspace::create_project_at(&data, Path::new(&location), name) {
                    Ok((record, session)) => {
                        self.sessions.open = false;
                        self.sessions.page = Page::Recent;
                        self.start_closing(Closing::Open(Opening {
                            id: record.id,
                            name: record.name,
                            folder: PathBuf::from(location),
                            session,
                        }));
                    }
                    Err(reason) => self.sessions.form_error = Some(reason),
                }
            }
            SessionChoice::RenameSession { project, session, name } => {
                let name = name.trim().to_string();
                if name.is_empty() {
                    return;
                }
                if self.project.as_ref().is_some_and(|open| open.session == session) {
                    if let Some(open) = self.project.as_mut() {
                        open.session_name = name;
                    }
                    self.write_project(true);
                } else if let Some(known) = workspace::known_projects(&data).into_iter().find(|known| known.entry.id == project)
                    && let Ok(mut file) = SessionFile::read(&known.folder, &session)
                {
                    file.name = name;
                    let _ = file.write(&known.folder);
                }
                self.refresh_sessions();
            }
            SessionChoice::RenameProject { project, name } => {
                let name = name.trim().to_string();
                if name.is_empty() {
                    return;
                }
                if let Some(known) = workspace::known_projects(&data).into_iter().find(|known| known.entry.id == project) {
                    let record = ProjectRecord { name: name.clone(), ..known.record };
                    if let Err(reason) = record.write(&known.folder) {
                        self.tell(format!("The project could not be renamed: {reason}"));
                    }
                }
                if let Some(open) = self.project.as_mut().filter(|open| open.id == project) {
                    open.name = name;
                }
                self.refresh_sessions();
            }
            SessionChoice::DeleteSession { project, session } => {
                let current = self.project.as_ref().is_some_and(|open| open.session == session);
                if current {
                    self.write_project(true);
                }
                if let Some(known) = workspace::known_projects(&data).into_iter().find(|known| known.entry.id == project)
                    && let Ok(mut file) = SessionFile::read(&known.folder, &session)
                {
                    file.trashed_at = Some(workspace::now_seconds());
                    let _ = file.write(&known.folder);
                }
                if current {
                    // The project's newest other session, or a new one.
                    self.sessions.open = false;
                    self.switch_to(&project, None);
                } else {
                    self.refresh_sessions();
                }
            }
            SessionChoice::DeleteProject { project } => {
                let _ = workspace::update_known(&data, &project, |entry| {
                    entry.trashed_at = Some(workspace::now_seconds());
                });
                if self.project.as_ref().is_some_and(|open| open.id == project) {
                    self.sessions.open = false;
                    self.open_next();
                } else {
                    self.refresh_sessions();
                }
            }
            SessionChoice::Restore(item) => {
                match item {
                    Trashed::Session { project, id } => {
                        if let Some(known) = workspace::known_projects(&data).into_iter().find(|known| known.entry.id == project)
                            && let Ok(mut file) = SessionFile::read(&known.folder, &id)
                        {
                            file.trashed_at = None;
                            let _ = file.write(&known.folder);
                        }
                    }
                    Trashed::Project(project) => {
                        let _ = workspace::update_known(&data, &project, |entry| entry.trashed_at = None);
                    }
                }
                self.refresh_sessions();
            }
            SessionChoice::EmptyTrash => self.flows.question = Some(Question::EmptyTrash),
            SessionChoice::LockProject { project, locked } => {
                if let Err(reason) = workspace::update_known(&data, &project, |entry| entry.locked = locked) {
                    self.tell(format!("The project could not be locked: {reason}"));
                }
                self.refresh_sessions();
            }
            SessionChoice::ArchiveProject { project, archived } => {
                let at = archived.then(workspace::now_seconds);
                if let Err(reason) = workspace::update_known(&data, &project, |entry| entry.archived_at = at) {
                    self.tell(format!("The project could not be archived: {reason}"));
                }
                if archived && self.project.as_ref().is_some_and(|open| open.id == project) {
                    self.sessions.open = false;
                    self.open_next();
                } else {
                    self.refresh_sessions();
                }
            }
            SessionChoice::PinProject { project, pinned } => {
                if let Err(reason) = workspace::update_known(&data, &project, |entry| entry.pinned = pinned) {
                    self.tell(format!("The project could not be pinned: {reason}"));
                }
                self.refresh_sessions();
            }
        }
    }

    /// Reads the menu's lists again: every session and project, and Trash.
    pub(super) fn refresh_sessions(&mut self) {
        let Some(data) = self.data.clone() else {
            return;
        };
        let now = workspace::now_seconds();
        let mut recent = Vec::new();
        let mut projects = Vec::new();
        let mut trash = Vec::new();
        for known in workspace::known_projects(&data) {
            let sessions = workspace::list_session_summaries(&known.folder);
            let live: Vec<&workspace::SessionSummary> = sessions.iter().filter(|session| session.trashed_at.is_none()).collect();
            if let Some(deleted) = known.entry.trashed_at {
                let count = live.len();
                trash.push((deleted, TrashRow {
                    item: Trashed::Project(known.entry.id.clone()),
                    name: known.record.name.clone(),
                    line: format!(
                        "Project · {} · Deleted {}",
                        if count == 1 { "1 session".to_string() } else { format!("{count} sessions") },
                        crate::sessions::ago(now, deleted).to_lowercase(),
                    ),
                }));
                continue;
            }
            for session in &sessions {
                match session.trashed_at {
                    Some(deleted) => trash.push((deleted, TrashRow {
                        item: Trashed::Session { project: known.entry.id.clone(), id: session.id.clone() },
                        name: session.name.clone(),
                        line: format!(
                            "Session · {} · Deleted {}",
                            known.record.name,
                            crate::sessions::ago(now, deleted).to_lowercase(),
                        ),
                    })),
                    None => recent.push(SessionRow {
                        project: known.entry.id.clone(),
                        project_name: known.record.name.clone(),
                        id: session.id.clone(),
                        name: session.name.clone(),
                        changed: session.updated_at,
                        folder: known.folder.display().to_string(),
                    }),
                }
            }
            projects.push(ProjectRow {
                id: known.entry.id.clone(),
                name: known.record.name.clone(),
                sessions: live.len(),
                used: known.entry.last_used,
                pinned: known.entry.pinned,
                locked: known.entry.locked,
                archived: known.entry.archived_at.is_some(),
            });
        }
        // Pinned projects first, then the most recently used.
        projects.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.used.cmp(&a.used)));
        recent.sort_by(|a, b| b.changed.cmp(&a.changed));
        trash.sort_by(|a, b| b.0.cmp(&a.0));
        self.sessions.recent = recent;
        self.sessions.projects = projects;
        self.sessions.trash = trash.into_iter().map(|(_, row)| row).collect();
        self.sessions.loaded = true;
        // The panel was drawn before the lists were read: show them now,
        // not at the next movement of the pointer.
        self.ctx.request_repaint();
        if self.sessions.form_root.is_empty()
            && let Some(root) = self.projects_root()
        {
            self.sessions.form_root = root.display().to_string();
        }
    }

    /// Opens that project's session, or its newest one, or a new one when it
    /// has none, closing the open session's tabs first.
    fn switch_to(&mut self, project: &str, session: Option<&str>) {
        if self.project.as_ref().is_some_and(|open| Some(open.session.as_str()) == session) {
            return;
        }
        let Some(data) = self.data.clone() else {
            return;
        };
        match self.opening(&data, project, session) {
            Ok(opening) => self.start_closing(Closing::Open(opening)),
            Err(reason) => self.tell(reason),
        }
    }

    /// A known project read, with the session to open: the one named, if it
    /// is not in Trash, or the newest, or a new one.
    fn opening(&self, data: &Path, project: &str, session: Option<&str>) -> Result<Opening, String> {
        let known = workspace::known_projects(data)
            .into_iter()
            .find(|known| known.entry.id == project)
            .ok_or_else(|| "That project could not be found.".to_string())?;
        let sessions: Vec<SessionFile> = workspace::list_sessions(&known.folder)
            .into_iter()
            .filter(|file| file.trashed_at.is_none())
            .collect();
        let chosen = session
            .and_then(|id| sessions.iter().find(|file| file.id == id).cloned())
            .or_else(|| sessions.first().cloned());
        let session = match chosen {
            Some(session) => session,
            None => workspace::new_session(&known.folder, workspace::UNTITLED_SESSION.to_string())?,
        };
        Ok(Opening {
            id: known.entry.id,
            name: known.record.name,
            folder: known.folder,
            session,
        })
    }

    /// After the open project went to Trash: the newest other project, or a
    /// new one made by itself.
    fn open_next(&mut self) {
        let Some(data) = self.data.clone() else {
            return;
        };
        let next = workspace::known_projects(&data)
            .into_iter()
            .find(|known| known.entry.trashed_at.is_none() && known.entry.archived_at.is_none())
            .map(|known| known.entry.id);
        match next.and_then(|id| self.opening(&data, &id, None).ok()) {
            Some(opening) => self.start_closing(Closing::Open(opening)),
            None => match self.made_by_itself(&data) {
                Ok(opening) => self.start_closing(Closing::Open(opening)),
                Err(reason) => self.tell(reason),
            },
        }
    }

    /// "Untitled Project" at `~/Inpainter/Untitled Project`, made with no
    /// question, because a person always has a project.
    fn made_by_itself(&self, data: &Path) -> Result<Opening, String> {
        let root = self.projects_root().ok_or_else(|| "There is no folder for projects.".to_string())?;
        let folder = workspace::default_project_folder(&root);
        let (record, session) = workspace::create_project_at(data, &folder, workspace::UNTITLED)
            .map_err(|reason| format!("A project could not be started: {reason}"))?;
        Ok(Opening {
            id: record.id,
            name: record.name,
            folder,
            session,
        })
    }

    /// Shows or hides the workspace panel, and keeps that with the session.
    pub(super) fn toggle_workspace(&mut self) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        project.workspace_open = !project.workspace_open;
        self.sent_viewport = None;
        self.write_project(true);
    }

    /// The person answered a sub-application's question. Its dismiss button
    /// during the quit flow cancels the flow: everything stays open.
    pub(super) fn note_answer(&mut self, ask: &Ask, button: &str) {
        if self.flows.closing.is_some() && ask.dismiss.as_deref() == Some(button) {
            self.stop_closing();
        }
    }

    /// A save dialog the quit opened was cancelled. The application stops its
    /// quit there, so the whole quit stops, as a dismissed question does.
    pub(super) fn note_save_cancelled(&mut self) {
        if self.flows.closing.is_some() {
            self.stop_closing();
        }
    }

    /// Everything stays open, as it was before the quit started.
    fn stop_closing(&mut self) {
        self.flows.closing = None;
        self.flows.kept = None;
        self.flows.save_paths.clear();
        for started in &mut self.apps {
            started.running.declared.quit_ready = false;
            started.running.declared.tab_log = None;
        }
    }

    /// Shows the host's own question, when it has one. Returns whether it did.
    pub(super) fn show_question(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) -> bool {
        let ask = match &self.flows.question {
            None => return false,
            Some(Question::Import { path, .. }) => Ask {
                id: "import".to_string(),
                title: IMPORT_TITLE.to_string(),
                lines: vec![import_line(path)],
                buttons: vec![button("import", "Import"), button("cancel", "Cancel")],
                dismiss: Some("cancel".to_string()),
            },
            Some(Question::EmptyTrash) => Ask {
                id: "empty-trash".to_string(),
                title: "Empty Trash?".to_string(),
                lines: vec![EMPTY_TRASH_LINE.to_string()],
                buttons: vec![button("empty", "Empty Trash"), button("cancel", "Cancel")],
                dismiss: Some("cancel".to_string()),
            },
        };
        if let Some(chosen) = crate::ask::show(ctx, QUESTION, &ask) {
            self.answer_question(&chosen);
        }
        true
    }

    /// The button chosen on the host's question.
    pub(super) fn answer_question(&mut self, chosen: &str) {
        if self.flows.question.is_none() {
            return;
        }
        if let Some(Question::Import { app, request, path }) = self.flows.question.take() {
            let folder = self.project.as_ref().map(|project| project.folder.clone());
            let copy = match (chosen, folder) {
                ("import", Some(folder)) => match workspace::import_copy(&path, &folder, "") {
                    Ok(copy) => Some(copy),
                    Err(reason) => {
                        self.tell(format!("{} could not be imported: {reason}", path.display()));
                        None
                    }
                },
                _ => None,
            };
            match copy {
                Some(copy) => self.claim_imported(app, request, copy),
                None => self.send(app, HostMessage::FileResult { id: request, path: None }),
            }
            return;
        }
        self.flows.question = None;
        if chosen == "empty"
            && let Some(data) = self.data.clone()
        {
            for line in workspace::empty_trash(&data) {
                self.tell(line);
            }
            self.refresh_sessions();
        }
    }

    /// Runs every frame: the window's close, the project's start, and the
    /// quit and restore flows in progress.
    pub(super) fn project_frame(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.settle_frame();
        self.close_frame(ctx);
        self.keeping_frame();
        self.closing_frame(ctx);
        self.restore_frame();
        self.write_project(false);
        let _ = frame;
    }

    fn settle_frame(&mut self) {
        match self.flows.settle {
            Settle::Waiting => {
                let declared = self.apps.iter().all(|started| !started.running.pending());
                if declared && self.flows.restore.is_none() && self.flows.closing.is_none() {
                    self.flows.settle = Settle::Grace(Instant::now() + SETTLE_GRACE);
                    self.rebaseline();
                }
            }
            Settle::Grace(until) => {
                self.rebaseline();
                if Instant::now() >= until {
                    self.flows.settle = Settle::Done;
                    // The starting tabs are part of the project, so the file
                    // matches them once they have arrived.
                    self.write_project(true);
                }
            }
            Settle::Done => {}
        }
    }

    /// The window's close: each sub-application resolves its unsaved work.
    /// The window closes once every one is ready or has exited, and the
    /// project is written then. A cancelled question leaves the window open.
    fn close_frame(&mut self, ctx: &egui::Context) {
        let requested = ctx.input(|input| input.viewport().close_requested());
        if !requested {
            return;
        }
        if self.locked() {
            self.assistant.send(Command::Stop);
        }
        let ready = self.apps_ready();
        match &self.flows.closing {
            Some(Closing::Quit { .. }) => {
                if !ready {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }
            }
            Some(_) => ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose),
            None => {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                if self.flows.question.is_none() && self.flows.restore.is_none() && self.flows.keeping.is_none() {
                    self.start_closing(Closing::Quit { closed: false });
                }
            }
        }
    }

    fn apps_ready(&self) -> bool {
        self.apps
            .iter()
            .all(|started| started.running.exited() || started.running.declared.quit_ready)
    }

    fn closing_frame(&mut self, ctx: &egui::Context) {
        if self.flows.closing.is_none() || !self.apps_ready() {
            return;
        }
        match self.flows.closing.take() {
            Some(Closing::Quit { closed: true }) => {
                self.flows.closing = Some(Closing::Quit { closed: true });
            }
            Some(Closing::Quit { closed: false }) => {
                self.write_kept();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                self.flows.closing = Some(Closing::Quit { closed: true });
            }
            Some(Closing::Open(opening)) => {
                self.write_kept();
                self.open_loaded(opening);
            }
            None => {}
        }
    }

    /// Every installed sub-application is started. One the project names is
    /// restored; one it does not name starts as it does at launch, so it is
    /// there when the workspace is shown.
    fn open_loaded(&mut self, opening: Opening) {
        let Opening { id, name, folder, session } = opening;
        let file = workspace::session_as_file(&folder, &name, &session);
        let installed = crate::apps::discover();
        let mut notes = Vec::new();
        let mut specs: Vec<AppSpec> = Vec::new();
        let mut restoring: Vec<bool> = Vec::new();
        let mut apps: Vec<RestoreApp> = Vec::new();
        let mut selected = None;
        for (index, tab) in file.tabs.iter().enumerate() {
            let known = self
                .launch
                .iter()
                .find(|spec| spec.id == tab.app)
                .cloned()
                .or_else(|| {
                    installed
                        .iter()
                        .find(|app| app.manifest.id == tab.app)
                        .map(|app| AppSpec {
                            id: app.manifest.id.clone(),
                            name: app.manifest.name.clone(),
                            executable: Some(app.executable.clone()),
                        })
                });
            let at = match specs.iter().position(|spec| spec.id == tab.app) {
                Some(at) => Some(at),
                None => match known {
                    Some(spec) => {
                        specs.push(spec);
                        restoring.push(true);
                        apps.push(RestoreApp {
                            tabs: Vec::new(),
                            opened: false,
                            ids: None,
                        });
                        Some(specs.len() - 1)
                    }
                    None => None,
                },
            };
            let Some(at) = at else {
                notes.push(note(format!(
                    "“{}” was left out: its application, {}, is not installed.",
                    tab.label, tab.app
                )));
                continue;
            };
            if let Some(file_path) = &tab.path
                && !Path::new(file_path).exists()
            {
                notes.push(note(format!(
                    "“{}” was left out: {file_path} no longer exists.",
                    tab.label
                )));
                continue;
            }
            if file.selected == Some(index) {
                selected = Some((at, apps[at].tabs.len()));
            }
            apps[at].tabs.push(RestoreTab {
                path: tab.path.clone(),
                camera: tab.camera,
            });
        }
        for (flag, wanted) in restoring.iter_mut().zip(&apps) {
            if wanted.tabs.is_empty() {
                *flag = false;
            }
        }
        for spec in self.launch.clone() {
            if specs.iter().any(|known| known.id == spec.id) {
                continue;
            }
            specs.push(spec);
            restoring.push(false);
            apps.push(RestoreApp {
                tabs: Vec::new(),
                opened: false,
                ids: None,
            });
        }
        self.restart(specs, &restoring, Some(&folder));
        if let Some(data) = &self.data
            && let Err(reason) = workspace::note_current(data, &id, &session.id)
        {
            notes.push(note(format!("The current session could not be noted: {reason}")));
        }
        self.project = Some(Project {
            id: id.clone(),
            name: file.name,
            folder,
            session: session.id,
            session_name: session.name,
            session_created: session.created_at,
            workspace_open: file.workspace_open,
        });
        // Each session keeps its own level; a new one starts on the default.
        self.level = session.level.clone();
        self.sessions.renaming = None;
        // The menu's lists are read now, so the panel opens complete.
        self.refresh_sessions();
        // What was being written or attached belongs to the session left.
        self.chat.chips.clear();
        self.chat.input.clear();
        self.sent_viewport = None;
        self.failed_signature = None;
        self.chat.conversation.clear();
        let mut conversation = file.conversation;
        conversation.extend(notes);
        self.assistant.send(Command::Restore(conversation));
        self.flows.restore = Some(Restore {
            apps,
            selected: selected.or(Some((0, 0))),
            started: Instant::now(),
            mapped: false,
            cameras: Vec::new(),
        });
    }

    /// At start: projects from before 0.7 move into folders of their own,
    /// then the current session opens, or the newest project's newest
    /// session, or a project made by itself when there is none.
    pub(super) fn open_current(&mut self) {
        let Some(data) = self.data.clone() else {
            return;
        };
        let mut lines = Vec::new();
        if let Some(root) = self.projects_root() {
            match workspace::migrate_old_projects_to(&data, &root) {
                Ok(notes) => lines.extend(notes),
                Err(reason) => lines.push(format!("Older projects could not be moved: {reason}")),
            }
        }
        let list = workspace::ProjectsList::load(&data).unwrap_or_default();
        let known = workspace::known_projects(&data);
        let live = |id: &str| {
            known
                .iter()
                .any(|known| known.entry.id == id && known.entry.trashed_at.is_none() && known.entry.archived_at.is_none())
        };
        let wanted = list
            .current_project
            .clone()
            .filter(|id| live(id))
            .or_else(|| {
                known
                    .iter()
                    .find(|known| known.entry.trashed_at.is_none() && known.entry.archived_at.is_none())
                    .map(|known| known.entry.id.clone())
            });
        let opening = match wanted {
            Some(id) => self.opening(&data, &id, list.current_session.as_deref()),
            None => self.made_by_itself(&data),
        };
        match opening {
            Ok(opening) => self.open_loaded(opening),
            Err(reason) => lines.push(reason),
        }
        for line in lines {
            self.tell(line);
        }
    }

    /// Stops every sub-application and starts these instead. `restoring` says
    /// which ones wait to be given their tabs, instead of opening their own.
    fn restart(&mut self, specs: Vec<AppSpec>, restoring: &[bool], folder: Option<&Path>) {
        for started in std::mem::take(&mut self.apps) {
            started.running.shutdown();
        }
        self.apps = specs
            .into_iter()
            .enumerate()
            .map(|(index, spec)| {
                let restore = restoring.get(index).copied().unwrap_or(false);
                let dir = self
                    .data
                    .as_ref()
                    .map(|data| data.join("apps").join(&spec.id))
                    .or_else(|| crate::apps::data_dir(&spec.id));
                let mut running = match &spec.executable {
                    Some(executable) => Running::start(executable, dir, restore),
                    None => Running::not_started(),
                };
                running.attach(&self.ctx);
                // The project's folder before anything else, so the tabs it
                // reopens and every new activity are inside it.
                if let Some(folder) = folder {
                    running.send(&HostMessage::Project {
                        folder: folder.display().to_string(),
                    });
                }
                Started {
                    id: spec.id,
                    name: spec.name,
                    executable: spec.executable,
                    running,
                }
            })
            .collect();
        self.selected = 0;
        self.sent_viewport = None;
        self.shown = None;
        self.surface.reset();
        self.live.reset();
        self.panel_open = false;
        self.routing = Default::default();
        self.flows.settle = Settle::Waiting;
    }

    /// Every open tab is asked to write its work first. Only when every one
    /// has, or its application does not answer, do the tabs close; a tab
    /// that could not be written stops the switch or quit, so nothing is
    /// lost.
    fn start_closing(&mut self, closing: Closing) {
        let mut waiting = Vec::new();
        for (index, started) in self.apps.iter_mut().enumerate() {
            if !started.running.answers() {
                continue;
            }
            started.running.declared.kept.clear();
            // Only a tab that saves to a file has anything to write.
            for tab in started.running.tabs().0.into_iter().filter(|tab| tab.saves) {
                started.running.send(&HostMessage::Keep { tab: tab.id.clone() });
                waiting.push((index, tab.id));
            }
        }
        if waiting.is_empty() {
            self.close_all(closing);
            return;
        }
        self.flows.keeping = Some(Keeping {
            closing,
            waiting,
            started: Instant::now(),
        });
    }

    /// Follows the answers to `keep`, then closes, or stops on a failure.
    fn keeping_frame(&mut self) {
        let Some(mut job) = self.flows.keeping.take() else {
            return;
        };
        let mut failed = Vec::new();
        for (index, started) in self.apps.iter_mut().enumerate() {
            while let Some((tab, _, error)) = started.running.declared.kept.pop_front() {
                job.waiting.retain(|(app, id)| !(*app == index && *id == tab));
                if let Some(error) = error {
                    let label = started
                        .running
                        .tabs()
                        .0
                        .into_iter()
                        .find(|entry| entry.id == tab)
                        .map(|entry| entry.label)
                        .unwrap_or(tab);
                    failed.push(format!("“{label}” could not be written: {error}"));
                }
            }
        }
        job.waiting.retain(|(app, _)| self.apps.get(*app).is_some_and(|started| !started.running.exited()));
        if !failed.is_empty() {
            for line in failed {
                self.tell(format!("{line} Nothing was closed, so nothing is lost."));
            }
            return;
        }
        if job.waiting.is_empty() || job.started.elapsed() >= KEEP_WAIT {
            self.close_all(job.closing);
            return;
        }
        self.flows.keeping = Some(job);
    }

    /// Every installed sub-application resolves its unsaved work, as when the
    /// window closes. Each is asked again, even one that was ready before.
    fn close_all(&mut self, closing: Closing) {
        self.flows.kept = Some(self.snapshot_tabs());
        self.flows.save_paths.clear();
        for started in &mut self.apps {
            started.running.declared.quit_ready = false;
            started.running.declared.tab_log = Some(Vec::new());
            started.running.send(&HostMessage::Quit);
        }
        self.flows.closing = Some(closing);
    }

    /// Moves a restore on: once a restarted sub-application has declared
    /// itself, it is asked to open each tab; once its tabs are reported,
    /// they are matched, and the selection and cameras are put back.
    fn restore_frame(&mut self) {
        let Some(mut job) = self.flows.restore.take() else {
            return;
        };
        let late = job.started.elapsed() >= RESTORE_WAIT;
        for (index, wanted) in job.apps.iter_mut().enumerate() {
            let Some(started) = self.apps.get_mut(index) else {
                wanted.opened = true;
                wanted.ids = Some(vec![None; wanted.tabs.len()]);
                continue;
            };
            if !wanted.opened && !started.running.pending() {
                for tab in &wanted.tabs {
                    started.running.send(&HostMessage::Open {
                        path: tab.path.clone(),
                    });
                }
                wanted.opened = true;
            }
            if wanted.opened && wanted.ids.is_none() {
                let reported = started.running.tabs().0;
                if reported.len() >= wanted.tabs.len() || started.running.exited() || late {
                    wanted.ids = Some(match_tabs(&wanted.tabs, &reported));
                }
            }
        }
        if !job.mapped && job.apps.iter().all(|wanted| wanted.ids.is_some()) {
            job.mapped = true;
            for (index, wanted) in job.apps.iter().enumerate() {
                let ids = wanted.ids.as_deref().unwrap_or_default();
                for (tab, id) in wanted.tabs.iter().zip(ids) {
                    if let (Some(camera), Some(id)) = (tab.camera, id) {
                        job.cameras.push((index, id.clone(), camera));
                    }
                }
            }
            if let Some((app, nth)) = job.selected
                && let Some(Some(id)) = job
                    .apps
                    .get(app)
                    .and_then(|wanted| wanted.ids.as_ref())
                    .and_then(|ids| ids.get(nth))
            {
                self.selected = app;
                let viewport = self.viewport;
                self.send(
                    app,
                    HostMessage::Command {
                        id: "select_tab".to_string(),
                        tab: Some(id.clone()),
                        arg: None,
                        node: None,
                        viewport,
                    },
                );
            }
        }
        if job.mapped {
            let mut waiting = Vec::new();
            for (app, tab, view) in std::mem::take(&mut job.cameras) {
                let Some(started) = self.apps.get_mut(app) else {
                    continue;
                };
                match started.running.declared.documents.get_mut(&tab) {
                    Some(display) => {
                        display.camera.origin = view.origin;
                        display.camera.zoom = view.zoom;
                        started.running.send(&HostMessage::Camera {
                            tab,
                            origin: view.origin,
                            zoom: view.zoom,
                        });
                    }
                    None if job.started.elapsed() < 2 * RESTORE_WAIT => {
                        waiting.push((app, tab, view));
                    }
                    None => {}
                }
            }
            job.cameras = waiting;
            if job.cameras.is_empty() {
                // Restored: what arrives now is the project as it was kept.
                self.flows.settle = Settle::Waiting;
                return;
            }
        }
        self.flows.restore = Some(job);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reported(id: &str, path: Option<&str>) -> protocol::TabEntry {
        protocol::TabEntry {
            id: id.to_string(),
            label: id.to_string(),
            dirty: false,
            close_waits: false,
            path: path.map(str::to_string),
            saves: true,
        }
    }

    fn wanted(path: Option<&str>) -> RestoreTab {
        RestoreTab {
            path: path.map(str::to_string),
            camera: None,
        }
    }

    /// Catches reopened tabs matched to the wrong file when the
    /// sub-application reports them in another order.
    #[test]
    fn reopened_tabs_match_by_file_then_in_order() {
        let ids = match_tabs(
            &[wanted(Some("/b")), wanted(None), wanted(Some("/a"))],
            &[
                reported("1", Some("/a")),
                reported("2", None),
                reported("3", Some("/b")),
            ],
        );
        assert_eq!(
            ids,
            vec![Some("3".to_string()), Some("2".to_string()), Some("1".to_string())]
        );
    }
}
