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
2. In `apps/<id>/Cargo.toml`, set the package `name` to the id. Leave the dependencies as they are: `protocol` only.
3. In `apps/<id>/manifest.json`, set `id` and `executable` to the id and `name` to the name. Leave `protocol` as it is.
4. In `apps/<id>/src/main.rs`, set the tab's `id` to the id and its `label` to the name, and change every other tab value that still says template to that same id. A scaffold keeps the contents message and the panel message, because the template declares the shared grid and the properties panel on the right. Remove one of those messages only when the task says this sub-application does not use it. Add no other messages.
5. In `apps/<id>/AGENTS.md`, change the heading to the name and replace the first paragraph with one sentence. If the task states what the sub-application is for, use that as the one sentence. If it does not, use: This sub-application is a scaffold copied from the template. Keep the rules unchanged.
6. Add `"apps/<id>"` to `members` in the workspace `Cargo.toml`.

## Check

- `cargo build` succeeds.
- `cargo run -p inpainter -- --app <id>` opens the Inpainter window with one tab labeled with the name, the empty toolbar, and only the shared components the sub-application declares. The host's common UI is all there: the tab strip with its menu button and plus, and the toolbar.
- With no host, `printf '{"type":"shutdown"}\n' | target/debug/<id>` prints the declaration, starting with `{"type":"hello","protocol":1}` and ending with `{"type":"end"}`, and exits with status 0.

## What a scaffold does not include

No tools, commands, menu rows, properties, documents, or saving. When the sub-application needs one of these, the work happens inside its own folder, and the host draws it only from what the sub-application declares. Never add a label, icon, control, or rule for one sub-application to the host.

## When the protocol cannot say something

The protocol in `protocol/` is the only way the host and a sub-application talk, and its source is the reference for every message. When a sub-application needs something the protocol cannot express, extend the protocol, teach the host to draw or handle it, and update this skill, all in the same change. Raise `PROTOCOL_VERSION` when an existing message changes meaning. Change the template only when every sub-application needs the change; it stays the smallest one that works. Do not work around the protocol with a side channel, a shared file, an environment variable, or a special case in the host.

## Bringing an existing program in

Scaffold first, so the host already shows the new sub-application's tab. Then, before writing code, map what carries over from the existing program: each file, function, constant, and asset it needs, and where it lands. Carry code over unchanged, or with named edits that the new structure requires. Live drawing, which the host does with its shared drawing code, is separated from the document, saving, and undo, which stay with the sub-application. Do not rewrite what the existing program already does.

## Finish

Report the id and name, the files created or changed, and the result of each check.
