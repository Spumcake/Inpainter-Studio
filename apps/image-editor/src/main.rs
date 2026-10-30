//! The Image Editor: one layered image per tab. The host draws the picture
//! and owns the camera and the pointer; this process owns every pixel,
//! layer, selection, step of history, and file.

mod assist;
mod declare;
mod doc;
mod files;
mod paint;
mod picture;
mod session;

use session::{Session, respond};
use std::io::{BufRead, Write};

fn main() {
    let restore = std::env::args().any(|arg| arg == protocol::RESTORE_FLAG);
    let mut session = if restore { Session::restored() } else { Session::launch() };
    let greeting = session.greeting();
    send(&greeting);
    // Lines arrive on their own thread, so the loop also wakes with no
    // message and writes a project's changes that are due.
    let (lines, incoming) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else {
                break;
            };
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    loop {
        let line = match incoming.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(line) => line,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                send(&session.tick());
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if line.contains("\"type\":\"shutdown\"") {
            break;
        }
        let replies = respond(&mut session, &line);
        let mut out = std::io::stdout().lock();
        for reply in replies {
            let _ = out.write_all(reply.as_bytes());
        }
        let _ = out.flush();
    }
    // The host is gone: whatever a project has not written yet is written.
    session.write_all();
}

fn send(messages: &[protocol::AppMessage]) {
    let mut out = std::io::stdout().lock();
    for message in messages {
        let _ = out.write_all(protocol::encode_app_message(message).as_bytes());
    }
    let _ = out.flush();
}
