use protocol::{
    AppMessage, ContentsKind, HostMessage, PanelSide, Skill, TabEntry, Tabs, Tool, ToolResult,
};
use serde_json::json;
use std::io::{BufRead, Write};

const TAB: &str = "template";

fn main() {
    // A workspace being reopened starts this with `--restore`: no tab
    // until the host sends `open`.
    let mut open = !std::env::args().any(|arg| arg == protocol::RESTORE_FLAG);
    let mut label = String::from("Template");

    let mut greeting = vec![AppMessage::Hello {
        protocol: protocol::PROTOCOL_VERSION,
    }];
    if open {
        greeting.push(AppMessage::Tab {
            id: TAB.to_string(),
            label: label.clone(),
        });
        greeting.extend(declare_tab());
    }
    greeting.push(AppMessage::Skills { skills: skills() });
    greeting.push(AppMessage::End);
    send(&greeting);

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        // A ping is answered so the host knows this program is responding,
        // and with nothing unsaved, a quit is ready at once.
        let replies = match protocol::parse_host_message(&line) {
            Ok(HostMessage::Shutdown) => break,
            Ok(HostMessage::Ping) => vec![AppMessage::Pong],
            Ok(HostMessage::Quit) => vec![AppMessage::QuitReady],
            Ok(HostMessage::Claim { id, .. }) => {
                vec![AppMessage::Claimed { id, yours: false }]
            }
            // The one tab, declared as the greeting declares it, then listed.
            Ok(HostMessage::Open { .. }) if !open => {
                open = true;
                let mut replies = declare_tab();
                replies.push(tabs(&label));
                replies
            }
            Ok(HostMessage::Invoke {
                call, tool, input, ..
            }) => {
                let new = input.get("label").and_then(|label| label.as_str()).map(str::trim);
                match new.filter(|new| open && !new.is_empty() && tool.ends_with("__rename_tab")) {
                    Some(new) => {
                        label = new.to_string();
                        let done = json!({ "label": label });
                        vec![tabs(&label), result(call, true, done)]
                    }
                    None => {
                        let error = if open { "give a label that is not empty" } else { "the tab is not open yet" };
                        vec![result(call, false, json!({ "error": error }))]
                    }
                }
            }
            _ => continue,
        };
        send(&replies);
    }
}

/// What the tab shows: the shared grid and the properties panel on the right.
fn declare_tab() -> Vec<AppMessage> {
    vec![
        AppMessage::Contents {
            tab: TAB.to_string(),
            kind: ContentsKind::Grid,
        },
        AppMessage::Panel {
            tab: TAB.to_string(),
            side: PanelSide::Right,
        },
    ]
}

/// The tab list: one tab, which is not a document saved to a file.
fn tabs(label: &str) -> AppMessage {
    AppMessage::Tabs(Tabs {
        tabs: vec![TabEntry {
            id: TAB.to_string(),
            label: label.to_string(),
            dirty: false,
            close_waits: false,
            path: None,
            saves: false,
        }],
        active: TAB.to_string(),
        new_tab: None,
        new_tab_tip: String::new(),
    })
}

/// What the assistant can do here: rename the tab.
fn skills() -> Vec<Skill> {
    vec![Skill {
        id: "tab".to_string(),
        name: "Tab name".to_string(),
        description: "Rename this application's tab.".to_string(),
        instructions: "Call rename_tab with the new label when asked to rename or relabel this tab."
            .to_string(),
        tools: vec![Tool {
            name: "rename_tab".to_string(),
            description: "Set the tab's label.".to_string(),
            input: json!({
                "type": "object",
                "properties": {"label": {"type": "string", "description": "The new label."}},
                "required": ["label"],
                "additionalProperties": false,
            }),
        }],
    }]
}

fn result(call: String, ok: bool, output: serde_json::Value) -> AppMessage {
    AppMessage::ToolResult(ToolResult {
        call,
        ok,
        output,
        images: Vec::new(),
    })
}

fn send(messages: &[AppMessage]) {
    let mut stdout = std::io::stdout().lock();
    for message in messages {
        let _ = stdout.write_all(protocol::encode_app_message(message).as_bytes());
    }
    let _ = stdout.flush();
}
