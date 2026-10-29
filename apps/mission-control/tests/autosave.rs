//! Mission Control as its own program, driven through the protocol alone.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Catches saving that waits for the host or for quitting: inside a
/// project, a changed notebook is in its file two seconds later, even when
/// the program is then killed with no `quit` and no `keep`.
#[test]
fn a_change_survives_a_kill_with_no_host() {
    let folder = std::env::temp_dir().join(format!("mission-control-kill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).expect("a scratch folder");
    let mut child = Command::new(env!("CARGO_BIN_EXE_mission-control"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("mission-control starts");
    let mut input = child.stdin.take().expect("stdin");
    let output = child.stdout.take().expect("stdout");
    std::thread::spawn(move || for _ in BufReader::new(output).lines() {});
    for message in [
        protocol::HostMessage::Project { folder: folder.display().to_string() },
        protocol::HostMessage::Command {
            id: "add_cell".into(),
            tab: Some("1".into()),
            arg: None,
            node: None,
            viewport: [800.0, 600.0],
        },
    ] {
        writeln!(input, "{}", serde_json::to_string(&message).expect("json")).expect("sent");
    }
    std::thread::sleep(Duration::from_millis(2500));
    child.kill().expect("killed");
    let _ = child.wait();
    assert!(folder.join("Untitled Notebook.smx").is_file(), "nothing was written before the kill");
    let _ = std::fs::remove_dir_all(folder);
}
