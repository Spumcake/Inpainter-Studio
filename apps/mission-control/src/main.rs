//! Mission Control: mission books. The host draws and runs the cells.

mod book;

use book::{Session, respond};
use std::io::{BufRead, Write};

fn main() {
    let restore = std::env::args().any(|arg| arg == protocol::RESTORE_FLAG);
    let mut session = if restore { Session::restored() } else { Session::launch() };
    let greeting = session.greeting();
    send(&greeting);
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.contains("\"type\":\"shutdown\"") {
            break;
        }
        for reply in respond(&mut session, &line) {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(reply.as_bytes());
            let _ = out.flush();
        }
    }
}

fn send(messages: &[protocol::AppMessage]) {
    let mut out = std::io::stdout().lock();
    for message in messages {
        let _ = out.write_all(protocol::encode_app_message(message).as_bytes());
    }
    let _ = out.flush();
}
