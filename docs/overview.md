# Inpainter Studio — overview

Inpainter Studio is the open-source computer graphics (CG) authoring application behind the Inpainter platform. It is where a person and an assistant shape the staged intermediates of a production together, before anything goes to a final render. It works fully on its own, with the person's own generation endpoints, such as OpenAI, fal.ai, or ComfyUI, or with the Inpainter platform.

Studio is one host window that runs separate sub-applications, each its own program owning one kind of creative work. Simple Sketch, a pressure-sensitive infinite-canvas sketching app, is the first. The person meets Studio as they would an agentic tool such as Codex: it opens on a conversation with the assistant, inside a project, and the work of the sub-applications opens beside that conversation. Each sub-application provides skills that tell the assistant what it can do with its work, and strategies above them describe how to use several sub-applications together. The assistant discovers its capabilities from those skills, plans from them, and carries out the plan. Working with the assistant and working by hand are one seamless flow: every sub-application works fully without the assistant, and with it, the assistant amplifies what the person does by hand.

## What is always true

**The shell.** The person only ever launches the host; the host starts sub-applications for them. The host always draws its own common UI, whatever sub-application is loaded, even one that declares nothing. The window opens on the main conversation, with the project's name at the top. A vertical bar at the window's right edge reveals the workspace. With it open, the conversation becomes a column on the left, about a quarter of the window's width, and the workspace fills the rest: a tab strip with a new-tab control, a menu, a tool area, and the sub-application's status when it stops or stops responding. Sub-applications fill these areas; they never create or remove them. The host also provides shared components that a sub-application chooses to use, such as the properties panel and the grid. The host draws a shared component, always the same way, only where a sub-application declares it; the sub-application declares whether it uses one, what it holds, and, for the panel, which side it sits on.

**The boundaries.** The host and each sub-application are separate programs. The host cannot import an app, and apps never import each other. The code structure enforces this, with separate crates and programs, not documents alone. One process serves every tab of a sub-application, and if it crashes or hangs, the host stays up. A sub-application can run with no host and be driven entirely through the protocol.

**The protocol.** Sub-applications declare their tools, commands, properties, views, and agent skills as data through one protocol. The host draws simple UI from those declarations. An app may draw its own custom view by running egui itself and sending the host what to paint; this stays unproven until an experiment shows that scrubbing a frame strip feels smooth.

**Projects.** Every conversation belongs to a project. Studio starts with a default project, and the person renames, switches, or creates projects from the project menu at the top. A person attaches folders to a project from anywhere on their computer. Studio owns its projects and keeps them in its own data folder, saving each as it changes; the person never handles a project's file and never saves one. A project remembers its main conversation and which files are open in which sub-applications, with their tabs, and brings them back when it is reopened. Each sub-application owns its own files: a project refers to them and never holds their work.

**Shared drawing.** Drawing, erasing, shapes, text, and ordering live in a shared library that the host uses for live editing, so pen input never crosses between processes. Each sub-application owns how its work is arranged, how it is saved, and its undo.

**The stack.** Rust, egui through eframe, and Iconoir icons. No Electron, Tauri, or web target. Do not fork existing software, but reuse good components.

## References

**Simple Sketch** 1.0.1 (at `887f6f4`) is part of Inpainter Studio. Its chrome is the host shell: the window, tab strip and new-tab button, menu button, toolbar, and the properties panel that opens from a tab at its edge, as the [interface description](https://github.com/Spumcake/Inpainter-Team/blob/master/INTERFACE.md) in the Inpainter-Team repository describes and shows. Its sketching is the Simple Sketch sub-application. Both were carried over from its code, not rewritten, and the shell is that chrome without what belongs only to sketching.

## What it will never be

A monolith that tries to do everything in one program. A general-purpose AI chat app: the assistant works on creative work owned by sub-applications.

## Words we use

- **Host:** Inpainter Studio itself: the shell, the protocol, and the assistant.
- **Sub-application:** a separate program that owns one kind of creative work.
- **Artifact:** a piece of work a sub-application owns and saves.
- **Declarations:** what a sub-application tells the host it offers, as data.
- **Common UI:** what the host always shows, whatever sub-application is loaded.
- **Shared component:** something the host draws the same way for any sub-application that declares it, such as the properties panel and the grid.
- **Project:** what every conversation belongs to: the folders the person attached, the main conversation, and the files open in the workspace.
- **Main conversation:** the conversation the person meets when Studio opens, in the current project.
- **Workspace:** the view beside the conversation where sub-applications' work is open in tabs, revealed from the bar at the window's right edge.
