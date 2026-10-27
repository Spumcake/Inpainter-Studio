//! Finding sub-applications. Each one is registered by its manifest,
//! `apps/<id>/manifest.json`, and its program sits beside the host's
//! executable. The host knows sub-applications only through these files.

use protocol::Manifest;
use std::path::{Path, PathBuf};

/// A sub-application whose manifest is valid and whose program exists.
pub struct App {
    pub manifest: Manifest,
    pub executable: PathBuf,
}

/// Folders that hold one folder per sub-application, in search order:
/// `apps` beside the host's executable, then, in a development build, the
/// workspace's own `apps` folder, found from the executable's own place.
fn roots(exe_dir: &Path) -> Vec<PathBuf> {
    #[allow(unused_mut)]
    let mut roots = vec![exe_dir.join("apps")];
    // `target/debug/inpainter` sits two folders below the workspace, which
    // stays true when the workspace is moved after it was built.
    #[cfg(debug_assertions)]
    if let Some(workspace) = exe_dir.parent().and_then(Path::parent) {
        roots.push(workspace.join("apps"));
    }
    roots
}

/// Every registered sub-application, ordered by id. An id found again in a
/// later folder is skipped. A manifest that does not parse, or whose program
/// is missing, is reported on stderr and skipped.
pub fn discover() -> Vec<App> {
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    let Some(exe_dir) = exe.parent() else {
        return Vec::new();
    };
    let mut apps: Vec<App> = Vec::new();
    for root in roots(exe_dir) {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut folders: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_dir())
            .collect();
        folders.sort();
        for folder in folders {
            let path = folder.join(protocol::MANIFEST_FILE);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let manifest = match protocol::parse_manifest(&text) {
                Ok(manifest) => manifest,
                Err(error) => {
                    eprintln!("inpainter: {}: {error}", path.display());
                    continue;
                }
            };
            if apps.iter().any(|app| app.manifest.id == manifest.id) {
                continue;
            }
            let executable = exe_dir.join(&manifest.executable);
            if !executable.is_file() {
                eprintln!(
                    "inpainter: {}: no program at {}",
                    manifest.id,
                    executable.display()
                );
                continue;
            }
            apps.push(App {
                manifest,
                executable,
            });
        }
    }
    apps.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    apps
}

/// The sub-applications to start. With no `--app`, every installed one, in id
/// order. With `--app`, each one named, in the order named. An unknown id
/// starts nothing.
pub fn launch() -> Vec<App> {
    let found = discover();
    let requested = requested();
    if requested.is_empty() {
        found
    } else {
        choose(found, &requested)
    }
}

/// The sub-applications `--app` names, in that order. An unknown id starts
/// nothing. An empty `requested` is not used here: [`launch`] starts all of them.
pub fn choose(apps: Vec<App>, requested: &[String]) -> Vec<App> {
    if requested.is_empty() {
        return apps.into_iter().take(1).collect();
    }
    let mut apps = apps;
    let mut chosen = Vec::new();
    for id in requested {
        match apps.iter().position(|app| app.manifest.id == *id) {
            Some(index) => chosen.push(apps.remove(index)),
            None => eprintln!("inpainter: no sub-application with id {id:?}"),
        }
    }
    chosen
}

/// Every `--app <id>` on the host's command line, in order.
pub fn requested() -> Vec<String> {
    let mut ids = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--app"
            && let Some(id) = args.next()
        {
            ids.push(id);
        }
    }
    ids
}

/// A sub-application's own folder in Inpainter's data folder, where it
/// keeps its preferences and recovery copies.
pub fn data_dir(id: &str) -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("io.github", "spumcake", "Inpainter")?;
    Some(dirs.data_dir().join("apps").join(id))
}
