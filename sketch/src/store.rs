//! `.smx` sketches and recovery copies. A `.smx` file is the snapshot JSON at
//! a path the person chooses. Recovery lives in `recovery/` next to
//! preferences. Leftover `sketch-<id>.json` files stay where they are: their
//! ids are reserved, and they are neither opened nor deleted.

use crate::document::{Allocator, Sketch, SketchId};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub fn normalize(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The same file, including two paths that canonicalize together.
pub fn same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

pub fn ensure_smx(path: PathBuf) -> PathBuf {
    let already = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("smx"));
    if already {
        path
    } else {
        let mut name = path.into_os_string();
        name.push(".smx");
        PathBuf::from(name)
    }
}

/// The tab title: the file's name without `.smx`.
pub fn file_title(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::trim)
        .filter(|stem| !stem.is_empty())
        .unwrap_or("Untitled")
        .to_string()
}

/// Reads a sketch. The error is a sentence for the person: why it could not
/// be read, or that the file is not a sketch.
pub fn read_smx(path: &Path) -> Result<Sketch, String> {
    let bytes = std::fs::read(path).map_err(|error| reason(&error))?;
    String::from_utf8(bytes)
        .ok()
        .and_then(|text| crate::document::snapshot::sketch_from_text(&text))
        .ok_or_else(|| "It is not a Simple Sketch file, or it is damaged.".to_string())
}

pub fn readable_smx(path: &Path) -> bool {
    read_smx(path).is_ok()
}

/// Writes the snapshot. The stored name is `name`. The path is not in the file.
/// A failed write leaves any previous file as it was, and the error is a
/// sentence for the person.
pub fn write_smx(path: &Path, sketch: &Sketch, name: &str) -> Result<(), String> {
    let bytes = smx_bytes(sketch, name).ok_or_else(|| "The sketch could not be encoded.".to_string())?;
    write_atomic_bytes(path, &bytes).map_err(|error| reason(&error))
}

/// Why a file operation failed, in words for the person.
pub fn reason(error: &std::io::Error) -> String {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::PermissionDenied => "You do not have permission to use this location.".to_string(),
        ErrorKind::NotFound => "The file or its folder no longer exists.".to_string(),
        ErrorKind::StorageFull => "The disk is full.".to_string(),
        ErrorKind::ReadOnlyFilesystem => "This location is read-only.".to_string(),
        _ => {
            // "Input/output error (os error 5)" becomes "Input/output error."
            let text = error.to_string();
            let text = text.split(" (os error").next().unwrap_or(&text).trim();
            format!("{text}.")
        }
    }
}

fn smx_bytes(sketch: &Sketch, name: &str) -> Option<Vec<u8>> {
    let mut value = sketch.snapshot_value();
    let object = value.as_object_mut()?;
    object.insert("name".to_string(), Value::String(name.to_string()));
    object.remove("path");
    serde_json::to_vec_pretty(&value).ok()
}

pub fn recovery_path(dir: &Path, id: SketchId) -> PathBuf {
    dir.join("recovery").join(format!("{}.json", id.get()))
}

/// One recovery copy. The file name is the sketch id. `path` is included when
/// the sketch has a file. The sketch's `.smx` is not opened or written.
pub fn write_recovery(dir: &Path, sketch: &Sketch) -> bool {
    let Some(bytes) = recovery_bytes(sketch) else {
        return false;
    };
    write_atomic_bytes(&recovery_path(dir, sketch.id()), &bytes).is_ok()
}

pub fn recovery_bytes(sketch: &Sketch) -> Option<Vec<u8>> {
    let mut value = sketch.snapshot_value();
    let object = value.as_object_mut()?;
    if let Some(path) = sketch.path().and_then(|path| path.to_str()) {
        object.insert("path".to_string(), Value::String(path.to_string()));
    } else {
        object.remove("path");
    }
    serde_json::to_vec_pretty(&value).ok()
}

pub fn delete_recovery(dir: &Path, id: SketchId) {
    let _ = std::fs::remove_file(recovery_path(dir, id));
}

pub struct LoadedRecovery {
    pub sketch: Sketch,
    pub path: Option<PathBuf>,
}

/// A recovery file whose snapshot id matches its name. Anything else is absent.
pub fn read_recovery(dir: &Path, id: SketchId) -> Option<LoadedRecovery> {
    let text = std::fs::read_to_string(recovery_path(dir, id)).ok()?;
    let (sketch, path) = snapshot_and_path(&text)?;
    if sketch.id() != id {
        return None;
    }
    Some(LoadedRecovery { sketch, path })
}

/// Valid recovery files, ascending id. Unreadable files and id mismatches are
/// absent. The caller offers them one at a time.
pub fn recovery_offers(dir: &Path) -> Vec<(SketchId, String)> {
    let mut offers = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir.join("recovery")) else {
        return offers;
    };
    for entry in entries.flatten() {
        let Some(id) = recovery_id(&entry.path()) else {
            continue;
        };
        let Some(loaded) = read_recovery(dir, id) else {
            continue;
        };
        let name = loaded
            .path
            .as_deref()
            .map(file_title)
            .unwrap_or_else(|| loaded.sketch.name().to_string());
        offers.push((id, name));
    }
    offers.sort_by_key(|(id, _)| id.get());
    offers
}

/// The next sketch id moves above every leftover `sketch-<id>.json` name and
/// every recovery file name, including a file that cannot be read. Node ids
/// move above those in a readable snapshot. Nothing is deleted or opened.
pub fn reserve(dir: &Path, ids: &mut Allocator, next_sketch: &mut u64) {
    reserve_named(dir, "sketch-", ".json", ids, next_sketch);
    reserve_named(&dir.join("recovery"), "", ".json", ids, next_sketch);
}

fn reserve_named(
    dir: &Path,
    prefix: &str,
    suffix: &str,
    ids: &mut Allocator,
    next_sketch: &mut u64,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name_id) = id_from_name(&entry.path(), prefix, suffix) else {
            continue;
        };
        *next_sketch = (*next_sketch).max(name_id.get() + 1);
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Some((sketch, _)) = snapshot_and_path(&text) else {
            continue;
        };
        *next_sketch = (*next_sketch).max(sketch.id().get() + 1);
        sketch.reserve_ids(ids);
    }
}

fn recovery_id(path: &Path) -> Option<SketchId> {
    id_from_name(path, "", ".json")
}

fn id_from_name(path: &Path, prefix: &str, suffix: &str) -> Option<SketchId> {
    let name = path.file_name()?.to_str()?;
    let rest = name.strip_prefix(prefix)?.strip_suffix(suffix)?;
    if rest.is_empty() || !rest.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let id: u64 = rest.parse().ok()?;
    if id == 0 {
        return None;
    }
    Some(SketchId::new(id))
}

fn snapshot_and_path(text: &str) -> Option<(Sketch, Option<PathBuf>)> {
    let value: Value = serde_json::from_str(text).ok()?;
    let path = value.as_object().and_then(path_from);
    let sketch = crate::document::snapshot::sketch_from_text(text)?;
    Some((sketch, path))
}

fn path_from(object: &Map<String, Value>) -> Option<PathBuf> {
    let text = object.get("path")?.as_str()?;
    if text.is_empty() {
        None
    } else {
        Some(PathBuf::from(text))
    }
}

/// Writes a temporary file beside `path`, flushes it to the disk, then
/// renames it over `path`, so a crash or power loss leaves either the old
/// file or the new one, never a partial one.
fn write_atomic_bytes(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension(match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => format!("{ext}.tmp"),
        None => "tmp".to_string(),
    });
    let written = std::fs::File::create(&temporary).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    if let Err(error) = written.and_then(|()| std::fs::rename(&temporary, path)) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    // The rename itself reaches the disk with the folder. Not every
    // platform can open a folder for this, so a failure here is ignored.
    if let Ok(folder) = std::fs::File::open(parent) {
        let _ = folder.sync_all();
    }
    Ok(())
}

static EMERGENCY: Mutex<Vec<(PathBuf, Vec<u8>)>> = Mutex::new(Vec::new());

/// Replaces the copies a panic hook would write. Does not touch the disk.
pub fn publish_emergency(files: Vec<(PathBuf, Vec<u8>)>) {
    let mut guard = EMERGENCY.lock().unwrap_or_else(|err| err.into_inner());
    *guard = files;
}

/// Writes the last published recovery copies. The panic hook calls this, and
/// a test can call it instead of panicking.
pub fn flush_emergency() {
    let files = {
        let guard = EMERGENCY.lock().unwrap_or_else(|err| err.into_inner());
        guard.clone()
    };
    for (path, bytes) in files {
        let _ = write_atomic_bytes(&path, &bytes);
    }
}
