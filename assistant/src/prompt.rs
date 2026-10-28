//! What the model is told and offered. The fixed text is generic; everything
//! about an application comes from what it declared.

use crate::Target;
use serde_json::{Map, Value, json};

/// The fixed part of the system prompt. It names no application, skill, or
/// tool.
pub const SYSTEM: &str = "\
You are the assistant in Inpainter, a desktop program for creative work. \
The person's work is open in tabs, and each tab belongs to one of the applications listed below. \
You work on that work only through the tools offered to you. \
A tool takes a `tab` when its application has open tabs: use the selected tab unless the request says otherwise. \
An application with no open tabs offers its tools without a tab.

- Read or look at the work before you change it, so you work from what is there now.
- Keep what the person made. Change only what the request asks for.
- Never say you did something you did not do. When a tool fails, say so.
- When no tool can do what is asked, say so plainly, and change nothing.
- Answer briefly: say what you did, or what you found.";

/// The note that begins each request.
pub const STALE: &str = "The person may have changed the work since the last request, \
so anything read before this message may be out of date.";

/// The system prompt for one model turn: the fixed text, then each running
/// application with its tabs and skills.
pub fn system(targets: &[Target], selected: Option<&(String, String)>) -> String {
    let mut text = String::from(SYSTEM);
    if targets.is_empty() {
        text.push_str("\n\nNo application is running, so no tool is offered.");
        return text;
    }
    text.push_str("\n\n# Applications running now");
    for target in targets {
        text.push_str(&format!("\n\n## {} (id `{}`)\n", target.name, target.app));
        if target.tabless {
            text.push_str("Its tools take no `tab`.\n");
        } else if target.tabs.is_empty() {
            text.push_str("Open tabs: none, so its tools are not offered.\n");
        } else {
            text.push_str("Open tabs:\n");
            for tab in &target.tabs {
                let mark = match selected {
                    Some((app, id)) if *app == target.app && *id == tab.id => " (selected)",
                    _ => "",
                };
                text.push_str(&format!("- \"{}\", tab `{}`{mark}\n", tab.label, tab.id));
            }
        }
        if target.skills.is_empty() {
            text.push_str("Skills: none.\n");
        }
        for skill in &target.skills {
            text.push_str(&format!("\n### Skill: {}\n", skill.name));
            if !skill.description.trim().is_empty() {
                text.push_str(skill.description.trim());
                text.push('\n');
            }
            if !skill.instructions.trim().is_empty() {
                text.push('\n');
                text.push_str(skill.instructions.trim());
                text.push('\n');
            }
            let keys: Vec<String> = skill
                .tools
                .iter()
                .map(|tool| format!("`{}`", protocol::tool_key(&target.app, &skill.id, &tool.name)))
                .collect();
            if !keys.is_empty() {
                text.push_str(&format!("\nIts tools: {}.\n", keys.join(", ")));
            }
        }
    }
    text.trim_end().to_string()
}

/// Every tool of every target, as OpenAI-style functions. A target with tabs
/// gets a required `tab` chosen from those tabs. A target with none does not.
pub fn tools(targets: &[Target]) -> Vec<Value> {
    let mut offered = Vec::new();
    for target in targets {
        if target.skills.is_empty() || (!target.tabless && target.tabs.is_empty()) {
            continue;
        }
        let tabs: Vec<Value> = target.tabs.iter().map(|tab| json!(tab.id)).collect();
        for skill in &target.skills {
            for tool in &skill.tools {
                let parameters = if target.tabless {
                    tool.input.clone()
                } else {
                    with_tab(&tool.input, &tabs)
                };
                offered.push(json!({
                    "type": "function",
                    "function": {
                        "name": protocol::tool_key(&target.app, &skill.id, &tool.name),
                        "description": tool.description,
                        "parameters": parameters,
                    }
                }));
            }
        }
    }
    offered
}

/// The tool's input schema, as an object, with `tab` added and required.
fn with_tab(input: &Value, tabs: &[Value]) -> Value {
    let mut schema = match input {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    };
    schema.insert(String::from("type"), json!("object"));
    let mut properties = match schema.remove("properties") {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    properties.insert(
        String::from("tab"),
        json!({
            "type": "string",
            "enum": tabs,
            "description": "The id of the tab to work on.",
        }),
    );
    schema.insert(String::from("properties"), Value::Object(properties));
    let mut required = vec![json!("tab")];
    if let Some(Value::Array(names)) = schema.get("required") {
        required.extend(names.iter().filter(|name| name.as_str() != Some("tab")).cloned());
    }
    schema.insert(String::from("required"), Value::Array(required));
    Value::Object(schema)
}
