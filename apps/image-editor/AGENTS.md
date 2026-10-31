# Image Editor — sub-application

The Image Editor edits a gallery of layered pictures per tab: a filmstrip of pictures to add, open, duplicate, rename, delete, and reorder, and on the picture shown brush and eraser with pen pressure, rectangle and lasso selections, move, crop, eyedropper, layers and groups with blend modes, opacity, fill, and locks, undo, autosave in the project, and PNG and JPEG in and out. The host draws the shown picture and the filmstrip as a `raster` tab, owns the camera and the pointer, and sends each finished gesture and each click on the strip. The contracts are Inpainter Studio 0.10's and 0.11's `systems.md` in the team repository.

## Rules for this sub-application

- It is its own program. Its crate depends on `protocol`, `serde`, `serde_json`, and `image` (PNG and JPEG only) and nothing else, never on the host or on another sub-application.
- Everything the host shows for it is declared through the protocol: its tabs, toolbar and settings row, menu, keys, the Layers page, and the raster contents. Nothing about it is hard-coded in the host.
- It owns every picture of a gallery, and every pixel, layer, selection, crop box, step of history, and file. The host only draws the picture and thumbnails this program writes.
- Each picture keeps its own history, and Undo acts on the picture shown. The filmstrip's changes (add, duplicate, remove, rename, reorder) are steps of the strip's own history, which Undo acts on after the host sends the strip's `focus`, until choosing a picture on the strip, a gesture, a panel edit, or a command on the picture gives the keyboard back. A thumbnail click is `focus` then `select`, so Undo after it is the shown picture's.
- It runs with no host, driven only through its standard input and standard output.
- It speaks the protocol defined in `protocol/` and nothing else. When it needs something the protocol cannot say, change the protocol, the host, and the scaffolding skill together. Do not work around the protocol.
- `manifest.json` registers it with the host. Its id, folder, crate, and program names are the same.

## Layout

- `src/picture.rs` — the picture: the tree of layers and groups (settings only), each layer's pixels by id, the blend modes, the locks, and the composite.
- `src/paint.rs` — the brush and eraser's coverage, which skips pixels already fully covered, and how a stroke changes pixels; the selection's coverage mask, its four ways of joining, feather, and outline.
- `src/doc.rs` — one picture as it is edited: every change a person or the assistant makes, each one undo step that keeps only the region that changed, and the composite brought up to date only where it changed.
- `src/gallery.rs` — one tab's gallery: its pictures in order, the one shown, and the filmstrip's history, which holds a removed picture, with its own history, until it is put back.
- `src/session.rs` — the tabs, the protocol messages, the filmstrip's commands and which of strip or picture has the keyboard, files and autosave, and the pictures and thumbnails written for the host.
- `src/declare.rs` — the toolbar and settings row as the approved views show them, the Layers page, the filmstrip, the menu, the keys, and the tab list.
- `src/assist.rs` — the assistant's "Image Editor" skill: the gallery's tools, `look` at any picture, and the shown picture's layers.
- `src/files.rs` — the `.ipx` file, PNG and JPEG in and out, and whole-file writes.

## What it writes for the host

After every change it sends `raster`, with the filmstrip in `film`. The picture shown is `<project>/.inpainter/cache/image-editor/<tab>-<picture>.rgba`, one file for each picture of the gallery, or the same name in a scratch folder under the system's temporary folder with no project: width × height × 4 bytes, RGBA with straight alpha, rows top to bottom, replaced whole by writing `<name>.partial` and renaming it. Each layer's thumbnail is `<tab>-<picture>-<layer id>.rgba` beside it, the picture's shape fitted into 56 × 40, and each picture's filmstrip thumbnail is `<tab>-film-<picture>.rgba`, its composite at 44 pixels tall and no more than 176 wide; `<picture>` is a number unique within the tab for this run. Each is written again only when its picture changed, so a picture shown again is the file and revision the host already holds. A changed picture's `patch` names the part that differs from the revision the host was last told, so the host reads only those rows.

A brush or eraser stroke arrives as it is drawn, in `stroke_part` gestures, and `doc.rs` paints each part at once with the finished stroke's brush, without a step of history: each pixel keeps the most coverage any piece gave it, and is painted again from the layer as it was before the stroke. The lines waiting are all read before the picture is sent, in `main.rs`. While a stroke is drawn, a view writes only the rows it changed, to `<tab>-<picture>-<revision>.patch`, named in its `patch.file`, and the last 32 are kept for a host that falls behind. Views say the picture is still being made until the program is idle, when `tick` writes the picture file whole under the same revision, sends the view without a file, and removes the row files. The finished `stroke` makes the painted stroke one step without painting it again; `stroke_cancel`, or any message that could change the picture, takes it off. The host's ping, the camera, and the viewport leave it be. Revisions start from the time the program started and rise with every file written, so a picture from an earlier run, or another picture of the gallery, is never taken for the current one.

The filmstrip's actions are `film_focus`, `film_select` (`value`: a position), `film_reorder` (`from`, `to`), `film_new`, `film_open`, and each thumbnail's `film_duplicate`, `film_rename`, and `film_delete` (`picture`: a position). Rename is a row with `rename` set: the host renames in place and sends the name as `value`. A `film_rename` with no `value` tells the person to ask the assistant, since the protocol's questions have no text field.

## The `.ipx` file

A tab's gallery in a project is one `.ipx` file, written by itself about a second after a change, as Mission Control's notebooks are. A writer thread in `session.rs` encodes and writes it, one file at a time and in order, from a snapshot the loop takes: a picture unchanged since its layers were encoded is not looked at again. A due write starts only when the program is idle, never on a message, and never while a stroke is drawn. Save, Save As, quit, and closing a tab wait for the writer's earlier writes first, and a picture changed while it was written stays unwritten. If the writer stops, its writes count as failed and are tried again. Which picture is shown is written too, inside a project.

1. The five bytes `IPX1` and a newline.
2. One line of compact JSON, the header, ended by a newline:
   - `version`: 2;
   - `shown`: the position of the picture shown, from 0;
   - `pictures`: every picture, in filmstrip order. Each has:
     - `name`;
     - `width` and `height`: the picture's size in pixels;
     - `active`: the active layer's id, if any;
     - `layers`: the top-level layers and groups, bottom to top. Each has `id`, `name`, `visible`, `blend` (`normal`, `multiply`, `screen`, `overlay`, `darken`, or `lighten`), `opacity` and `fill` (0 to 1), `locks` (`transparency`, `pixels`, `position`, `all`), and `offset` (`[x, y]` in picture pixels). A layer has `png`: `[start, length]` of its PNG among the bytes after the header line. A group has `children` instead, bottom to top, and no `png`. Layer ids are unique within their picture.
3. Each layer's pixels as an RGBA PNG, one after another, every picture's in turn.

Version 1, 0.10's file, has one picture's `width`, `height`, `active`, and `layers` in the header itself, with no `pictures` or `shown`. It opens as a gallery of one, named as the file is, and is written as version 2 after its next change.

A layer's pixels may be any size and sit anywhere: a new layer is the picture's size at `[0, 0]`, an image opened as a layer keeps its own size, centred, and a moved layer keeps its pixels past the picture's edge.

## Checks

`cargo test -p image-editor` runs the unit tests and `tests/session.rs`, which starts the built program and drives it through protocol lines alone. `cargo test --release -p image-editor -- --ignored --nocapture` times a stroke and its composite on a 1920 × 1080 image.
