//! The host's new parts for the assistant and the workspace know no
//! sub-application, skill, or tool by name. Everything they show or route
//! comes from declarations and the protocol. The chrome carried over from
//! the reference names its drawing surface and is not searched.

use std::path::Path;

/// The chat panel, the routing between the assistant and sub-applications,
/// the workspace file, and the workspace's flows.
const SEARCHED: &[&str] = &[
    "src/chat.rs",
    "src/route.rs",
    "src/workspace.rs",
    "src/window/flows.rs",
    "src/sessions.rs",
    "src/dialogs.rs",
];

/// Words that would mean one of these parts was written for a particular
/// sub-application or its skills.
const NAMES: &[&str] = &[
    "simple sketch",
    "simple-sketch",
    "sketch",
    "canvas",
    "stroke",
    "shape",
    "circle",
    "template",
];

/// Catches a panel, routing, or workspace that knows a sub-application.
#[test]
fn the_new_host_parts_name_no_sub_application_skill_or_tool() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut found = Vec::new();
    for file in SEARCHED {
        let text = std::fs::read_to_string(root.join(file))
            .unwrap_or_else(|error| panic!("{file}: {error}"));
        for (number, line) in text.lines().enumerate() {
            let lower = line.to_lowercase();
            for name in NAMES {
                if lower.contains(name) {
                    found.push(format!("{file}:{}: {name:?} in {}", number + 1, line.trim()));
                }
            }
        }
    }
    assert!(found.is_empty(), "named:\n{}", found.join("\n"));
}

/// Catches the host depending on a sub-application's crate.
#[test]
fn the_host_depends_on_no_sub_application() {
    let manifest = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("the host's manifest");
    assert!(!manifest.contains("apps/"), "the host depends on a sub-application:\n{manifest}");
}

/// The skills a built sub-application declares, read from its greeting.
fn declared_skills(program: &str) -> Vec<(String, protocol::Skill)> {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/debug").join(program);
    assert!(path.exists(), "{} is missing: run `cargo build` first", path.display());
    let mut child = Command::new(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut skills = Vec::new();
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        if let Ok(protocol::AppMessage::Skills { skills: declared }) =
            protocol::parse_app_message(&line.unwrap())
        {
            skills = declared;
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(!skills.is_empty(), "{program} declared no skills");
    skills.into_iter().map(|skill| (program.to_string(), skill)).collect()
}

/// Every Rust file of a crate's `src`, without its test code: test files,
/// and the inline `mod tests` at a file's end.
fn product_code(src: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut folders = vec![src.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                folders.push(path);
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            // Window checks name the programs they drive. They are not the host.
            let in_checks = path.components().any(|component| component.as_os_str() == "checks");
            if !name.ends_with(".rs") || name == "tests.rs" || name == "checks.rs" || in_checks {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let product = match text.find("#[cfg(test)]\nmod tests") {
                Some(at) => text[..at].to_string(),
                None => text,
            };
            out.push((path.display().to_string(), product));
        }
    }
    out
}

/// Catches a host or an assistant written for particular skills: no skill
/// id, tool name, or tool key that Simple Sketch or the template declares
/// appears in their product code.
#[test]
fn no_declared_skill_or_tool_is_named_in_the_host_or_the_assistant() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut words = Vec::new();
    for (program, skill) in declared_skills("simple-sketch")
        .into_iter()
        .chain(declared_skills("template"))
        // Mission Control declares no skills yet; its own words are searched for below.
    {
        words.push(format!("\"{}\"", skill.id));
        for tool in &skill.tools {
            words.push(format!("\"{}\"", tool.name));
            words.push(protocol::tool_key(&program, &skill.id, &tool.name));
        }
    }
    // `tab` is the protocol's own word: the host gives every tool a `tab`.
    // A skill that happens to share it names nothing the host was written for.
    words.retain(|word| word != "\"tab\"");
    // The application's own name. `cells` and `cell` are the protocol's words.
    words.push(String::from("mission-control"));
    words.push(String::from("mission control"));
    words.push(String::from("Mission Control"));
    let mut found = Vec::new();
    for crate_src in [root.join("src"), root.join("../assistant/src")] {
        for (file, text) in product_code(&crate_src) {
            for word in &words {
                if text.contains(word.as_str()) {
                    found.push(format!("{file}: {word}"));
                }
            }
        }
    }
    assert!(found.is_empty(), "named:\n{}", found.join("\n"));
}
