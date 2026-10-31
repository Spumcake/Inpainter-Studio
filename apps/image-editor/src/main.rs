//! The Image Editor: a gallery of layered pictures per tab. The host draws
//! the picture shown and the filmstrip, and owns the camera and the
//! pointer; this process owns every pixel, layer, selection, step of
//! history, and file.

mod assist;
mod declare;
mod doc;
mod files;
mod gallery;
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
        // Every line already waiting is read before the picture of a stroke
        // being drawn is sent, so painting never falls behind the pen.
        let mut waiting = vec![line];
        while let Ok(line) = incoming.try_recv() {
            waiting.push(line);
        }
        let mut stop = false;
        let mut out = std::io::stdout().lock();
        for line in waiting {
            if line.contains("\"type\":\"shutdown\"") {
                stop = true;
                break;
            }
            for reply in respond(&mut session, &line) {
                let _ = out.write_all(reply.as_bytes());
            }
        }
        for message in session.take_live() {
            let _ = out.write_all(protocol::encode_app_message(&message).as_bytes());
        }
        let _ = out.flush();
        drop(out);
        if stop {
            break;
        }
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
