# AGENTS.md — Inpainter Studio

Inpainter Studio is the open-source computer graphics (CG) authoring application behind the Inpainter platform. One host program draws a shared shell, and each kind of work is a separate sub-application that the host starts and draws through one protocol. This repository holds the code, and what Studio is lives in `docs/overview.md`. The product's mission and version documents live in the team repository, https://github.com/Spumcake/Inpainter-Team.

The stack is Rust, egui through eframe, and Iconoir icons.

## Layout

- `host/` — the `inpainter` program: the window, the session's conversation and the session menu (`sessions.rs`), the shell's common UI (tab strip, menu button, toolbar) shown when the workspace is open, the projects and their sessions (`workspace.rs`, `window/flows.rs`), the routing of the assistant's tool calls to sub-applications, the shared components a sub-application can declare (the properties panel with its outliner, the grid, the drawing surface, and cells), the questions and file dialogs a sub-application asks for, and finding and running sub-applications. It draws every toolbar item, menu row, panel row, tab, and cell from what was declared, and sends back the action each control carries. It runs every cell. It never depends on a sub-application.
- `protocol/` — the contract between the host and sub-applications: the messages, one JSON object per line, the declarations, the cell, the skills a sub-application provides to the assistant, and the manifest.
- `assistant/` — the assistant: the conversation, the agent loop, and the OpenRouter client. It runs on its own thread, and the host talks to it through the types in `assistant/src/lib.rs`. A chat answer can arrive in parts. It knows no sub-application, skill, or tool by name; everything it offers the model comes from declared skills, including the one tool the host declares.
- `draw/` — the shared document model of the drawing surface: documents, steps, ink, text, the camera, and gestures. The host draws and previews with it; a sub-application that owns such documents commits and records with it. It has no session, history, registry, store, or preferences.
- `apps/<id>/` — one sub-application per folder: its crate, its `manifest.json`, and its own `AGENTS.md`. It depends on `protocol`, and on `draw` when it uses the drawing surface, never on the host or another sub-application.
- `apps/template/` — the smallest correct sub-application. New ones are copied from it.
- `apps/mission-control/` — notebooks. It stores cells and declares them. The host draws and runs them.
- `.project/scripts/check.sh` — the one check to run before calling work done (see Working rules).
- `client/` — `sketch-client`, the no-host check: it drives `simple-sketch` through the protocol alone, including every skill tool, one undo per request, and `open` and `keep`, and compares every reply with an in-process session. Its coverage check fails when a hand action that changes a sketch has no tool.

## Rules

- The host and the assistant know sub-applications only through their manifests and the protocol. Do not put a sub-application's name, label, tool, skill, or rule in either.
- Every sub-application runs with no host, driven only through the protocol.
- A change to the protocol updates the **scaffolding** skill in the same commit.

## Working rules for agents

These hold for every agent building Studio, in every session, as the owner set on 2026-10-29.

- **Done means checked.** Before calling any work done, run `.project/scripts/check.sh`. It builds everything, runs the whole workspace's tests and the no-host check, and refuses files that were only reformatted. It must end with "All checks passed." Report its output, not a partial run such as `-p inpainter`.
- **Never run `cargo fmt` or `rustfmt`.** The code is not machine-formatted. Match the formatting around each edit by hand.
- **Fix your own mistakes without asking.** That covers a compile error in any crate, a failing or newly failing test, a boundary test, and stray changes. These are not questions for the owner.
- **A change you did not make is someone else's.** The owner or another agent may edit files while you work, this one included. Keep their changes, never revert them, reread any instructions that changed, and carry on.
- **Ask the owner only for two things.** The first is a decision that changes the product, meaning what a person sees or can do, or stored data, dependencies, or spending. The second is a contract in the version's `systems.md` that proves wrong. Decide everything else inside the approved tasks and systems, and record each departure as "As built" in `systems.md`.
- **Build a version through to the end.** Work through every step of the handover without stopping between them, and report once, when the whole build passes the check. A reviewer checks the build after that.
- **Never touch the owner's own data.** Tests and scenarios use scratch folders for the data folder and for projects, never `~/.local/share/inpainter` or `~/Inpainter`. Make no real model calls. Run screenshots only on their own virtual display, never on the owner's screen, and never script the desktop's file dialogs.
- **Do not commit or push** unless the owner asks.

## Skills

Developer skills for working on this code are in `skills/`, one folder each with a `SKILL.md`. Read the one that matches the task before starting.

- `skills/scaffolding/` — when a task creates a new sub-application, or brings an existing program into Inpainter as one.

These are for agents developing Inpainter. They are not the assistant skills a sub-application declares to Inpainter's assistant through the protocol.

## Commands

From the repository root: `cargo build`, `cargo test`, and `cargo run -p inpainter`. `cargo run -p inpainter` builds only the host, so run `cargo build` first whenever a sub-application changed; the host starts the sub-application programs already built in `target/`. `cargo run -p inpainter -- --app <id>` chooses which sub-application the host starts; `--app` may be given more than once, and every one named runs in the same window. Without `--app`, every installed sub-application starts. `target/debug/sketch-client` runs the no-host check after a build. Run tests with `TMPDIR=$PWD/target/tmp` after `cargo build`: the host's checks start the built `simple-sketch`, `template`, and `mission-control` programs and use a scripted stand-in for OpenRouter. The stand-in answers a chat in parts. The assistant needs `OPENROUTER_API_KEY` in the environment; without it, a request replies that no key is set. With `INPAINTER_TRACE=1`, the host writes every message to and from sub-applications to standard error. A project is a folder the person chooses, by default under `~/Inpainter/`. Each holds its activities as `.smx` files and Studio's own `.inpainter/` folder: `project.json` and one file per session in `sessions/`, with paths relative to the project. The data folder, `~/.local/share/inpainter/` on this computer, keeps `projects.json`, the list of projects and which session is current. Projects from before 0.7 move into folders of their own at the first start, and their old files are kept in `projects-before-0.7/`. The logo menu's only row is Quit.
