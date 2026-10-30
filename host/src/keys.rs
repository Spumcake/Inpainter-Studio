//! Endpoint keys. They go into the system keyring, under the service
//! "Inpainter" and the user `endpoint:<id>`, and never into a file Studio
//! writes elsewhere. Where no keyring answers, or when
//! `INPAINTER_NO_KEYRING` is set, as tests and screenshot scenarios do, they
//! go into `<data folder>/keys.json`, readable only by the person.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const SERVICE: &str = "Inpainter";
const FILE: &str = "keys.json";

/// A key went to the private file this run, so the page says so.
static FELL_BACK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Where a key is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Store {
    Keyring,
    File,
}

fn user(endpoint: &str) -> String {
    format!("endpoint:{endpoint}")
}

fn keyring_allowed() -> bool {
    !cfg!(test) && std::env::var_os("INPAINTER_NO_KEYRING").is_none()
}

fn file(data: &Path) -> PathBuf {
    data.join(FILE)
}

fn read_file(data: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(file(data))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_file(data: &Path, keys: &BTreeMap<String, String>) -> Result<(), String> {
    use std::io::Write;
    std::fs::create_dir_all(data).map_err(|error| error.to_string())?;
    let path = file(data);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut out = options.open(&path).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    let text = serde_json::to_string_pretty(keys).map_err(|error| error.to_string())?;
    out.write_all(text.as_bytes()).map_err(|error| error.to_string())
}

/// Keeps `key` for the endpoint, in the keyring when it answers, otherwise
/// in the private file. Returns where it went.
pub(crate) fn set(data: &Path, endpoint: &str, key: &str) -> Result<Store, String> {
    if keyring_allowed()
        && let Ok(entry) = keyring::Entry::new(SERVICE, &user(endpoint))
        && entry.set_password(key).is_ok()
    {
        // A copy left in the file from before would outlive the change.
        let mut keys = read_file(data);
        if keys.remove(endpoint).is_some() {
            write_file(data, &keys)?;
        }
        return Ok(Store::Keyring);
    }
    let mut keys = read_file(data);
    keys.insert(endpoint.to_string(), key.to_string());
    write_file(data, &keys)?;
    FELL_BACK.store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(Store::File)
}

/// The endpoint's key, from the keyring or the private file.
pub(crate) fn get(data: &Path, endpoint: &str) -> Option<String> {
    if keyring_allowed()
        && let Ok(entry) = keyring::Entry::new(SERVICE, &user(endpoint))
        && let Ok(key) = entry.get_password()
    {
        return Some(key);
    }
    read_file(data).remove(endpoint).filter(|key| !key.is_empty())
}

/// Forgets the endpoint's key, wherever it is.
pub(crate) fn delete(data: &Path, endpoint: &str) {
    if keyring_allowed()
        && let Ok(entry) = keyring::Entry::new(SERVICE, &user(endpoint))
    {
        let _ = entry.delete_credential();
    }
    let mut keys = read_file(data);
    if keys.remove(endpoint).is_some() {
        let _ = write_file(data, &keys);
    }
}

/// The keys the window has read, so the keyring is asked once per
/// endpoint. With no data folder, as in frame tests, keys live only here.
#[derive(Default)]
pub(crate) struct Keys {
    data: Option<PathBuf>,
    read: std::collections::HashMap<String, Option<String>>,
}

impl Keys {
    pub(crate) fn new(data: Option<PathBuf>) -> Self {
        Self { data, read: Default::default() }
    }

    pub(crate) fn get(&mut self, endpoint: &str) -> Option<String> {
        if let Some(known) = self.read.get(endpoint) {
            return known.clone();
        }
        let key = self.data.as_deref().and_then(|data| get(data, endpoint));
        self.read.insert(endpoint.to_string(), key.clone());
        key
    }

    /// Keeps a key, or forgets it when `key` is empty.
    pub(crate) fn set(&mut self, endpoint: &str, key: &str) -> Result<(), String> {
        let key = key.trim();
        if let Some(data) = &self.data {
            if key.is_empty() {
                delete(data, endpoint);
            } else {
                set(data, endpoint, key)?;
            }
        }
        self.read
            .insert(endpoint.to_string(), Some(key.to_string()).filter(|key| !key.is_empty()));
        Ok(())
    }

    pub(crate) fn delete(&mut self, endpoint: &str) {
        if let Some(data) = &self.data {
            delete(data, endpoint);
        }
        self.read.insert(endpoint.to_string(), None);
    }
}

/// Whether keys go to the private file, so the page can say so.
pub(crate) fn uses_file() -> bool {
    if !keyring_allowed() || FELL_BACK.load(std::sync::atomic::Ordering::Relaxed) {
        return true;
    }
    keyring::Entry::store_status().is_err()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Catches a key written where anyone can read it: a test never reaches
    /// the keyring, and the private file is the person's alone.
    #[test]
    fn a_key_is_kept_privately_and_forgotten() {
        let data = std::env::temp_dir().join(format!("inpainter-keys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        assert_eq!(set(&data, "e1", "sk-test").unwrap(), Store::File);
        assert_eq!(get(&data, "e1").as_deref(), Some("sk-test"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(data.join(FILE)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        delete(&data, "e1");
        assert_eq!(get(&data, "e1"), None);
        let _ = std::fs::remove_dir_all(&data);
    }
}
