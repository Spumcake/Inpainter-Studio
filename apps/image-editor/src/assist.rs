//! The assistant's "Image Editor" skill: it sees the picture and its layers,
//! and manages the layers. Painting stays the person's. Each call goes
//! through the same document changes the Layers page makes, so it honours
//! the same locks, and every call of one request is one undo step.

use crate::doc::Doc;
use crate::files;
use protocol::{AppMessage, Skill, Tool, ToolResult};
use serde_json::{Value, json};

/// The longest side of the picture the assistant looks at.
const LOOK_SIZE: u32 = 1024;

pub fn skills() -> Vec<Skill> {
    let layer = json!({"type": "integer", "description": "The layer's or group's id, from list_layers."});
    let object = |properties: Value, required: &[&str]| {
        json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
    };
    vec![Skill {
        id: "image".into(),
        name: "Image Editor".into(),
        description: "See the image open in an Image Editor tab, and manage its layers: add, rename, hide or show, reorder, and delete them.".into(),
        instructions: "Call list_layers first: it gives each layer's id, name, kind, and settings, top of the stack first, as the Layers panel shows them. Call look to see the picture itself. Positions in move_layer count from the top among the layer's siblings, 0 being the top. A layer whose lock is on refuses what it locks; renaming and deleting are always allowed. You cannot paint; painting stays the person's.".into(),
        tools: vec![
            Tool {
                name: "look".into(),
                description: "See the picture as it is composited now, scaled down to at most 1024 pixels on its longer side.".into(),
                input: object(json!({}), &[]),
            },
            Tool {
                name: "list_layers".into(),
                description: "List the picture's size and every layer and group, top first, with ids, names, visibility, blend mode, opacity, fill, and locks.".into(),
                input: object(json!({}), &[]),
            },
            Tool {
                name: "add_layer".into(),
                description: "Add a clear layer the picture's size at the top of the picture, or at the top of a group. Returns its id.".into(),
                input: object(
                    json!({
                        "name": {"type": "string", "description": "The layer's name."},
                        "group": {"type": "integer", "description": "A group's id to add it inside. Leave out for the top of the picture."},
                    }),
                    &["name"],
                ),
            },
            Tool {
                name: "rename_layer".into(),
                description: "Rename a layer or group.".into(),
                input: object(json!({"layer": layer, "name": {"type": "string", "description": "The new name."}}), &["layer", "name"]),
            },
            Tool {
                name: "set_visible".into(),
                description: "Show or hide a layer or group.".into(),
                input: object(json!({"layer": layer, "visible": {"type": "boolean"}}), &["layer", "visible"]),
            },
            Tool {
                name: "move_layer".into(),
                description: "Move a layer or group up or down among its siblings.".into(),
                input: object(
                    json!({"layer": layer, "to": {"type": "integer", "minimum": 0, "description": "The new position among its siblings, 0 being the top."}}),
                    &["layer", "to"],
                ),
            },
            Tool {
                name: "delete_layer".into(),
                description: "Delete a layer, or a group with everything in it. The picture keeps at least one layer.".into(),
                input: object(json!({"layer": layer}), &["layer"]),
            },
        ],
    }]
}

/// Runs one tool on a tab's document. Returns what to tell the model, and
/// a picture for it to look at.
pub fn run(doc: &mut Doc, tool: &str, input: &Value, group: &str) -> (Result<Value, String>, Option<String>) {
    let layer = || input.get("layer").and_then(Value::as_u64).ok_or_else(|| String::from("Give the layer's id as `layer`."));
    let by = Some(group);
    let outcome = match tool {
        "look" => {
            let (width, height) = (doc.picture.width, doc.picture.height);
            let (small_w, small_h, small) = files::shrink(width, height, doc.composite(), LOOK_SIZE);
            return match files::encode_png(small_w, small_h, &small) {
                Ok(png) => (
                    Ok(json!({"width": width, "height": height, "shown_at": [small_w, small_h]})),
                    Some(files::base64(&png)),
                ),
                Err(reason) => (Err(reason), None),
            };
        }
        "list_layers" => Ok(list(doc)),
        "add_layer" => {
            let name = input.get("name").and_then(Value::as_str).unwrap_or("");
            let group = input.get("group").and_then(Value::as_u64);
            doc.add_layer(name, group, by).map(|id| json!({"id": id}))
        }
        "rename_layer" => match (layer(), input.get("name").and_then(Value::as_str)) {
            (Ok(id), Some(name)) => doc.rename(id, name, by).map(|()| json!({"id": id, "name": name.trim()})),
            (Err(reason), _) => Err(reason),
            (_, None) => Err("Give the new name as `name`.".into()),
        },
        "set_visible" => match (layer(), input.get("visible").and_then(Value::as_bool)) {
            (Ok(id), Some(visible)) => doc.set_visible(id, visible, by).map(|()| json!({"id": id, "visible": visible})),
            (Err(reason), _) => Err(reason),
            (_, None) => Err("Give `visible` as true or false.".into()),
        },
        "move_layer" => match (layer(), input.get("to").and_then(Value::as_u64)) {
            (Ok(id), Some(to)) => doc.reorder(id, to as usize, by).map(|()| json!({"id": id, "to": to})),
            (Err(reason), _) => Err(reason),
            (_, None) => Err("Give the new position as `to`.".into()),
        },
        "delete_layer" => layer().and_then(|id| doc.delete(id, by).map(|()| json!({"deleted": id}))),
        other => Err(format!("The Image Editor has no tool {other}.")),
    };
    (outcome, None)
}

/// Every layer, top first, as the Layers panel lists them.
fn list(doc: &Doc) -> Value {
    fn rows(doc: &Doc, nodes: &[crate::picture::Node], parent: Option<u64>, out: &mut Vec<Value>) {
        let count = nodes.len();
        for (index, node) in nodes.iter().enumerate().rev() {
            let size = doc.picture.pixels.get(&node.id).map(|pixels| [pixels.width, pixels.height]);
            let mut row = json!({
                "id": node.id,
                "name": node.name,
                "kind": if node.is_group() { "group" } else { "layer" },
                "parent": parent,
                "position": count - 1 - index,
                "visible": node.visible,
                "active": doc.active == Some(node.id),
                "blend": node.blend.label(),
                "opacity": (node.opacity * 100.0).round(),
                "locks": {
                    "transparency": node.locks.transparency,
                    "pixels": node.locks.pixels,
                    "position": node.locks.position,
                    "all": node.locks.all,
                },
            });
            if !node.is_group() {
                row["fill"] = json!((node.fill * 100.0).round());
                row["offset"] = json!(node.offset);
                row["size"] = json!(size);
            }
            out.push(row);
            if let Some(children) = &node.children {
                rows(doc, children, Some(node.id), out);
            }
        }
    }
    let mut out = Vec::new();
    rows(doc, &doc.picture.nodes, None, &mut out);
    json!({"width": doc.picture.width, "height": doc.picture.height, "layers": out})
}

pub fn result(call: String, outcome: Result<Value, String>) -> AppMessage {
    result_with(call, outcome, None)
}

pub fn result_with(call: String, outcome: Result<Value, String>, image: Option<String>) -> AppMessage {
    let (ok, output) = match outcome {
        Ok(output) => (true, output),
        Err(error) => (false, json!({"error": error})),
    };
    AppMessage::ToolResult(ToolResult { call, ok, output, images: image.into_iter().collect() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picture::Picture;

    fn call(doc: &mut Doc, tool: &str, input: Value, group: &str) -> Result<Value, String> {
        run(doc, tool, &input, group).0
    }

    /// Catches a tool that does nothing, one that ignores a lock, and calls
    /// of one request that undo one by one.
    #[test]
    fn the_assistant_manages_layers_and_honours_locks() {
        let mut doc = Doc::new(Picture::blank(64, 36));
        let background = doc.active.unwrap();
        let made = call(&mut doc, "add_layer", json!({"name": "Sky"}), "r1").unwrap();
        let sky = made["id"].as_u64().unwrap();
        call(&mut doc, "rename_layer", json!({"layer": sky, "name": "Clouds"}), "r1").unwrap();
        call(&mut doc, "set_visible", json!({"layer": sky, "visible": false}), "r1").unwrap();
        call(&mut doc, "move_layer", json!({"layer": sky, "to": 1}), "r1").unwrap();
        let listed = call(&mut doc, "list_layers", json!({}), "r1").unwrap();
        let layers = listed["layers"].as_array().unwrap();
        assert_eq!(layers[0]["id"], background, "the move did not put Clouds under the background");
        assert_eq!(layers[1]["name"], "Clouds");
        assert_eq!(layers[1]["visible"], false);
        // The four calls are one step.
        assert!(doc.undo());
        assert_eq!(doc.picture.nodes.len(), 1);
        assert!(!doc.can_undo());
        assert!(doc.redo());

        // Lock all refuses a reorder and a layer added into the group it
        // locks; renaming and deleting stay allowed.
        let group = doc.group_active().unwrap();
        doc.toggle_lock(group, 3).unwrap();
        assert!(call(&mut doc, "add_layer", json!({"name": "In", "group": group}), "r2").is_err());
        assert!(call(&mut doc, "move_layer", json!({"layer": group, "to": 1}), "r2").is_err());
        assert!(call(&mut doc, "rename_layer", json!({"layer": group, "name": "Kept"}), "r2").is_ok());
        assert!(call(&mut doc, "delete_layer", json!({"layer": group}), "r2").is_ok());
        assert!(call(&mut doc, "delete_layer", json!({"layer": background}), "r3").is_err(), "the last layer was deleted");
        assert!(call(&mut doc, "paint", json!({}), "r3").is_err());
    }

    /// Catches a look that sends no picture, or one that is not a PNG.
    #[test]
    fn look_sends_the_picture() {
        let mut doc = Doc::new(Picture::blank(2048, 1024));
        let (outcome, image) = run(&mut doc, "look", &json!({}), "r1");
        let outcome = outcome.unwrap();
        assert_eq!(outcome["shown_at"], json!([1024, 512]));
        let image = image.expect("no picture");
        // A PNG's signature, in base64.
        assert!(image.starts_with("iVBORw0KGgo"), "{}", &image[..20]);
        let names: Vec<String> = skills()[0].tools.iter().map(|tool| tool.name.clone()).collect();
        assert_eq!(names, ["look", "list_layers", "add_layer", "rename_layer", "set_visible", "move_layer", "delete_layer"]);
    }
}
