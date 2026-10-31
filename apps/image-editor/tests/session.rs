//! The Image Editor as its own program, driven through the protocol alone.

use protocol::{AppMessage, HostMessage, RasterGesture, RasterView};
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

struct Program {
    child: Child,
    input: ChildStdin,
    output: Receiver<AppMessage>,
}

impl Program {
    fn start(restore: bool) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_image-editor"));
        if restore {
            command.arg(protocol::RESTORE_FLAG);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("image-editor starts");
        let input = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (lines, output) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let message = protocol::parse_app_message(&line).expect("every line is a message");
                if lines.send(message).is_err() {
                    break;
                }
            }
        });
        Self { child, input, output }
    }

    fn send(&mut self, message: HostMessage) {
        writeln!(self.input, "{}", serde_json::to_string(&message).expect("json")).expect("sent");
    }

    fn command(&mut self, id: &str, arg: serde_json::Value) {
        self.send(HostMessage::Command {
            id: id.into(),
            tab: Some("1".into()),
            arg: Some(arg),
            node: None,
            viewport: [1200.0, 800.0],
        });
    }

    /// The first message `pick` takes, within a few seconds.
    fn wait<T>(&self, mut pick: impl FnMut(&AppMessage) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let message = self.output.recv_timeout(left).expect("the awaited message never came");
            if let Some(found) = pick(&message) {
                return found;
            }
        }
    }

    fn raster(&self, newer_than: u64) -> RasterView {
        self.wait(|message| match message {
            AppMessage::Raster(view) if view.revision > newer_than => Some(view.clone()),
            _ => None,
        })
    }
}

impl Drop for Program {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn scratch(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("image-editor-session-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch folder");
    dir.canonicalize().expect("canonical")
}

fn pixel(path: &Path, width: u32, x: u32, y: u32) -> [u8; 4] {
    let bytes = std::fs::read(path).expect("the picture file");
    let at = ((y * width + x) * 4) as usize;
    [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]
}

/// Catches a picture the host could not read, a stroke that reaches no
/// pixel, an undo that does not restore it, and an image that waits for
/// Save: inside a project, the program, given only protocol lines, sends a
/// `raster` whose picture file is width × height × 4 bytes in the project's
/// cache; a stroke paints it; undo restores it; and a second later the
/// image is in the project as an `.ipx` file that reopens with its layers.
#[test]
fn a_stroke_reaches_the_picture_and_the_project() {
    let folder = scratch("stroke");
    let mut program = Program::start(false);
    let first = program.raster(0);
    assert_eq!((first.width, first.height), (1920, 1080));
    assert!(first.camera.is_some(), "a new picture came without a camera");

    program.send(HostMessage::Project { folder: folder.display().to_string() });
    let view = program.raster(0);
    let picture = PathBuf::from(&view.picture);
    assert!(picture.starts_with(folder.join(".inpainter/cache/image-editor")), "{}", picture.display());
    assert_eq!(std::fs::metadata(&picture).unwrap().len(), 1920 * 1080 * 4);
    assert_eq!(pixel(&picture, 1920, 500, 300), [255, 255, 255, 255]);

    program.command("tool", json!({ "tool": "brush" }));
    program.send(HostMessage::RasterGesture {
        tab: "1".into(),
        gesture: RasterGesture::Stroke { points: vec![[400.0, 300.0, 1.0], [600.0, 300.0, 1.0]] },
    });
    let painted = program.raster(view.revision);
    assert_eq!(painted.picture, view.picture);
    assert_eq!(std::fs::metadata(&picture).unwrap().len(), 1920 * 1080 * 4);
    assert_eq!(pixel(&picture, 1920, 500, 300), [0, 0, 0, 255], "the stroke did not reach the picture");
    assert_eq!(pixel(&picture, 1920, 500, 400), [255, 255, 255, 255]);

    program.command("undo", json!({}));
    let undone = program.raster(painted.revision);
    assert_eq!(pixel(&PathBuf::from(&undone.picture), 1920, 500, 300), [255, 255, 255, 255]);
    program.command("redo", json!({}));
    program.raster(undone.revision);

    // A second after the change it is written by itself.
    let written = folder.join("Untitled Asset.ipx");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !written.is_file() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(written.is_file(), "nothing was written into the project");
    drop(program);

    let mut again = Program::start(true);
    again.send(HostMessage::Project { folder: folder.display().to_string() });
    again.send(HostMessage::Claim { id: "c1".into(), path: written.display().to_string() });
    assert!(again.wait(|message| match message {
        AppMessage::Claimed { id, yours } if id == "c1" => Some(*yours),
        _ => None,
    }));
    again.send(HostMessage::Open { path: Some(written.display().to_string()) });
    let reopened = again.raster(0);
    assert_eq!((reopened.width, reopened.height), (1920, 1080));
    assert_eq!(pixel(&PathBuf::from(&reopened.picture), 1920, 500, 300), [0, 0, 0, 255], "the stroke was not kept");
    let rows = again.wait(|message| match message {
        AppMessage::Properties(properties) => properties.layers.as_ref().map(|layers| layers.rows.len()),
        _ => None,
    });
    assert_eq!(rows, 1);
    drop(again);
    let _ = std::fs::remove_dir_all(folder);
}

/// Catches Open and Export that do not go through the host's file dialog,
/// or that lose the picture's size or pixels on the way.
#[test]
fn open_and_export_go_through_the_file_dialog() {
    let folder = scratch("files");
    let source = folder.join("Sunset.png");
    let mut rgba = Vec::new();
    for y in 0..30u32 {
        for x in 0..40u32 {
            rgba.extend_from_slice(&[(x * 6) as u8, (y * 8) as u8, 77, 255]);
        }
    }
    let mut png = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut png),
        &rgba,
        40,
        30,
        image::ExtendedColorType::Rgba8,
    )
    .unwrap();
    std::fs::write(&source, png).unwrap();

    let mut program = Program::start(false);
    program.send(HostMessage::Project { folder: folder.display().to_string() });
    program.raster(0);
    program.command("open", json!({}));
    let asked = program.wait(|message| match message {
        AppMessage::File { id, kind: protocol::FileKind::Open, .. } => Some(id.clone()),
        _ => None,
    });
    program.send(HostMessage::FileResult { id: asked, path: Some(source.display().to_string()) });
    let opened = program.raster(0);
    assert_eq!((opened.width, opened.height), (40, 30));
    assert_eq!(std::fs::read(&opened.picture).unwrap(), rgba);

    program.send(HostMessage::Command {
        id: "export_png".into(),
        tab: Some(opened.tab.clone()),
        arg: None,
        node: None,
        viewport: [1200.0, 800.0],
    });
    let asked = program.wait(|message| match message {
        AppMessage::File { id, kind: protocol::FileKind::Save, .. } => Some(id.clone()),
        _ => None,
    });
    let exported = folder.join("Out.png");
    program.send(HostMessage::FileResult { id: asked, path: Some(exported.display().to_string()) });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !exported.is_file() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let back = image::open(&exported).expect("the export reads").into_rgba8();
    assert_eq!(back.into_raw(), rgba);

    // The same picture opened as a layer of the image is a second layer.
    program.send(HostMessage::Command {
        id: "open_layer".into(),
        tab: Some(opened.tab.clone()),
        arg: None,
        node: None,
        viewport: [1200.0, 800.0],
    });
    let asked = program.wait(|message| match message {
        AppMessage::File { id, kind: protocol::FileKind::Open, .. } => Some(id.clone()),
        _ => None,
    });
    program.send(HostMessage::FileResult { id: asked, path: Some(source.display().to_string()) });
    let rows = program.wait(|message| match message {
        AppMessage::Properties(properties) => properties.layers.as_ref().map(|layers| layers.rows.len()).filter(|rows| *rows > 1),
        _ => None,
    });
    assert_eq!(rows, 2);
    drop(program);
    let _ = std::fs::remove_dir_all(folder);
}

/// Catches a + tile that adds nothing the host can show, or a stroke on an
/// added picture that reaches another: through protocol lines alone, the
/// + row's action adds a white picture shown on the strip and the canvas,
/// a stroke paints only it, a click on the first thumbnail shows the
/// first picture, still white, again, and the + tile's open row adds a PNG
/// as a picture of its own.
#[test]
fn a_picture_added_from_the_strip_is_painted_alone() {
    let mut program = Program::start(false);
    let first = program.raster(0);
    let film = first.film.clone().expect("no filmstrip");
    assert_eq!(film.items.len(), 1);
    let first_thumb = std::fs::read(&film.items[0].thumbnail.path).expect("the first thumbnail");

    let send = |program: &mut Program, action: &protocol::Action| {
        program.command(&action.id, action.arg.clone().unwrap_or(serde_json::Value::Null));
    };
    send(&mut program, &film.focus);
    send(&mut program, film.add[0].action.as_ref().expect("the + row has no action"));
    let added = program.wait(|message| match message {
        AppMessage::Raster(view) if view.film.as_ref().is_some_and(|film| film.items.len() == 2) => Some(view.clone()),
        _ => None,
    });
    let strip = added.film.clone().unwrap();
    assert_eq!(strip.shown, 1, "the added picture is not the one shown");
    assert_eq!(strip.items[1].name, "Untitled Asset 2");
    assert_eq!((added.width, added.height), (1920, 1080));

    program.command("tool", json!({ "tool": "brush" }));
    program.send(HostMessage::RasterGesture {
        tab: "1".into(),
        gesture: RasterGesture::Stroke { points: vec![[400.0, 300.0, 1.0], [600.0, 300.0, 1.0]] },
    });
    let painted = program.raster(added.revision);
    assert_eq!(pixel(Path::new(&painted.picture), 1920, 500, 300), [0, 0, 0, 255], "the stroke missed the added picture");
    let strip = painted.film.clone().unwrap();
    assert_eq!(std::fs::read(&strip.items[0].thumbnail.path).unwrap(), first_thumb, "the stroke reached the first picture");
    let dark = std::fs::read(&strip.items[1].thumbnail.path).unwrap();
    assert!(dark.chunks(4).any(|pixel| pixel[0] < 128), "the added picture's thumbnail does not show the stroke");

    send(&mut program, &strip.focus);
    send(&mut program, &strip.select.with_value(json!(0)));
    // The first picture comes back as the file and revision first sent,
    // which the host still holds.
    let shown = program.wait(|message| match message {
        AppMessage::Raster(view) if view.film.as_ref().is_some_and(|film| film.shown == 0) => Some(view.clone()),
        _ => None,
    });
    assert_eq!((shown.picture.as_str(), shown.revision), (first.picture.as_str(), first.revision));
    assert_eq!(pixel(Path::new(&shown.picture), 1920, 500, 300), [255, 255, 255, 255], "the first picture was painted");

    // The + tile's second row opens a PNG through the host's file dialog
    // as a third picture, shown, and named after its file.
    let folder = scratch("film-open");
    let source = folder.join("Sunset.png");
    image::RgbaImage::from_pixel(40, 30, image::Rgba([200, 90, 30, 255])).save(&source).unwrap();
    let strip = shown.film.clone().unwrap();
    send(&mut program, &strip.focus);
    send(&mut program, strip.add[1].action.as_ref().expect("the + tile's open row has no action"));
    let asked = program.wait(|message| match message {
        AppMessage::File { id, kind: protocol::FileKind::Open, .. } => Some(id.clone()),
        _ => None,
    });
    program.send(HostMessage::FileResult { id: asked, path: Some(source.display().to_string()) });
    let opened = program.wait(|message| match message {
        AppMessage::Raster(view) if view.film.as_ref().is_some_and(|film| film.items.len() == 3) => Some(view.clone()),
        _ => None,
    });
    let strip = opened.film.unwrap();
    assert_eq!(strip.shown, 2, "the opened picture is not the one shown");
    assert_eq!(strip.items[2].name, "Sunset");
    assert_eq!((opened.width, opened.height), (40, 30));
    assert_eq!(pixel(Path::new(&opened.picture), 40, 5, 5), [200, 90, 30, 255]);
    drop(program);
    let _ = std::fs::remove_dir_all(folder);
}
