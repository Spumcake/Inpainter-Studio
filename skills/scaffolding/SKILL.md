---
name: scaffolding
description: Set up a new Inpainter sub-application the conventional way by copying apps/template — its own program and crate, a manifest the host finds, declarations through the protocol, and no dependency on the host. Use when a task needs a new sub-application, or brings an existing program into Inpainter as one. Sets up the skeleton only; the features come after.
---

# Scaffold a sub-application

A sub-application is a separate program that owns one kind of creative work. The host finds it through its manifest, starts it, and draws what it declares through the protocol. This skill sets one up so that it follows those conventions from the first commit. It does not build the sub-application's features. Those come after, as ordinary work inside the scaffold.

Copy the template. Do not write a sub-application from scratch or from a description: `apps/template` is the smallest one that is known to work, and every new one starts as an exact copy of it.

## Choose the names

- **Id:** lowercase letters, digits, and hyphens, such as `simple-sketch`. The folder `apps/<id>`, the crate name, the program name, and the manifest's `id` are all this value.
- **Name:** what people see, such as `Simple Sketch`. It is the label of a tab that has no file.

Check that `apps/<id>` does not exist yet.

## Steps

1. Copy the folder `apps/template` to `apps/<id>`, with every file in it.
2. In `apps/<id>/Cargo.toml`, set the package `name` to the id. Leave the dependencies as they are: `protocol`, and `serde_json` for a skill's input schema and results.
3. In `apps/<id>/manifest.json`, set `id` and `executable` to the id and `name` to the name. Leave `protocol` as it is.
4. In `apps/<id>/src/main.rs`, set `TAB` to the id and the starting `label` to the name. A scaffold keeps the contents message and the panel message, because the template declares the shared grid and the properties panel on the right. Remove one of those messages only when the task says this sub-application does not use it. Keep the template's skill, which renames the tab, until the task gives the sub-application skills of its own; change its `name`, `description`, and `instructions` only to name the new sub-application's tab. Add no other messages.
5. In `apps/<id>/AGENTS.md`, change the heading to the name and replace the first paragraph with one sentence. If the task states what the sub-application is for, use that as the one sentence. If it does not, use: This sub-application is a scaffold copied from the template. Keep the rules unchanged.
6. Add `"apps/<id>"` to `members` in the workspace `Cargo.toml`.

## Check

- `cargo build` succeeds.
- `cargo run -p inpainter -- --app <id>` opens the Inpainter window with one tab labeled with the name, the empty toolbar, and only the shared components the sub-application declares. The host's common UI is all there: the tab strip with its menu button and plus, and the toolbar.
- With no host, `printf '{"type":"shutdown"}\n' | target/debug/<id>` prints the declaration, starting with `{"type":"hello","protocol":1}`, with a `skills` line before it ends with `{"type":"end"}`, and exits with status 0.
- With no host, `printf '{"type":"shutdown"}\n' | target/debug/<id> --restore` prints the declaration with no `tab`, `contents`, or `panel` line. Then `printf '{"type":"open"}\n{"type":"shutdown"}\n' | target/debug/<id> --restore` prints, after the declaration, the tab's `contents` and `panel`, then a `tabs` line listing it.
- With no host, `printf '{"type":"invoke","call":"c1","tab":"<id>","tool":"<id>__tab__rename_tab","input":{"label":"Renamed"},"group":"g1"}\n{"type":"shutdown"}\n' | target/debug/<id>` prints, after the declaration, a `tabs` line with the label Renamed, then `{"type":"result","call":"c1","ok":true,...}`.
- With no host, `printf '{"type":"ping"}\n{"type":"quit"}\n{"type":"shutdown"}\n' | target/debug/<id>` prints the declaration, then `{"type":"pong"}` and `{"type":"quit_ready"}`. The template answers both; a scaffold keeps those answers, or the host shows it as not responding and its window cannot close.

## What a scaffold does not include

No tools, commands, menu rows, properties, documents, or saving, and no skill beyond renaming the tab. When the sub-application needs one of these, the work happens inside its own folder, and the host draws it only from what the sub-application declares. Never add a label, icon, control, or rule for one sub-application to the host.

## What a sub-application can declare later

After `end`, a sub-application can send these at any time; each replaces the one sent before. The types are in `protocol/src/declare.rs`, and Simple Sketch's `src/session/declare.rs` is a complete example.

- `tabs` — the tab list, the active tab, and the action the plus runs. A tab reported here and not in the greeting shows the contents and panel the greeting declared for the first tab.
- `keys`, `toolbar`, `menu`, `properties` — shortcuts, toolbar items, menu rows, and panel rows. Every control carries an `action`; when it is used the host sends `command {id, tab, arg}` with that action's id and argument, adding `value` to the argument when the control has one (a typed name, a slider's number, a sort position). A panel row's edit is `property {prop, value, preview}`. The host never reads an action's id, except the drawing surface's own `canvas.zoom_in`, `canvas.zoom_out`, `canvas.fit`, and `canvas.zoom_to_selection`, and the window's `window.maximize`.
- `contents {kind: "canvas"}`, `document`, and `canvas` — the drawing surface. The host previews each gesture from the declared tool and sends `gesture`, `select`, and `camera`; the sub-application commits and sends the whole `document` back.
- `contents {kind: "cells"}` and `cells {tab, cells, selected}` — a list of cells. The host draws every cell and runs every cell. The sub-application stores them. `run_cell {tab, cell}` and `stop_cell {tab, cell}` ask the host to run or stop one. The host sends `edit_cell` when a prompt is committed, `cell_result` when a run finishes, fails, or is stopped, `insert_cell` for a cell dragged in, and `select_cell` when one is selected. A cell's `kind` is a string. `text` and `image` are the kinds this version runs; an `image` cell's output is a picture, a part `{path, media_type}` whose path is relative to the project. Any other kind is kept and drawn, and Run stays disabled. `running` is not a stored state. A run's `cell_result` names the `endpoint` and `model` it ran on, and its `effort` when one was set; keep all three with the run, so Run again repeats it.
- `ask` and `file` — a question or a file dialog. The host answers with `answer` or `file_result`.
- `quit_ready` after `quit`, and `pong` after `ping`.
- `skills` — what the assistant can do here, replacing the skills sent before. Send it with the declaration, before `end`, and again whenever the skills change. Each skill has an `id`, a `name`, a `description`, `instructions` the assistant reads before using it (units, coordinates, what to read first, how ids work), and `tools`, each with a `name`, a `description`, and `input`, a JSON Schema for an object. The host and the assistant know no skill by name: write these for a model that knows nothing else about the sub-application. The types are in `protocol/src/skills.rs`.

## The assistant and the workspace

The host sends these; a sub-application that declares no skills never receives `invoke`.

- `invoke {call, tab, tool, input, group}` — run one tool on that tab. `tool` is `<application id>__<skill id>__<tool name>`, made by `protocol::tool_key`. Carry it out through the same path the person's hand action takes, so undo, saving, and what the host shows are the same as for hand work. Send any `document`, `tabs`, or other message the call caused first, then `result {call, ok, output, images?}`: `output` is JSON for the model to read, such as what was made and its ids; with `ok: false` nothing changed and `output` says why; `images` holds PNG pictures in standard base64. Every change made under one `group`, the assistant's request, is one undo step, until another group or a change by hand. Undo and redo stay with the person, so they are never tools.
- `open {path?}` — open that file in a new tab or, with no path, the tab the sub-application opens of its own at start. A tab opened this way is declared as the greeting declares tabs: send its `contents` and, when it uses the panel, its `panel`, before the `tabs` that lists it.
- `claim {id, path}` — say whether this file is the application's own work. Answer `claimed {id, yours}`. The host asks the application that opened the dialog first, then the others. The first that claims it receives `open`. A file nobody claims goes to the application that asked, which reports its own error. The host never reads the file to decide.
- `keep {tab}` — make sure that tab's file is written. Answer `kept {tab, path?, error?}`: `path` with no `error` when written, no `path` and no `error` when clean or untouched, and `error` when writing failed.
- Each `tabs` entry carries `path`, the file the tab shows once it has one, and `saves`, whether the tab is a document that can be saved to a file. The template's tab does not save.
- Started with `--restore` (`protocol::RESTORE_FLAG`), a sub-application opens no document of its own: its greeting has no `tab`, `contents`, or `panel`, and it waits for `open`. The host uses this to reopen a workspace.
- `project {folder}` — the current project's folder as an absolute path, sent before any `open`. Inside a project a sub-application that saves documents:
  - writes each document into that folder by itself, about a second after it changes, with no Save, Save As, or save question; Simple Sketch and Mission Control show how, with a loop that also wakes with no message;
  - names a new document's file after its tab, adding " 2", " 3", and on when a file of that name exists in the folder, and never replaces another file;
  - writes everything before a tab closes, before `kept`, and before `quit_ready`, so closing and quitting ask nothing;
  - refuses every path outside the folder, from `open`, `file_result`, `claim`, and its own recent files, and opens nothing in its place.

  The host asks before a file from outside comes in, and sends only the imported copy's path.

The host starts each sub-application with `--data-dir <folder>`, its own folder inside Inpainter's data folder, for its preferences and recovery files.

## When the protocol cannot say something

The protocol in `protocol/` is the only way the host and a sub-application talk, and its source is the reference for every message. When a sub-application needs something the protocol cannot express, extend the protocol, teach the host to draw or handle it, and update this skill, all in the same change. Raise `PROTOCOL_VERSION` when an existing message changes meaning. Change the template only when every sub-application needs the change; it stays the smallest one that works. Do not work around the protocol with a side channel, a shared file, an environment variable, or a special case in the host.

## Bringing an existing program in

Scaffold first, so the host already shows the new sub-application's tab. Then, before writing code, map what carries over from the existing program: each file, function, constant, and asset it needs, and where it lands. Carry code over unchanged, or with named edits that the new structure requires. Live drawing, which the host does with its shared drawing code, is separated from the document, saving, and undo, which stay with the sub-application. Do not rewrite what the existing program already does.

## Finish

Report the id and name, the files created or changed, and the result of each check.
