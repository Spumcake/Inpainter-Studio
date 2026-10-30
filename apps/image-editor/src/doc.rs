//! One image as a tab edits it: the picture, the active layer, the
//! selection, the composite, and the undo history, with every change a person
//! or the assistant can make. Each change is one step; a step stores the
//! region that changed, before and after, not the whole picture.

use crate::paint::{self, Brush, Combine, Mask};
use crate::picture::{Blend, Node, Picture, Pixels, Place, Rect};

/// One part of a step, which undo puts back and redo puts forward.
#[derive(Debug, Clone)]
enum Change {
    /// A region of one layer's pixels, in its own pixels.
    Pixels { id: u64, rect: Rect, before: Vec<u8>, after: Vec<u8> },
    /// One layer's whole buffer, when its size changes.
    Buffer { id: u64, before: Pixels, after: Pixels },
    /// The tree's settings and order, which hold no pixels.
    Tree { before: Vec<Node>, after: Vec<Node> },
    Active { before: Option<u64>, after: Option<u64> },
    Selection { before: Option<Mask>, after: Option<Mask> },
    Size { before: [u32; 2], after: [u32; 2] },
}

#[derive(Debug, Clone, Default)]
struct Step {
    changes: Vec<Change>,
    /// The assistant's request this step belongs to: every call under one
    /// request is one step.
    group: Option<String>,
}

/// Why a change was refused, in words a person or the assistant reads.
pub type Refusal = String;

pub struct Doc {
    pub picture: Picture,
    pub active: Option<u64>,
    pub selection: Option<Mask>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// The tree before a held slider began, until it is let go.
    pending: Option<(Vec<Node>, Option<u64>)>,
    /// The composite, RGBA8 straight alpha, and the part of it that is out
    /// of date.
    composite: Vec<u8>,
    stale: Option<Rect>,
    /// Rises whenever the composite changes.
    pub revision: u64,
    /// Rises with every change to the image, so its tab knows it is
    /// unwritten.
    pub edits: u64,
    outlines: Option<Vec<Vec<[f64; 2]>>>,
    /// How many layers and groups have been made, for their names.
    made_layers: u32,
    made_groups: u32,
}

impl Doc {
    pub fn new(picture: Picture) -> Self {
        let active = picture.nodes.last().map(|node| node.id);
        let made_layers = picture.all_layers().len() as u32;
        let mut doc = Self {
            picture,
            active,
            selection: None,
            undo: Vec::new(),
            redo: Vec::new(),
            pending: None,
            composite: Vec::new(),
            stale: None,
            revision: 0,
            edits: 0,
            outlines: None,
            made_layers,
            made_groups: 0,
        };
        doc.made_groups = doc.count_groups();
        doc.stale = Some(doc.picture.bounds());
        doc
    }

    fn count_groups(&self) -> u32 {
        let mut ids = Vec::new();
        for node in &self.picture.nodes {
            ids.extend(self.picture.ids_within(node.id));
        }
        ids.iter().filter(|id| self.picture.find(**id).is_some_and(Node::is_group)).count() as u32
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The composite, brought up to date.
    pub fn composite(&mut self) -> &[u8] {
        self.refresh();
        &self.composite
    }

    /// Recomposites only the part that changed. Returns whether anything did.
    pub fn refresh(&mut self) -> bool {
        let size = self.picture.width as usize * self.picture.height as usize * 4;
        if self.composite.len() != size {
            self.composite = vec![0; size];
            self.stale = Some(self.picture.bounds());
        }
        let Some(rect) = self.stale.take() else {
            return false;
        };
        self.picture.composite_into(rect, &mut self.composite);
        self.revision += 1;
        true
    }

    fn touch(&mut self, rect: Rect) {
        let rect = rect.intersect(&self.picture.bounds());
        if rect.is_empty() {
            return;
        }
        self.stale = Some(match self.stale {
            Some(stale) => stale.union(&rect),
            None => rect,
        });
    }

    fn touch_all(&mut self) {
        self.stale = Some(self.picture.bounds());
    }

    /// The selection's outlines, for the marching edge.
    pub fn outlines(&mut self) -> Vec<Vec<[f64; 2]>> {
        if self.outlines.is_none() {
            self.outlines = Some(self.selection.as_ref().map(Mask::outlines).unwrap_or_default());
        }
        self.outlines.clone().unwrap_or_default()
    }

    fn commit(&mut self, changes: Vec<Change>, group: Option<&str>) {
        if changes.is_empty() {
            return;
        }
        self.edits += 1;
        self.redo.clear();
        if let Some(group) = group
            && let Some(last) = self.undo.last_mut()
            && last.group.as_deref() == Some(group)
        {
            last.changes.extend(changes);
            return;
        }
        self.undo.push(Step {
            changes,
            group: group.map(String::from),
        });
    }

    pub fn undo(&mut self) -> bool {
        self.finish_edit();
        let Some(step) = self.undo.pop() else {
            return false;
        };
        for change in step.changes.iter().rev() {
            self.put(change, false);
        }
        self.edits += 1;
        self.redo.push(step);
        true
    }

    pub fn redo(&mut self) -> bool {
        self.finish_edit();
        let Some(step) = self.redo.pop() else {
            return false;
        };
        for change in &step.changes {
            self.put(change, true);
        }
        self.edits += 1;
        self.undo.push(step);
        true
    }

    /// Puts one change's after, or its before.
    fn put(&mut self, change: &Change, forward: bool) {
        match change {
            Change::Pixels { id, rect, before, after } => {
                let bytes = if forward { after } else { before };
                if let Some(pixels) = self.picture.pixels.get_mut(id) {
                    pixels.write(*rect, bytes);
                }
                let offset = self.picture.find(*id).map(|node| node.offset).unwrap_or([0, 0]);
                self.touch(Rect::new(rect.x + offset[0], rect.y + offset[1], rect.w, rect.h));
            }
            Change::Buffer { id, before, after } => {
                let pixels = if forward { after } else { before };
                self.picture.pixels.insert(*id, pixels.clone());
                self.touch_all();
            }
            Change::Tree { before, after } => {
                self.picture.nodes = if forward { after.clone() } else { before.clone() };
                self.touch_all();
            }
            Change::Active { before, after } => {
                self.active = if forward { *after } else { *before };
            }
            Change::Selection { before, after } => {
                self.selection = if forward { after.clone() } else { before.clone() };
                self.outlines = None;
            }
            Change::Size { before, after } => {
                let size = if forward { after } else { before };
                self.picture.width = size[0];
                self.picture.height = size[1];
                self.touch_all();
            }
        }
    }

    /// The tree changed by `edit`, as one step; nothing when `edit` refuses
    /// or changes nothing.
    fn edit_tree(
        &mut self,
        group: Option<&str>,
        edit: impl FnOnce(&mut Self) -> Result<(), Refusal>,
    ) -> Result<(), Refusal> {
        self.finish_edit();
        let before = self.picture.nodes.clone();
        let active = self.active;
        if let Err(refusal) = edit(self) {
            self.picture.nodes = before;
            self.active = active;
            return Err(refusal);
        }
        let mut changes = Vec::new();
        if self.picture.nodes != before {
            changes.push(Change::Tree { before, after: self.picture.nodes.clone() });
            self.touch_all();
        }
        if self.active != active {
            changes.push(Change::Active { before: active, after: self.active });
        }
        self.commit(changes, group);
        Ok(())
    }

    // Painting.

    /// A brush or eraser stroke on the active layer, inside the selection
    /// when there is one. One step.
    pub fn stroke(&mut self, points: &[[f64; 3]], brush: &Brush) -> Result<(), Refusal> {
        self.finish_edit();
        if let Some(refusal) = self.paint_refusal(brush.erase) {
            return Err(refusal);
        }
        let Some(id) = self.active else {
            return Ok(());
        };
        let locks = self.picture.locks(id);
        let Some(coverage) = paint::stroke_coverage(points, brush) else {
            return Ok(());
        };
        let clip = self.picture.bounds();
        let wanted = coverage.rect.intersect(&clip);
        if wanted.is_empty() {
            return Ok(());
        }
        let mut changes = Vec::new();
        // A layer moved off part of the picture grows to cover it again, so
        // a stroke there is not lost.
        let layer_rect = self.picture.node_rect(id).unwrap_or(clip);
        if !layer_rect.contains(&wanted) {
            let grown = layer_rect.union(&clip);
            let before_tree = self.picture.nodes.clone();
            let old = self.picture.pixels.get(&id).cloned().unwrap_or_else(|| Pixels::clear(1, 1));
            let new = old.cut(Rect::new(grown.x - layer_rect.x, grown.y - layer_rect.y, grown.w, grown.h));
            if let Some(node) = self.picture.find_mut(id) {
                node.offset = [grown.x, grown.y];
            }
            self.picture.pixels.insert(id, new.clone());
            changes.push(Change::Tree { before: before_tree, after: self.picture.nodes.clone() });
            changes.push(Change::Buffer { id, before: old, after: new });
        }
        let offset = self.picture.find(id).map(|node| node.offset).unwrap_or([0, 0]);
        let pixels = self.picture.pixels.get_mut(&id).ok_or("The active layer has no pixels.")?;
        let local = Rect::new(wanted.x - offset[0], wanted.y - offset[1], wanted.w, wanted.h);
        let before = pixels.read(local);
        let changed = paint::apply(pixels, offset, &coverage, clip, self.selection.as_ref(), brush, locks.keeps_alpha());
        if let Some(changed) = changed {
            // Only the part that changed is kept.
            let after = pixels.read(changed);
            let mut kept = Vec::with_capacity(after.len());
            for y in 0..changed.h {
                let start = (((changed.y - local.y + y) * local.w + (changed.x - local.x)) * 4) as usize;
                kept.extend_from_slice(&before[start..start + changed.w as usize * 4]);
            }
            changes.push(Change::Pixels { id, rect: changed, before: kept, after });
            self.touch(Rect::new(changed.x + offset[0], changed.y + offset[1], changed.w, changed.h));
        }
        if !changes.is_empty() {
            self.touch_all_if_grown(&changes);
        }
        self.commit(changes, None);
        Ok(())
    }

    /// Why a brush, or the eraser with `erase`, cannot paint on the active
    /// target, in plain words; nothing when it can.
    pub fn paint_refusal(&self, erase: bool) -> Option<Refusal> {
        let Some(node) = self.active.and_then(|id| self.picture.find(id)) else {
            return Some("Choose a layer to paint on.".into());
        };
        if node.is_group() {
            return Some("Choose a layer to paint on; a group holds layers.".into());
        }
        if !node.visible {
            return Some(format!("{} is hidden.", node.name));
        }
        let id = node.id;
        let locks = self.picture.locks(id);
        if locks.keeps_pixels() {
            return Some(match self.locked_by(id, |locks| locks.all) {
                Some(name) => format!("{name} is locked."),
                None => format!("{}'s pixels are locked.", self.locked_by(id, |locks| locks.pixels)?),
            });
        }
        if erase && locks.keeps_alpha() {
            let name = self.locked_by(id, |locks| locks.transparency || locks.all)?;
            return Some(format!("{name}'s transparency is locked, so it cannot be erased."));
        }
        None
    }

    /// Why the Move tool cannot move the active layer or group, in plain
    /// words; nothing when it can.
    pub fn move_refusal(&self) -> Option<Refusal> {
        let Some(id) = self.active.filter(|id| self.picture.find(*id).is_some()) else {
            return Some("Choose a layer to move.".into());
        };
        let layer = self.picture.layers_within(id).into_iter().find(|layer| self.picture.locks(*layer).keeps_position())?;
        Some(match self.locked_by(layer, |locks| locks.all) {
            Some(name) => format!("{name} is locked."),
            None => format!("{}'s position is locked.", self.locked_by(layer, |locks| locks.position)?),
        })
    }

    /// The name of the node, or the nearest group around it, whose own locks
    /// hold `lock`.
    fn locked_by(&self, id: u64, lock: impl Fn(&crate::picture::Locks) -> bool) -> Option<String> {
        let mut current = Some(id);
        while let Some(at) = current {
            let node = self.picture.find(at)?;
            if lock(&node.locks) {
                return Some(node.name.clone());
            }
            current = self.picture.place(at).and_then(|place| place.parent);
        }
        None
    }

    fn touch_all_if_grown(&mut self, changes: &[Change]) {
        if changes.iter().any(|change| matches!(change, Change::Buffer { .. })) {
            self.touch_all();
        }
    }

    /// The visible composite's colour at a picture pixel.
    pub fn pick(&mut self, at: [f64; 2]) -> Option<[u8; 4]> {
        let x = at[0].floor();
        let y = at[1].floor();
        if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
            return None;
        }
        let (x, y) = (x as usize, y as usize);
        let width = self.picture.width as usize;
        if x >= width || y >= self.picture.height as usize {
            return None;
        }
        let composite = self.composite();
        let at = (y * width + x) * 4;
        Some([composite[at], composite[at + 1], composite[at + 2], composite[at + 3]])
    }

    // Selecting.

    /// Joins a shape into the selection, feathered. One step.
    pub fn select(&mut self, mut shape: Mask, how: Combine, feather: f64) {
        self.finish_edit();
        shape.feather(feather);
        let before = self.selection.clone();
        let mut next = match (&self.selection, how) {
            (_, Combine::New) | (None, Combine::Add) => Some(shape),
            (None, _) => None,
            (Some(current), _) => {
                let mut current = current.clone();
                current.combine(&shape, how);
                Some(current)
            }
        };
        if next.as_ref().is_some_and(Mask::is_clear) {
            next = None;
        }
        self.set_selection(before, next);
    }

    pub fn deselect(&mut self) {
        self.finish_edit();
        let before = self.selection.clone();
        self.set_selection(before, None);
    }

    pub fn invert(&mut self) {
        self.finish_edit();
        let before = self.selection.clone();
        let Some(mut next) = before.clone() else {
            return;
        };
        next.invert();
        let next = (!next.is_clear()).then_some(next);
        self.set_selection(before, next);
    }

    fn set_selection(&mut self, before: Option<Mask>, after: Option<Mask>) {
        if before == after {
            return;
        }
        self.selection = after.clone();
        self.outlines = None;
        self.commit(vec![Change::Selection { before, after }], None);
    }

    // Moving and cropping.

    /// Moves the active layer, or every layer of the active group. One step.
    pub fn move_active(&mut self, dx: i32, dy: i32) -> Result<(), Refusal> {
        if dx == 0 && dy == 0 {
            return Ok(());
        }
        if let Some(refusal) = self.move_refusal() {
            return Err(refusal);
        }
        let Some(id) = self.active else {
            return Ok(());
        };
        let layers = self.picture.layers_within(id);
        self.edit_tree(None, |doc| {
            for layer in layers {
                if let Some(node) = doc.picture.find_mut(layer) {
                    node.offset = [node.offset[0] + dx, node.offset[1] + dy];
                }
            }
            Ok(())
        })
    }

    /// Crops the picture and every layer to `rect`, in picture pixels. One
    /// step. A box that covers the whole picture changes nothing.
    pub fn crop(&mut self, rect: [f64; 4]) -> Result<(), Refusal> {
        self.finish_edit();
        if rect.iter().any(|value| !value.is_finite()) {
            return Err("The crop box is not a box.".into());
        }
        let x0 = rect[0].round().max(0.0) as i32;
        let y0 = rect[1].round().max(0.0) as i32;
        let x1 = ((rect[0] + rect[2]).round() as i32).min(self.picture.width as i32);
        let y1 = ((rect[1] + rect[3]).round() as i32).min(self.picture.height as i32);
        let crop = Rect::new(x0, y0, x1 - x0, y1 - y0);
        if crop.w < 1 || crop.h < 1 {
            return Err("The crop box is empty.".into());
        }
        if crop == self.picture.bounds() {
            return Ok(());
        }
        let mut changes = Vec::new();
        let before_tree = self.picture.nodes.clone();
        for id in self.picture.all_layers() {
            let Some(layer) = self.picture.node_rect(id) else {
                continue;
            };
            let kept = layer.intersect(&crop);
            let (cut, offset) = if kept.is_empty() {
                (Rect::new(0, 0, 1, 1), [0, 0])
            } else {
                (Rect::new(kept.x - layer.x, kept.y - layer.y, kept.w, kept.h), [kept.x - crop.x, kept.y - crop.y])
            };
            let Some(old) = self.picture.pixels.get(&id).cloned() else {
                continue;
            };
            let mut new = old.cut(cut);
            if kept.is_empty() {
                new = Pixels::clear(1, 1);
            }
            self.picture.pixels.insert(id, new.clone());
            if let Some(node) = self.picture.find_mut(id) {
                node.offset = offset;
            }
            changes.push(Change::Buffer { id, before: old, after: new });
        }
        changes.push(Change::Tree { before: before_tree, after: self.picture.nodes.clone() });
        changes.push(Change::Size {
            before: [self.picture.width, self.picture.height],
            after: [crop.w as u32, crop.h as u32],
        });
        let selection = self.selection.as_ref().map(|mask| mask.cut(crop)).filter(|mask| !mask.is_clear());
        changes.push(Change::Selection { before: self.selection.clone(), after: selection.clone() });
        self.picture.width = crop.w as u32;
        self.picture.height = crop.h as u32;
        self.selection = selection;
        self.outlines = None;
        self.touch_all();
        self.commit(changes, None);
        Ok(())
    }

    // Layers.

    /// Where a new layer goes: above the active layer among its siblings,
    /// at the top of the active group, or at the top of the picture.
    fn new_place(&self) -> Place {
        let top = Place { parent: None, index: self.picture.nodes.len() };
        let Some(id) = self.active else {
            return top;
        };
        match self.picture.find(id) {
            Some(node) if node.is_group() => Place {
                parent: Some(id),
                index: node.children.as_ref().map_or(0, Vec::len),
            },
            Some(_) => self.picture.place(id).map(|place| Place { index: place.index + 1, ..place }).unwrap_or(top),
            None => top,
        }
    }

    fn refuse_inside_locked(&self, parent: Option<u64>) -> Result<(), Refusal> {
        if let Some(parent) = parent
            && self.picture.locks(parent).all
        {
            return Err("That group is locked.".into());
        }
        Ok(())
    }

    /// A clear layer the picture's size, active. `name` empty names it
    /// "Layer N". `group` puts it at the top of that group. One step.
    pub fn add_layer(&mut self, name: &str, group: Option<u64>, by: Option<&str>) -> Result<u64, Refusal> {
        let place = match group {
            Some(group) => match self.picture.find(group) {
                Some(node) if node.is_group() => Place {
                    parent: Some(group),
                    index: node.children.as_ref().map_or(0, Vec::len),
                },
                Some(_) => return Err(format!("Layer {group} is not a group.")),
                None => return Err(format!("There is no group {group}.")),
            },
            None if by.is_some() => Place { parent: None, index: self.picture.nodes.len() },
            None => self.new_place(),
        };
        self.refuse_inside_locked(place.parent)?;
        let pixels = Pixels::clear(self.picture.width, self.picture.height);
        self.add_node(name, pixels, [0, 0], place, by)
    }

    /// An image as a new layer, centred, above the active layer. One step.
    pub fn add_image(&mut self, name: &str, pixels: Pixels) -> Result<u64, Refusal> {
        let place = self.new_place();
        self.refuse_inside_locked(place.parent)?;
        let offset = [
            (self.picture.width as i32 - pixels.width as i32) / 2,
            (self.picture.height as i32 - pixels.height as i32) / 2,
        ];
        self.add_node(name, pixels, offset, place, None)
    }

    fn add_node(&mut self, name: &str, pixels: Pixels, offset: [i32; 2], place: Place, by: Option<&str>) -> Result<u64, Refusal> {
        let id = self.picture.take_id();
        self.made_layers += 1;
        let name = if name.trim().is_empty() { format!("Layer {}", self.made_layers) } else { name.trim().to_string() };
        self.picture.pixels.insert(id, pixels);
        let mut node = Node::layer(id, &name);
        node.offset = offset;
        self.edit_tree(by, |doc| {
            if !doc.picture.insert(node, place) {
                return Err("There is nowhere to put the layer.".into());
            }
            doc.active = Some(id);
            Ok(())
        })?;
        Ok(id)
    }

    /// Wraps the active layer in a new group, which keeps it active, or
    /// makes an empty group, active, when none is active. One step.
    pub fn group_active(&mut self) -> Result<u64, Refusal> {
        let id = self.picture.take_id();
        self.made_groups += 1;
        let name = format!("Group {}", self.made_groups);
        let active = self.active.filter(|active| self.picture.find(*active).is_some());
        if let Some(place) = active.and_then(|active| self.picture.place(active)) {
            self.refuse_inside_locked(place.parent)?;
        }
        self.edit_tree(None, |doc| {
            match active {
                Some(active) => {
                    let (node, place) = doc.picture.remove(active).ok_or("The active layer is gone.")?;
                    doc.picture.insert(Node::group(id, &name, vec![node]), place);
                }
                None => {
                    let at = doc.picture.nodes.len();
                    doc.picture.insert(Node::group(id, &name, Vec::new()), Place { parent: None, index: at });
                    doc.active = Some(id);
                }
            }
            Ok(())
        })?;
        Ok(id)
    }

    /// Puts a group's layers where the group was. One step.
    pub fn ungroup(&mut self, id: u64) -> Result<(), Refusal> {
        if !self.picture.find(id).is_some_and(Node::is_group) {
            return Err(format!("Layer {id} is not a group."));
        }
        self.edit_tree(None, |doc| {
            let (node, place) = doc.picture.remove(id).ok_or("The group is gone.")?;
            let children = node.children.unwrap_or_default();
            doc.active = children.last().map(|child| child.id);
            for (offset, child) in children.into_iter().enumerate() {
                doc.picture.insert(child, Place { parent: place.parent, index: place.index + offset });
            }
            Ok(())
        })
    }

    /// Deletes a layer or a group with everything in it, even when locked.
    /// The picture keeps at least one layer. One step.
    pub fn delete(&mut self, id: u64, by: Option<&str>) -> Result<(), Refusal> {
        let place = self.picture.place(id).ok_or_else(|| format!("There is no layer {id}."))?;
        if place.parent.is_none() && self.picture.nodes.len() == 1 {
            return Err("The picture keeps at least one layer.".into());
        }
        self.edit_tree(by, |doc| {
            doc.picture.remove(id);
            if doc.active.is_some_and(|active| doc.picture.find(active).is_none()) {
                let siblings = doc.picture.siblings(place.parent).cloned().unwrap_or_default();
                doc.active = siblings
                    .get(place.index.saturating_sub(1))
                    .or_else(|| siblings.first())
                    .map(|node| node.id)
                    .or(place.parent)
                    .or_else(|| doc.picture.nodes.last().map(|node| node.id));
            }
            Ok(())
        })
    }

    /// Renames a layer, even when locked. One step.
    pub fn rename(&mut self, id: u64, name: &str, by: Option<&str>) -> Result<(), Refusal> {
        let name = name.trim();
        if name.is_empty() {
            return Err("A layer's name cannot be empty.".into());
        }
        self.edit_tree(by, |doc| {
            doc.picture.find_mut(id).ok_or_else(|| format!("There is no layer {id}."))?.name = name.to_string();
            Ok(())
        })
    }

    pub fn set_visible(&mut self, id: u64, visible: bool, by: Option<&str>) -> Result<(), Refusal> {
        self.edit_tree(by, |doc| {
            doc.picture.find_mut(id).ok_or_else(|| format!("There is no layer {id}."))?.visible = visible;
            Ok(())
        })
    }

    pub fn activate(&mut self, id: u64) {
        if self.picture.find(id).is_some() && self.active != Some(id) {
            self.finish_edit();
            // Choosing a layer is not a step of its own.
            self.active = Some(id);
        }
    }

    /// Moves a layer among its siblings: `to` counts from the top, as the
    /// Layers list shows them. One step.
    pub fn reorder(&mut self, id: u64, to: usize, by: Option<&str>) -> Result<(), Refusal> {
        let place = self.picture.place(id).ok_or_else(|| format!("There is no layer {id}."))?;
        self.move_node(id, place.parent, to, by)
    }

    /// Moves a layer or group into `parent`, a group or the top level, `to`
    /// rows from the top of its children as the Layers list shows them, or
    /// among its siblings when `parent` is where it is. "Lock all" on it or
    /// around it keeps it where it is, and a group locked so takes no new
    /// layers. A group never goes into itself. One step.
    pub fn move_node(&mut self, id: u64, parent: Option<u64>, to: usize, by: Option<&str>) -> Result<(), Refusal> {
        let place = self.picture.place(id).ok_or_else(|| format!("There is no layer {id}."))?;
        if self.picture.locks(id).all {
            return Err("The layer is locked.".into());
        }
        if let Some(group) = parent {
            match self.picture.find(group) {
                Some(node) if node.is_group() => {}
                Some(_) => return Err(format!("Layer {group} is not a group.")),
                None => return Err(format!("There is no group {group}.")),
            }
            if self.picture.ids_within(id).contains(&group) {
                return Err("A group cannot go inside itself.".into());
            }
            if parent != place.parent {
                self.refuse_inside_locked(parent)?;
            }
        }
        let count = self.picture.siblings(parent).map_or(0, Vec::len);
        // The siblings it goes among, without itself.
        let others = if parent == place.parent { count - 1 } else { count };
        let index = others - to.min(others);
        if parent == place.parent && index == place.index {
            return Ok(());
        }
        self.edit_tree(by, |doc| {
            let (node, _) = doc.picture.remove(id).ok_or("The layer is gone.")?;
            doc.picture.insert(node, Place { parent, index });
            Ok(())
        })
    }

    /// Turns one of the active layer's locks on or off. One step.
    pub fn toggle_lock(&mut self, id: u64, lock: usize) -> Result<(), Refusal> {
        self.edit_tree(None, |doc| {
            doc.picture.find_mut(id).ok_or_else(|| format!("There is no layer {id}."))?.locks.toggle(lock);
            Ok(())
        })
    }

    /// Sets the active layer's blend mode, opacity, or fill. While a slider
    /// is held (`preview`), the changes wait to become one step when it is
    /// let go.
    pub fn set_setting(&mut self, setting: Setting, preview: bool) -> Result<(), Refusal> {
        let id = self.active.ok_or("There is no active layer.")?;
        if self.picture.locks(id).all {
            return Err("The layer is locked.".into());
        }
        if self.pending.is_none() {
            self.pending = Some((self.picture.nodes.clone(), self.active));
        }
        let node = self.picture.find_mut(id).ok_or("There is no active layer.")?;
        match setting {
            Setting::Blend(blend) => node.blend = blend,
            Setting::Opacity(value) => node.opacity = value.clamp(0.0, 1.0) as f32,
            Setting::Fill(value) => node.fill = value.clamp(0.0, 1.0) as f32,
        }
        self.edits += 1;
        self.touch_all();
        if !preview {
            self.finish_edit();
        }
        Ok(())
    }

    /// A held slider was let go: its changes become one step.
    pub fn finish_edit(&mut self) {
        let Some((before, active)) = self.pending.take() else {
            return;
        };
        if before != self.picture.nodes {
            self.commit(
                vec![
                    Change::Tree { before, after: self.picture.nodes.clone() },
                    Change::Active { before: active, after: self.active },
                ],
                None,
            );
        }
    }
}

/// A setting of the Layers page.
#[derive(Debug, Clone, Copy)]
pub enum Setting {
    Blend(Blend),
    Opacity(f64),
    Fill(f64),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brush() -> Brush {
        Brush { rgb: [0, 0, 255], size: 8.0, hardness: 1.0, opacity: 1.0, pressure: false, erase: false }
    }

    fn eraser() -> Brush {
        Brush { erase: true, ..brush() }
    }

    fn line() -> Vec<[f64; 3]> {
        vec![[10.0, 20.0, 1.0], [50.0, 20.0, 1.0]]
    }

    /// Catches a stroke that paints outside the selection, or not under its
    /// path, and an undo that does not put every byte back.
    #[test]
    fn a_stroke_paints_under_it_inside_the_selection_and_undoes_exactly() {
        let mut doc = Doc::new(Picture::blank(64, 40));
        let original = doc.composite().to_vec();
        doc.select(Mask::rect(64, 40, [0.0, 0.0], [30.0, 40.0]), Combine::New, 0.0);
        doc.stroke(&line(), &brush()).unwrap();
        let after = doc.composite().to_vec();
        let at = |bytes: &[u8], x: usize, y: usize| bytes[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4].to_vec();
        assert_eq!(at(&after, 20, 20), vec![0, 0, 255, 255], "nothing painted under the stroke");
        for y in 0..40 {
            for x in 30..64 {
                assert_eq!(at(&after, x, y), at(&original, x, y), "painted outside the selection at {x},{y}");
            }
        }
        assert_eq!(at(&after, 20, 2), at(&original, 20, 2), "painted far from the stroke");
        assert!(doc.undo());
        assert_eq!(doc.composite(), &original[..], "undo left a difference");
        assert!(doc.undo(), "the selection was not its own step");
        assert!(doc.selection.is_none());
        assert!(doc.redo() && doc.redo());
        assert_eq!(doc.composite(), &after[..]);
    }

    /// Catches each lock not refusing what it locks, and refusing what it
    /// does not.
    #[test]
    fn each_lock_refuses_what_it_locks() {
        let mut doc = Doc::new(Picture::blank(32, 32));
        let id = doc.active.unwrap();
        let set = |doc: &mut Doc, lock: usize| {
            doc.picture.find_mut(id).unwrap().locks = Default::default();
            doc.toggle_lock(id, lock).unwrap();
        };
        let points = [[16.0, 16.0, 1.0]];

        // Transparency: painting keeps alpha, erasing is refused.
        set(&mut doc, 0);
        let pixel = (16 * 32 + 16) * 4;
        doc.picture.pixels.get_mut(&id).unwrap().data[pixel + 3] = 0;
        doc.stroke(&points, &brush()).unwrap();
        assert_eq!(doc.picture.pixels[&id].data[pixel + 3], 0, "painting made a clear pixel show");
        assert_eq!(doc.picture.pixels[&id].data[pixel], 0, "painting changed no colour");
        assert!(doc.stroke(&points, &eraser()).is_err());
        assert!(doc.move_active(3, 0).is_ok());

        // Pixels: no painting, no erasing; moving is fine.
        set(&mut doc, 1);
        assert!(doc.stroke(&points, &brush()).is_err());
        assert!(doc.stroke(&points, &eraser()).is_err());
        assert!(doc.move_active(3, 0).is_ok());

        // Position: no moving; painting is fine.
        set(&mut doc, 2);
        assert!(doc.move_active(3, 0).is_err());
        assert!(doc.stroke(&points, &brush()).is_ok());

        // All: none of the three, nor settings; renaming and deleting stay.
        set(&mut doc, 3);
        assert!(doc.stroke(&points, &brush()).is_err());
        assert!(doc.stroke(&points, &eraser()).is_err());
        assert!(doc.move_active(3, 0).is_err());
        assert!(doc.set_setting(Setting::Opacity(0.5), false).is_err());
        assert!(doc.rename(id, "Kept", None).is_ok());
        doc.add_layer("", None, None).unwrap();
        assert!(doc.delete(id, None).is_ok());
    }

    /// Catches a move that does not change the offset, a crop that leaves a
    /// size or offset behind, and a crop that cannot be undone.
    #[test]
    fn move_and_crop_change_offsets_and_sizes() {
        let mut doc = Doc::new(Picture::blank(100, 80));
        let background = doc.active.unwrap();
        let layer = doc.add_layer("Top", None, None).unwrap();
        doc.move_active(7, -3).unwrap();
        assert_eq!(doc.picture.find(layer).unwrap().offset, [7, -3]);
        assert_eq!(doc.picture.find(background).unwrap().offset, [0, 0]);
        let before = doc.composite().to_vec();
        doc.crop([10.0, 20.0, 50.0, 40.0]).unwrap();
        assert_eq!((doc.picture.width, doc.picture.height), (50, 40));
        assert_eq!(doc.composite().len(), 50 * 40 * 4);
        let pixels = &doc.picture.pixels[&background];
        assert_eq!((pixels.width, pixels.height), (50, 40));
        assert_eq!(doc.picture.find(background).unwrap().offset, [0, 0]);
        assert_eq!(doc.picture.find(layer).unwrap().offset, [0, 0]);
        doc.undo();
        assert_eq!((doc.picture.width, doc.picture.height), (100, 80));
        assert_eq!(doc.picture.find(layer).unwrap().offset, [7, -3]);
        assert_eq!(doc.composite(), &before[..]);
    }

    /// How long a stroke and its composite take on a new 1920 × 1080 image
    /// with a few layers. Run with `--release --ignored`; a debug build is
    /// many times slower.
    #[test]
    #[ignore]
    fn a_stroke_composites_quickly() {
        let mut doc = Doc::new(Picture::blank(1920, 1080));
        doc.add_layer("A", None, None).unwrap();
        doc.add_layer("B", None, None).unwrap();
        doc.refresh();
        let points: Vec<[f64; 3]> = (0..200).map(|step| [200.0 + f64::from(step) * 6.0, 500.0 + f64::from(step % 40), 0.8]).collect();
        let started = std::time::Instant::now();
        doc.stroke(&points, &Brush { size: 40.0, ..brush() }).unwrap();
        doc.refresh();
        let stroke = started.elapsed();
        let started = std::time::Instant::now();
        doc.touch_all();
        doc.refresh();
        let whole = started.elapsed();
        eprintln!("stroke and its composite: {stroke:?}; the whole composite: {whole:?}");
        if !cfg!(debug_assertions) {
            assert!(stroke < std::time::Duration::from_millis(100), "{stroke:?}");
        }
    }

    /// Catches a layer moved off part of the picture that can no longer be
    /// painted there.
    #[test]
    fn a_moved_layer_still_takes_paint_everywhere() {
        let mut doc = Doc::new(Picture::blank(40, 40));
        let id = doc.add_layer("Top", None, None).unwrap();
        doc.move_active(20, 0).unwrap();
        doc.stroke(&[[5.0, 5.0, 1.0]], &brush()).unwrap();
        assert_eq!(&doc.composite()[(5 * 40 + 5) * 4..(5 * 40 + 5) * 4 + 4], &[0, 0, 255, 255]);
        assert_eq!(doc.picture.find(id).unwrap().offset, [0, 0]);
        doc.undo();
        assert_eq!(doc.picture.find(id).unwrap().offset, [20, 0]);
        assert_eq!(&doc.composite()[(5 * 40 + 5) * 4..(5 * 40 + 5) * 4 + 4], &[255, 255, 255, 255]);
    }

    /// Catches a held slider that makes one step per value, and a group of
    /// the assistant's calls that does not undo as one.
    #[test]
    fn a_held_slider_and_a_request_are_one_step_each() {
        let mut doc = Doc::new(Picture::blank(8, 8));
        for value in [0.9, 0.7, 0.5] {
            doc.set_setting(Setting::Opacity(value), true).unwrap();
        }
        doc.finish_edit();
        assert!(doc.undo());
        assert!(!doc.can_undo());
        assert_eq!(doc.picture.nodes[0].opacity, 1.0);

        let a = doc.add_layer("A", None, Some("request-1")).unwrap();
        doc.rename(a, "B", Some("request-1")).unwrap();
        doc.add_layer("C", None, Some("request-2")).unwrap();
        assert!(doc.undo());
        assert_eq!(doc.picture.nodes.len(), 2);
        assert!(doc.undo());
        assert_eq!(doc.picture.nodes.len(), 1, "one request took two undos");
    }

    /// Catches reorder counting from the bottom when the list counts from
    /// the top, and groups that lose their layers.
    #[test]
    fn layers_reorder_group_and_ungroup() {
        let mut doc = Doc::new(Picture::blank(8, 8));
        let background = doc.active.unwrap();
        let a = doc.add_layer("A", None, None).unwrap();
        let b = doc.add_layer("B", None, None).unwrap();
        let order = |doc: &Doc| doc.picture.nodes.iter().map(|node| node.id).collect::<Vec<_>>();
        assert_eq!(order(&doc), vec![background, a, b]);
        doc.reorder(background, 0, None).unwrap();
        assert_eq!(order(&doc), vec![a, b, background]);
        doc.activate(a);
        let group = doc.group_active().unwrap();
        assert_eq!(doc.picture.find(group).unwrap().children.as_ref().unwrap()[0].id, a);
        let inside = doc.add_layer("Inside", None, None).unwrap();
        assert_eq!(doc.picture.place(inside).unwrap().parent, Some(group));
        doc.ungroup(group).unwrap();
        assert_eq!(order(&doc), vec![a, inside, b, background]);
        doc.delete(b, None).unwrap();
        assert!(doc.picture.find(b).is_none());
        doc.undo();
        assert!(doc.picture.find(b).is_some());
    }
    /// Catches a layer that cannot go into a group or come out of one, a
    /// move that takes more than one undo, and the folder button taking the
    /// active layer away from the layer it wrapped.
    #[test]
    fn layers_go_into_and_out_of_groups_in_one_step() {
        let mut doc = Doc::new(Picture::blank(8, 8));
        let background = doc.active.unwrap();
        let a = doc.add_layer("A", None, None).unwrap();
        let b = doc.add_layer("B", None, None).unwrap();
        doc.activate(a);
        let group = doc.group_active().unwrap();
        assert_eq!(doc.active, Some(a), "the folder button took the wrapped layer's place as active");
        let order = |doc: &Doc, parent: Option<u64>| doc.picture.siblings(parent).unwrap().iter().map(|node| node.id).collect::<Vec<_>>();
        assert_eq!(order(&doc, None), vec![background, group, b]);

        // Into the group, at its top.
        doc.move_node(background, Some(group), 0, None).unwrap();
        assert_eq!(order(&doc, Some(group)), vec![a, background]);
        assert_eq!(order(&doc, None), vec![group, b]);
        assert!(doc.undo());
        assert_eq!(order(&doc, None), vec![background, group, b], "moving into a group took more than one undo");
        assert_eq!(order(&doc, Some(group)), vec![a]);
        assert!(doc.redo());

        // Out of the group, between the two top-level rows: B at the top,
        // then it, then the group.
        doc.move_node(a, None, 1, None).unwrap();
        assert_eq!(order(&doc, None), vec![group, a, b]);
        assert_eq!(order(&doc, Some(group)), vec![background]);
        assert!(doc.undo());
        assert_eq!(order(&doc, Some(group)), vec![a, background]);
        assert_eq!(order(&doc, None), vec![group, b]);
    }

    /// Catches a group put inside itself or its own group, and locks that do
    /// not keep layers where they are.
    #[test]
    fn moving_into_a_group_honours_locks_and_never_nests_a_group_in_itself() {
        let mut doc = Doc::new(Picture::blank(8, 8));
        let background = doc.active.unwrap();
        let outer = doc.group_active().unwrap();
        doc.activate(outer);
        let inner = doc.add_layer("", None, None).unwrap();
        doc.activate(inner);
        let inner_group = doc.group_active().unwrap();
        let before = doc.picture.nodes.clone();
        assert!(doc.move_node(outer, Some(outer), 0, None).is_err(), "a group went into itself");
        assert!(doc.move_node(outer, Some(inner_group), 0, None).is_err(), "a group went into its own group");
        assert_eq!(doc.picture.nodes, before);

        let other = doc.add_layer("Other", None, Some("top")).unwrap();
        // A group locked whole takes no new layers.
        doc.toggle_lock(inner_group, 3).unwrap();
        assert!(doc.move_node(other, Some(inner_group), 0, None).is_err(), "a locked group took a layer");
        // Nor does a layer in it leave.
        assert!(doc.move_node(inner, None, 0, None).is_err(), "a layer left a locked group");
        doc.toggle_lock(inner_group, 3).unwrap();
        // A layer locked whole stays where it is.
        doc.toggle_lock(other, 3).unwrap();
        assert!(doc.move_node(other, Some(outer), 0, None).is_err(), "a locked layer moved");
        doc.toggle_lock(other, 3).unwrap();
        doc.move_node(other, Some(outer), 0, None).unwrap();
        assert_eq!(doc.picture.place(other).unwrap().parent, Some(outer));
        assert_eq!(doc.picture.place(background).unwrap().parent, Some(outer));
    }

    /// Catches a refused stroke or move with no reason to show, or one that
    /// names the wrong thing.
    #[test]
    fn refusals_say_why_in_plain_words() {
        let mut doc = Doc::new(Picture::blank(8, 8));
        let background = doc.active.unwrap();
        assert_eq!(doc.paint_refusal(false), None);
        assert_eq!(doc.move_refusal(), None);
        doc.toggle_lock(background, 3).unwrap();
        assert_eq!(doc.paint_refusal(false).as_deref(), Some("Background is locked."));
        assert_eq!(doc.move_refusal().as_deref(), Some("Background is locked."));
        doc.toggle_lock(background, 3).unwrap();
        doc.toggle_lock(background, 0).unwrap();
        assert_eq!(doc.paint_refusal(false), None);
        assert_eq!(doc.paint_refusal(true).as_deref(), Some("Background's transparency is locked, so it cannot be erased."));
        doc.toggle_lock(background, 0).unwrap();
        doc.toggle_lock(background, 2).unwrap();
        assert_eq!(doc.move_refusal().as_deref(), Some("Background's position is locked."));
        assert_eq!(doc.paint_refusal(true), None);
        let group = doc.group_active().unwrap();
        doc.activate(group);
        assert_eq!(doc.paint_refusal(false).as_deref(), Some("Choose a layer to paint on; a group holds layers."));
        assert_eq!(doc.move_refusal().as_deref(), Some("Background's position is locked."));
        doc.activate(background);
        doc.set_visible(background, false, None).unwrap();
        assert_eq!(doc.paint_refusal(false).as_deref(), Some("Background is hidden."));
    }
}
