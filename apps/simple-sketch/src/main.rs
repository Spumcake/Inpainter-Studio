//! Simple Sketch as a sub-application. It has no window. The host draws the
//! shell and the ink; this process owns arrangement, save, and undo.

use std::io::{BufRead, Write};
use std::path::Path;

use protocol::{
    AppMessage, BarControl, ChoiceControl, ContentsKind, HostMessage, MenuControl, MenuRow,
    PanelSide, PropSection, SliderControl, ToolButton, ToolbarGroup,
};
use serde::Serialize;
use sketch::document::SketchId;
use sketch::registry::{self, BarControl as ToolBar, CommandId, SelectItem, SketchMenuRow, Tool};
use sketch::schema::{self, SectionId};
use sketch::session::{FileError, SaveResult, Session};

fn main() {
    let mut session = Session::new();
    let mut stdout = std::io::stdout();
    write_greeting(&session, &mut stdout);
    let _ = stdout.flush();

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        let Ok(message) = protocol::parse_host_message(&line) else {
            continue;
        };
        match message {
            HostMessage::Shutdown => break,
            HostMessage::Command {
                id,
                viewport,
                path,
                replace,
                discard,
                tool,
            } => {
                apply_command(
                    &mut session,
                    id.as_deref(),
                    viewport,
                    path.as_deref(),
                    replace.unwrap_or(false),
                    discard.unwrap_or(false),
                    tool.as_deref(),
                    &mut stdout,
                );
                let _ = stdout.flush();
            }
            HostMessage::Document { sketch, step } => {
                if session.record_document(&sketch, step) && step {
                    write_document(&session, &mut stdout);
                    write_menu(&session, &mut stdout);
                    let _ = stdout.flush();
                }
            }
        }
    }
}

fn write_greeting(session: &Session, out: &mut impl Write) {
    write_message(
        out,
        &AppMessage::Hello {
            protocol: protocol::PROTOCOL_VERSION,
        },
    );
    write_tabs(session, out);
    write_menu(session, out);
    write_toolbar(session, out);
    write_properties(session, out);
    write_message(out, &AppMessage::End);
    write_documents(session, out);
}

fn write_tabs(session: &Session, out: &mut impl Write) {
    for tab in session.tabs() {
        let id = tab_id(tab.id);
        write_message(
            out,
            &AppMessage::Tab {
                id: id.clone(),
                label: tab.name.to_string(),
            },
        );
        write_message(
            out,
            &AppMessage::Contents {
                tab: id.clone(),
                kind: ContentsKind::Grid,
            },
        );
        write_message(
            out,
            &AppMessage::Panel {
                tab: id,
                side: PanelSide::Right,
            },
        );
    }
}

fn write_menu(session: &Session, out: &mut impl Write) {
    let rows = registry::sketch_menu()
        .iter()
        .map(|row| match row {
            SketchMenuRow::Command(id) => MenuRow::Command {
                command: snake(*id),
                enabled: session.command_enabled(*id),
            },
            SketchMenuRow::Recent => MenuRow::Recent { recent: true },
        })
        .collect();
    write_message(out, &AppMessage::Menu { rows });
}

fn write_toolbar(session: &Session, out: &mut impl Write) {
    let mut groups = vec![
        command_group(registry::file_commands()),
        command_group(registry::history_commands()),
        command_group(registry::canvas_commands()),
        ToolbarGroup::Tools {
            tools: registry::tools()
                .iter()
                .map(|tool| ToolButton {
                    id: snake(tool.id),
                    current: tool.id == session.tool(),
                })
                .collect(),
        },
    ];
    if session.tool() == Tool::Select {
        let controls = registry::select_actions()
            .iter()
            .map(|item| match item {
                SelectItem::Button(id) => BarControl::Command {
                    command: snake(*id),
                },
                SelectItem::Menu(menu) => BarControl::Menu {
                    menu: MenuControl {
                        label: menu.label.to_string(),
                        icon: menu.icon.to_string(),
                        commands: menu.commands.iter().copied().map(snake).collect(),
                    },
                },
            })
            .collect();
        groups.push(ToolbarGroup::Controls { controls });
    } else {
        let bar = registry::tool(session.tool()).bar;
        if !bar.is_empty() {
            groups.push(ToolbarGroup::Controls {
                controls: bar.iter().map(bar_control).collect(),
            });
        }
    }
    groups.push(command_group(registry::view_commands()));
    write_message(out, &AppMessage::Toolbar { groups });
}

fn write_properties(session: &Session, out: &mut impl Write) {
    let sections = registry::tool(session.tool())
        .sections
        .iter()
        .copied()
        .map(prop_section)
        .collect();
    write_message(out, &AppMessage::Properties { sections });
}

fn write_documents(session: &Session, out: &mut impl Write) {
    let active = session.active_id().get();
    let dirty = session.active_dirty();
    for sketch in session.snapshots() {
        let id = sketch.get("id").and_then(|value| value.as_u64());
        write_message(
            out,
            &AppMessage::Document {
                dirty: id == Some(active) && dirty,
                sketch,
            },
        );
    }
}

fn write_document(session: &Session, out: &mut impl Write) {
    write_message(
        out,
        &AppMessage::Document {
            sketch: session.active_snapshot(),
            dirty: session.active_dirty(),
        },
    );
}

fn apply_command(
    session: &mut Session,
    id: Option<&str>,
    viewport: Option<[f64; 2]>,
    path: Option<&str>,
    replace: bool,
    discard: bool,
    tool: Option<&str>,
    out: &mut impl Write,
) {
    if id.is_none()
        && let Some(name) = tool
        && let Some(tool) = parse_tool(name)
    {
        session.set_tool(tool);
        write_toolbar(session, out);
        write_properties(session, out);
        return;
    }
    let Some(name) = id else {
        return;
    };
    let Some(command) = parse_command(name) else {
        return;
    };
    let viewport = viewport.unwrap_or([0.0, 0.0]);
    let tabs_before = tab_labels(session);
    match command {
        CommandId::Open => {
            let Some(path) = path else {
                return;
            };
            if !session.open_path(Path::new(path)) {
                write_file_error(session, out);
            }
        }
        CommandId::Save | CommandId::SaveAs | CommandId::SaveCopy => {
            let Some(path) = path else {
                return;
            };
            let copy = command == CommandId::SaveCopy;
            match session.save_path(session.active_id(), Path::new(path), copy, replace) {
                SaveResult::Wrote => {}
                SaveResult::NeedsReplace(path) => {
                    write_message(
                        out,
                        &AppMessage::Result {
                            id: tab_id(session.active_id()),
                            path: Some(path.display().to_string()),
                            saving: None,
                            file: None,
                            reason: None,
                        },
                    );
                    return;
                }
                SaveResult::Failed => {
                    write_file_error(session, out);
                    return;
                }
            }
        }
        CommandId::Close => {
            if discard {
                let _ = session.discard(session.active_id());
            } else {
                let _ = session.request_close(session.active_id());
            }
        }
        _ => {
            let _ = session.perform(command, viewport);
        }
    }
    let tabs_after = tab_labels(session);
    if tabs_after != tabs_before {
        write_tabs(session, out);
        write_menu(session, out);
        write_documents(session, out);
        return;
    }
    write_document(session, out);
    write_menu(session, out);
    if session.tool() != Tool::Brush || command == CommandId::Undo || command == CommandId::Redo {
        write_toolbar(session, out);
        write_properties(session, out);
    }
}

fn write_file_error(session: &mut Session, out: &mut impl Write) {
    let Some(FileError {
        saving,
        file,
        reason,
    }) = session.take_file_error()
    else {
        return;
    };
    write_message(
        out,
        &AppMessage::Result {
            id: tab_id(session.active_id()),
            path: None,
            saving: Some(saving),
            file: Some(file),
            reason: Some(reason),
        },
    );
}

fn tab_labels(session: &Session) -> Vec<(String, String)> {
    session
        .tabs()
        .iter()
        .map(|tab| (tab_id(tab.id), tab.name.to_string()))
        .collect()
}

fn tab_id(id: SketchId) -> String {
    id.get().to_string()
}

fn command_group(ids: &[CommandId]) -> ToolbarGroup {
    ToolbarGroup::Commands {
        commands: ids.iter().copied().map(snake).collect(),
    }
}

fn bar_control(control: &ToolBar) -> BarControl {
    match control {
        ToolBar::Size => BarControl::Size { size: true },
        ToolBar::Divider => BarControl::Divider { divider: true },
        ToolBar::Slider {
            prop,
            label,
            min,
            max,
        } => BarControl::Slider {
            slider: SliderControl {
                prop: snake(*prop),
                label: (*label).to_string(),
                min: *min,
                max: *max,
            },
        },
        ToolBar::Choice { prop, options } => BarControl::Choice {
            choice: ChoiceControl {
                prop: snake(*prop),
                options: options
                    .iter()
                    .map(|(value, label)| ((*value).to_string(), (*label).to_string()))
                    .collect(),
            },
        },
        ToolBar::List => BarControl::List { list: true },
    }
}

fn prop_section(id: SectionId) -> PropSection {
    let desc = schema::section(id);
    PropSection {
        id: snake(id),
        props: desc.props.iter().copied().map(snake).collect(),
    }
}

fn snake(value: impl Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        _ => String::new(),
    }
}

fn parse_command(name: &str) -> Option<CommandId> {
    serde_json::from_value(serde_json::Value::String(name.to_string())).ok()
}

fn parse_tool(name: &str) -> Option<Tool> {
    serde_json::from_value(serde_json::Value::String(name.to_string())).ok()
}

fn write_message(out: &mut impl Write, message: &AppMessage) {
    let _ = out.write_all(protocol::encode_app_message(message).as_bytes());
}
