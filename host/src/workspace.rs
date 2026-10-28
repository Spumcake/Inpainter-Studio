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
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The version this host writes and reads.
pub(crate) const VERSION: u32 = 1;
/// A project's name until the person renames it, and the next ones when that
/// name is already taken.
pub(crate) const UNTITLED: &str = "Untitled Project";
/// The file each project is stored as.
const FILE_NAME: &str = "project.json";
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

/// Why a project file could not be read.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ReadError {
    /// A newer file than this host understands.
    Version(u32),
    Other(String),
}

impl ProjectFile {
    pub(crate) fn to_text(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("a project always encodes as JSON");
        text.push('\n');
        text
    }

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

    /// Writes beside the file first and then replaces it, so a failed write
    /// never leaves half a project.
    pub(crate) fn write(&self, path: &Path) -> Result<(), String> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        }
        let mut partial = path.as_os_str().to_owned();
        partial.push(".partial");
        let partial = PathBuf::from(partial);
        std::fs::write(&partial, self.to_text()).map_err(|error| error.to_string())?;
        std::fs::rename(&partial, path).map_err(|error| {
            let _ = std::fs::remove_file(&partial);
            error.to_string()
        })
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

pub(crate) fn projects_dir(data: &Path) -> PathBuf {
    data.join("projects")
}

pub(crate) fn project_dir(data: &Path, id: &str) -> PathBuf {
    projects_dir(data).join(id)
}

pub(crate) fn project_path(data: &Path, id: &str) -> PathBuf {
    project_dir(data, id).join(FILE_NAME)
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

fn fresh_id(data: &Path) -> String {
    let millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let mut n = 0u32;
    loop {
        let id = if n == 0 {
            format!("{millis:x}")
        } else {
            format!("{millis:x}-{n}")
        };
        if !project_dir(data, &id).exists() {
            return id;
        }
        n += 1;
    }
}

/// "Untitled Project", then "Untitled Project 2", and on.
pub(crate) fn untitled_name(taken: &[String]) -> String {
    if !taken.iter().any(|name| name == UNTITLED) {
        return UNTITLED.to_string();
    }
    let mut n = 2u32;
    loop {
        let name = format!("{UNTITLED} {n}");
        if !taken.iter().any(|known| known == &name) {
            return name;
        }
        n += 1;
    }
}

/// A project this host could read, with when its file was last changed.
pub(crate) struct Listed {
    pub id: String,
    pub modified: SystemTime,
    pub file: ProjectFile,
}

/// Every readable project, newest change first. A file this host cannot read
/// is left out.
pub(crate) fn list_projects(data: &Path) -> Vec<Listed> {
    let Ok(entries) = std::fs::read_dir(projects_dir(data)) else {
        return Vec::new();
    };
    let mut listed = Vec::new();
    for entry in entries.filter_map(|entry| entry.ok()) {
        let id = entry.file_name().to_string_lossy().into_owned();
        if !single_component(&id) {
            continue;
        }
        let path = entry.path().join(FILE_NAME);
        let Ok(file) = ProjectFile::read(&path) else {
            continue;
        };
        let modified = std::fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        listed.push(Listed { id, modified, file });
    }
    listed.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.id.cmp(&b.id)));
    listed
}

/// Creates an empty project and writes it. The name is "Untitled Project",
/// numbered when that name is taken.
pub(crate) fn create_project(data: &Path) -> Result<(String, ProjectFile), String> {
    let taken: Vec<String> = list_projects(data).into_iter().map(|listed| listed.file.name).collect();
    let file = ProjectFile {
        version: VERSION,
        name: untitled_name(&taken),
        folders: Vec::new(),
        tabs: Vec::new(),
        selected: None,
        workspace_open: false,
        conversation: Vec::new(),
    };
    let id = fresh_id(data);
    std::fs::create_dir_all(project_files(data, &id)).map_err(|error| error.to_string())?;
    file.write(&project_path(data, &id))?;
    Ok((id, file))
}

/// An id Studio itself makes: hexadecimal and hyphens, nothing else.
fn own_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Deletes the project's folder in the data folder. An id Studio did not make
/// is refused, and the folder's real path has to stay inside the projects folder.
pub(crate) fn delete_project(data: &Path, id: &str) -> Result<(), String> {
    if !own_id(id) {
        return Err(format!("“{id}” is not a project."));
    }
    let projects = projects_dir(data);
    let dir = project_dir(data, id);
    if !dir.is_dir() {
        return Ok(());
    }
    let projects = projects.canonicalize().map_err(|error| error.to_string())?;
    let dir = dir.canonicalize().map_err(|error| error.to_string())?;
    if dir == projects || !dir.starts_with(&projects) {
        return Err(format!("“{id}” is not a project."));
    }
    std::fs::remove_dir_all(&dir).map_err(|error| error.to_string())
}

/// The model the picker chose, by id, with the name it shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ModelChoice {
    pub id: String,
    pub name: String,
}

/// What the host remembers between runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelChoice>,
    /// The current project's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Where these settings live. None in frame tests, which never write.
    #[serde(skip)]
    file: Option<PathBuf>,
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
    /// read it again and change only this one value.
    fn update(&mut self, change: impl FnOnce(&mut Settings)) {
        if self.file.is_some() {
            let fresh = Self::load(self.file.clone());
            self.project = fresh.project;
            self.model = fresh.model;
        }
        change(self);
        self.write();
    }

    pub(crate) fn choose_model(&mut self, choice: ModelChoice) {
        self.update(|settings| settings.model = Some(choice));
    }

    pub(crate) fn choose_project(&mut self, id: &str) {
        let id = id.to_string();
        self.update(|settings| settings.project = Some(id));
    }
}

/// The models the picker offers, in its order, as the owner asked on
/// 2026-10-15: a few recent, well-known models in the class of Claude Haiku
/// 5.5 and Claude Sonnet 5.5, each able to use tools and see pictures. One
/// the OpenRouter list no longer offers is left out.
pub(crate) const PICKER: &[&str] = &[
    "anthropic/claude-haiku-5.5",
    "anthropic/claude-sonnet-5.5",
    "anthropic/claude-opus-5.5",
    "openai/gpt-6-luna",
    "openai/gpt-6.1-sol",
    "google/gemini-3.8-flash",
    "google/gemini-4-argon",
];

/// The models of [`PICKER`] that the list offers, in the picker's order.
pub(crate) fn picker_models(list: Vec<assistant::Model>) -> Vec<assistant::Model> {
    PICKER
        .iter()
        .filter_map(|id| list.iter().find(|model| model.id == *id && model.images).cloned())
        .collect()
}

/// The model to use when none is chosen: the newest Claude Haiku the list
/// offers, or else the first model in it.
pub(crate) fn default_model(models: &[assistant::Model]) -> Option<&assistant::Model> {
    // The newest Claude Haiku, as the owner chose on 2026-10-15.
    let haiku = models
        .iter()
        .filter(|model| {
            let id = model.id.to_ascii_lowercase();
            // A variant such as `:batch` or `:free` is never the default.
            id.contains("claude") && id.contains("haiku") && !id.contains(':')
        })
        .max_by(|a, b| version_of(&a.id).cmp(&version_of(&b.id)));
    haiku.or_else(|| models.first())
}

/// The numbers in a model id, in order, so `claude-sonnet-4.5` is newer
/// than `claude-sonnet-4` and `claude-3.7-sonnet`.
fn version_of(id: &str) -> Vec<u32> {
    let name = id.split_once('/').map_or(id, |(_, name)| name);
    let name = name.split_once(':').map_or(name, |(name, _)| name);
    name.split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty() && part.len() < 6)
        .filter_map(|part| part.parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use assistant::{Entry, Model, Role};

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
    fn untitled_projects_are_numbered_when_the_name_is_taken() {
        assert_eq!(untitled_name(&[]), "Untitled Project");
        assert_eq!(
            untitled_name(&["Untitled Project".to_string()]),
            "Untitled Project 2"
        );
        assert_eq!(
            untitled_name(&["Untitled Project".to_string(), "Untitled Project 2".to_string()]),
            "Untitled Project 3"
        );
    }

    /// Catches settings that forget the model or the current project, or
    /// that another run's write is lost.
    #[test]
    fn settings_keep_the_model_and_the_current_project() {
        let folder = scratch("set");
        let file = folder.join("settings.json");
        let mut settings = Settings::load(Some(file.clone()));
        settings.choose_project("first");
        let mut other = Settings::load(Some(file.clone()));
        other.choose_model(ModelChoice {
            id: "x/y".to_string(),
            name: "Y".to_string(),
        });
        settings.choose_project("second");
        let read = Settings::load(Some(file));
        assert_eq!(read.project.as_deref(), Some("second"));
        assert_eq!(read.model.map(|model| model.id).as_deref(), Some("x/y"));
        std::fs::remove_dir_all(&folder).unwrap();
    }

    /// Catches a missing current project that does not fall back to the one
    /// changed most recently.
    #[test]
    fn the_newest_project_is_listed_first() {
        let folder = scratch("list");
        let mut older = sample();
        older.name = "Older".to_string();
        older.write(&project_path(&folder, "older")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut newer = sample();
        newer.name = "Newer".to_string();
        newer.write(&project_path(&folder, "newer")).unwrap();
        let listed = list_projects(&folder);
        assert_eq!(listed[0].id, "newer");
        assert_eq!(listed[1].id, "older");
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn a_project_is_created_with_its_files_folder_and_a_dialog_starts_there() {
        let folder = scratch("files");
        let (id, _) = create_project(&folder).unwrap();
        let files = project_files(&folder, &id);
        assert!(files.is_dir(), "the project's folder was not created");
        assert_eq!(dialog_directory(None, Some(&files)).as_deref(), Some(files.as_path()));
        let elsewhere = folder.join("elsewhere");
        assert_eq!(
            dialog_directory(Some(&elsewhere), Some(&files)).as_deref(),
            Some(elsewhere.as_path())
        );
        delete_project(&folder, &id).unwrap();
        assert!(!project_dir(&folder, &id).exists(), "removing the project left its folder");
        std::fs::remove_dir_all(&folder).unwrap();
    }

    /// A folder stays in the project file, which is what reopening reads.
    #[test]
    fn an_attached_folder_stays_in_the_project_file() {
        let folder = scratch("folders");
        let mut file = sample();
        file.folders.push("/anywhere/else".to_string());
        let (id, created) = create_project(&folder).unwrap();
        assert_eq!(created.name, UNTITLED);
        let path = project_path(&folder, &id);
        file.name = created.name;
        file.write(&path).unwrap();
        let read = ProjectFile::read(&path).unwrap();
        assert_eq!(
            read.folders,
            vec!["/home/someone/Notes".to_string(), "/anywhere/else".to_string()]
        );
        std::fs::remove_dir_all(&folder).unwrap();
    }

    /// Removing a project deletes its folder in the data folder and nothing
    /// the project only refers to.
    #[test]
    fn removing_a_project_touches_nothing_outside_its_folder() {
        let folder = scratch("remove");
        let outside = folder.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let papers = outside.join("Papers");
        std::fs::create_dir_all(&papers).unwrap();
        let work = outside.join("work.file");
        std::fs::write(&work, "the work itself").unwrap();
        let (id, mut file) = create_project(&folder).unwrap();
        file.folders = vec![papers.display().to_string()];
        file.tabs = vec![ProjectTab {
            app: "one-app".to_string(),
            path: Some(work.display().to_string()),
            label: "work".to_string(),
            camera: None,
        }];
        file.write(&project_path(&folder, &id)).unwrap();
        delete_project(&folder, &id).unwrap();
        assert!(!project_dir(&folder, &id).exists());
        assert_eq!(std::fs::read_to_string(&work).unwrap(), "the work itself");
        assert!(papers.is_dir());
        std::fs::remove_dir_all(&folder).unwrap();
    }

    /// An id Studio did not make, or a folder whose real path is outside the
    /// projects folder, is not deleted.
    #[test]
    fn delete_project_stays_inside_the_projects_folder() {
        let folder = scratch("delete-guard");
        let outside = folder.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("stay"), "stay").unwrap();
        let (id, _) = create_project(&folder).unwrap();
        assert!(delete_project(&folder, "../outside").is_err());
        assert!(delete_project(&folder, "Notes").is_err());
        assert!(outside.join("stay").is_file());
        let dir = project_dir(&folder, &id);
        std::fs::remove_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(&outside, &dir).unwrap();
        assert!(delete_project(&folder, &id).is_err());
        assert_eq!(std::fs::read_to_string(outside.join("stay")).unwrap(), "stay");
        std::fs::remove_file(&dir).unwrap();
        assert!(delete_project(&folder, &id).is_ok());
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

    /// Catches a picker that offers every model OpenRouter lists, or one
    /// that cannot see pictures.
    #[test]
    fn the_picker_offers_its_short_list_in_order() {
        let model = |id: &str, images: bool| Model {
            id: id.to_string(),
            name: id.to_string(),
            images,
        };
        let list = vec![
            model("some/other-model", true),
            model("anthropic/claude-sonnet-5.5", true),
            model("anthropic/claude-sonnet-5.5:batch", true),
            model("anthropic/claude-haiku-5.5", true),
            model("openai/gpt-6-luna", false),
        ];
        let ids: Vec<String> = picker_models(list).into_iter().map(|model| model.id).collect();
        assert_eq!(ids, ["anthropic/claude-haiku-5.5", "anthropic/claude-sonnet-5.5"]);
    }

    #[test]
    fn the_default_model_is_the_newest_haiku() {
        let model = |id: &str| Model {
            id: id.to_string(),
            name: id.to_string(),
            images: true,
        };
        let models = vec![
            model("openai/gpt-5"),
            model("anthropic/claude-sonnet-5.5"),
            model("anthropic/claude-3.5-haiku"),
            model("anthropic/claude-haiku-5.5"),
            // Seen on 2026-10-15: the batch variant sorts after the plain one.
            model("anthropic/claude-haiku-5.5:batch"),
            model("anthropic/claude-haiku-4.5"),
            model("anthropic/claude-opus-4.1"),
        ];
        assert_eq!(
            default_model(&models).map(|model| model.id.as_str()),
            Some("anthropic/claude-haiku-5.5")
        );
        assert_eq!(
            default_model(&models[..1]).map(|model| model.id.as_str()),
            Some("openai/gpt-5")
        );
    }
}
