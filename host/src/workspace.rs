//! The project store and the host's settings.
//!
//! A project keeps one conversation and the files sub-applications have open:
//! the tabs in order, which one is selected, each tab's camera, the folders
//! the person attached, and whether the workspace is shown. It refers to the
//! files and never holds their work. The conversation's entries never hold a
//! tool's input or output.
//!
//! Projects live in `<Inpainter data folder>/projects/<id>/project.json`. The
//! id is chosen once, when the project is created, and a rename does not
//! change it. The settings remember the model and which project is current.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

/// The version this host writes and reads.
pub(crate) const VERSION: u32 = 1;
/// A project's name until the person renames it, and the next ones when that
/// name is already taken.
pub(crate) const UNTITLED: &str = "Untitled Project";
/// The file each project is stored as.
const FILE_NAME: &str = "project.json";
const PROJECTS_FILE: &str = "projects.json";
const PROJECT_META_DIR: &str = ".inpainter";
const SESSIONS_DIR: &str = "sessions";
/// The lock that keeps Studio to one window.
const LOCK_NAME: &str = "lock";

/// What a second start should tell the person, when the lock is held.
pub(crate) const ALREADY_OPEN: &str = "Inpainter is already open.";

/// The project file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ProjectFile {
    pub version: u32,
    pub name: String,
    /// Absolute paths the person attached, in the order attached.
    #[serde(default)]
    pub folders: Vec<String>,
    /// The tab strip, in order.
    pub tabs: Vec<ProjectTab>,
    /// The selected tab's index in `tabs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<usize>,
    /// The workspace is shown beside the conversation.
    #[serde(default)]
    pub workspace_open: bool,
    #[serde(default)]
    pub conversation: Vec<assistant::Entry>,
}

/// One tab: which sub-application shows which file, and where it looks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ProjectTab {
    /// The sub-application's manifest id.
    pub app: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<View>,
}

/// A tab's camera: the top-left point it shows and its zoom.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct View {
    pub origin: [f64; 2],
    pub zoom: f64,
}

/// Every project Studio knows, and what is current.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ProjectsList {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<KnownProject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_session: Option<String>,
}

/// One project entry in `projects.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct KnownProject {
    pub id: String,
    pub folder: String,
    pub last_used: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trashed_at: Option<u64>,
    /// Kept at the top of the menu's list, above the most recently used.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,    /// Its name, Trash, and archiving are held, and so is deleting its
    /// sessions, so nothing is lost by accident.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// When it was archived: off the menu's lists, never deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<u64>,
}

/// `.inpainter/project.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ProjectRecord {
    pub version: u32,
    pub id: String,
    pub name: String,
}

/// `.inpainter/sessions/<id>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SessionFile {
    pub version: u32,
    pub id: String,
    pub name: String,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trashed_at: Option<u64>,
    #[serde(default)]
    pub tabs: Vec<SessionTab>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<usize>,
    #[serde(default)]
    pub workspace_open: bool,
    #[serde(default)]
    pub conversation: Vec<assistant::Entry>,
    /// The level this session's composer chose. None means the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
}

/// A tab entry in a session file, with a project-relative path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SessionTab {
    pub app: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<View>,
}

/// Why a project file could not be read.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ReadError {
    /// A newer file than this host understands.
    Version(u32),
    Other(String),
}

impl ProjectFile {
    pub(crate) fn parse(text: &str) -> Result<Self, ReadError> {
        let file: Self = serde_json::from_str(text).map_err(|error| ReadError::Other(error.to_string()))?;
        if file.version != VERSION {
            return Err(ReadError::Version(file.version));
        }
        Ok(file)
    }

    pub(crate) fn read(path: &Path) -> Result<Self, ReadError> {
        let text = std::fs::read_to_string(path).map_err(|error| ReadError::Other(error.to_string()))?;
        Self::parse(&text)
    }

}

fn to_text<T: Serialize>(value: &T) -> String {
    let mut text = serde_json::to_string_pretty(value).expect("value always encodes as JSON");
    text.push('\n');
    text
}

fn write_atomic(path: &Path, text: String) -> Result<(), String> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    }
    let mut partial = path.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    std::fs::write(&partial, text).map_err(|error| error.to_string())?;
    std::fs::rename(&partial, path).map_err(|error| {
        let _ = std::fs::remove_file(&partial);
        error.to_string()
    })
}

impl ProjectsList {
    pub(crate) fn path(data: &Path) -> PathBuf {
        data.join(PROJECTS_FILE)
    }

    pub(crate) fn load(data: &Path) -> Result<Self, String> {
        let path = Self::path(data);
        if !path.exists() {
            return Ok(Self {
                version: VERSION,
                ..Self::default()
            });
        }
        let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        let mut list = serde_json::from_str::<Self>(&text).map_err(|error| error.to_string())?;
        if list.version == 0 {
            list.version = VERSION;
        }
        Ok(list)
    }

    pub(crate) fn write(&self, data: &Path) -> Result<(), String> {
        write_atomic(&Self::path(data), to_text(self))
    }
}

impl ProjectRecord {
    pub(crate) fn parse(text: &str) -> Result<Self, ReadError> {
        let record: Self =
            serde_json::from_str(text).map_err(|error| ReadError::Other(error.to_string()))?;
        if record.version != VERSION {
            return Err(ReadError::Version(record.version));
        }
        Ok(record)
    }

    pub(crate) fn read(project_folder: &Path) -> Result<Self, ReadError> {
        let text = std::fs::read_to_string(project_record_path(project_folder))
            .map_err(|error| ReadError::Other(error.to_string()))?;
        Self::parse(&text)
    }

    pub(crate) fn write(&self, project_folder: &Path) -> Result<(), String> {
        write_atomic(&project_record_path(project_folder), to_text(self))
    }
}

impl SessionFile {
    pub(crate) fn parse(text: &str) -> Result<Self, ReadError> {
        let session: Self =
            serde_json::from_str(text).map_err(|error| ReadError::Other(error.to_string()))?;
        if session.version != VERSION {
            return Err(ReadError::Version(session.version));
        }
        Ok(session)
    }

    pub(crate) fn read(project_folder: &Path, id: &str) -> Result<Self, ReadError> {
        let text = std::fs::read_to_string(session_path(project_folder, id))
            .map_err(|error| ReadError::Other(error.to_string()))?;
        Self::parse(&text)
    }

    pub(crate) fn write(&self, project_folder: &Path) -> Result<(), String> {
        write_atomic(&session_path(project_folder, &self.id), to_text(self))
    }
}

/// A line for a project file this host does not understand.
pub(crate) fn version_line(version: u32) -> String {
    format!(
        "It is version {version} of the project file, and this Inpainter reads version {VERSION}."
    )
}

/// Seconds since the Unix epoch, as conversation entries keep time.
pub(crate) fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// `<Inpainter data folder>`. On this computer that is `~/.local/share/inpainter`.
pub(crate) fn data_folder() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("io.github", "spumcake", "Inpainter")?;
    Some(dirs.data_dir().to_path_buf())
}

/// `~/Inpainter`, where a project's folder is created by default.
pub(crate) fn default_projects_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Inpainter"))
}

pub(crate) fn projects_dir(data: &Path) -> PathBuf {
    data.join("projects")
}

pub(crate) fn project_dir(data: &Path, id: &str) -> PathBuf {
    projects_dir(data).join(id)
}

pub(crate) fn project_path(data: &Path, id: &str) -> PathBuf {
    project_dir(data, id).join(FILE_NAME)
}

pub(crate) fn project_meta_dir(project_folder: &Path) -> PathBuf {
    project_folder.join(PROJECT_META_DIR)
}

pub(crate) fn project_record_path(project_folder: &Path) -> PathBuf {
    project_meta_dir(project_folder).join(FILE_NAME)
}

pub(crate) fn sessions_dir(project_folder: &Path) -> PathBuf {
    project_meta_dir(project_folder).join(SESSIONS_DIR)
}

pub(crate) fn session_path(project_folder: &Path, id: &str) -> PathBuf {
    sessions_dir(project_folder).join(format!("{id}.json"))
}

/// The folder a project's own files are saved in. Created with the project
/// and removed with it, because it lives inside the project folder.
pub(crate) fn project_files(data: &Path, id: &str) -> PathBuf {
    project_dir(data, id).join("files")
}

/// The directory a file dialog opens in. A request that names a folder uses
/// that. Otherwise the project's own files folder, when there is one.
pub(crate) fn dialog_directory(requested: Option<&Path>, files: Option<&Path>) -> Option<PathBuf> {
    requested
        .map(Path::to_path_buf)
        .or_else(|| files.map(Path::to_path_buf))
}

/// The lock file. The system releases it when the process ends, even in a crash.
pub(crate) fn lock_path(data: &Path) -> PathBuf {
    data.join(LOCK_NAME)
}

/// Held for the life of the process. Dropping it releases the lock.
pub(crate) struct InstanceLock {
    _file: File,
}

/// Why the lock could not be taken.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum LockFail {
    /// Another process holds it.
    Busy,
    /// The data folder could not be used.
    Unavailable,
}

/// Takes the lock in the data folder. A file left behind with nothing holding
/// it does not count as busy.
pub(crate) fn acquire_lock() -> Result<InstanceLock, LockFail> {
    let Some(folder) = data_folder() else {
        return Err(LockFail::Unavailable);
    };
    try_lock(&lock_path(&folder))
}

/// The words a second start shows, when the lock is the reason it stops.
pub(crate) fn second_start_message(fail: LockFail) -> Option<&'static str> {
    match fail {
        LockFail::Busy => Some(ALREADY_OPEN),
        LockFail::Unavailable => None,
    }
}

/// Locks `path`, creating it and its folder. `Err(Busy)` when another open
/// handle holds it.
pub(crate) fn try_lock(path: &Path) -> Result<InstanceLock, LockFail> {
    if let Some(folder) = path.parent()
        && std::fs::create_dir_all(folder).is_err()
    {
        return Err(LockFail::Unavailable);
    }
    let file = match OpenOptions::new().read(true).write(true).create(true).open(path) {
        Ok(file) => file,
        Err(_) => return Err(LockFail::Unavailable),
    };
    match file.try_lock() {
        Ok(()) => Ok(InstanceLock { _file: file }),
        Err(TryLockError::WouldBlock) => Err(LockFail::Busy),
        Err(TryLockError::Error(_)) => Err(LockFail::Unavailable),
    }
}

/// An id that is one path component, so a project folder cannot escape the
/// projects directory.
fn single_component(id: &str) -> bool {
    !id.is_empty()
        && !id.contains('/')
        && !id.contains('\\')
        && id != "."
        && id != ".."
        && !id.contains('\0')
}

fn clean_relative(path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        return None;
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::CurDir => {}
            Component::Normal(part) => clean.push(part),
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => return None,
        }
    }
    if clean.as_os_str().is_empty() {
        return None;
    }
    Some(clean)
}

fn as_slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) fn resolve_in_project(project_folder: &Path, relative: &str) -> Option<PathBuf> {
    let clean = clean_relative(Path::new(relative))?;
    Some(project_folder.join(clean))
}

pub(crate) fn relative_to_project(project_folder: &Path, path: &Path) -> Option<String> {
    let project = project_folder.canonicalize().ok()?;
    let absolute = if path.is_absolute() {
        path.canonicalize().ok()?
    } else {
        project.join(path).canonicalize().ok()?
    };
    let relative = absolute.strip_prefix(&project).ok()?;
    let clean = clean_relative(relative)?;
    Some(as_slash(&clean))
}

fn unique_path(folder: &Path, stem: &str, ext: Option<&str>) -> PathBuf {
    let stem = stem.trim();
    let stem = if stem.is_empty() { "Untitled" } else { stem };
    let mut index = 2u32;
    let plain = folder.join(match ext {
        Some(ext) if !ext.is_empty() => format!("{stem}.{ext}"),
        _ => stem.to_string(),
    });
    if !plain.exists() {
        return plain;
    }
    loop {
        let name = format!("{stem} {index}");
        let file_name = match ext {
            Some(ext) if !ext.is_empty() => format!("{name}.{ext}"),
            _ => name,
        };
        let candidate = folder.join(file_name);
        if !candidate.exists() {
            return candidate;
        }
        index += 1;
    }
}

fn fresh_session_id(project_folder: &Path) -> String {
    loop {
        let id = protocol::cell_id();
        if !session_path(project_folder, &id).exists() {
            return id;
        }
    }
}

fn copy_into(
    source: &Path,
    target_folder: &Path,
    wanted_relative: Option<&Path>,
) -> Result<String, String> {
    if !source.is_file() {
        return Err("it does not exist as a file".to_string());
    }
    let mut target = if let Some(relative) = wanted_relative.and_then(clean_relative) {
        target_folder.join(relative)
    } else {
        let stem = source
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Imported");
        let ext = source.extension().and_then(|ext| ext.to_str());
        unique_path(target_folder, stem, ext)
    };
    if target.exists() {
        let parent = target.parent().unwrap_or(target_folder);
        let stem = target.file_stem().and_then(|stem| stem.to_str()).unwrap_or("Imported");
        let ext = target.extension().and_then(|ext| ext.to_str());
        target = unique_path(parent, stem, ext);
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::copy(source, &target).map_err(|error| error.to_string())?;
    let relative = target
        .strip_prefix(target_folder)
        .map_err(|error| error.to_string())?;
    Ok(as_slash(relative))
}

fn copy_tree_into(
    source_folder: &Path,
    target_folder: &Path,
) -> Result<HashMap<PathBuf, String>, String> {
    let mut mapped = HashMap::new();
    if !source_folder.is_dir() {
        return Ok(mapped);
    }
    let mut stack = vec![source_folder.to_path_buf()];
    while let Some(folder) = stack.pop() {
        for entry in std::fs::read_dir(folder).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let wanted = path
                .strip_prefix(source_folder)
                .map_err(|error| error.to_string())?;
            let relative = copy_into(&path, target_folder, Some(wanted))?;
            let source = path.canonicalize().unwrap_or(path.clone());
            mapped.insert(source, relative);
        }
    }
    Ok(mapped)
}

fn find_migrated_folder(projects_root: &Path, id: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(projects_root).ok()?;
    for entry in entries.filter_map(|entry| entry.ok()) {
        let folder = entry.path();
        if !folder.is_dir() {
            continue;
        }
        if ProjectRecord::read(&folder).ok().is_some_and(|record| record.id == id) {
            return Some(folder);
        }
    }
    None
}

fn migration_note(line: String) -> assistant::Entry {
    assistant::Entry {
        role: assistant::Role::Note,
        text: line,
        time: now_seconds(),
        attachments: Vec::new(),
        tool: None,
        ok: None,
        cell: None,
        aside: true,
    }
}

pub(crate) fn migrate_old_projects_to(
    data: &Path,
    projects_root: &Path,
) -> Result<Vec<String>, String> {
    let old_root = projects_dir(data);
    if !old_root.is_dir() {
        return Ok(Vec::new());
    }
    std::fs::create_dir_all(projects_root).map_err(|error| error.to_string())?;
    let mut list = ProjectsList::load(data)?;
    list.version = VERSION;
    let mut known = HashSet::new();
    for project in &list.projects {
        known.insert(project.id.clone());
    }
    let mut notes = Vec::new();
    for entry in std::fs::read_dir(&old_root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if !single_component(&id) || known.contains(&id) {
            continue;
        }
        let old_project_path = project_path(data, &id);
        if !old_project_path.is_file() {
            continue;
        }
        let old = match ProjectFile::read(&old_project_path) {
            Ok(file) => file,
            Err(ReadError::Version(version)) => {
                notes.push(version_line(version));
                continue;
            }
            Err(ReadError::Other(reason)) => {
                notes.push(format!("“{id}” was not moved: {reason}"));
                continue;
            }
        };
        let target_name = if old.name.trim().is_empty() {
            UNTITLED.to_string()
        } else {
            old.name.clone()
        };
        let target_folder = find_migrated_folder(projects_root, &id)
            .unwrap_or_else(|| unique_path(projects_root, &target_name, None));
        if let Err(error) = std::fs::create_dir_all(project_meta_dir(&target_folder)) {
            notes.push(format!("“{}” was not moved: {}", old.name, error));
            continue;
        }
        let old_files = project_files(data, &id).canonicalize().ok();
        let copied_files = match copy_tree_into(&project_files(data, &id), &target_folder) {
            Ok(mapped) => mapped,
            Err(error) => {
                notes.push(format!("“{}” was not moved: {error}", old.name));
                continue;
            }
        };
        let mut copied_tabs: HashMap<PathBuf, String> = HashMap::new();
        let mut missing_tabs = Vec::new();
        let mut copied_session_tabs = Vec::new();
        let mut selected = None;
        let mut tab_copy_failed = false;
        for (index, tab) in old.tabs.iter().enumerate() {
            let Some(path) = &tab.path else {
                if old.selected == Some(index) {
                    selected = Some(copied_session_tabs.len());
                }
                copied_session_tabs.push(SessionTab {
                    app: tab.app.clone(),
                    path: None,
                    label: tab.label.clone(),
                    camera: tab.camera,
                });
                continue;
            };
            let source = Path::new(path);
            let canonical = match source.canonicalize() {
                Ok(path) => path,
                Err(_) => {
                    missing_tabs.push(format!("“{}” was left out: {path} no longer exists.", tab.label));
                    continue;
                }
            };
            let mapped = if let Some(files_root) = &old_files {
                if canonical.starts_with(files_root) {
                    copied_files.get(&canonical).cloned()
                } else {
                    None
                }
            } else {
                None
            };
            let copied = if let Some(mapped) = mapped {
                mapped
            } else if let Some(mapped) = copied_tabs.get(&canonical) {
                mapped.clone()
            } else {
                match copy_into(&canonical, &target_folder, None) {
                    Ok(relative) => {
                        copied_tabs.insert(canonical.clone(), relative.clone());
                        relative
                    }
                    Err(error) => {
                        notes.push(format!("“{}” was not moved: {error}", old.name));
                        tab_copy_failed = true;
                        break;
                    }
                }
            };
            copied_session_tabs.push(SessionTab {
                app: tab.app.clone(),
                path: Some(copied),
                label: tab.label.clone(),
                camera: tab.camera,
            });
            if old.selected == Some(index) {
                selected = Some(copied_session_tabs.len() - 1);
            }
        }
        if tab_copy_failed {
            continue;
        }
        let mut conversation = old.conversation.clone();
        for line in missing_tabs {
            conversation.push(migration_note(line));
        }
        let attachments_folder = target_folder.join("attachments");
        let mut attachment_failed = None::<String>;
        let mut missing_attachments = Vec::new();
        for conversation_entry in &mut conversation {
            let mut rewritten = Vec::new();
            for attachment in &conversation_entry.attachments {
                let attachment_path = Path::new(attachment);
                if !attachment_path.is_file() {
                    missing_attachments.push(format!(
                        "An attachment was left out while moving “{}”: {attachment} no longer exists.",
                        old.name
                    ));
                    continue;
                }
                match copy_into(attachment_path, &attachments_folder, None) {
                    Ok(copied) => rewritten.push(format!("attachments/{copied}")),
                    Err(error) => {
                        attachment_failed = Some(error);
                        break;
                    }
                }
            }
            if attachment_failed.is_some() {
                break;
            }
            conversation_entry.attachments = rewritten;
        }
        for line in missing_attachments {
            conversation.push(migration_note(line));
        }
        if let Some(error) = attachment_failed {
            notes.push(format!("“{}” was not moved: {error}", old.name));
            continue;
        }
        let session_id = fresh_session_id(&target_folder);
        let session = SessionFile {
            version: VERSION,
            id: session_id.clone(),
            name: "Untitled Session".to_string(),
            created_at: now_seconds(),
            updated_at: now_seconds(),
            trashed_at: None,
            tabs: copied_session_tabs,
            selected,
            workspace_open: old.workspace_open,
            conversation,
            level: None,
        };
        let record = ProjectRecord {
            version: VERSION,
            id: id.clone(),
            name: old.name.clone(),
        };
        if let Err(error) = record
            .write(&target_folder)
            .and_then(|_| session.write(&target_folder))
        {
            notes.push(format!("“{}” was not moved: {error}", old.name));
            continue;
        }
        list.projects.push(KnownProject {
            id: id.clone(),
            folder: target_folder.display().to_string(),
            last_used: now_seconds(),
            trashed_at: None,
            pinned: false,
            locked: false,
            archived_at: None,
        });
        known.insert(id.clone());
        let previous_project = list.current_project.clone();
        let previous_session = list.current_session.clone();
        if previous_project.is_none() {
            list.current_project = Some(id.clone());
            list.current_session = Some(session_id);
        }
        if let Err(error) = list.write(data) {
            list.projects.pop();
            list.current_project = previous_project;
            list.current_session = previous_session;
            notes.push(format!("“{}” was not moved: {error}", old.name));
            continue;
        }
        let backup_root = data.join("projects-before-0.7");
        if let Err(error) = std::fs::create_dir_all(&backup_root) {
            notes.push(format!(
                "“{}” moved but backup folder failed: {error}",
                old.name
            ));
            continue;
        }
        if let Err(error) = std::fs::rename(project_dir(data, &id), backup_root.join(&id)) {
            notes.push(format!(
                "“{}” moved but old backup rename failed: {error}",
                old.name
            ));
        }
    }
    Ok(notes)
}

/// What the host remembers between runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Settings {
    /// The current project's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The conversation panel's width beside the workspace, as the person
    /// last dragged it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_width: Option<f32>,
    /// Where New Project suggests new projects, and where Studio makes one
    /// by itself. None means `~/Inpainter`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects_root: Option<String>,
    /// Where Studio sends prompts. Their keys are never here: see `keys.rs`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoints: Vec<EndpointSetting>,
    /// The skills, the built-in Discussion first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillSetting>,
    /// The assistant's levels, in the picker's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub levels: Vec<Level>,
    /// The level a new session starts on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_level: Option<String>,
    /// Anything another Studio wrote, such as the model picked before 0.8,
    /// kept as it was.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
    /// Where these settings live. None in frame tests, which never write.
    #[serde(skip)]
    file: Option<PathBuf>,
}

/// What kind of service an endpoint is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EndpointKind {
    Openrouter,
    Openai,
    Comfyui,
}

impl EndpointKind {
    pub(crate) const ALL: [EndpointKind; 3] = [EndpointKind::Openrouter, EndpointKind::Openai, EndpointKind::Comfyui];

    pub(crate) fn label(self) -> &'static str {
        match self {
            EndpointKind::Openrouter => "OpenRouter",
            EndpointKind::Openai => "OpenAI-compatible",
            EndpointKind::Comfyui => "ComfyUI",
        }
    }

    /// What the form fills in for a new endpoint of this kind.
    pub(crate) fn suggested(self) -> (&'static str, &'static str) {
        match self {
            EndpointKind::Openrouter => ("OpenRouter", "https://openrouter.ai/api/v1"),
            EndpointKind::Openai => ("Ollama", "http://localhost:11434/v1"),
            EndpointKind::Comfyui => ("Local ComfyUI", "http://127.0.0.1:8188"),
        }
    }

    /// A chat endpoint answers prompts; ComfyUI runs graphs.
    pub(crate) fn chats(self) -> bool {
        self != EndpointKind::Comfyui
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct EndpointSetting {
    pub id: String,
    pub name: String,
    pub kind: EndpointKind,
    pub address: String,
}

/// The built-in skill's id.
pub(crate) const DISCUSSION: &str = "discussion";

/// What the Discussion skill's prompt starts as: how the assistant speaks,
/// apart from Studio's own rules for its tools.
pub(crate) const DEFAULT_VOICE: &str = "\
Speak plainly and briefly, one idea per sentence.
Say what you did, or what you found, then what the person can do next.";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SkillSetting {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// An endpoint's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// The model it runs on, for a skill other than Discussion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub prompt: String,
}

/// How hard a level thinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Effort {
    Low,
    Medium,
    High,
}

impl Effort {
    pub(crate) fn label(effort: Option<Effort>) -> &'static str {
        match effort {
            None => "No effort set",
            Some(Effort::Low) => "Low effort",
            Some(Effort::Medium) => "Medium effort",
            Some(Effort::High) => "High effort",
        }
    }

    pub(crate) fn from_word(word: &str) -> Option<Effort> {
        match word {
            "low" => Some(Effort::Low),
            "medium" => Some(Effort::Medium),
            "high" => Some(Effort::High),
            _ => None,
        }
    }

    pub(crate) fn word(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Level {
    pub id: String,
    pub name: String,
    /// An endpoint's id.
    #[serde(default)]
    pub endpoint: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
}

impl Settings {
    /// `<Inpainter data folder>/settings.json`.
    pub(crate) fn default_file() -> Option<PathBuf> {
        Some(data_folder()?.join("settings.json"))
    }

    /// The settings in `file`, or none remembered yet.
    pub(crate) fn load(file: Option<PathBuf>) -> Self {
        let mut settings = file
            .as_deref()
            .and_then(|file| std::fs::read_to_string(file).ok())
            .and_then(|text| serde_json::from_str::<Settings>(&text).ok())
            .unwrap_or_default();
        settings.file = file;
        settings
    }

    fn write(&self) {
        let Some(file) = &self.file else {
            return;
        };
        if let Some(folder) = file.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(file, text);
        }
    }

    /// Another run may have changed the file since this one read it, so
    /// read it again, make this one change, and write it.
    pub(crate) fn update(&mut self, change: impl FnOnce(&mut Settings)) {
        if self.file.is_some() {
            *self = Self::load(self.file.clone());
        }
        change(self);
        self.write();
    }

    pub(crate) fn endpoint(&self, id: &str) -> Option<&EndpointSetting> {
        self.endpoints.iter().find(|endpoint| endpoint.id == id)
    }

    pub(crate) fn discussion(&self) -> Option<&SkillSetting> {
        self.skills.iter().find(|skill| skill.id == DISCUSSION)
    }

    /// The Discussion skill's endpoint.
    pub(crate) fn discussion_endpoint(&self) -> Option<&EndpointSetting> {
        self.endpoint(self.discussion()?.endpoint.as_deref()?)
    }

    /// The level `chosen` names, or else the default, or else the first.
    pub(crate) fn level_or_default(&self, chosen: Option<&str>) -> Option<&Level> {
        let find = |id: Option<&str>| id.and_then(|id| self.levels.iter().find(|level| level.id == id));
        find(chosen).or_else(|| find(self.default_level.as_deref())).or_else(|| self.levels.first())
    }

    pub(crate) fn skill(&self, id: &str) -> Option<&SkillSetting> {
        self.skills.iter().find(|skill| skill.id == id)
    }

    /// The skills using an endpoint, by name, so its deletion can be refused.
    pub(crate) fn users_of(&self, endpoint: &str) -> Vec<String> {
        self.skills
            .iter()
            .filter(|skill| skill.endpoint.as_deref() == Some(endpoint))
            .map(|skill| skill.name.clone())
            .collect()
    }

    /// Settings from before 0.8 gain an OpenRouter endpoint, the Discussion
    /// skill on it with the default voice, and the five default levels with
    /// Haiku Medium chosen. Returns the new endpoint's id, so its key can be
    /// stored, or none when there were endpoints already.
    pub(crate) fn first_start(&mut self, openrouter: &str) -> Option<String> {
        if !self.endpoints.is_empty() {
            return None;
        }
        let id = protocol::cell_id();
        let level = |name: &str, model: &str, effort: Effort| Level {
            id: protocol::cell_id(),
            name: name.to_string(),
            endpoint: id.clone(),
            model: model.to_string(),
            effort: Some(effort),
        };
        let haiku = "anthropic/claude-haiku-5.5";
        let sonnet = "anthropic/claude-sonnet-5.5";
        let levels = vec![
            level("Haiku Low", haiku, Effort::Low),
            level("Haiku Medium", haiku, Effort::Medium),
            level("Haiku High", haiku, Effort::High),
            level("Sonnet Medium", sonnet, Effort::Medium),
            level("Sonnet High", sonnet, Effort::High),
        ];
        let default = levels[1].id.clone();
        let endpoint = EndpointSetting {
            id: id.clone(),
            name: "OpenRouter".to_string(),
            kind: EndpointKind::Openrouter,
            address: openrouter.to_string(),
        };
        let discussion = SkillSetting {
            id: DISCUSSION.to_string(),
            name: "Discussion".to_string(),
            description: "How the assistant talks with you".to_string(),
            endpoint: Some(id.clone()),
            model: None,
            prompt: DEFAULT_VOICE.to_string(),
        };
        self.update(move |settings| {
            if !settings.endpoints.is_empty() {
                return;
            }
            settings.endpoints.push(endpoint);
            settings.skills.retain(|skill| skill.id != DISCUSSION);
            settings.skills.insert(0, discussion);
            settings.levels = levels;
            settings.default_level = Some(default);
        });
        Some(id)
    }

    pub(crate) fn choose_chat_width(&mut self, width: f32) {
        self.update(|settings| settings.chat_width = Some(width));
    }

}

/// A new session's name: "Untitled Session", then "Untitled Session 2",
/// and on, past the names `taken`.
pub(crate) const UNTITLED_SESSION: &str = "Untitled Session";

pub(crate) fn untitled_session_name(taken: &[String]) -> String {
    if !taken.iter().any(|name| name == UNTITLED_SESSION) {
        return UNTITLED_SESSION.to_string();
    }
    (2u32..)
        .map(|n| format!("{UNTITLED_SESSION} {n}"))
        .find(|name| !taken.contains(name))
        .unwrap_or_else(|| UNTITLED_SESSION.to_string())
}

/// A session with no conversation and no tabs, written into the project.
pub(crate) fn new_session(project_folder: &Path, name: String) -> Result<SessionFile, String> {
    let now = now_seconds();
    let session = SessionFile {
        version: VERSION,
        id: fresh_session_id(project_folder),
        name,
        created_at: now,
        updated_at: now,
        trashed_at: None,
        tabs: Vec::new(),
        selected: None,
        workspace_open: false,
        conversation: Vec::new(),
        level: None,
    };
    session.write(project_folder)?;
    Ok(session)
}

/// Every session of a project this host can read, newest change first,
/// those in Trash included.
pub(crate) fn list_sessions(project_folder: &Path) -> Vec<SessionFile> {
    let Ok(entries) = std::fs::read_dir(sessions_dir(project_folder)) else {
        return Vec::new();
    };
    let mut sessions: Vec<SessionFile> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .filter_map(|text| SessionFile::parse(&text).ok())
        .collect();
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| a.id.cmp(&b.id)));
    sessions
}

/// What the session menu lists of a session: its name, when it changed,
/// and whether it is in Trash. Its conversation is skipped, not read, so
/// the menu opens at once however long the conversations are.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SessionSummary {
    pub id: String,
    pub name: String,
    pub updated_at: u64,
    #[serde(default)]
    pub trashed_at: Option<u64>,
}

/// Every session of a project this host can read, as the menu lists it,
/// newest change first, those in Trash included.
pub(crate) fn list_session_summaries(project_folder: &Path) -> Vec<SessionSummary> {
    let Ok(entries) = std::fs::read_dir(sessions_dir(project_folder)) else {
        return Vec::new();
    };
    let mut sessions: Vec<SessionSummary> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| std::fs::read(entry.path()).ok())
        .filter_map(|bytes| serde_json::from_slice::<SessionSummary>(&bytes).ok())
        .collect();
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| a.id.cmp(&b.id)));
    sessions
}

/// Why a folder cannot become a new project, or none when it can: it must
/// not exist yet, or be an empty folder.
pub(crate) fn unusable_project_folder(folder: &Path) -> Option<String> {
    if !folder.is_absolute() {
        return Some("Choose a full location.".to_string());
    }
    if !folder.exists() {
        return None;
    }
    if !folder.is_dir() {
        return Some("A file of that name is already there.".to_string());
    }
    match std::fs::read_dir(folder) {
        Ok(mut entries) => entries.next().map(|_| "That folder is not empty.".to_string()),
        Err(error) => Some(error.to_string()),
    }
}

/// Makes a project in `folder`, which must not exist or must be empty, with
/// its first session. The project is added to `projects.json` as current.
pub(crate) fn create_project_at(data: &Path, folder: &Path, name: &str) -> Result<(ProjectRecord, SessionFile), String> {
    if let Some(reason) = unusable_project_folder(folder) {
        return Err(reason);
    }
    std::fs::create_dir_all(project_meta_dir(folder)).map_err(|error| error.to_string())?;
    let record = ProjectRecord {
        version: VERSION,
        id: protocol::cell_id(),
        name: name.trim().to_string(),
    };
    record.write(folder)?;
    let session = new_session(folder, UNTITLED_SESSION.to_string())?;
    let mut list = ProjectsList::load(data)?;
    list.version = VERSION;
    list.projects.push(KnownProject {
        id: record.id.clone(),
        folder: folder.display().to_string(),
        last_used: now_seconds(),
        trashed_at: None,
        pinned: false,
        locked: false,
        archived_at: None,
    });
    list.current_project = Some(record.id.clone());
    list.current_session = Some(session.id.clone());
    list.write(data)?;
    Ok((record, session))
}

/// The folder for a project Studio makes by itself: `<root>/Untitled
/// Project`, then `<root>/Untitled Project 2`, and on, past folders that
/// already hold something.
pub(crate) fn default_project_folder(root: &Path) -> PathBuf {
    let plain = root.join(UNTITLED);
    if unusable_project_folder(&plain).is_none() {
        return plain;
    }
    (2u32..)
        .map(|n| root.join(format!("{UNTITLED} {n}")))
        .find(|folder| unusable_project_folder(folder).is_none())
        .unwrap_or(plain)
}

/// A known project, read: its entry, folder, and record.
pub(crate) struct Known {
    pub entry: KnownProject,
    pub folder: PathBuf,
    pub record: ProjectRecord,
}

/// Every project in `projects.json` whose folder can be read, most recently
/// used first, those in Trash included.
pub(crate) fn known_projects(data: &Path) -> Vec<Known> {
    let Ok(list) = ProjectsList::load(data) else {
        return Vec::new();
    };
    let mut known: Vec<Known> = list
        .projects
        .into_iter()
        .filter_map(|entry| {
            let folder = PathBuf::from(&entry.folder);
            let record = ProjectRecord::read(&folder).ok()?;
            Some(Known { entry, folder, record })
        })
        .collect();
    known.sort_by(|a, b| b.entry.last_used.cmp(&a.entry.last_used));
    known
}

/// Changes one project's entry in `projects.json`, reading it fresh.
pub(crate) fn update_known(data: &Path, id: &str, change: impl FnOnce(&mut KnownProject)) -> Result<(), String> {
    let mut list = ProjectsList::load(data)?;
    if let Some(entry) = list.projects.iter_mut().find(|entry| entry.id == id) {
        change(entry);
    }
    list.write(data)
}

/// Records the current project and session, and when the project was used.
pub(crate) fn note_current(data: &Path, project: &str, session: &str) -> Result<(), String> {
    let mut list = ProjectsList::load(data)?;
    list.version = VERSION;
    list.current_project = Some(project.to_string());
    list.current_session = Some(session.to_string());
    if let Some(entry) = list.projects.iter_mut().find(|entry| entry.id == project) {
        entry.last_used = now_seconds();
    }
    list.write(data)
}

/// A session as the host's flows hold a project: its tabs with paths made
/// absolute in `folder`, and its attachments too. A path that would leave
/// the folder is dropped.
pub(crate) fn session_as_file(folder: &Path, name: &str, session: &SessionFile) -> ProjectFile {
    let tabs = session
        .tabs
        .iter()
        .map(|tab| ProjectTab {
            app: tab.app.clone(),
            path: tab
                .path
                .as_deref()
                .and_then(|path| resolve_in_project(folder, path))
                .map(|path| path.display().to_string()),
            label: tab.label.clone(),
            camera: tab.camera,
        })
        .collect();
    let mut conversation = session.conversation.clone();
    for entry in &mut conversation {
        entry.attachments = entry
            .attachments
            .iter()
            .filter_map(|path| resolve_in_project(folder, path))
            .map(|path| path.display().to_string())
            .collect();
    }
    ProjectFile {
        version: VERSION,
        name: name.to_string(),
        folders: Vec::new(),
        tabs,
        selected: session.selected,
        workspace_open: session.workspace_open,
        conversation,
    }
}

/// The host's project as one session's file, with every path made relative
/// to `folder`. A tab or attachment outside the folder is never written.
pub(crate) fn file_as_session(folder: &Path, file: &ProjectFile, mut session: SessionFile) -> SessionFile {
    let mut selected = None;
    let mut tabs = Vec::new();
    for (index, tab) in file.tabs.iter().enumerate() {
        let path = match &tab.path {
            Some(path) => match relative_to_project(folder, Path::new(path)) {
                Some(relative) => Some(relative),
                None => continue,
            },
            None => None,
        };
        if file.selected == Some(index) {
            selected = Some(tabs.len());
        }
        tabs.push(SessionTab {
            app: tab.app.clone(),
            path,
            label: tab.label.clone(),
            camera: tab.camera,
        });
    }
    let mut conversation = file.conversation.clone();
    for entry in &mut conversation {
        entry.attachments = entry
            .attachments
            .iter()
            .filter_map(|path| relative_to_project(folder, Path::new(path)))
            .collect();
    }
    session.tabs = tabs;
    session.selected = selected;
    session.workspace_open = file.workspace_open;
    session.conversation = conversation;
    session.updated_at = now_seconds();
    session
}

/// Copies a file from outside the project into `<folder>/<into>`, under a
/// free name, and returns the copy's path. The original is not changed.
pub(crate) fn import_copy(source: &Path, folder: &Path, into: &str) -> Result<PathBuf, String> {
    let target = if into.is_empty() { folder.to_path_buf() } else { folder.join(into) };
    let relative = copy_into(source, &target, None)?;
    Ok(target.join(relative))
}

/// Moves a folder to the system's trash. A test moves it into its own data
/// folder instead, so no test ever touches the person's trash.
fn to_system_trash(data: &Path, folder: &Path) -> Result<(), String> {
    #[cfg(not(test))]
    {
        let _ = data;
        trash::delete(folder).map_err(|error| error.to_string())
    }
    #[cfg(test)]
    {
        let bin = data.join("test-system-trash");
        std::fs::create_dir_all(&bin).map_err(|error| error.to_string())?;
        let name = folder.file_name().map(|name| name.to_os_string()).unwrap_or_default();
        std::fs::rename(folder, bin.join(name)).map_err(|error| error.to_string())
    }
}

/// Empties Trash: each project in it goes to the system's trash with its
/// folder and leaves `projects.json`, and each session in it is deleted from
/// its project, whose activities stay. Returns a line for anything that
/// could not be done.
pub(crate) fn empty_trash(data: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    let Ok(mut list) = ProjectsList::load(data) else {
        return vec!["Studio's list of projects could not be read.".to_string()];
    };
    let mut kept = Vec::new();
    for entry in std::mem::take(&mut list.projects) {
        let folder = PathBuf::from(&entry.folder);
        if entry.trashed_at.is_some() {
            if folder.exists()
                && let Err(error) = to_system_trash(data, &folder)
            {
                lines.push(format!("{} could not be moved to the trash: {error}", folder.display()));
                kept.push(entry);
            }
            continue;
        }
        for session in list_sessions(&folder).into_iter().filter(|session| session.trashed_at.is_some()) {
            let _ = std::fs::remove_file(session_path(&folder, &session.id));
        }
        kept.push(entry);
    }
    list.projects = kept;
    if let Err(error) = list.write(data) {
        lines.push(error);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use assistant::{Entry, Role};

    fn entry(role: Role, text: &str) -> Entry {
        Entry {
            role,
            text: text.to_string(),
            time: 1_760_000_000,
            attachments: Vec::new(),
            tool: None,
            ok: None,
            cell: None,
            aside: false,
        }
    }

    fn sample() -> ProjectFile {
        let mut person = entry(Role::Person, "Make it larger");
        person.attachments = vec!["/home/someone/guide.png".to_string()];
        let mut tool = entry(Role::Tool, "");
        tool.tool = Some("one-app__edit__grow".to_string());
        tool.ok = Some(true);
        ProjectFile {
            version: VERSION,
            name: "Notes".to_string(),
            folders: vec!["/home/someone/Notes".to_string()],
            tabs: vec![
                ProjectTab {
                    app: "one-app".to_string(),
                    path: Some("/home/someone/work.file".to_string()),
                    label: "work".to_string(),
                    camera: Some(View {
                        origin: [-120.5, 40.0],
                        zoom: 1.25,
                    }),
                },
                ProjectTab {
                    app: "other-app".to_string(),
                    path: None,
                    label: "Other".to_string(),
                    camera: None,
                },
            ],
            selected: Some(1),
            workspace_open: true,
            conversation: vec![person, tool, entry(Role::Assistant, "Done.")],
        }
    }

    /// How 0.6 wrote a project, kept here so the move of old projects can
    /// be tested against the real thing.
    trait OldProject {
        fn to_text(&self) -> String;
        fn write(&self, path: &Path) -> Result<(), String>;
    }

    impl OldProject for ProjectFile {
        fn to_text(&self) -> String {
            let mut text = serde_json::to_string_pretty(self).unwrap();
            text.push('\n');
            text
        }

        fn write(&self, path: &Path) -> Result<(), String> {
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
            std::fs::write(path, self.to_text()).map_err(|error| error.to_string())
        }
    }

    /// An old project in `data`, as 0.6 made one, with its files folder.
    fn create_project(data: &Path) -> Result<(String, ProjectFile), String> {
        let id = format!("old-{}", protocol::cell_id());
        let file = ProjectFile {
            version: VERSION,
            name: UNTITLED.to_string(),
            folders: Vec::new(),
            tabs: Vec::new(),
            selected: None,
            workspace_open: false,
            conversation: Vec::new(),
        };
        std::fs::create_dir_all(project_files(data, &id)).map_err(|error| error.to_string())?;
        file.write(&project_path(data, &id))?;
        Ok((id, file))
    }

    fn scratch(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("inpainter-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// Catches a project that loses a tab's file, camera, the selection, a
    /// folder, or a line of the conversation on the way to its file and back.
    #[test]
    fn the_project_file_round_trips() {
        let file = sample();
        let text = file.to_text();
        assert_eq!(ProjectFile::parse(&text).unwrap(), file);
        assert!(!text.contains("\"input\"") && !text.contains("\"output\""));

        let folder = scratch("project");
        let path = project_path(&folder, "one");
        file.write(&path).unwrap();
        assert_eq!(ProjectFile::read(&path).unwrap(), file);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    /// Catches a newer project version read as if this host understood it.
    #[test]
    fn a_newer_project_version_is_refused() {
        let mut file = sample();
        file.version = VERSION + 1;
        assert_eq!(
            ProjectFile::parse(&file.to_text()),
            Err(ReadError::Version(VERSION + 1))
        );
        assert!(version_line(VERSION + 1).contains("version 2"));
    }

    /// Catches a second untitled project that reuses the first one's name.
    #[test]
    fn untitled_sessions_are_numbered_when_the_name_is_taken() {
        assert_eq!(untitled_session_name(&[]), "Untitled Session");
        let taken = ["Untitled Session".to_string()];
        assert_eq!(untitled_session_name(&taken), "Untitled Session 2");
        let taken = ["Untitled Session".to_string(), "Untitled Session 2".to_string()];
        assert_eq!(untitled_session_name(&taken), "Untitled Session 3");
    }

    /// Catches a project made over a folder's files, or one Studio makes by
    /// itself over another project: New Project takes only a folder that is
    /// missing or empty, and the made-by-itself folder skips taken ones.
    #[test]
    fn a_project_is_made_only_where_nothing_is() {
        let data = scratch("make-data");
        let root = scratch("make-root");
        let full = root.join("Full");
        std::fs::create_dir_all(&full).unwrap();
        std::fs::write(full.join("mine.txt"), "mine").unwrap();
        assert!(create_project_at(&data, &full, "Full").is_err());
        assert!(!project_meta_dir(&full).exists());
        assert!(create_project_at(&data, Path::new("relative"), "Relative").is_err());

        let first = default_project_folder(&root);
        assert_eq!(first, root.join("Untitled Project"));
        let (record, session) = create_project_at(&data, &first, UNTITLED).unwrap();
        assert_eq!(session.name, "Untitled Session");
        assert_eq!(ProjectRecord::read(&first).unwrap(), record);
        assert_eq!(default_project_folder(&root), root.join("Untitled Project 2"));
        let list = ProjectsList::load(&data).unwrap();
        assert_eq!(list.current_project.as_deref(), Some(record.id.as_str()));
        assert_eq!(list.current_session.as_deref(), Some(session.id.as_str()));
        std::fs::remove_dir_all(&data).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Catches settings that lose another run's change, or a field an
    /// older or newer Studio wrote.
    #[test]
    fn settings_keep_each_change_and_unknown_fields() {
        let folder = scratch("set");
        let file = folder.join("settings.json");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(&file, r#"{"model":{"id":"x/y","name":"Y"},"chat_width":300.0}"#).unwrap();
        let mut settings = Settings::load(Some(file.clone()));
        let mut other = Settings::load(Some(file.clone()));
        other.update(|settings| settings.projects_root = Some("/elsewhere".into()));
        settings.choose_chat_width(480.0);
        let read = Settings::load(Some(file.clone()));
        assert_eq!(read.chat_width, Some(480.0));
        assert_eq!(read.projects_root.as_deref(), Some("/elsewhere"));
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"x/y\""), "{text}");
        std::fs::remove_dir_all(&folder).unwrap();
    }

    /// Catches a first start that leaves Studio with nowhere to send a
    /// prompt, or one that runs again over the person's own settings.
    #[test]
    fn the_first_start_makes_openrouter_discussion_and_five_levels() {
        let folder = scratch("first");
        let mut settings = Settings::load(Some(folder.join("settings.json")));
        let endpoint = settings.first_start("https://stand-in/v1").expect("made");
        assert_eq!(settings.discussion_endpoint().map(|e| e.address.as_str()), Some("https://stand-in/v1"));
        assert_eq!(settings.discussion().map(|s| s.prompt.as_str()), Some(DEFAULT_VOICE));
        let names: Vec<&str> = settings.levels.iter().map(|level| level.name.as_str()).collect();
        assert_eq!(names, ["Haiku Low", "Haiku Medium", "Haiku High", "Sonnet Medium", "Sonnet High"]);
        assert!(settings.levels.iter().all(|level| level.endpoint == endpoint));
        assert_eq!(settings.level_or_default(None).map(|l| l.name.as_str()), Some("Haiku Medium"));
        let sonnet = settings.levels[4].id.clone();
        assert_eq!(settings.level_or_default(Some(&sonnet)).map(|l| l.name.as_str()), Some("Sonnet High"));
        assert_eq!(settings.level_or_default(Some("gone")).map(|l| l.name.as_str()), Some("Haiku Medium"));
        assert_eq!(settings.first_start("https://other/v1"), None);
        assert_eq!(Settings::load(Some(folder.join("settings.json"))).endpoints.len(), 1);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    /// A lock another handle holds stops a second start. A lock file left
    /// behind, with nothing holding it, does not.
    #[test]
    fn a_held_lock_stops_a_second_start_and_a_leftover_file_does_not() {
        let folder = scratch("lock");
        let path = lock_path(&folder);
        let held = try_lock(&path).unwrap();
        assert!(matches!(try_lock(&path), Err(LockFail::Busy)));
        assert_eq!(second_start_message(LockFail::Busy), Some(ALREADY_OPEN));
        drop(held);
        assert!(path.exists(), "the lock file stays when the holder is gone");
        let again = try_lock(&path).unwrap();
        drop(again);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn projects_list_and_session_files_round_trip() {
        let folder = scratch("projects-list");
        let list = ProjectsList {
            version: VERSION,
            projects: vec![KnownProject {
                id: "p1".to_string(),
                folder: "/tmp/one".to_string(),
                last_used: 10,
                trashed_at: None,
                pinned: false,
                locked: false,
                archived_at: None,
            }],
            current_project: Some("p1".to_string()),
            current_session: Some("s1".to_string()),
        };
        list.write(&folder).unwrap();
        assert_eq!(ProjectsList::load(&folder).unwrap(), list);

        let project = folder.join("Project");
        let record = ProjectRecord {
            version: VERSION,
            id: "p1".to_string(),
            name: "Project".to_string(),
        };
        record.write(&project).unwrap();
        assert_eq!(ProjectRecord::read(&project).unwrap(), record);

        let session = SessionFile {
            version: VERSION,
            id: "s1".to_string(),
            name: "Untitled Session".to_string(),
            created_at: 1,
            updated_at: 2,
            trashed_at: None,
            tabs: vec![SessionTab {
                app: "one-app".to_string(),
                path: Some("work/file.smx".to_string()),
                label: "Work".to_string(),
                camera: None,
            }],
            selected: Some(0),
            workspace_open: true,
            conversation: vec![entry(Role::Assistant, "Done.")],
            level: Some("l1".to_string()),
        };
        session.write(&project).unwrap();
        assert_eq!(SessionFile::read(&project, "s1").unwrap(), session);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn path_helpers_keep_everything_inside_the_project() {
        let folder = scratch("path-helpers");
        let project = folder.join("Project");
        std::fs::create_dir_all(project.join("sub")).unwrap();
        std::fs::write(project.join("sub/work.smx"), "x").unwrap();
        std::fs::write(folder.join("outside.smx"), "x").unwrap();

        assert!(resolve_in_project(&project, "sub/work.smx").is_some());
        assert!(resolve_in_project(&project, "../outside.smx").is_none());
        assert!(relative_to_project(&project, &folder.join("outside.smx")).is_none());
        assert_eq!(
            relative_to_project(&project, &project.join("sub/work.smx")),
            Some("sub/work.smx".to_string())
        );
        assert_eq!(
            resolve_in_project(&project, "sub/work.smx"),
            Some(project.join("sub/work.smx"))
        );
        assert_eq!(
            relative_to_project(&project, &folder.join("outside.smx")),
            None
        );
        assert_eq!(resolve_in_project(&project, "../outside.smx"), None);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn old_projects_migrate_into_folder_backed_projects() {
        let data = scratch("migrate-data");
        let projects_root = scratch("migrate-home");
        let (id, mut old) = create_project(&data).unwrap();
        let outside = data.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let tab_file = outside.join("Notes.smx");
        std::fs::write(&tab_file, "tab").unwrap();
        old.tabs = vec![ProjectTab {
            app: "one-app".to_string(),
            path: Some(tab_file.display().to_string()),
            label: "Notes".to_string(),
            camera: None,
        }];
        let attachment = outside.join("Ref.png");
        std::fs::write(&attachment, "image").unwrap();
        let mut message = entry(Role::Person, "Use this");
        message.attachments = vec![attachment.display().to_string()];
        old.conversation = vec![message];
        old.write(&project_path(&data, &id)).unwrap();
        std::fs::write(project_files(&data, &id).join("extra.smx"), "extra").unwrap();

        let notes = migrate_old_projects_to(&data, &projects_root).unwrap();
        assert!(notes.is_empty(), "unexpected migration notes: {notes:?}");

        let list = ProjectsList::load(&data).unwrap();
        let known = list
            .projects
            .into_iter()
            .find(|project| project.id == id)
            .unwrap();
        let folder = PathBuf::from(known.folder);
        assert!(folder.starts_with(&projects_root));
        let record = ProjectRecord::read(&folder).unwrap();
        assert_eq!(record.id, id);

        let session_id = list.current_session.unwrap();
        let session = SessionFile::read(&folder, &session_id).unwrap();
        assert_eq!(session.tabs.len(), 1);
        let tab_path = session.tabs[0].path.as_ref().unwrap();
        assert!(!tab_path.starts_with('/'));
        assert_eq!(session.conversation[0].attachments.len(), 1);
        assert!(session.conversation[0].attachments[0].starts_with("attachments/"));

        assert!(data.join("projects-before-0.7").join(record.id).is_dir());
        std::fs::remove_dir_all(&data).unwrap();
        std::fs::remove_dir_all(&projects_root).unwrap();
    }

    #[test]
    fn migration_reuses_an_existing_folder_for_the_same_project_id() {
        let data = scratch("migrate-repeat-data");
        let projects_root = scratch("migrate-repeat-home");
        let (id, mut old) = create_project(&data).unwrap();
        old.name = "Notes".to_string();
        old.write(&project_path(&data, &id)).unwrap();
        let outside = data.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let source = outside.join("Notes.smx");
        std::fs::write(&source, "note").unwrap();
        old.tabs = vec![ProjectTab {
            app: "one-app".to_string(),
            path: Some(source.display().to_string()),
            label: "Notes".to_string(),
            camera: None,
        }];
        old.write(&project_path(&data, &id)).unwrap();

        let existing = projects_root.join("Notes");
        std::fs::create_dir_all(project_meta_dir(&existing)).unwrap();
        ProjectRecord {
            version: VERSION,
            id: id.clone(),
            name: "Notes".to_string(),
        }
        .write(&existing)
        .unwrap();

        let notes = migrate_old_projects_to(&data, &projects_root).unwrap();
        assert!(notes.is_empty(), "unexpected migration notes: {notes:?}");
        assert!(!projects_root.join("Notes 2").exists());
        let list = ProjectsList::load(&data).unwrap();
        let known = list.projects.into_iter().find(|project| project.id == id).unwrap();
        assert_eq!(PathBuf::from(known.folder), existing);
        assert!(data.join("projects-before-0.7").join(id).is_dir());
        std::fs::remove_dir_all(&data).unwrap();
        std::fs::remove_dir_all(&projects_root).unwrap();
    }

    #[test]
    fn migration_reports_one_failed_project_and_continues_the_rest() {
        let data = scratch("migrate-fail-data");
        let projects_root = scratch("migrate-fail-home");
        let (bad_id, mut bad) = create_project(&data).unwrap();
        bad.name = "Unreadable".to_string();
        bad.tabs = vec![ProjectTab {
            app: "one-app".to_string(),
            path: Some("/proc/1/mem".to_string()),
            label: "Locked".to_string(),
            camera: None,
        }];
        bad.write(&project_path(&data, &bad_id)).unwrap();

        let (good_id, mut good) = create_project(&data).unwrap();
        good.name = "Readable".to_string();
        let outside = data.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let readable = outside.join("Notes.smx");
        std::fs::write(&readable, "notes").unwrap();
        good.tabs = vec![ProjectTab {
            app: "one-app".to_string(),
            path: Some(readable.display().to_string()),
            label: "Notes".to_string(),
            camera: None,
        }];
        good.write(&project_path(&data, &good_id)).unwrap();

        let notes = migrate_old_projects_to(&data, &projects_root).unwrap();
        assert!(
            notes.iter().any(|line| line.contains("Unreadable") && line.contains("was not moved")),
            "missing failure note: {notes:?}"
        );
        let list = ProjectsList::load(&data).unwrap();
        assert!(list.projects.iter().any(|project| project.id == good_id));
        assert!(!list.projects.iter().any(|project| project.id == bad_id));
        assert!(project_dir(&data, &bad_id).is_dir());
        assert!(data.join("projects-before-0.7").join(good_id).is_dir());
        std::fs::remove_dir_all(&data).unwrap();
        std::fs::remove_dir_all(&projects_root).unwrap();
    }

    #[test]
    fn migration_keeps_nested_files_paths_and_maps_tabs_to_them() {
        let data = scratch("migrate-nested-data");
        let projects_root = scratch("migrate-nested-home");
        let (id, mut old) = create_project(&data).unwrap();
        old.name = "Nested".to_string();
        let nested = project_files(&data, &id).join("pages/chapter");
        std::fs::create_dir_all(&nested).unwrap();
        let nested_file = nested.join("Notes.smx");
        std::fs::write(&nested_file, "nested").unwrap();
        old.tabs = vec![ProjectTab {
            app: "one-app".to_string(),
            path: Some(nested_file.display().to_string()),
            label: "Notes".to_string(),
            camera: None,
        }];
        old.write(&project_path(&data, &id)).unwrap();

        let notes = migrate_old_projects_to(&data, &projects_root).unwrap();
        assert!(notes.is_empty(), "unexpected migration notes: {notes:?}");
        let list = ProjectsList::load(&data).unwrap();
        let known = list.projects.into_iter().find(|project| project.id == id).unwrap();
        let folder = PathBuf::from(known.folder);
        let session = SessionFile::read(&folder, &list.current_session.unwrap()).unwrap();
        assert_eq!(session.tabs[0].path.as_deref(), Some("pages/chapter/Notes.smx"));
        assert!(folder.join("pages/chapter/Notes.smx").is_file());
        assert!(!folder.join("Notes.smx").is_file());
        std::fs::remove_dir_all(&data).unwrap();
        std::fs::remove_dir_all(&projects_root).unwrap();
    }
}
