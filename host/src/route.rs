//! Routing between the assistant and the running sub-applications.
//!
//! The host offers the assistant every running sub-application, with its
//! name, its open tabs, and the skills it declared. When the assistant calls
//! a tool, the host sends `invoke` to the process of that tool's application
//! and passes back the `result` it answers. A process that has exited, or
//! stops answering, gets the call answered for it with `ok: false`. The host
//! never knows a skill or a tool by name.

use crate::window::Started;
use assistant::{Command, TabInfo, Target};
use protocol::{HostMessage, ToolResult};
use serde_json::json;
use std::time::{Duration, Instant};

/// A call with no answer after this long is answered as failed.
const NO_RESULT: Duration = Duration::from_secs(120);

/// One call sent to a sub-application and not yet answered.
struct Pending {
    call: String,
    /// The index of the sub-application in the window's list.
    app: usize,
    sent: Instant,
}

#[derive(Default)]
pub(crate) struct Routing {
    pending: Vec<Pending>,
    /// The targets the assistant was last told about.
    offered: Option<Vec<Target>>,
}

/// The host's own id, for the tool it offers. The assistant is not told to
/// treat this id differently from any other.
pub(crate) const HOST_ID: &str = "inpainter";

/// The host's tool: a prompt that runs as a cell. Learned the same way as
/// any application's skill.
pub(crate) fn host_skill() -> protocol::Skill {
    protocol::Skill {
        id: "prompt".into(),
        name: "Prompt".into(),
        description: "Write a prompt and run it as a cell in the conversation.".into(),
        instructions: "When the person asks for a piece of writing, or to write or run a prompt, call run_text_prompt with the prompt. Do not write the piece in the reply. After the tool returns, reply briefly.".into(),
        tools: vec![protocol::Tool {
            name: "run_text_prompt".into(),
            description: "Show a text prompt as a cell and run it.".into(),
            input: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "The prompt to run."}
                },
                "required": ["prompt"],
                "additionalProperties": false
            }),
        }],
    }
}

fn host_target() -> Target {
    Target {
        app: HOST_ID.into(),
        name: "Inpainter".into(),
        tabs: Vec::new(),
        skills: vec![host_skill()],
        tabless: true,
    }
}

/// Every running sub-application, and the host's own tools, as a request offers them.
pub(crate) fn targets(apps: &[Started]) -> Vec<Target> {
    let mut offered: Vec<Target> = apps
        .iter()
        .filter(|started| !started.running.exited() && !started.running.pending())
        .map(|started| Target {
            app: started.id.clone(),
            name: started.name.clone(),
            tabs: started
                .running
                .tabs()
                .0
                .into_iter()
                .map(|tab| TabInfo {
                    id: tab.id,
                    label: tab.label,
                })
                .collect(),
            skills: started.running.declared.skills.clone(),
            tabless: false,
        })
        .collect();
    offered.push(host_target());
    offered
}

fn failed(call: String, reason: &str) -> ToolResult {
    ToolResult {
        call,
        ok: false,
        output: json!({ "error": reason }),
        images: Vec::new(),
    }
}

impl Routing {
    /// The assistant asks for a tool. Returns an answer at once when the
    /// call cannot be sent.
    pub(crate) fn call(
        &mut self,
        apps: &mut [Started],
        call: String,
        app: &str,
        message: HostMessage,
    ) -> Option<Command> {
        let Some(index) = apps.iter().position(|started| started.id == app) else {
            return Some(Command::Result(failed(
                call,
                "No running application has that id.",
            )));
        };
        let started = &mut apps[index];
        if started.running.exited() {
            return Some(Command::Result(failed(
                call,
                &format!("{} has exited.", started.name),
            )));
        }
        started.running.send(&message);
        self.pending.push(Pending {
            call,
            app: index,
            sent: Instant::now(),
        });
        None
    }

    /// The answers that arrived this frame, and failures for calls whose
    /// process exited, stopped responding, or never answered.
    pub(crate) fn poll(&mut self, apps: &mut [Started]) -> Vec<Command> {
        let mut out = Vec::new();
        for (index, started) in apps.iter_mut().enumerate() {
            while let Some(result) = started.running.declared.results.pop_front() {
                let waiting = self
                    .pending
                    .iter()
                    .position(|pending| pending.app == index && pending.call == result.call);
                // A late answer, after its call was given up, is dropped.
                if let Some(position) = waiting {
                    self.pending.remove(position);
                    out.push(Command::Result(result));
                }
            }
        }
        let mut kept = Vec::new();
        for pending in std::mem::take(&mut self.pending) {
            let reason = match apps.get(pending.app) {
                None => Some("The application is no longer running.".to_string()),
                Some(started) if started.running.exited() => {
                    Some(format!("{} has exited.", started.name))
                }
                Some(started) if started.running.not_responding() => {
                    Some(format!("{} is not responding.", started.name))
                }
                Some(started) if pending.sent.elapsed() >= NO_RESULT => {
                    Some(format!("{} did not answer.", started.name))
                }
                Some(_) => None,
            };
            match reason {
                Some(reason) => out.push(Command::Result(failed(pending.call, &reason))),
                None => kept.push(pending),
            }
        }
        self.pending = kept;
        out
    }

    /// The running sub-applications, when they changed since the assistant
    /// was last told.
    pub(crate) fn changed_targets(&mut self, apps: &[Started]) -> Option<Vec<Target>> {
        let now = targets(apps);
        if self.offered.as_ref() == Some(&now) {
            return None;
        }
        self.offered = Some(now.clone());
        Some(now)
    }

    /// The request ended: no call is waited for any longer.
    pub(crate) fn end(&mut self) {
        self.pending.clear();
    }
}
