# Image Editor — sub-application

The Image Editor edits one layered image per tab: brush and eraser with pen pressure, rectangle and lasso selections, move, crop, eyedropper, layers and groups with blend modes, opacity, fill, and locks, undo, autosave in the project, and PNG and JPEG in and out. The host draws its picture as a `raster` tab, owns the camera and the pointer, and sends each finished gesture. The contract is Inpainter Studio 0.10's `systems.md` in the team repository.

## Rules for this sub-application

- It is its own program. Its crate depends on `protocol`, `serde`, `serde_json`, and `image` (PNG and JPEG only) and nothing else, never on the host or on another sub-application.
- Everything the host shows for it is declared through the protocol: its tabs, toolbar and settings row, menu, keys, the Layers page, and the raster contents. Nothing about it is hard-coded in the host.
- It owns every pixel, layer, selection, crop box, step of history, and file. The host only draws the picture this program writes.
- It runs with no host, driven only through its standard input and standard output.
- It speaks the protocol defined in `protocol/` and nothing else. When it needs something the protocol cannot say, change the protocol, the host, and the scaffolding skill together. Do not work around the protocol.
- `manifest.json` registers it with the host. Its id, folder, crate, and program names are the same.

## Layout

- `src/picture.rs` — the picture: the tree of layers and groups (settings only), each layer's pixels by id, the blend modes, the locks, and the composite.
- `src/paint.rs` — the brush and eraser's dabs and how a stroke changes pixels; the selection's coverage mask, its four ways of joining, feather, and outline.
- `src/doc.rs` — one tab's image: every change a person or the assistant makes, each one undo step that keeps only the region that changed, and the composite brought up to date only where it changed.
- `src/session.rs` — the tabs, the protocol messages, files and autosave, and the pictures written for the host.
- `src/declare.rs` — the toolbar and settings row as the approved views show them, the Layers page, the menu, the keys, and the tab list.
- `src/assist.rs` — the assistant's "Image Editor" skill.
- `src/files.rs` — the `.ipx` file, PNG and JPEG in and out, and whole-file writes.

## What it writes for the host

After every change it sends `raster`. The picture is `<project>/.inpainter/cache/image-editor/<tab>.rgba`, or the same name in a scratch folder under the system's temporary folder with no project: width × height × 4 bytes, RGBA with straight alpha, rows top to bottom, replaced whole by writing `<name>.partial` and renaming it. Each layer's thumbnail is `<tab>-<layer id>.rgba` beside it, the picture's shape fitted into 56 × 40. Revisions start from the time the program started, so a picture from an earlier run is never taken for a current one.

## The `.ipx` file

An image in a project is one `.ipx` file, written by itself about a second after a change, as Mission Control's notebooks are.

1. The five bytes `IPX1` and a newline.
2. One line of compact JSON, the header, ended by a newline:
   - `version`: 1;
   - `width` and `height`: the picture's size in pixels;
   - `active`: the active layer's id, if any;
   - `layers`: the top-level layers and groups, bottom to top. Each has `id`, `name`, `visible`, `blend` (`normal`, `multiply`, `screen`, `overlay`, `darken`, or `lighten`), `opacity` and `fill` (0 to 1), `locks` (`transparency`, `pixels`, `position`, `all`), and `offset` (`[x, y]` in picture pixels). A layer has `png`: `[start, length]` of its PNG among the bytes after the header line. A group has `children` instead, bottom to top, and no `png`.
3. Each layer's pixels as an RGBA PNG, one after another.

A layer's pixels may be any size and sit anywhere: a new layer is the picture's size at `[0, 0]`, an image opened as a layer keeps its own size, centred, and a moved layer keeps its pixels past the picture's edge.

## Checks

`cargo test -p image-editor` runs the unit tests and `tests/session.rs`, which starts the built program and drives it through protocol lines alone. `cargo test --release -p image-editor -- --ignored --nocapture` times a stroke and its composite on a 1920 × 1080 image.
