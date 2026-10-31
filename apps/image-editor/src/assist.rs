//! The assistant's "Image Editor" skill: it sees the tab's pictures and
//! their layers, arranges the gallery, and manages the shown picture's
//! layers. Painting stays the person's. Each call goes through the same
//! changes the filmstrip and the Layers page make, so it honours the same
//! locks. Every layer call of one request is one undo step of the picture;
//! each gallery call is one step of the strip's history.

use crate::doc::Doc;
use crate::files;
use crate::gallery::Gallery;
use protocol::{AppMessage, Skill, Tool, ToolResult};
use serde_json::{Value, json};

/// The longest side of the picture the assistant looks at.
const LOOK_SIZE: u32 = 1024;

pub fn skills() -> Vec<Skill> {
    let layer = json!({"type": "integer", "description": "The layer's or group's id, from list_layers."});
    let picture = json!({"type": "integer", "minimum": 0, "description": "The picture's position on the filmstrip, from list_pictures, 0 being the first."});
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
        description: "See the pictures in an Image Editor tab's gallery, arrange them on its filmstrip (show, add, duplicate, rename, reorder, and remove pictures), and manage the shown picture's layers: add, rename, hide or show, reorder, and delete them.".into(),
        instructions: "An Image Editor tab is a gallery: a filmstrip of pictures, one of them shown. Call list_pictures to see them in order, with which is shown; pictures are named by their position, 0 being the first, and positions change when pictures are added, moved, or removed. The layer tools act on the picture shown, so call show_picture first to work on another. Call list_layers first: it gives each layer's id, name, kind, and settings, top of the stack first, as the Layers panel shows them. Call look to see a picture itself. Positions in move_layer count from the top among the layer's siblings, 0 being the top. A layer whose lock is on refuses what it locks; renaming and deleting are always allowed. The gallery keeps at least one picture. You cannot paint; painting stays the person's.".into(),
        tools: vec![
            Tool {
                name: "look".into(),
                description: "See a picture as it is composited now, scaled down to at most 1024 pixels on its longer side: the shown one, or another.".into(),
                input: object(json!({"picture": {"type": "integer", "minimum": 0, "description": "The picture's position, from list_pictures. Leave out for the picture shown."}}), &[]),
            },
            Tool {
                name: "list_pictures".into(),
                description: "List the gallery's pictures in filmstrip order, with each one's position, name, size, and number of layers, and which is shown.".into(),
                input: object(json!({}), &[]),
            },
            Tool {
                name: "show_picture".into(),
                description: "Show a picture, with its layers, as a click on its thumbnail does.".into(),
                input: object(json!({"picture": picture}), &["picture"]),
            },
            Tool {
                name: "add_picture".into(),
                description: "Add a new white 1920 × 1080 picture at the end of the filmstrip, and show it. Returns its position and name.".into(),
                input: object(
                    json!({"name": {"type": "string", "description": "The picture's name. Leave out for \"Untitled Asset\"; a name already used gets a number."}}),
                    &[],
                ),
            },
            Tool {
                name: "duplicate_picture".into(),
                description: "Copy a picture, with its layers, beside it, named \"… copy\", and show the copy. Returns its position and name.".into(),
                input: object(json!({"picture": picture}), &["picture"]),
            },
            Tool {
                name: "rename_picture".into(),
                description: "Rename a picture.".into(),
                input: object(json!({"picture": picture, "name": {"type": "string", "description": "The new name."}}), &["picture", "name"]),
            },
            Tool {
                name: "move_picture".into(),
                description: "Move a picture along the filmstrip.".into(),
                input: object(
                    json!({"picture": picture, "to": {"type": "integer", "minimum": 0, "description": "The picture's new position, 0 being the first."}}),
                    &["picture", "to"],
                ),
            },
            Tool {
                name: "remove_picture".into(),
                description: "Remove a picture from the gallery. The person can undo it from the filmstrip. The last picture cannot be removed.".into(),
                input: object(json!({"picture": picture}), &["picture"]),
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

/// Runs one tool on a tab's gallery: a gallery tool on the gallery, `look`
/// on the picture it names, and a layer tool on the picture shown. Returns
/// what to tell the model, and a picture for it to look at.
pub fn run(gallery: &mut Gallery, tool: &str, input: &Value, group: &str) -> (Result<Value, String>, Option<String>) {
    let picture = || {
        input.get("picture").and_then(Value::as_u64).map(|index| index as usize).ok_or_else(|| String::from("Give the picture's position as `picture`."))
    };
    let named = |index: usize, gallery: &Gallery| json!({"picture": index, "name": gallery.frames[index].name});
    let outcome = match tool {
        "look" => {
            let index = match input.get("picture") {
                None | Some(Value::Null) => gallery.shown,
                Some(_) => match picture() {
                    Ok(index) if index < gallery.frames.len() => index,
                    Ok(index) => return (Err(format!("There is no picture {index}; the gallery holds {}.", gallery.frames.len())), None),
                    Err(reason) => return (Err(reason), None),
                },
            };
            let (outcome, look) = run_doc(&mut gallery.frames[index].doc, tool, input, group);
            let outcome = outcome.map(|mut output| {
                output["picture"] = json!(index);
                output["name"] = json!(gallery.frames[index].name);
                output
            });
            return (outcome, look);
        }
        "list_pictures" => Ok(json!({
            "shown": gallery.shown,
            "pictures": gallery
                .frames
                .iter()
                .enumerate()
                .map(|(index, frame)| json!({
                    "picture": index,
                    "name": frame.name,
                    "width": frame.doc.picture.width,
                    "height": frame.doc.picture.height,
                    "layers": frame.doc.picture.all_layers().len(),
                    "shown": index == gallery.shown,
                }))
                .collect::<Vec<_>>(),
        })),
        "show_picture" => picture().and_then(|index| gallery.select(index).map(|()| named(index, gallery))),
        "add_picture" => {
            let name = input.get("name").and_then(Value::as_str).filter(|name| !name.trim().is_empty()).unwrap_or(crate::session::UNTITLED);
            let index = gallery.add(name, crate::session::new_doc(), None);
            Ok(named(index, gallery))
        }
        "duplicate_picture" => picture().and_then(|index| gallery.duplicate(index)).map(|index| named(index, gallery)),
        "rename_picture" => match (picture(), input.get("name").and_then(Value::as_str)) {
            (Ok(index), Some(name)) => gallery.rename(index, name).map(|()| named(index, gallery)),
            (Err(reason), _) => Err(reason),
            (_, None) => Err("Give the new name as `name`.".into()),
        },
        "move_picture" => match (picture(), input.get("to").and_then(Value::as_u64)) {
            (Ok(index), Some(to)) => {
                let to = (to as usize).min(gallery.frames.len().saturating_sub(1));
                gallery.move_to(index, to).map(|()| named(to, gallery))
            }
            (Err(reason), _) => Err(reason),
            (_, None) => Err("Give the new position as `to`.".into()),
        },
        "remove_picture" => picture().and_then(|index| {
            let name = gallery.frames.get(index).map(|frame| frame.name.clone());
            gallery.remove(index).map(|()| json!({"removed": index, "name": name, "shown": gallery.shown}))
        }),
        _ => return run_doc(&mut gallery.frame_mut().doc, tool, input, group),
    };
    (outcome, None)
}

/// Runs one tool on a picture's document.
pub fn run_doc(doc: &mut Doc, tool: &str, input: &Value, group: &str) -> (Result<Value, String>, Option<String>) {
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
        run_doc(doc, tool, &input, group).0
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
        let (outcome, image) = run_doc(&mut doc, "look", &json!({}), "r1");
        let outcome = outcome.unwrap();
        assert_eq!(outcome["shown_at"], json!([1024, 512]));
        let image = image.expect("no picture");
        // A PNG's signature, in base64.
        assert!(image.starts_with("iVBORw0KGgo"), "{}", &image[..20]);
        let names: Vec<String> = skills()[0].tools.iter().map(|tool| tool.name.clone()).collect();
        assert_eq!(
            names,
            [
                "look", "list_pictures", "show_picture", "add_picture", "duplicate_picture", "rename_picture",
                "move_picture", "remove_picture", "list_layers", "add_layer", "rename_layer", "set_visible", "move_layer",
                "delete_layer",
            ]
        );
    }

    /// Catches a gallery tool that does nothing, one that is more or less
    /// than one step of the strip's history, a look that ignores `picture`,
    /// and a layer tool that acts on a picture not shown.
    #[test]
    fn the_assistant_arranges_the_gallery_one_step_each() {
        let mut gallery = Gallery::one("Untitled Asset", Doc::new(Picture::blank(64, 36)));
        let call = |gallery: &mut Gallery, tool: &str, input: Value| run(gallery, tool, &input, "r1").0;
        let added = call(&mut gallery, "add_picture", json!({"name": "Beach"})).unwrap();
        assert_eq!(added, json!({"picture": 1, "name": "Beach"}));
        let copy = call(&mut gallery, "duplicate_picture", json!({"picture": 1})).unwrap();
        assert_eq!(copy, json!({"picture": 2, "name": "Beach copy"}));
        call(&mut gallery, "rename_picture", json!({"picture": 0, "name": "Sky"})).unwrap();
        call(&mut gallery, "move_picture", json!({"picture": 0, "to": 2})).unwrap();
        call(&mut gallery, "add_picture", json!({})).unwrap();
        call(&mut gallery, "remove_picture", json!({"picture": 3})).unwrap();
        let listed = call(&mut gallery, "list_pictures", json!({})).unwrap();
        let names: Vec<&str> = listed["pictures"].as_array().unwrap().iter().map(|row| row["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["Beach", "Beach copy", "Sky"]);
        assert_eq!(listed["pictures"][0]["width"], 1920, "an added picture is not 1920 wide");

        // A layer tool acts on the picture shown, and look on the one named.
        call(&mut gallery, "show_picture", json!({"picture": 2})).unwrap();
        assert_eq!(gallery.frame().name, "Sky");
        call(&mut gallery, "add_layer", json!({"name": "Notes"})).unwrap();
        assert_eq!(gallery.frames[2].doc.picture.nodes.len(), 2);
        assert_eq!(gallery.frames[0].doc.picture.nodes.len(), 1, "a layer tool reached a picture not shown");
        let (looked, image) = run(&mut gallery, "look", &json!({"picture": 0}), "r1");
        assert_eq!(looked.unwrap()["width"], 1920);
        assert!(image.is_some());
        assert!(run(&mut gallery, "look", &json!({"picture": 9}), "r1").0.is_err());
        assert!(call(&mut gallery, "show_picture", json!({"picture": 9})).is_err());

        // Six gallery calls, six steps: each undo takes back exactly one.
        let mut counts = Vec::new();
        while gallery.undo() {
            counts.push(gallery.frames.len());
        }
        assert_eq!(counts, [4, 3, 3, 3, 2, 1]);
        assert_eq!(gallery.frames[0].name, "Untitled Asset");
        // The last picture stays.
        assert!(call(&mut gallery, "remove_picture", json!({"picture": 0})).is_err());
    }
}
