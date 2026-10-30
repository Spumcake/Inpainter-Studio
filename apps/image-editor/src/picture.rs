//! A layered picture: its size, a tree of layers and groups, each layer's
//! pixels, and the composite of them all.
//!
//! The tree holds only each node's settings, so a step of history can keep a
//! copy of it cheaply. Pixels live apart, by layer id, and stay there after a
//! layer is deleted, so undoing the delete brings them back untouched.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A rectangle of whole pixels. Empty when either side is not positive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn intersect(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        Rect::new(x, y, (right - x).max(0), (bottom - y).max(0))
    }

    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Rect::new(x, y, right - x, bottom - y)
    }

    pub fn contains(&self, other: &Rect) -> bool {
        other.is_empty() || self.intersect(other) == *other
    }
}

/// How a layer mixes with what is under it: the standard separable modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Blend {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
}

impl Blend {
    pub const ALL: [Blend; 6] = [
        Blend::Normal,
        Blend::Multiply,
        Blend::Screen,
        Blend::Overlay,
        Blend::Darken,
        Blend::Lighten,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Blend::Normal => "Normal",
            Blend::Multiply => "Multiply",
            Blend::Screen => "Screen",
            Blend::Overlay => "Overlay",
            Blend::Darken => "Darken",
            Blend::Lighten => "Lighten",
        }
    }

    pub fn index(self) -> usize {
        Blend::ALL.iter().position(|blend| *blend == self).unwrap_or(0)
    }

    /// One channel: `b` the backdrop, `s` the source, both from 0 to 1.
    fn mix(self, b: f32, s: f32) -> f32 {
        match self {
            Blend::Normal => s,
            Blend::Multiply => b * s,
            Blend::Screen => b + s - b * s,
            // Hard light with the layers swapped.
            Blend::Overlay => {
                if b <= 0.5 {
                    2.0 * b * s
                } else {
                    1.0 - 2.0 * (1.0 - b) * (1.0 - s)
                }
            }
            Blend::Darken => b.min(s),
            Blend::Lighten => b.max(s),
        }
    }
}

/// A layer's four locks. `all` also counts as each of the other three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Locks {
    #[serde(default)]
    pub transparency: bool,
    #[serde(default)]
    pub pixels: bool,
    #[serde(default)]
    pub position: bool,
    #[serde(default)]
    pub all: bool,
}

impl Locks {
    pub fn get(&self, index: usize) -> bool {
        match index {
            0 => self.transparency,
            1 => self.pixels,
            2 => self.position,
            _ => self.all,
        }
    }

    pub fn toggle(&mut self, index: usize) {
        match index {
            0 => self.transparency = !self.transparency,
            1 => self.pixels = !self.pixels,
            2 => self.position = !self.position,
            _ => self.all = !self.all,
        }
    }

    /// These locks and a parent's, together.
    fn with(self, other: Locks) -> Locks {
        Locks {
            transparency: self.transparency || other.transparency,
            pixels: self.pixels || other.pixels,
            position: self.position || other.position,
            all: self.all || other.all,
        }
    }

    pub fn keeps_alpha(&self) -> bool {
        self.transparency || self.all
    }

    pub fn keeps_pixels(&self) -> bool {
        self.pixels || self.all
    }

    pub fn keeps_position(&self) -> bool {
        self.position || self.all
    }
}

/// One layer or group. A group has `children`, bottom to top.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: u64,
    pub name: String,
    pub visible: bool,
    pub blend: Blend,
    /// Scales the layer, or the whole group, from 0 to 1.
    pub opacity: f32,
    /// Scales only the layer's own pixels' alpha, from 0 to 1.
    pub fill: f32,
    pub locks: Locks,
    /// Where the layer's pixels sit in the picture. Not used by a group.
    pub offset: [i32; 2],
    pub children: Option<Vec<Node>>,
}

impl Node {
    pub fn layer(id: u64, name: &str) -> Self {
        Self {
            id,
            name: name.to_string(),
            visible: true,
            blend: Blend::Normal,
            opacity: 1.0,
            fill: 1.0,
            locks: Locks::default(),
            offset: [0, 0],
            children: None,
        }
    }

    pub fn group(id: u64, name: &str, children: Vec<Node>) -> Self {
        Self {
            children: Some(children),
            ..Self::layer(id, name)
        }
    }

    pub fn is_group(&self) -> bool {
        self.children.is_some()
    }
}

/// A layer's pixels: RGBA8, straight alpha, rows top to bottom.
#[derive(Debug, Clone, PartialEq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Pixels {
    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let mut data = Vec::with_capacity(width as usize * height as usize * 4);
        for _ in 0..width as usize * height as usize {
            data.extend_from_slice(&rgba);
        }
        Self { width, height, data }
    }

    pub fn clear(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; width as usize * height as usize * 4],
        }
    }

    /// The bytes of `rect`, in this buffer's own pixels, row after row.
    pub fn read(&self, rect: Rect) -> Vec<u8> {
        let mut out = Vec::with_capacity(rect.w.max(0) as usize * rect.h.max(0) as usize * 4);
        for y in rect.y..rect.bottom() {
            let start = (y as usize * self.width as usize + rect.x as usize) * 4;
            out.extend_from_slice(&self.data[start..start + rect.w as usize * 4]);
        }
        out
    }

    /// Puts back bytes `read` took from `rect`.
    pub fn write(&mut self, rect: Rect, bytes: &[u8]) {
        let row = rect.w as usize * 4;
        for (index, y) in (rect.y..rect.bottom()).enumerate() {
            let start = (y as usize * self.width as usize + rect.x as usize) * 4;
            self.data[start..start + row].copy_from_slice(&bytes[index * row..(index + 1) * row]);
        }
    }

    /// A part of these pixels, with what falls outside them transparent.
    /// `rect` is in this buffer's own pixels and may reach past its edges.
    pub fn cut(&self, rect: Rect) -> Pixels {
        let mut out = Pixels::clear(rect.w.max(1) as u32, rect.h.max(1) as u32);
        let own = Rect::new(0, 0, self.width as i32, self.height as i32);
        let shared = own.intersect(&rect);
        if !shared.is_empty() {
            let bytes = self.read(shared);
            out.write(Rect::new(shared.x - rect.x, shared.y - rect.y, shared.w, shared.h), &bytes);
        }
        out
    }
}

/// The picture: its size, the tree, and every layer's pixels.
#[derive(Debug, Clone)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// Bottom to top.
    pub nodes: Vec<Node>,
    pub pixels: HashMap<u64, Pixels>,
    pub next_id: u64,
}

/// Where a node sits: its parent group, or none at the top, and its index
/// among its siblings, bottom first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    pub parent: Option<u64>,
    pub index: usize,
}

impl Picture {
    /// A new picture with one white "Background" layer.
    pub fn blank(width: u32, height: u32) -> Self {
        let mut picture = Self {
            width,
            height,
            nodes: Vec::new(),
            pixels: HashMap::new(),
            next_id: 1,
        };
        let id = picture.take_id();
        picture.nodes.push(Node::layer(id, "Background"));
        picture.pixels.insert(id, Pixels::filled(width, height, [255, 255, 255, 255]));
        picture
    }

    /// A picture made of one image, as its only layer.
    pub fn from_pixels(name: &str, pixels: Pixels) -> Self {
        let mut picture = Self {
            width: pixels.width,
            height: pixels.height,
            nodes: Vec::new(),
            pixels: HashMap::new(),
            next_id: 1,
        };
        let id = picture.take_id();
        picture.nodes.push(Node::layer(id, name));
        picture.pixels.insert(id, pixels);
        picture
    }

    pub fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width as i32, self.height as i32)
    }

    pub fn find(&self, id: u64) -> Option<&Node> {
        find_in(&self.nodes, id)
    }

    pub fn find_mut(&mut self, id: u64) -> Option<&mut Node> {
        find_in_mut(&mut self.nodes, id)
    }

    pub fn place(&self, id: u64) -> Option<Place> {
        place_in(&self.nodes, None, id)
    }

    /// The siblings under `parent`, or the top level.
    pub fn siblings(&self, parent: Option<u64>) -> Option<&Vec<Node>> {
        match parent {
            None => Some(&self.nodes),
            Some(id) => self.find(id)?.children.as_ref(),
        }
    }

    pub fn siblings_mut(&mut self, parent: Option<u64>) -> Option<&mut Vec<Node>> {
        match parent {
            None => Some(&mut self.nodes),
            Some(id) => self.find_mut(id)?.children.as_mut(),
        }
    }

    /// The node's own locks and every enclosing group's, together.
    pub fn locks(&self, id: u64) -> Locks {
        let mut locks = Locks::default();
        let mut current = Some(id);
        while let Some(at) = current {
            let Some(node) = self.find(at) else {
                break;
            };
            locks = locks.with(node.locks);
            current = self.place(at).and_then(|place| place.parent);
        }
        locks
    }

    /// The node and every node inside it.
    pub fn ids_within(&self, id: u64) -> Vec<u64> {
        let mut out = Vec::new();
        if let Some(node) = self.find(id) {
            collect_ids(node, &mut out);
        }
        out
    }

    /// Every layer that has pixels, under `id`, or `id` itself.
    pub fn layers_within(&self, id: u64) -> Vec<u64> {
        self.ids_within(id)
            .into_iter()
            .filter(|id| self.find(*id).is_some_and(|node| !node.is_group()))
            .collect()
    }

    /// Every layer with pixels in the picture.
    pub fn all_layers(&self) -> Vec<u64> {
        let mut out = Vec::new();
        for node in &self.nodes {
            collect_ids(node, &mut out);
        }
        out.retain(|id| self.find(*id).is_some_and(|node| !node.is_group()));
        out
    }

    /// A layer's box in the picture, or a group's: the union of its layers'.
    pub fn node_rect(&self, id: u64) -> Option<Rect> {
        let node = self.find(id)?;
        if node.is_group() {
            let mut rect = Rect::new(0, 0, 0, 0);
            for layer in self.layers_within(id) {
                if let Some(inner) = self.node_rect(layer) {
                    rect = rect.union(&inner);
                }
            }
            return (!rect.is_empty()).then_some(rect);
        }
        let pixels = self.pixels.get(&id)?;
        Some(Rect::new(node.offset[0], node.offset[1], pixels.width as i32, pixels.height as i32))
    }

    /// Takes a node out of the tree, and says where it was.
    pub fn remove(&mut self, id: u64) -> Option<(Node, Place)> {
        let place = self.place(id)?;
        let siblings = self.siblings_mut(place.parent)?;
        Some((siblings.remove(place.index), place))
    }

    /// Puts a node in at `place`, its index clamped to the siblings.
    pub fn insert(&mut self, node: Node, place: Place) -> bool {
        let Some(siblings) = self.siblings_mut(place.parent) else {
            return false;
        };
        let index = place.index.min(siblings.len());
        siblings.insert(index, node);
        true
    }

    /// Composites `rect` of the picture into `out`, the whole picture as
    /// RGBA8 with straight alpha.
    pub fn composite_into(&self, rect: Rect, out: &mut [u8]) {
        let rect = rect.intersect(&self.bounds());
        if rect.is_empty() {
            return;
        }
        let row_bytes = self.width as usize * 4;
        let rows = &mut out[rect.y as usize * row_bytes..rect.bottom() as usize * row_bytes];
        // A large area is split into bands of rows, one per core, as each
        // pixel is composited by itself.
        let cores = std::thread::available_parallelism().map_or(1, usize::from);
        let bands = if (rect.w as usize) * (rect.h as usize) < 65_536 { 1 } else { cores.min(rect.h as usize) };
        let band_rows = (rect.h as usize).div_ceil(bands);
        if bands <= 1 {
            self.composite_band(rect, rows);
            return;
        }
        std::thread::scope(|scope| {
            for (index, chunk) in rows.chunks_mut(band_rows * row_bytes).enumerate() {
                let y = rect.y + (index * band_rows) as i32;
                let band = Rect::new(rect.x, y, rect.w, (chunk.len() / row_bytes) as i32);
                scope.spawn(move || self.composite_band(band, chunk));
            }
        });
    }

    /// Composites `rect` into `rows`, the picture's rows it covers.
    fn composite_band(&self, rect: Rect, rows: &mut [u8]) {
        let mut buffer = vec![[0.0f32; 4]; rect.w as usize * rect.h as usize];
        self.composite_nodes(&self.nodes, rect, &mut buffer);
        let row_bytes = self.width as usize * 4;
        for row in 0..rect.h as usize {
            let start = row * row_bytes + rect.x as usize * 4;
            let line = &mut rows[start..start + rect.w as usize * 4];
            for (column, pixel) in buffer[row * rect.w as usize..(row + 1) * rect.w as usize].iter().enumerate() {
                let alpha = pixel[3];
                let byte = &mut line[column * 4..column * 4 + 4];
                if alpha <= 0.0 {
                    byte.copy_from_slice(&[0, 0, 0, 0]);
                    continue;
                }
                for channel in 0..3 {
                    byte[channel] = to_byte(pixel[channel] / alpha);
                }
                byte[3] = to_byte(alpha);
            }
        }
    }

    /// The whole composite. The editor keeps its own and redoes only what
    /// changed; the tests take it whole.
    #[cfg(test)]
    pub fn composite(&self) -> Vec<u8> {
        let mut out = vec![0; self.width as usize * self.height as usize * 4];
        self.composite_into(self.bounds(), &mut out);
        out
    }

    /// Blends `nodes`, bottom to top, into `buffer`: `rect` of the picture,
    /// premultiplied.
    fn composite_nodes(&self, nodes: &[Node], rect: Rect, buffer: &mut [[f32; 4]]) {
        let table = unit_table();
        for node in nodes.iter().filter(|node| node.visible && node.opacity > 0.0) {
            if let Some(children) = &node.children {
                // A group composites its children by themselves, then
                // blends the result as one layer.
                let mut inner = vec![[0.0f32; 4]; buffer.len()];
                self.composite_nodes(children, rect, &mut inner);
                for (dst, src) in buffer.iter_mut().zip(inner.iter()) {
                    if src[3] <= 0.0 {
                        continue;
                    }
                    let alpha = src[3];
                    let rgb = [src[0] / alpha, src[1] / alpha, src[2] / alpha];
                    blend_pixel(dst, rgb, alpha * node.opacity, node.blend);
                }
                continue;
            }
            let Some(pixels) = self.pixels.get(&node.id) else {
                continue;
            };
            let layer = Rect::new(node.offset[0], node.offset[1], pixels.width as i32, pixels.height as i32);
            let shared = layer.intersect(&rect);
            if shared.is_empty() {
                continue;
            }
            let scale = node.opacity * node.fill;
            if scale <= 0.0 {
                continue;
            }
            for y in shared.y..shared.bottom() {
                let source_row = (y - layer.y) as usize * pixels.width as usize;
                let target_row = (y - rect.y) as usize * rect.w as usize;
                for x in shared.x..shared.right() {
                    let at = (source_row + (x - layer.x) as usize) * 4;
                    let alpha = pixels.data[at + 3];
                    if alpha == 0 {
                        continue;
                    }
                    let rgb = [
                        table[pixels.data[at] as usize],
                        table[pixels.data[at + 1] as usize],
                        table[pixels.data[at + 2] as usize],
                    ];
                    let dst = &mut buffer[target_row + (x - rect.x) as usize];
                    blend_pixel(dst, rgb, table[alpha as usize] * scale, node.blend);
                }
            }
        }
    }
}

/// One source pixel, straight colour and alpha, over a premultiplied
/// backdrop. Where the backdrop is opaque the blend function takes the
/// source's place; where it is clear the source shows as itself.
fn blend_pixel(dst: &mut [f32; 4], rgb: [f32; 3], alpha: f32, blend: Blend) {
    if alpha <= 0.0 {
        return;
    }
    let backdrop = dst[3];
    let mut mixed = rgb;
    if blend != Blend::Normal && backdrop > 0.0 {
        for channel in 0..3 {
            let b = dst[channel] / backdrop;
            mixed[channel] = (1.0 - backdrop) * rgb[channel] + backdrop * blend.mix(b, rgb[channel]);
        }
    }
    for channel in 0..3 {
        dst[channel] = alpha * mixed[channel] + dst[channel] * (1.0 - alpha);
    }
    dst[3] = alpha + backdrop * (1.0 - alpha);
}

fn to_byte(value: f32) -> u8 {
    (value * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Each byte as a fraction, so the composite does not divide per pixel.
fn unit_table() -> &'static [f32; 256] {
    static TABLE: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0.0; 256];
        for (index, value) in table.iter_mut().enumerate() {
            *value = index as f32 / 255.0;
        }
        table
    })
}

fn collect_ids(node: &Node, out: &mut Vec<u64>) {
    out.push(node.id);
    if let Some(children) = &node.children {
        for child in children {
            collect_ids(child, out);
        }
    }
}

fn find_in(nodes: &[Node], id: u64) -> Option<&Node> {
    for node in nodes {
        if node.id == id {
            return Some(node);
        }
        if let Some(found) = node.children.as_deref().and_then(|children| find_in(children, id)) {
            return Some(found);
        }
    }
    None
}

fn find_in_mut(nodes: &mut [Node], id: u64) -> Option<&mut Node> {
    for node in nodes {
        if node.id == id {
            return Some(node);
        }
        if let Some(found) = node.children.as_deref_mut().and_then(|children| find_in_mut(children, id)) {
            return Some(found);
        }
    }
    None
}

fn place_in(nodes: &[Node], parent: Option<u64>, id: u64) -> Option<Place> {
    for (index, node) in nodes.iter().enumerate() {
        if node.id == id {
            return Some(Place { parent, index });
        }
        if let Some(found) = node.children.as_deref().and_then(|children| place_in(children, Some(node.id), id)) {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-layer picture of one pixel: an opaque backdrop and a source
    /// with the blend mode and alpha given.
    fn pair(backdrop: [u8; 4], source: [u8; 4], blend: Blend) -> [u8; 4] {
        let mut picture = Picture::from_pixels("under", Pixels::filled(1, 1, backdrop));
        let id = picture.take_id();
        let mut node = Node::layer(id, "over");
        node.blend = blend;
        picture.nodes.push(node);
        picture.pixels.insert(id, Pixels::filled(1, 1, source));
        let out = picture.composite();
        [out[0], out[1], out[2], out[3]]
    }

    /// Catches a blend mode computed with the wrong formula, or the layers
    /// swapped: each mode on known pixels, against the standard formulas
    /// worked by hand.
    #[test]
    fn each_blend_mode_on_known_pixels() {
        let b = [200, 100, 50, 255];
        let s = [100, 150, 250, 255];
        assert_eq!(pair(b, s, Blend::Normal), [100, 150, 250, 255]);
        // 200·100/255 = 78.4, 100·150/255 = 58.8, 50·250/255 = 49.0.
        assert_eq!(pair(b, s, Blend::Multiply), [78, 59, 49, 255]);
        // b + s − bs: 200+100−78.4 = 221.6, 100+150−58.8 = 191.2, 50+250−49 = 251.
        assert_eq!(pair(b, s, Blend::Screen), [222, 191, 251, 255]);
        // Overlay: b > ½ screens, b ≤ ½ multiplies, both doubled.
        // R: 1 − 2(1−200/255)(1−100/255) = 0.7378 → 188.
        // G: 2·(100/255)(150/255) = 0.4614 → 118.
        // B: 2·(50/255)(250/255) = 0.3845 → 98.
        assert_eq!(pair(b, s, Blend::Overlay), [188, 118, 98, 255]);
        assert_eq!(pair(b, s, Blend::Darken), [100, 100, 50, 255]);
        assert_eq!(pair(b, s, Blend::Lighten), [200, 150, 250, 255]);
        // A half-clear multiply source mixes halfway: 200·0.5 + 78.4·0.5.
        let half = pair(b, [100, 150, 250, 128], Blend::Multiply);
        assert_eq!(half[0], 139);
        // Over a clear backdrop a blend mode shows the source as itself.
        assert_eq!(pair([0, 0, 0, 0], s, Blend::Multiply), [100, 150, 250, 255]);
    }

    /// Catches opacity and fill ignored, and a group that blends its
    /// children one by one instead of as one: a Multiply group of two
    /// Normal layers, the top hiding the bottom, multiplies once.
    #[test]
    fn opacity_fill_and_groups() {
        let mut picture = Picture::from_pixels("under", Pixels::filled(1, 1, [200, 200, 200, 255]));
        let a = picture.take_id();
        let b = picture.take_id();
        let group = picture.take_id();
        picture.pixels.insert(a, Pixels::filled(1, 1, [0, 0, 0, 255]));
        picture.pixels.insert(b, Pixels::filled(1, 1, [255, 255, 255, 255]));
        let mut node = Node::group(group, "group", vec![Node::layer(a, "a"), Node::layer(b, "b")]);
        node.blend = Blend::Multiply;
        picture.nodes.push(node);
        // White multiplied over grey leaves grey; black, had it been
        // multiplied by itself, would make black.
        assert_eq!(&picture.composite()[..4], &[200, 200, 200, 255]);
        picture.find_mut(b).unwrap().visible = false;
        assert_eq!(&picture.composite()[..4], &[0, 0, 0, 255]);
        picture.find_mut(group).unwrap().opacity = 0.5;
        assert_eq!(&picture.composite()[..4], &[100, 100, 100, 255]);
        picture.find_mut(group).unwrap().opacity = 1.0;
        picture.find_mut(a).unwrap().fill = 0.25;
        assert_eq!(&picture.composite()[..4], &[150, 150, 150, 255]);
        picture.find_mut(a).unwrap().fill = 1.0;
        picture.find_mut(a).unwrap().opacity = 0.0;
        assert_eq!(&picture.composite()[..4], &[200, 200, 200, 255]);
    }

    /// Catches a group's lock not reaching the layers inside it.
    #[test]
    fn a_group_lock_reaches_its_layers() {
        let mut picture = Picture::blank(4, 4);
        let inner = picture.take_id();
        let group = picture.take_id();
        picture.pixels.insert(inner, Pixels::clear(4, 4));
        let mut node = Node::group(group, "group", vec![Node::layer(inner, "inner")]);
        node.locks.position = true;
        picture.nodes.push(node);
        assert!(picture.locks(inner).keeps_position());
        assert!(!picture.locks(inner).keeps_pixels());
    }
}
