//! The OpenRouter key from a `.env` file, so the person does not set it in
//! every terminal. Studio reads `<Inpainter data folder>/.env`, then `.env`
//! in the folder it was started from. Only `OPENROUTER_API_KEY` is taken: a
//! stray file must not point Studio, and the key, at another server, or reach
//! the sub-applications' environment. A key already set wins, and so does
//! the first file that sets it. The value is never written anywhere.

use std::path::PathBuf;

/// The files read, in order.
pub(crate) fn files(data: Option<PathBuf>) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = data.into_iter().map(|data| data.join(".env")).collect();
    if let Ok(here) = std::env::current_dir() {
        files.push(here.join(".env"));
    }
    files
}

/// `KEY=value` lines, with `export ` and surrounding quotes allowed. Blank
/// lines and lines starting with `#` are skipped.
pub(crate) fn parse(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let value = value.trim();
        let value = ['"', '\'']
            .iter()
            .find_map(|quote| value.strip_prefix(*quote).and_then(|v| v.strip_suffix(*quote)))
            .unwrap_or(value);
        pairs.push((key.to_string(), value.to_string()));
    }
    pairs
}

/// The only variable a `.env` file may set.
const KEY: &str = "OPENROUTER_API_KEY";

/// Sets the key from the files when it is not set yet. Called first in
/// `main`, before any other thread starts.
pub(crate) fn load(files: &[PathBuf]) {
    for file in files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for (key, value) in parse(&text) {
            if key == KEY && std::env::var_os(&key).is_none() {
                // SAFETY: `main` calls this before it starts any thread.
                unsafe { std::env::set_var(&key, value) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    /// Catches a key read with its quotes, its `export`, or a comment.
    #[test]
    fn env_lines_parse_as_a_shell_would_set_them() {
        let text = "# keys\nexport OPENROUTER_API_KEY=\"sk-or-test\"\nOTHER='two words'\n\nnot a line\n=nokey\n";
        assert_eq!(
            parse(text),
            vec![
                ("OPENROUTER_API_KEY".to_string(), "sk-or-test".to_string()),
                ("OTHER".to_string(), "two words".to_string()),
            ]
        );
    }
}
