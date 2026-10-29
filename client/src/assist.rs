//! The assistant's side of the protocol, with no host and no model: every
//! tool of Simple Sketch's skills through `invoke`, one undo per request,
//! and a workspace's `--restore`, `open`, `keep`, and `kept`. Each reply is
//! compared with an in-process `Server` given the same messages.

use crate::{Link, VIEWPORT, brush_gesture, canvas_and_source, command, document_of, exchange, gesture};
use protocol::{AppMessage, FileKind, HostMessage, Skill};
use serde_json::{Value, json};
use simple_sketch::serve::Server;
use std::path::{Path, PathBuf};

/// One process, its in-process twin, and the tab being worked on.
struct Pair {
    link: Link,
    oracle: Server,
    tab: String,
    calls: u64,
    /// The last document sent for `tab`.
    shown: Value,
}

impl Pair {
    fn exchange(&mut self, message: HostMessage) -> Result<Vec<AppMessage>, String> {
        let replies = exchange(&mut self.link, &mut self.oracle, message)?;
        if let Some((tab, sketch, _)) = document_of(&replies)
            && tab == self.tab
        {
            self.shown = sketch;
        }
        Ok(replies)
    }

    /// One tool, as the host sends it for the assistant. Returns the
    /// result's `ok`, `output`, the number of pictures, and every reply.
    fn invoke(&mut self, tool: &str, input: Value, group: &str) -> Result<(bool, Value, usize, Vec<AppMessage>), String> {
        self.calls += 1;
        let call = format!("call-{}", self.calls);
        let replies = self.exchange(HostMessage::Invoke {
            call: call.clone(),
            tab: self.tab.clone(),
            tool: protocol::tool_key("simple-sketch", tool.split_once("__").map_or("", |(skill, _)| skill), tool.split_once("__").map_or(tool, |(_, name)| name)),
            input,
            group: group.to_string(),
        })?;
        let position = replies
            .iter()
            .position(|message| matches!(message, AppMessage::ToolResult(_)))
            .ok_or_else(|| format!("{tool} sent no result"))?;
        if position + 1 != replies.len() {
            return Err(format!("{tool} sent messages after its result"));
        }
        let AppMessage::ToolResult(result) = &replies[position] else {
            unreachable!();
        };
        if result.call != call {
            return Err(format!("{tool}'s result answers another call"));
        }
        Ok((result.ok, result.output.clone(), result.images.len(), replies))
    }

    /// A tool that must succeed.
    fn ok(&mut self, tool: &str, input: Value, group: &str) -> Result<Value, String> {
        let (ok, output, _, _) = self.invoke(tool, input, group)?;
        if !ok {
            return Err(format!("{tool} failed: {output}"));
        }
        Ok(output)
    }

    /// A tool that must change nothing and say why.
    fn refused(&mut self, tool: &str, input: Value, group: &str) -> Result<(), String> {
        let (ok, output, _, replies) = self.invoke(tool, input, group)?;
        if ok || output.get("error").and_then(Value::as_str).is_none() {
            return Err(format!("{tool} should have refused, and said why: {output}"));
        }
        if document_of(&replies).is_some() {
            return Err(format!("{tool} refused but sent a changed document"));
        }
        Ok(())
    }

}

fn made(output: &Value, kind: &str) -> Result<(u64, Vec<(String, u64)>), String> {
    let object = output
        .get("made")
        .and_then(Value::as_array)
        .and_then(|made| made.iter().find(|object| object.get("kind").and_then(Value::as_str) == Some(kind)))
        .ok_or_else(|| format!("no {kind} was reported made: {output}"))?;
    let id = object.get("id").and_then(Value::as_u64).ok_or("a made object has no id")?;
    let palette = object
        .get("palette")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|color| Some((color.get("role")?.as_str()?.to_string(), color.get("id")?.as_u64()?)))
        .collect();
    Ok((id, palette))
}

fn role(palette: &[(String, u64)], name: &str) -> Result<u64, String> {
    palette
        .iter()
        .find(|(role, _)| role == name)
        .map(|(_, id)| *id)
        .ok_or_else(|| format!("no {name} color"))
}

/// The objects of a document, front to back, without the camera.
fn contents(document: &Value) -> Value {
    document.get("nodes").cloned().unwrap_or(Value::Null)
}

fn objects_in(document: &Value) -> usize {
    document.get("nodes").and_then(Value::as_array).map_or(0, Vec::len)
}

/// Every tool, one request's undo, and a save. Returns the saved file.
pub(crate) fn check(dir: &Path) -> Result<PathBuf, String> {
    let link = Link::start(&[])?;
    let mut pair = Pair {
        link,
        oracle: Server::new(None),
        tab: String::new(),
        calls: 0,
        shown: Value::Null,
    };
    let expected = crate::as_read(pair.oracle.greeting());
    let greeting = pair.link.read_count(expected.len())?;
    if greeting != expected {
        return Err("the declaration with skills does not match the session".into());
    }
    let skills: Vec<Skill> = greeting
        .iter()
        .find_map(|message| match message {
            AppMessage::Skills { skills } => Some(skills.clone()),
            _ => None,
        })
        .ok_or("simple-sketch declared no skills")?;
    let end = greeting.iter().position(|message| matches!(message, AppMessage::End));
    let declared = greeting.iter().position(|message| matches!(message, AppMessage::Skills { .. }));
    if declared > end {
        return Err("the skills are not part of the declaration".into());
    }
    let (tab, first, _) = document_of(&greeting).ok_or("the greeting has no document")?;
    pair.tab = tab;
    pair.shown = first.clone();
    pair.exchange(HostMessage::Viewport { viewport: VIEWPORT })?;

    let mut used = std::collections::BTreeSet::new();
    let mut note = |tool: &str| {
        used.insert(tool.to_string());
    };

    // One request, every tool.
    let r1 = "request-1";
    let read = pair.ok("look__read_sketch", json!({}), r1)?;
    note("look__read_sketch");
    let canvas = read.get("active_canvas").and_then(Value::as_u64).ok_or("no active canvas")?;
    let pink = read
        .get("brush_colors")
        .and_then(Value::as_array)
        .and_then(|colors| colors.iter().find(|color| color.get("name").and_then(Value::as_str) == Some("Pink")))
        .and_then(|color| color.get("id").and_then(Value::as_u64))
        .ok_or("no Pink brush color")?;
    let layer = read["objects_front_to_back"][0]["layers_top_to_bottom"][0]["id"]
        .as_u64()
        .ok_or("the canvas has no layer")?;

    let circle = pair.ok("draw__add_shape", json!({"kind": "ellipse", "from": [0, 0], "to": [100, 100]}), r1)?;
    let (ellipse, ellipse_palette) = made(&circle, "ellipse")?;
    let square = pair.ok("draw__add_shape", json!({"kind": "rectangle", "from": [150, 0], "to": [260, 90], "constrain": true, "properties": {"corner_radius": 6}}), r1)?;
    let (rectangle, _) = made(&square, "rectangle")?;
    let bounds = &square["made"][0]["bounds"];
    let side = |axis: usize| bounds["max"][axis].as_f64().unwrap_or(0.0) - bounds["min"][axis].as_f64().unwrap_or(0.0);
    if (side(0) - side(1)).abs() > 0.5 {
        return Err(format!("constrain did not make a square: {bounds}"));
    }
    let words = pair.ok("draw__add_text", json!({"text": "Hello", "at": [60, -90], "properties": {"font_size": 48}}), r1)?;
    let (text, text_palette) = made(&words, "text")?;
    if words["made"][0]["properties"]["font_size"] != json!(48.0) {
        return Err("add_text did not set its font size".into());
    }
    pair.refused("draw__add_text", json!({"text": "Again", "at": [70, -80]}), r1)?;
    let line = pair.ok("draw__add_shape", json!({"kind": "line", "from": [100, 50], "to": [150, 50]}), r1)?;
    let (line, line_palette) = made(&line, "line")?;
    note("draw__add_shape");
    note("draw__add_text");

    let stroke = pair.ok(
        "draw__draw_stroke",
        json!({"points": [[0, 150, 0.4], [30, 165, 0.6], [60, 150, 0.8], [90, 165, 0.6], [120, 150, 0.4]], "color": pink, "canvas": canvas}),
        r1,
    )?;
    let new_stroke = stroke["new_strokes"][0].as_u64().ok_or("the stroke reported no new stroke")?;
    if stroke.get("new_layers").and_then(Value::as_array).is_none_or(Vec::is_empty) {
        // The Pink layer exists from the canvas's palette; a stroke into it adds none.
    }
    pair.ok("draw__draw_stroke", json!({"points": [[0, 190], [120, 190]], "color": layer}), r1)?;
    pair.refused("draw__draw_stroke", json!({"points": [[0, 0]]}), r1)?;
    note("draw__draw_stroke");
    let erased = pair.ok("draw__erase", json!({"points": [[60, 120], [60, 210]], "size": 12}), r1)?;
    if !erased["erased_strokes"].as_array().is_some_and(|ids| ids.contains(&json!(new_stroke))) {
        return Err(format!("the erase did not cross the stroke: {erased}"));
    }
    note("draw__erase");

    pair.ok("draw__transform", json!({"objects": [ellipse], "move": [10, 0], "scale": 1.2}), r1)?;
    pair.ok("draw__transform", json!({"objects": [rectangle], "rotate": 15, "about": [205, 45]}), r1)?;
    pair.refused("draw__transform", json!({"objects": [text], "rotate": 30}), r1)?;
    note("draw__transform");
    pair.ok("draw__set_text_box", json!({"object": text, "width": 300}), r1)?;
    note("draw__set_text_box");

    let all_text = json!({
        "family": "monospace", "font_size": 40, "bold": true, "italic": true, "underline": true,
        "align": "center", "valign": "middle", "list": "bullets", "line_height": 1.5,
        "letter_spacing": 2, "text_padding": 8, "text_radius": 6, "all_caps": true,
        "auto_size": false, "wrap_text": true
    });
    let styled = pair.ok("style__set_object_properties", json!({"objects": [text], "properties": all_text}), r1)?;
    for key in all_text.as_object().into_iter().flatten().map(|(key, _)| key) {
        if !styled["took"][key].as_array().is_some_and(|ids| ids.contains(&json!(text))) {
            return Err(format!("text did not take {key}: {styled}"));
        }
    }
    pair.ok("style__set_object_properties", json!({"objects": [rectangle], "properties": {"corner_radius": 20}}), r1)?;
    pair.ok("style__set_object_properties", json!({"objects": [canvas], "properties": {"padding": 32, "name": "Sky"}}), r1)?;
    pair.refused("style__set_object_properties", json!({"objects": [line], "properties": {"name": "Bridge"}}), r1)?;
    pair.ok("style__set_object_properties", json!({"objects": [line], "properties": {"locked": true}}), r1)?;
    pair.refused("style__set_object_properties", json!({"objects": [line], "properties": {"visible": false, "locked": true}}), r1)?;
    pair.ok("style__set_object_properties", json!({"objects": [line], "properties": {"locked": false, "visible": false}}), r1)?;
    pair.ok("style__set_object_properties", json!({"objects": [line], "properties": {"visible": true}}), r1)?;
    note("style__set_object_properties");

    let outline = role(&ellipse_palette, "outline")?;
    let fill = role(&ellipse_palette, "fill")?;
    pair.ok("style__set_color_properties", json!({"object": ellipse, "color": outline, "properties": {"rgb": "#ff69b4", "dash": "dashed", "color_size": 6, "opacity": 0.8}}), r1)?;
    pair.ok("style__set_color_properties", json!({"object": ellipse, "color": fill, "properties": {"visible": true, "hex": "#ffe4e1"}}), r1)?;
    pair.ok("style__set_color_properties", json!({"object": line, "color": role(&line_palette, "stroke")?, "properties": {"cap": "flat", "rgb": [20, 40, 200]}}), r1)?;
    pair.ok("style__set_color_properties", json!({"object": text, "color": role(&text_palette, "background")?, "properties": {"visible": true, "rgb": "#fff3b0"}}), r1)?;
    pair.ok("style__set_color_properties", json!({"object": canvas, "color": layer, "properties": {"name": "Ink", "locked": true, "visible": false}}), r1)?;
    pair.ok("style__set_color_properties", json!({"object": canvas, "color": layer, "properties": {"locked": false, "visible": true}}), r1)?;
    pair.refused("style__set_color_properties", json!({"object": ellipse, "color": outline, "properties": {"locked": true}}), r1)?;
    note("style__set_color_properties");
    pair.ok("style__set_text", json!({"object": text, "text": "Hello there"}), r1)?;
    note("style__set_text");

    let selected = pair.ok("edit__select", json!({"mode": "replace", "objects": [ellipse, rectangle]}), r1)?;
    if selected["selection"] != json!([ellipse, rectangle]) {
        return Err(format!("select did not replace the selection: {selected}"));
    }
    pair.ok("edit__select", json!({"mode": "add", "objects": [line]}), r1)?;
    let removed = pair.ok("edit__select", json!({"mode": "remove", "objects": [line]}), r1)?;
    if removed["selection"] != json!([ellipse, rectangle]) {
        return Err(format!("select did not remove: {removed}"));
    }
    pair.ok("edit__select", json!({"mode": "replace", "area": {"min": [-20, -20], "max": [400, 120]}}), r1)?;
    pair.ok("edit__select", json!({"mode": "clear"}), r1)?;
    note("edit__select");
    pair.refused("edit__group", json!({}), r1)?;
    let grouped = pair.ok("edit__group", json!({"objects": [ellipse, rectangle]}), r1)?;
    let (group, _) = made(&grouped, "group")?;
    note("edit__group");
    pair.ok("edit__ungroup", json!({"objects": [group]}), r1)?;
    note("edit__ungroup");
    for order in ["send_to_back", "bring_forward", "bring_to_front", "send_backward"] {
        pair.ok("edit__arrange", json!({"order": order, "objects": [ellipse]}), r1)?;
    }
    note("edit__arrange");
    for edge in ["left", "center", "right"] {
        pair.ok("edit__align", json!({"edge": edge, "objects": [ellipse, rectangle]}), r1)?;
    }
    note("edit__align");
    for turn in ["rotate_clockwise", "rotate_counterclockwise", "rotate_half", "flip_horizontal", "flip_vertical"] {
        pair.ok("edit__turn", json!({"turn": turn, "objects": [rectangle]}), r1)?;
    }
    pair.refused("edit__turn", json!({"turn": "rotate_half", "objects": [text]}), r1)?;
    note("edit__turn");
    for direction in ["left", "right", "up", "down"] {
        pair.ok("edit__nudge", json!({"direction": direction, "objects": [line], "steps": 2, "large": direction == "down"}), r1)?;
    }
    note("edit__nudge");
    pair.refused("edit__clipboard", json!({"action": "paste"}), r1)?;
    let copied = pair.ok("edit__clipboard", json!({"action": "copy", "objects": [line]}), r1)?;
    if copied["copied"] != json!([line]) {
        return Err(format!("copy did not say what it copied: {copied}"));
    }
    let pasted = pair.ok("edit__clipboard", json!({"action": "paste"}), r1)?;
    let (copy, _) = made(&pasted, "line")?;
    pair.ok("edit__clipboard", json!({"action": "cut", "objects": [copy]}), r1)?;
    note("edit__clipboard");
    let deleted = pair.ok("edit__delete", json!({"objects": [line]}), r1)?;
    if !deleted["removed_objects"].as_array().is_some_and(|ids| ids.contains(&json!(line))) {
        return Err(format!("delete did not remove the line: {deleted}"));
    }
    note("edit__delete");
    pair.ok("edit__arrange", json!({"order": "send_to_back", "objects": [ellipse]}), r1)?;
    pair.ok("edit__move_object", json!({"object": ellipse, "position": 0}), r1)?;
    pair.refused("edit__move_object", json!({"object": ellipse, "position": 0}), r1)?;
    note("edit__move_object");
    let fresh = pair.ok("edit__new_canvas", json!({}), r1)?;
    let (canvas_two, _) = made(&fresh, "canvas")?;
    note("edit__new_canvas");
    let active = pair.ok("edit__set_active_canvas", json!({"canvas": canvas}), r1)?;
    if active["active_canvas"] != json!(canvas) {
        return Err("set_active_canvas did not make the canvas active".into());
    }
    pair.refused("edit__set_active_canvas", json!({"canvas": ellipse}), r1)?;
    note("edit__set_active_canvas");
    let duplicated = pair.ok("edit__duplicate_layer", json!({"canvas": canvas, "layer": layer}), r1)?;
    let copy_layer = duplicated["new_layers"][0].as_u64().ok_or("duplicate_layer made no layer")?;
    note("edit__duplicate_layer");
    pair.ok("edit__move_layer", json!({"canvas": canvas, "layer": copy_layer, "position": 3}), r1)?;
    note("edit__move_layer");
    pair.ok("edit__delete_layer", json!({"canvas": canvas, "layer": copy_layer}), r1)?;
    note("edit__delete_layer");
    let _ = canvas_two;

    let (ok, picture, images, _) = pair.invoke("look__look_at", json!({}), r1)?;
    if !ok || images != 1 || picture["width"].as_u64().is_none() {
        return Err(format!("look_at gave no picture: {picture}"));
    }
    let (ok, _, images, _) = pair.invoke("look__look_at", json!({"object": ellipse, "size": 256}), r1)?;
    if !ok || images != 1 {
        return Err("look_at could not picture one object".into());
    }
    let (ok, _, images, _) = pair.invoke("look__look_at", json!({"visible_area": true, "size": 320}), r1)?;
    if !ok || images != 1 {
        return Err("look_at could not picture the visible area".into());
    }
    note("look__look_at");
    pair.refused("look__nothing_here", json!({}), r1)?;

    // Every declared tool was driven.
    for skill in &skills {
        for tool in &skill.tools {
            let key = format!("{}__{}", skill.id, tool.name);
            if !used.contains(&key) {
                return Err(format!("the drive never used {key}"));
            }
        }
    }

    // One undo takes the whole request back, and redo brings it back.
    let after = contents(&pair.shown);
    let tab = pair.tab.clone();
    pair.exchange(command("undo", Some(&tab), None))?;
    if contents(&pair.shown) != contents(&first) {
        return Err("one undo did not take the whole request back".into());
    }
    pair.exchange(command("redo", Some(&tab), None))?;
    if contents(&pair.shown) != after {
        return Err("redo did not bring the request back".into());
    }

    // A hand edit ends a request's step: two requests around a hand stroke
    // are three undo steps.
    let start = objects_in(&pair.shown);
    pair.ok("draw__add_shape", json!({"kind": "rectangle", "from": [400, 0], "to": [450, 40]}), "request-2")?;
    pair.ok("draw__add_shape", json!({"kind": "ellipse", "from": [460, 0], "to": [500, 40]}), "request-2")?;
    pair.exchange(HostMessage::Select {
        tab: tab.clone(),
        op: protocol::SelectOp::Click { node: canvas, add: false },
    })?;
    let (canvas_id, source) = canvas_and_source(&pair.shown).ok_or("no canvas to draw on by hand")?;
    pair.exchange(gesture(&tab, brush_gesture(canvas_id, source)))?;
    pair.ok("draw__add_shape", json!({"kind": "line", "from": [400, 60], "to": [500, 60]}), "request-2")?;
    let strokes = crate::stroke_counts(&pair.shown).0;
    let mut counts = Vec::new();
    for _ in 0..3 {
        pair.exchange(command("undo", Some(&tab), None))?;
        counts.push((objects_in(&pair.shown), crate::stroke_counts(&pair.shown).0));
    }
    if counts != vec![(start + 2, strokes), (start + 2, strokes - 1), (start, strokes - 1)] {
        return Err(format!("requests and a hand edit did not undo one step each: {counts:?}"));
    }

    // Saved like any other work.
    let saved = save(&mut pair, dir, "skills")?;
    pair.link.send(&HostMessage::Shutdown)?;
    pair.link.finish()?;
    Ok(saved)
}

/// Saves the shown sketch through Simple Sketch's own Save, in the process
/// and the session, and checks the two files are the same.
fn save(pair: &mut Pair, dir: &Path, folder: &str) -> Result<PathBuf, String> {
    let replies = pair.exchange(command("save", Some(&pair.tab.clone()), None))?;
    let Some(AppMessage::File { id, kind: FileKind::Save, name: Some(name), .. }) = replies.first().cloned() else {
        return Err("save did not ask where to save".into());
    };
    let process = dir.join(folder).join("process").join(&name);
    let oracle = dir.join(folder).join("oracle").join(&name);
    answer_file(pair, dir, folder, id, Some((&process, &oracle)))?;
    let written = std::fs::read(&process).map_err(|_| "simple-sketch wrote no file")?;
    let reference = std::fs::read(&oracle).map_err(|_| "the session wrote no file")?;
    if written != reference {
        return Err("the file saved after the skills does not match the session's".into());
    }
    Ok(process)
}

/// Answers a file request with a path in each folder, or a cancel, and
/// compares the replies with the folder names made the same.
fn answer_file(
    pair: &mut Pair,
    dir: &Path,
    folder: &str,
    id: String,
    paths: Option<(&Path, &Path)>,
) -> Result<Vec<AppMessage>, String> {
    if let Some((process, oracle)) = paths {
        for path in [process, oracle] {
            std::fs::create_dir_all(path.parent().expect("a folder")).map_err(|e| e.to_string())?;
        }
    }
    pair.link.send(&HostMessage::FileResult {
        id: id.clone(),
        path: paths.map(|(process, _)| process.display().to_string()),
    })?;
    let expected = pair
        .oracle
        .handle(HostMessage::FileResult {
            id,
            path: paths.map(|(_, oracle)| oracle.display().to_string()),
        })
        .ok_or("the session stopped")?;
    let expected = crate::as_read(expected);
    let replies = pair.link.read_count(expected.len())?;
    let same = |messages: &[AppMessage]| -> Vec<String> {
        messages
            .iter()
            .map(|message| {
                protocol::encode_app_message(message).replace(
                    &dir.join(folder).join("process").display().to_string(),
                    &dir.join(folder).join("oracle").display().to_string(),
                )
            })
            .collect()
    };
    if same(&replies) != same(&expected) {
        return Err("the reply to a file does not match the session".into());
    }
    Ok(replies)
}

/// A workspace reopened: started with `--restore`, Simple Sketch opens no
/// sketch until `open`; reopens a saved file and a new untitled sketch; and
/// gives the untitled one a file through its own Save when `keep` asks.
pub(crate) fn workspace(dir: &Path, saved: &Path) -> Result<(), String> {
    let mut pair = Pair {
        link: Link::start(&[protocol::RESTORE_FLAG])?,
        oracle: Server::start(None, true),
        tab: String::new(),
        calls: 0,
        shown: Value::Null,
    };
    let expected = crate::as_read(pair.oracle.greeting());
    let greeting = pair.link.read_count(expected.len())?;
    if greeting != expected {
        return Err("the restoring declaration does not match the session".into());
    }
    if greeting.iter().any(|message| matches!(message, AppMessage::Tab { .. } | AppMessage::Document { .. } | AppMessage::Tabs(_))) {
        return Err("with --restore, simple-sketch opened a sketch of its own".into());
    }
    pair.exchange(HostMessage::Viewport { viewport: VIEWPORT })?;

    let path = saved.display().to_string();
    let replies = pair.exchange(HostMessage::Open { path: Some(path.clone()) })?;
    let tabs = tabs_of(&replies).ok_or("open sent no tabs")?;
    let [opened] = tabs.tabs.as_slice() else {
        return Err("open of a file did not make exactly one tab".into());
    };
    if opened.path.as_deref() != Some(path.as_str()) || !opened.saves {
        return Err("the opened tab does not report its file".into());
    }
    declared_before_tabs(&replies, &opened.id)?;
    let (_, document, camera) = document_of(&replies).ok_or("open sent no document")?;
    if !camera || crate::stroke_counts(&document).0 == 0 {
        return Err("the opened sketch is not the saved one".into());
    }

    let replies = pair.exchange(HostMessage::Open { path: None })?;
    let tabs = tabs_of(&replies).ok_or("open {} sent no tabs")?;
    let untitled = tabs.tabs.iter().find(|tab| tab.path.is_none()).ok_or("open {} made no untitled tab")?.id.clone();
    if tabs.tabs.len() != 2 || tabs.active != untitled {
        return Err("open {} did not add a new untitled sketch".into());
    }
    declared_before_tabs(&replies, &untitled)?;
    pair.tab = untitled.clone();
    pair.ok("draw__add_shape", json!({"kind": "ellipse", "from": [0, 0], "to": [40, 40]}), "request-w")?;

    // A tab that has a file is kept as it is.
    let replies = pair.exchange(HostMessage::Keep { tab: opened.id.clone() })?;
    if !matches!(replies.last(), Some(AppMessage::Kept { tab, path: Some(kept), .. }) if *tab == opened.id && *kept == path) {
        return Err("keep of a saved tab did not answer with its file".into());
    }
    // A cancelled Save keeps no file.
    let replies = pair.exchange(HostMessage::Keep { tab: untitled.clone() })?;
    let Some(AppMessage::File { id, kind: FileKind::Save, name: Some(name), .. }) = replies.first().cloned() else {
        return Err("keep of an untitled sketch did not run its Save".into());
    };
    let replies = answer_file(&mut pair, dir, "keep", id, None)?;
    if !matches!(replies.last(), Some(AppMessage::Kept { tab, path: None, .. }) if *tab == untitled) {
        return Err("a cancelled keep did not answer with no file".into());
    }
    // A chosen file is saved and named in `kept`, after the tabs that show it.
    let replies = pair.exchange(HostMessage::Keep { tab: untitled.clone() })?;
    let Some(AppMessage::File { id, .. }) = replies.first().cloned() else {
        return Err("keep did not ask for a file again".into());
    };
    let process = dir.join("keep").join("process").join(&name);
    let oracle = dir.join("keep").join("oracle").join(&name);
    let replies = answer_file(&mut pair, dir, "keep", id, Some((&process, &oracle)))?;
    let wanted = process.display().to_string();
    if !matches!(replies.last(), Some(AppMessage::Kept { tab, path: Some(kept), .. }) if *tab == untitled && *kept == wanted) {
        return Err("keep did not answer with the file it saved".into());
    }
    let tabs = tabs_of(&replies).ok_or("keep sent no tabs with the new file")?;
    if !tabs.tabs.iter().any(|tab| tab.id == untitled && tab.path.as_deref() == Some(wanted.as_str())) {
        return Err("the kept tab does not report its new file".into());
    }
    if std::fs::read(&process).ok() != std::fs::read(&oracle).ok() || !process.is_file() {
        return Err("the kept file does not match the session's".into());
    }
    pair.link.send(&HostMessage::Shutdown)?;
    pair.link.finish()
}

/// A tab opened by `open` is declared, its contents and panel, before the
/// `tabs` that lists it, as the greeting declares its tabs.
fn declared_before_tabs(replies: &[AppMessage], tab: &str) -> Result<(), String> {
    let at = |wanted: fn(&AppMessage, &str) -> bool| replies.iter().position(|message| wanted(message, tab));
    let contents = at(|message, tab| matches!(message, AppMessage::Contents { tab: id, kind: protocol::ContentsKind::Canvas } if id == tab));
    let panel = at(|message, tab| matches!(message, AppMessage::Panel { tab: id, .. } if id == tab));
    let tabs = at(|message, tab| matches!(message, AppMessage::Tabs(tabs) if tabs.tabs.iter().any(|entry| entry.id == tab)));
    match (contents, panel, tabs) {
        (Some(contents), Some(panel), Some(tabs)) if contents < tabs && panel < tabs => Ok(()),
        _ => Err(format!("the opened tab {tab} was not declared before the tabs that list it")),
    }
}

fn tabs_of(messages: &[AppMessage]) -> Option<protocol::Tabs> {
    messages.iter().rev().find_map(|message| match message {
        AppMessage::Tabs(tabs) => Some(tabs.clone()),
        _ => None,
    })
}
