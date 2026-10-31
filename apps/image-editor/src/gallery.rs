//! A tab's gallery: its pictures in order, which one is shown, and the
//! filmstrip's own history. Each picture keeps its own layers, selection,
//! crop box, and history in its `Doc`. Adding, duplicating, removing,
//! renaming, and moving pictures are steps of the strip's history instead,
//! undone while the strip has the keyboard, as the owner set on 2026-10-30.

use crate::doc::Doc;
use crate::files::PngCache;
use crate::session::free_names;
use protocol::{DocumentCamera, Thumbnail};
use std::collections::HashMap;

/// One picture of a gallery: a frame of its filmstrip.
pub struct Frame {
    /// Unique within its tab, for the files written for the host.
    pub uid: u64,
    pub name: String,
    pub doc: Doc,
    /// The crop box while the crop tool is chosen: `[x, y, width, height]`.
    pub crop: Option<[f64; 4]>,
    /// The camera as the host last reported it, or as this sent it.
    pub camera: Option<DocumentCamera>,
    /// The host has reported moving this picture's camera.
    pub host_camera: bool,
    /// Each layer's thumbnail, and the bytes written for it.
    pub thumbs: HashMap<u64, (Vec<u8>, Thumbnail)>,
    /// The composite revision last written to this picture's file for the
    /// host, and the revision the host was told.
    pub written: Option<(u64, u64)>,
    /// The picture file is older than the revision the host was told: a
    /// stroke being drawn sent only its rows.
    pub behind: bool,
    /// The files of rows sent while a stroke was drawn, oldest first, and
    /// the last change sent, sent again until the picture file catches up.
    pub patches: std::collections::VecDeque<std::path::PathBuf>,
    pub last_patch: Option<protocol::RasterPatch>,
    /// The filmstrip's thumbnail: the composite revision it was made from,
    /// its bytes, and where they are.
    pub film: Option<(u64, Vec<u8>, Thumbnail)>,
    pub pngs: PngCache,
}

/// One step of the strip's history. A picture taken out of the gallery is
/// held by its step, with its own history, until the step is put back.
enum Change {
    /// A picture put in at `at`; held here while the step is undone.
    Add { at: usize, frame: Option<Box<Frame>> },
    /// A picture taken out from `at`; held here while the step is done.
    Remove { at: usize, frame: Option<Box<Frame>> },
    Rename { at: usize, before: String, after: String },
    /// `to` counted after the moved picture is taken out.
    Move { from: usize, to: usize },
}

struct Step {
    change: Change,
    /// The picture shown before the step, by position then.
    shown_before: usize,
}

pub struct Gallery {
    /// Never empty.
    pub frames: Vec<Frame>,
    pub shown: usize,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Rises with every change to the strip, done or undone.
    pub edits: u64,
    next_uid: u64,
    /// The cell and picture this gallery was opened from as new work, kept
    /// in its file.
    pub source: Option<protocol::OpenedFrom>,
}

impl Gallery {
    /// A gallery of `pictures`, named, showing `shown`.
    pub fn new(pictures: Vec<(String, Doc)>, shown: usize) -> Self {
        let mut gallery = Self { frames: Vec::new(), shown: 0, undo: Vec::new(), redo: Vec::new(), edits: 0, next_uid: 1, source: None };
        for (name, doc) in pictures {
            let frame = gallery.frame_of(&name, doc);
            gallery.frames.push(frame);
        }
        assert!(!gallery.frames.is_empty(), "a gallery holds at least one picture");
        gallery.shown = shown.min(gallery.frames.len() - 1);
        gallery
    }

    pub fn one(name: &str, doc: Doc) -> Self {
        Self::new(vec![(name.to_string(), doc)], 0)
    }

    fn frame_of(&mut self, name: &str, doc: Doc) -> Frame {
        let uid = self.next_uid;
        self.next_uid += 1;
        Frame {
            uid,
            name: name.to_string(),
            doc,
            crop: None,
            camera: None,
            host_camera: false,
            thumbs: HashMap::new(),
            written: None,
            behind: false,
            patches: std::collections::VecDeque::new(),
            last_patch: None,
            film: None,
            pngs: PngCache::default(),
        }
    }

    /// The picture shown.
    pub fn frame(&self) -> &Frame {
        &self.frames[self.shown]
    }

    pub fn frame_mut(&mut self) -> &mut Frame {
        &mut self.frames[self.shown]
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// `wanted`, or the first name after it that no picture has.
    pub fn free_name(&self, wanted: &str) -> String {
        let wanted = wanted.trim();
        free_names(wanted)
            .find(|name| !self.frames.iter().any(|frame| frame.name == *name))
            .unwrap_or_else(|| wanted.to_string())
    }

    /// Shows the picture at `index`. Choosing a picture is not a step.
    pub fn select(&mut self, index: usize) -> Result<(), String> {
        self.check(index)?;
        self.shown = index;
        Ok(())
    }

    fn check(&self, index: usize) -> Result<(), String> {
        if index < self.frames.len() {
            Ok(())
        } else {
            Err(format!("There is no picture {index}; the gallery holds {}.", self.frames.len()))
        }
    }

    fn commit(&mut self, change: Change, shown_before: usize) {
        self.undo.push(Step { change, shown_before });
        self.redo.clear();
        self.edits += 1;
    }

    /// Adds a picture, named `name` or the first free name after it, at
    /// `at`, or at the end, and shows it. One step. Returns its position.
    pub fn add(&mut self, name: &str, doc: Doc, at: Option<usize>) -> usize {
        let name = self.free_name(name);
        let frame = self.frame_of(&name, doc);
        let at = at.unwrap_or(self.frames.len()).min(self.frames.len());
        let before = self.shown;
        self.put(at, frame);
        self.commit(Change::Add { at, frame: None }, before);
        at
    }

    /// A copy of a picture, its layers as they are now with a history of
    /// its own, beside it and shown, named "… copy". One step.
    pub fn duplicate(&mut self, index: usize) -> Result<usize, String> {
        self.check(index)?;
        let source = &mut self.frames[index];
        source.doc.finish_edit();
        let mut doc = Doc::new(source.doc.picture.clone());
        doc.active = source.doc.active;
        let name = format!("{} copy", source.name);
        Ok(self.add(&name, doc, Some(index + 1)))
    }

    /// Takes a picture out. The gallery keeps at least one. One step.
    pub fn remove(&mut self, index: usize) -> Result<(), String> {
        self.check(index)?;
        if self.frames.len() == 1 {
            return Err("The gallery keeps at least one picture.".into());
        }
        let before = self.shown;
        let frame = self.take(index);
        self.commit(Change::Remove { at: index, frame: Some(Box::new(frame)) }, before);
        Ok(())
    }

    /// Renames a picture. One step; the same name is none.
    pub fn rename(&mut self, index: usize, name: &str) -> Result<(), String> {
        self.check(index)?;
        let name = name.trim();
        if name.is_empty() {
            return Err("A picture's name cannot be empty.".into());
        }
        let before = std::mem::replace(&mut self.frames[index].name, name.to_string());
        if before != name {
            let shown = self.shown;
            self.commit(Change::Rename { at: index, before, after: name.to_string() }, shown);
        }
        Ok(())
    }

    /// Moves a picture to `to`, counted after it is taken out, as the strip's
    /// drag sends it. The picture shown stays shown. One step.
    pub fn move_to(&mut self, from: usize, to: usize) -> Result<(), String> {
        self.check(from)?;
        let to = to.min(self.frames.len() - 1);
        if from == to {
            return Ok(());
        }
        let before = self.shown;
        self.shift(from, to);
        self.shown = self.follow(before, from, to);
        self.commit(Change::Move { from, to }, before);
        Ok(())
    }

    /// Where the picture at `index` is after the one at `from` moves to `to`.
    fn follow(&self, index: usize, from: usize, to: usize) -> usize {
        if index == from {
            to
        } else {
            let out = if index > from { index - 1 } else { index };
            if out >= to { out + 1 } else { out }
        }
    }

    fn shift(&mut self, from: usize, to: usize) {
        let frame = self.frames.remove(from);
        self.frames.insert(to, frame);
    }

    /// Puts a picture in at `at` and shows it.
    fn put(&mut self, at: usize, frame: Frame) {
        self.frames.insert(at, frame);
        self.shown = at;
    }

    /// Takes the picture at `at` out. The one shown stays shown; when it is
    /// the one taken, the next is shown, or the one before at the end.
    fn take(&mut self, at: usize) -> Frame {
        let frame = self.frames.remove(at);
        if self.shown > at {
            self.shown -= 1;
        } else if self.shown == at {
            self.shown = at.min(self.frames.len() - 1);
        }
        frame
    }

    /// Undoes the strip's last step. The picture it touched is shown, or,
    /// for an added one, the picture shown before it was added.
    pub fn undo(&mut self) -> bool {
        let Some(mut step) = self.undo.pop() else {
            return false;
        };
        match &mut step.change {
            Change::Add { at, frame } => {
                let shown_before = step.shown_before;
                *frame = Some(Box::new(self.take(*at)));
                self.shown = shown_before.min(self.frames.len() - 1);
            }
            Change::Remove { at, frame } => {
                if let Some(frame) = frame.take() {
                    self.put(*at, *frame);
                }
            }
            Change::Rename { at, before, .. } => {
                self.frames[*at].name = before.clone();
                self.shown = *at;
            }
            Change::Move { from, to } => {
                self.shift(*to, *from);
                self.shown = *from;
            }
        }
        self.redo.push(step);
        self.edits += 1;
        true
    }

    /// Does the strip's last undone step again, showing the picture it
    /// touched, or, for a removed one, the picture shown after it went.
    pub fn redo(&mut self) -> bool {
        let Some(mut step) = self.redo.pop() else {
            return false;
        };
        match &mut step.change {
            Change::Add { at, frame } => {
                if let Some(frame) = frame.take() {
                    self.put(*at, *frame);
                }
            }
            Change::Remove { at, frame } => {
                *frame = Some(Box::new(self.take(*at)));
            }
            Change::Rename { at, after, .. } => {
                self.frames[*at].name = after.clone();
                self.shown = *at;
            }
            Change::Move { from, to } => {
                self.shift(*from, *to);
                self.shown = *to;
            }
        }
        self.undo.push(step);
        self.edits += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picture::Picture;

    fn blank() -> Doc {
        Doc::new(Picture::blank(8, 6))
    }

    fn names(gallery: &Gallery) -> Vec<&str> {
        gallery.frames.iter().map(|frame| frame.name.as_str()).collect()
    }

    /// Catches a strip step that undo does not reverse, or redo does not
    /// do again: add, duplicate, remove, rename, and move, with the shown
    /// picture following each.
    #[test]
    fn the_strips_undo_reverses_each_change() {
        let mut gallery = Gallery::one("Untitled Asset", blank());
        assert_eq!(gallery.add("Untitled Asset", blank(), None), 1);
        assert_eq!(names(&gallery), ["Untitled Asset", "Untitled Asset 2"]);
        assert_eq!(gallery.shown, 1);
        gallery.select(0).unwrap();
        assert_eq!(gallery.duplicate(0).unwrap(), 1);
        assert_eq!(names(&gallery), ["Untitled Asset", "Untitled Asset copy", "Untitled Asset 2"]);
        gallery.rename(2, " Beach ").unwrap();
        gallery.move_to(2, 0).unwrap();
        assert_eq!(names(&gallery), ["Beach", "Untitled Asset", "Untitled Asset copy"]);
        assert_eq!(gallery.frame().name, "Untitled Asset copy", "a move changed the picture shown");
        gallery.select(2).unwrap();
        gallery.remove(2).unwrap();
        assert_eq!(names(&gallery), ["Beach", "Untitled Asset"]);
        assert_eq!(gallery.shown, 1);

        // Each undo puts one change back, and shows the picture it touched.
        assert!(gallery.undo());
        assert_eq!(names(&gallery), ["Beach", "Untitled Asset", "Untitled Asset copy"]);
        assert_eq!(gallery.shown, 2);
        gallery.select(1).unwrap();
        assert!(gallery.undo());
        assert_eq!(names(&gallery), ["Untitled Asset", "Untitled Asset copy", "Beach"]);
        assert_eq!(gallery.shown, 2, "the moved picture is not shown after its move is undone");
        assert!(gallery.undo());
        assert_eq!(names(&gallery), ["Untitled Asset", "Untitled Asset copy", "Untitled Asset 2"]);
        assert!(gallery.undo());
        assert_eq!(names(&gallery), ["Untitled Asset", "Untitled Asset 2"]);
        assert_eq!(gallery.shown, 0, "undoing a duplicate does not show what was shown before it");
        assert!(gallery.undo());
        assert_eq!(names(&gallery), ["Untitled Asset"]);
        assert!(!gallery.undo());

        for _ in 0..5 {
            assert!(gallery.redo());
        }
        assert_eq!(names(&gallery), ["Beach", "Untitled Asset"]);
        assert!(!gallery.can_redo());
    }

    /// Catches the last picture deleted, and a removed picture that comes
    /// back without its own history.
    #[test]
    fn the_last_picture_stays_and_a_removed_one_keeps_its_history() {
        let mut gallery = Gallery::one("Only", blank());
        assert!(gallery.remove(0).is_err());
        assert_eq!(gallery.frames.len(), 1);
        assert!(!gallery.can_undo(), "a refused delete became a step");
        gallery.add("Second", blank(), None);
        gallery.frame_mut().doc.add_layer("Ink", None, None).unwrap();
        gallery.remove(1).unwrap();
        assert!(gallery.remove(0).is_err());
        gallery.undo();
        assert_eq!(gallery.frame().name, "Second");
        assert!(gallery.frame().doc.can_undo(), "the picture came back without its history");
        assert!(gallery.frame_mut().doc.undo());
        assert_eq!(gallery.frame().doc.picture.nodes.len(), 1);
    }

    /// Catches a move that loses track of the picture shown.
    #[test]
    fn a_move_keeps_the_shown_picture_shown() {
        let mut gallery = Gallery::new(
            ["A", "B", "C", "D"].into_iter().map(|name| (name.to_string(), blank())).collect(),
            1,
        );
        gallery.move_to(0, 3).unwrap();
        assert_eq!(names(&gallery), ["B", "C", "D", "A"]);
        assert_eq!(gallery.frame().name, "B");
        gallery.move_to(3, 1).unwrap();
        assert_eq!(names(&gallery), ["B", "A", "C", "D"]);
        assert_eq!(gallery.frame().name, "B");
        gallery.select(2).unwrap();
        gallery.move_to(1, 2).unwrap();
        assert_eq!(names(&gallery), ["B", "C", "A", "D"]);
        assert_eq!(gallery.frame().name, "C");
    }
}
