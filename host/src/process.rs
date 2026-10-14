//! The sub-application's process. The host draws a declaration only after `end`.

use std::io::{BufRead, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use protocol::{Accept, AppMessage, Declaration, HostMessage, ParseError};

/// How long a closing window waits for the sub-application to exit after
/// `shutdown` before it stops the process.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// What the reader thread sends. The frame thread is the only writer of the
/// declaration that is drawn.
enum PipeEvent {
    Line(AppMessage),
    Invalid(ParseError),
    Closed,
}

/// The sub-application's process, or the record that it never started.
pub struct Running {
    child: Option<Child>,
    events: Option<Receiver<PipeEvent>>,
    reader: Option<JoinHandle<()>>,
    /// Empty until `Accept::Ready`, then the accepted value.
    shown: Declaration,
    building: Declaration,
    /// True until Ready, Failed, a closed pipe before `end`, or an exit before `end`.
    pending: bool,
    exited: bool,
}

impl Running {
    /// Spawns the sub-application's program, which `apps::discover` found
    /// from its manifest. A failed spawn is the same as
    /// [`Self::not_started`]: the window still opens.
    pub fn start(executable: &std::path::Path) -> Self {
        let mut command = Command::new(executable);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => return Self::not_started(),
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Self::not_started();
        };
        let (tx, rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            let lines = std::io::BufReader::new(stdout).lines();
            for line in lines {
                let event = match line {
                    Ok(line) => match protocol::parse_app_message(&line) {
                        Ok(parsed) => PipeEvent::Line(parsed),
                        Err(error) => PipeEvent::Invalid(error),
                    },
                    Err(_) => {
                        let _ = tx.send(PipeEvent::Closed);
                        return;
                    }
                };
                if tx.send(event).is_err() {
                    return;
                }
            }
            let _ = tx.send(PipeEvent::Closed);
        });
        Self {
            child: Some(child),
            events: Some(rx),
            reader: Some(reader),
            shown: Declaration::empty(),
            building: Declaration::empty(),
            pending: true,
            exited: false,
        }
    }

    /// No process: no sub-application was found, or frame tests that do not
    /// spawn one.
    pub fn not_started() -> Self {
        Self {
            child: None,
            events: None,
            reader: None,
            shown: Declaration::empty(),
            building: Declaration::empty(),
            pending: false,
            exited: true,
        }
    }

    /// No process, showing the declaration these messages build. Frame tests
    /// use this to draw what a sub-application declared without spawning one.
    #[cfg(test)]
    pub fn declared(messages: Vec<AppMessage>) -> Self {
        let mut declaration = Declaration::empty();
        for message in messages {
            if declaration.accept(message) != Accept::More {
                break;
            }
        }
        Self {
            shown: declaration,
            ..Self::not_started()
        }
    }

    pub fn declaration(&self) -> &Declaration {
        &self.shown
    }

    pub fn declaration_mut(&mut self) -> &mut Declaration {
        &mut self.shown
    }

    /// Writes one message to the sub-application. A process that never
    /// started, or whose pipe has closed, drops it.
    pub fn send(&mut self, message: &HostMessage) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        let Some(stdin) = child.stdin.as_mut() else {
            return;
        };
        let _ = stdin.write_all(protocol::encode_host_message(message).as_bytes());
        let _ = stdin.flush();
    }

    /// Drains parsed lines, then checks whether the child has exited.
    /// Lines already queued are applied before that exit is recorded.
    pub fn poll(&mut self) {
        let queued = self.drain_events();
        for event in queued {
            self.apply(event);
        }
        if self.exited {
            return;
        }
        let Some(child) = self.child.as_mut() else {
            return;
        };
        match child.try_wait() {
            Ok(None) => {}
            Ok(Some(_)) | Err(_) => self.note_exit(),
        }
    }

    pub fn pending(&self) -> bool {
        self.pending
    }

    /// This slice keeps the window up and does not draw a status from this.
    #[allow(dead_code)]
    pub fn exited(&self) -> bool {
        self.exited
    }

    /// Tells a still-running sub-application to exit and waits up to
    /// `SHUTDOWN_GRACE`. One that has not exited by then is stopped, so a stuck process
    /// cannot keep the window from closing.
    pub fn shutdown(mut self) {
        if !self.exited {
            if let Some(mut child) = self.child.take() {
                if let Some(mut stdin) = child.stdin.take() {
                    let message = protocol::encode_host_message(&HostMessage::Shutdown);
                    let _ = stdin.write_all(message.as_bytes());
                    let _ = stdin.flush();
                }
                let deadline = Instant::now() + SHUTDOWN_GRACE;
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) if Instant::now() < deadline => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        _ => {
                            let _ = child.kill();
                            let _ = child.wait();
                            break;
                        }
                    }
                }
            }
            self.exited = true;
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }

    fn drain_events(&self) -> Vec<PipeEvent> {
        let Some(events) = &self.events else {
            return Vec::new();
        };
        let mut queued = Vec::new();
        while let Ok(event) = events.try_recv() {
            queued.push(event);
        }
        queued
    }

    fn apply(&mut self, event: PipeEvent) {
        if !self.pending {
            // A later line is applied. A line the parser rejects does not
            // clear the shell or the live copy.
            if let PipeEvent::Line(line) = event {
                let _ = self.shown.accept(line);
            }
            return;
        }
        match event {
            PipeEvent::Line(line) => match self.building.accept(line) {
                Accept::More => {}
                Accept::Ready => {
                    self.shown = std::mem::replace(&mut self.building, Declaration::empty());
                    self.pending = false;
                }
                Accept::Failed => self.abandon(),
            },
            PipeEvent::Invalid(error) => {
                // The failed line is not drawn.
                let _ = error;
                self.abandon();
            }
            PipeEvent::Closed => self.abandon(),
        }
    }

    fn note_exit(&mut self) {
        self.exited = true;
        if self.pending {
            self.abandon();
        }
    }

    fn abandon(&mut self) {
        self.building = Declaration::empty();
        self.shown = Declaration::empty();
        self.pending = false;
    }
}
