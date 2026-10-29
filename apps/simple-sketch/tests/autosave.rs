//! Simple Sketch as its own program, driven through the protocol alone.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Catches saving that waits for the host or for quitting: inside a
/// project, a change is in its file two seconds later, even when the
/// program is then killed with no `quit` and no `keep`.
#[test]
fn a_change_survives_a_kill_with_no_host() {
    let folder = std::env::temp_dir().join(format!("simple-sketch-kill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).expect("a scratch folder");
    let mut child = Command::new(env!("CARGO_BIN_EXE_simple-sketch"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("simple-sketch starts");
    let mut input = child.stdin.take().expect("stdin");
    let output = child.stdout.take().expect("stdout");
    let (lines, incoming) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(output).lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let mut tab = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while tab.is_none() && Instant::now() < deadline {
        let Ok(line) = incoming.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        if let Ok(protocol::AppMessage::Tab { id, .. }) = serde_json::from_str(&line) {
            tab = Some(id);
        }
    }
    let tab = tab.expect("a tab");
    let send = |input: &mut std::process::ChildStdin, message: protocol::HostMessage| {
        writeln!(input, "{}", serde_json::to_string(&message).expect("json")).expect("sent");
    };
    send(&mut input, protocol::HostMessage::Project { folder: folder.display().to_string() });
    send(
        &mut input,
        protocol::HostMessage::Command {
            id: "new_canvas".into(),
            tab: Some(tab),
            arg: None,
            node: None,
            viewport: [800.0, 600.0],
        },
    );
    std::thread::sleep(Duration::from_millis(2500));
    child.kill().expect("killed");
    let _ = child.wait();
    assert!(folder.join("Untitled 1.smx").is_file(), "nothing was written before the kill");
    let _ = std::fs::remove_dir_all(folder);
}
