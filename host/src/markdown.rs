//! Markdown as the conversation guide draws it: headings, paragraphs,
//! nested lists, quotes, code blocks, tables, and rules, with bold,
//! italics, inline code, and links inside them. A text is laid out once into
//! [`Laid`], whose height a list allocates and whose shapes are then painted,
//! so the space given and the space drawn always agree.

use crate::typescale as ts;
use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{self, Color32, Galley, Painter, Pos2, Rect, Stroke, pos2, vec2};
use std::sync::Arc;

/// One thing to draw, placed relative to the text's top-left.
enum Piece {
    Text(egui::Vec2, Arc<Galley>),
    Fill(Rect, Color32, Option<Color32>),
    Bar(Rect),
    Rule(f32, f32, f32),
    Disc(Pos2),
}

/// A text laid out to a width: what to draw, and how tall it is.
pub(crate) struct Laid {
    pieces: Vec<Piece>,
    pub height: f32,
    /// Where the last line of text ends, for a streaming caret.
    pub end: Option<Pos2>,
}

impl Laid {
    pub(crate) fn paint(&self, painter: &Painter, origin: Pos2) {
        for piece in &self.pieces {
            match piece {
                Piece::Text(at, galley) => {
                    painter.galley(origin + *at, galley.clone(), ts::TEXT);
                }
                Piece::Fill(rect, fill, edge) => {
                    let rect = rect.translate(origin.to_vec2());
                    painter.rect_filled(rect, 6.0, *fill);
                    if let Some(edge) = edge {
                        painter.rect_stroke(rect, 6.0, Stroke::new(1.0, *edge), egui::StrokeKind::Inside);
                    }
                }
                Piece::Bar(rect) => {
                    painter.rect_filled(rect.translate(origin.to_vec2()), 1.5, ts::BORDER);
                }
                Piece::Rule(x0, x1, y) => {
                    painter.hline((origin.x + x0)..=(origin.x + x1), origin.y + y, Stroke::new(1.0, ts::HAIRLINE));
                }
                Piece::Disc(at) => {
                    painter.circle_filled(origin + at.to_vec2(), 2.25, ts::TEXT);
                }
            }
        }
    }
}

/// One block of a Markdown text.
#[derive(Debug, PartialEq)]
enum Block {
    Heading(usize, String),
    Paragraph(String),
    /// A list item: its depth from 0, and its number when ordered.
    Item(usize, Option<String>, String),
    Code(String),
    Quote(String),
    Table(Vec<Vec<String>>),
    Rule,
}

fn parse(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut lines = text.lines().peekable();
    let flush = |paragraph: &mut Vec<String>, blocks: &mut Vec<Block>| {
        if !paragraph.is_empty() {
            blocks.push(Block::Paragraph(paragraph.join(" ")));
            paragraph.clear();
        }
    };
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            flush(&mut paragraph, &mut blocks);
            let mut code = Vec::new();
            for inner in lines.by_ref() {
                if inner.trim_start().starts_with("```") {
                    break;
                }
                code.push(inner.to_string());
            }
            blocks.push(Block::Code(code.join("\n")));
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut paragraph, &mut blocks);
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Heading(hashes, trimmed[hashes + 1..].trim().to_string()));
            continue;
        }
        if trimmed.len() >= 3 && trimmed.chars().all(|c| c == '-' || c == '*' || c == '_') {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
            continue;
        }
        if let Some(quote) = trimmed.strip_prefix('>') {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Quote(quote.trim().to_string()));
            continue;
        }
        if trimmed.starts_with('|') {
            flush(&mut paragraph, &mut blocks);
            let mut rows = vec![cells(trimmed)];
            while lines.peek().is_some_and(|next| next.trim().starts_with('|')) {
                let next = lines.next().unwrap_or_default().trim();
                let divider = next.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '));
                if !divider {
                    rows.push(cells(next));
                }
            }
            blocks.push(Block::Table(rows));
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let depth = indent / 2;
        if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Item(depth, None, item.to_string()));
            continue;
        }
        let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && trimmed[digits..].starts_with(". ") {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Item(depth, Some(trimmed[..digits].to_string()), trimmed[digits + 2..].to_string()));
            continue;
        }
        paragraph.push(trimmed.to_string());
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

fn cells(row: &str) -> Vec<String> {
    row.trim().trim_matches('|').split('|').map(|cell| cell.trim().to_string()).collect()
}

/// `text` laid out to `width`, in `color`.
pub(crate) fn lay_out(painter: &Painter, text: &str, color: Color32, width: f32) -> Laid {
    let width = width.max(40.0);
    let mut pieces = Vec::new();
    let mut y = 0.0;
    let mut end = None;
    let blocks = parse(text);
    let mut previous: Option<&Block> = None;
    for block in &blocks {
        // The gap above this block, by the guide's spacing.
        let gap = match (previous, block) {
            (None, _) => 0.0,
            (_, Block::Heading(..)) => ts::HEADING_ABOVE,
            (Some(Block::Heading(..)), _) => ts::HEADING_BELOW,
            (_, Block::Code(_)) => ts::CODE_ABOVE,
            (Some(Block::Code(_)), _) => ts::CODE_BELOW,
            (Some(Block::Item(..)), Block::Item(..)) => ts::LIST_ITEM,
            _ => ts::PARAGRAPH,
        };
        y += gap;
        match block {
            Block::Heading(level, words) => {
                let (size, line) = ts::heading(*level);
                let galley = painter.layout_job(inline(words, color, size, line, true, width));
                end = Some(caret_end(&galley, vec2(0.0, y)));
                let h = galley.size().y;
                pieces.push(Piece::Text(vec2(0.0, y), galley));
                y += h;
            }
            Block::Paragraph(words) => {
                let galley = painter.layout_job(inline(words, color, ts::BODY, ts::BODY_LINE, false, width));
                end = Some(caret_end(&galley, vec2(0.0, y)));
                let h = galley.size().y;
                pieces.push(Piece::Text(vec2(0.0, y), galley));
                y += h;
            }
            Block::Item(depth, number, words) => {
                let indent = ts::LIST_INDENT * (*depth as f32 + 1.0);
                let galley = painter.layout_job(inline(words, color, ts::BODY, ts::BODY_LINE, false, width - indent));
                match number {
                    Some(number) => {
                        let mark = painter.layout_job(inline(&format!("{number}."), color, ts::BODY, ts::BODY_LINE, false, 60.0));
                        let at = vec2(indent - 6.0 - mark.size().x, y);
                        pieces.push(Piece::Text(at, mark));
                    }
                    None => pieces.push(Piece::Disc(pos2(indent - 10.0, y + ts::BODY_LINE / 2.0))),
                }
                end = Some(caret_end(&galley, vec2(indent, y)));
                let h = galley.size().y;
                pieces.push(Piece::Text(vec2(indent, y), galley));
                y += h;
            }
            Block::Quote(words) => {
                let galley = painter.layout_job(inline(words, ts::MUTED, ts::BODY, ts::BODY_LINE, false, width - 14.0));
                let h = galley.size().y;
                pieces.push(Piece::Bar(Rect::from_min_size(pos2(0.0, y), vec2(3.0, h))));
                end = Some(caret_end(&galley, vec2(14.0, y)));
                pieces.push(Piece::Text(vec2(14.0, y), galley));
                y += h;
            }
            Block::Code(code) => {
                let mut job = LayoutJob::single_section(code.clone(), format(ts::code(), color, ts::CODE_LINE, Color32::TRANSPARENT));
                job.wrap.max_width = width - 2.0 * ts::CODE_PAD;
                let galley = painter.layout_job(job);
                let h = galley.size().y + 2.0 * ts::CODE_PAD;
                pieces.push(Piece::Fill(Rect::from_min_size(pos2(0.0, y), vec2(width, h)), ts::CODE_FILL, Some(ts::BORDER)));
                end = Some(caret_end(&galley, vec2(ts::CODE_PAD, y + ts::CODE_PAD)));
                pieces.push(Piece::Text(vec2(ts::CODE_PAD, y + ts::CODE_PAD), galley));
                y += h;
            }
            Block::Table(rows) => {
                y += table(painter, rows, color, width, y, &mut pieces);
                end = None;
            }
            Block::Rule => {
                pieces.push(Piece::Rule(0.0, width, y + 4.0));
                y += 8.0;
            }
        }
        previous = Some(block);
    }
    Laid { pieces, height: y, end }
}

/// A table: a bold header row, hairlines between rows, columns shared out
/// by how much each needs. Returns its height.
fn table(painter: &Painter, rows: &[Vec<String>], color: Color32, width: f32, top: f32, pieces: &mut Vec<Piece>) -> f32 {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
    let pad = 8.0;
    let mut natural = vec![0.0_f32; columns];
    for (row_index, row) in rows.iter().enumerate() {
        for (index, cell) in row.iter().enumerate() {
            // The header row is bold, so it is measured bold.
            let w = painter.layout_job(inline(cell, color, ts::BODY, ts::BODY_LINE, row_index == 0, f32::INFINITY)).size().x + 2.0 * pad + 1.0;
            natural[index] = natural[index].max(w);
        }
    }
    let total: f32 = natural.iter().sum();
    let scale = if total > width { width / total } else { 1.0 };
    let widths: Vec<f32> = natural.iter().map(|w| (w * scale).max(32.0)).collect();
    let line_w: f32 = widths.iter().sum::<f32>().min(width);
    let mut y = top;
    for (index, row) in rows.iter().enumerate() {
        let header = index == 0;
        let mut x = 0.0;
        let mut tallest: f32 = 0.0;
        let mut laid = Vec::new();
        for (column, cell) in row.iter().enumerate() {
            let w = widths.get(column).copied().unwrap_or(32.0);
            let galley = painter.layout_job(inline(cell, color, ts::BODY, ts::BODY_LINE, header, w - 2.0 * pad));
            tallest = tallest.max(galley.size().y);
            laid.push((x + pad, galley));
            x += w;
        }
        for (x, galley) in laid {
            pieces.push(Piece::Text(vec2(x, y + 6.0), galley));
        }
        y += tallest + 12.0;
        pieces.push(Piece::Rule(0.0, line_w, y));
    }
    y - top
}

/// Where a caret goes after a galley's last character.
fn caret_end(galley: &Galley, at: egui::Vec2) -> Pos2 {
    match galley.rows.last() {
        Some(row) => pos2(at.x + row.rect().right(), at.y + row.rect().top()),
        None => pos2(at.x, at.y),
    }
}

fn format(font_id: egui::FontId, color: Color32, line: f32, background: Color32) -> TextFormat {
    TextFormat {
        font_id,
        color,
        background,
        line_height: Some(line),
        // Smaller text, such as inline code, sits on the same baseline.
        valign: egui::Align::BOTTOM,
        ..Default::default()
    }
}

/// One line of words with `**bold**`, `*italic*` or `_italic_`,
/// `` `code` ``, and `[links](to)` shown as their words, underlined.
fn inline(text: &str, color: Color32, size: f32, line: f32, strong: bool, width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = width.max(20.0);
    let mut bold = false;
    let mut italic = false;
    let mut plain = String::new();
    let mut rest = text;
    let push = |job: &mut LayoutJob, plain: &mut String, bold: bool, italic: bool, underline: bool| {
        if !plain.is_empty() {
            let mut format = format(ts::inter(size, bold || strong, italic), color, line, Color32::TRANSPARENT);
            if underline {
                format.underline = Stroke::new(1.0, color);
            }
            job.append(plain, 0.0, format);
            plain.clear();
        }
    };
    while let Some(c) = rest.chars().next() {
        if rest.starts_with("**") {
            push(&mut job, &mut plain, bold, italic, false);
            bold = !bold;
            rest = &rest[2..];
            continue;
        }
        if (c == '*' || c == '_') && rest.len() > 1 {
            push(&mut job, &mut plain, bold, italic, false);
            italic = !italic;
            rest = &rest[1..];
            continue;
        }
        if c == '`'
            && let Some(close) = rest[1..].find('`')
        {
            push(&mut job, &mut plain, bold, italic, false);
            job.append(&rest[1..1 + close], 0.0, format(ts::code(), color, line, ts::CODE_FILL));
            rest = &rest[close + 2..];
            continue;
        }
        if c == '['
            && let Some(close) = rest.find("](")
            && let Some(end) = rest[close..].find(')')
        {
            push(&mut job, &mut plain, bold, italic, false);
            plain.push_str(&rest[1..close]);
            push(&mut job, &mut plain, bold, italic, true);
            rest = &rest[close + end + 1..];
            continue;
        }
        plain.push(c);
        rest = &rest[c.len_utf8()..];
    }
    push(&mut job, &mut plain, bold, italic, false);
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Catches Markdown shown as written, or parsed into the wrong blocks.
    #[test]
    fn markdown_parses_into_its_blocks() {
        let text = "## The Last Signal\n\nHe hears a ship that *should not exist*.\n\n- one\n  - nested\n1. first\n\n```\nlet x = 1;\n```\n> said\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n---";
        let blocks = parse(text);
        assert_eq!(blocks[0], Block::Heading(2, "The Last Signal".into()));
        assert_eq!(blocks[1], Block::Paragraph("He hears a ship that *should not exist*.".into()));
        assert_eq!(blocks[2], Block::Item(0, None, "one".into()));
        assert_eq!(blocks[3], Block::Item(1, None, "nested".into()));
        assert_eq!(blocks[4], Block::Item(0, Some("1".into()), "first".into()));
        assert_eq!(blocks[5], Block::Code("let x = 1;".into()));
        assert_eq!(blocks[6], Block::Quote("said".into()));
        assert_eq!(blocks[7], Block::Table(vec![vec!["a".into(), "b".into()], vec!["1".into(), "2".into()]]));
        assert_eq!(blocks[8], Block::Rule);
    }

    /// Catches mixed sizes: prose uses the guide's body size and line, code
    /// its code size, and the marks are gone.
    #[test]
    fn inline_text_uses_the_type_scale() {
        let job = inline("He **answers** a *ship* with `code`.", ts::TEXT, ts::BODY, ts::BODY_LINE, false, 400.0);
        assert!(!job.text.contains('*') && !job.text.contains('`'));
        for section in &job.sections {
            let size = section.format.font_id.size;
            assert!(size == ts::BODY || size == ts::CODE, "an off-scale size {size}");
            assert_eq!(section.format.line_height, Some(ts::BODY_LINE));
        }
    }
}
