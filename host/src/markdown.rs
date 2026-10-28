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
    let text = &visible(text);
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

/// What a stretch of Markdown source is, for colouring it as Jupyter's
/// editor does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Token {
    Plain,
    Heading,
    Quote,
    /// The `*` and `_` that mark emphasis.
    Mark,
    Link,
    Code,
    /// U+FE0E and U+FE0F, which pick an emoji's style and draw nothing.
    Hidden,
}

/// Jupyter's (CodeMirror's) colours for Markdown source.
pub(crate) fn token_color(token: Token) -> Color32 {
    match token {
        Token::Plain | Token::Hidden => ts::TEXT,
        Token::Heading => Color32::from_rgb(0x00, 0x00, 0xFF),
        Token::Quote => Color32::from_rgb(0x00, 0x99, 0x00),
        Token::Mark => Color32::from_rgb(0xAA, 0x22, 0xFF),
        Token::Link => Color32::from_rgb(0x00, 0x00, 0xCC),
        Token::Code => Color32::from_rgb(0x77, 0x00, 0x88),
    }
}

fn hidden(c: char) -> bool {
    c == '\u{FE0E}' || c == '\u{FE0F}'
}

/// The text with the characters that pick an emoji's style removed, since
/// no face draws them.
pub(crate) fn visible(text: &str) -> String {
    text.chars().filter(|c| !hidden(*c)).collect()
}

/// Markdown source cut into stretches that cover every byte once, in
/// order: whole lines for headings, quotes, and fenced code, and within
/// other lines, links, inline code, and emphasis marks.
pub(crate) fn source_tokens(text: &str) -> Vec<(std::ops::Range<usize>, Token)> {
    let mut spans: Vec<(std::ops::Range<usize>, Token)> = Vec::new();
    let mut push = |range: std::ops::Range<usize>, token: Token| {
        if range.is_empty() {
            return;
        }
        match spans.last_mut() {
            Some((last, kind)) if *kind == token && last.end == range.start => last.end = range.end,
            _ => spans.push((range, token)),
        }
    };
    let mut fenced = false;
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let body = &line[indent.min(3)..];
        let fence = indent <= 3 && (body.starts_with("```") || body.starts_with("~~~"));
        let hashes = body.chars().take_while(|c| *c == '#').count();
        let heading = indent <= 3
            && (1..=6).contains(&hashes)
            && body[hashes..].chars().next().is_none_or(|c| c == ' ' || c == '\n');
        let whole = if fence || fenced {
            Some(Token::Code)
        } else if heading {
            Some(Token::Heading)
        } else {
            None
        };
        if fence {
            fenced = !fenced;
        }
        match whole {
            Some(token) => line_with_hidden(&mut push, start, line, token),
            None => {
                let base = if indent <= 3 && body.starts_with('>') { Token::Quote } else { Token::Plain };
                inline_tokens(&mut push, start, line, base);
            }
        }
        start += line.len();
    }
    spans
}

/// Any other prompt: plain, with only the emoji style selectors hidden.
pub(crate) fn source_tokens_plain(text: &str) -> Vec<(std::ops::Range<usize>, Token)> {
    let mut spans: Vec<(std::ops::Range<usize>, Token)> = Vec::new();
    line_with_hidden(
        &mut |range: std::ops::Range<usize>, token: Token| match spans.last_mut() {
            Some((last, kind)) if *kind == token && last.end == range.start => last.end = range.end,
            _ => spans.push((range, token)),
        },
        0,
        text,
        Token::Plain,
    );
    spans
}

fn line_with_hidden(push: &mut impl FnMut(std::ops::Range<usize>, Token), start: usize, line: &str, token: Token) {
    for (at, c) in line.char_indices() {
        let kind = if hidden(c) { Token::Hidden } else { token };
        push(start + at..start + at + c.len_utf8(), kind);
    }
}

/// One line that is not a heading or code: links and inline code in their
/// colours, emphasis marks in theirs (or the quote's), the rest in `base`.
fn inline_tokens(push: &mut impl FnMut(std::ops::Range<usize>, Token), start: usize, line: &str, base: Token) {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let at = |index: usize| chars.get(index).map_or(line.len(), |(offset, _)| *offset);
    let lead = chars.iter().take_while(|(_, c)| *c == ' ').count();
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index].1;
        // Inline code: a run of backticks up to the same run.
        if c == '`' {
            let run = chars[index..].iter().take_while(|(_, c)| *c == '`').count();
            let close = (index + run..chars.len()).find(|&k| {
                chars[k..].iter().take_while(|(_, c)| *c == '`').count() == run
                    && (k == 0 || chars[k - 1].1 != '`')
            });
            if let Some(close) = close {
                line_with_hidden(push, start + at(index), &line[at(index)..at(close + run)], Token::Code);
                index = close + run;
                continue;
            }
        }
        // A link, or an image: [text](target) on one line.
        if c == '[' || (c == '!' && chars.get(index + 1).is_some_and(|(_, c)| *c == '[')) {
            let open = if c == '!' { index + 1 } else { index };
            if let Some(middle) = (open + 1..chars.len().saturating_sub(1))
                .find(|&k| chars[k].1 == ']' && chars[k + 1].1 == '(')
                && let Some(end) = (middle + 2..chars.len()).find(|&k| chars[k].1 == ')')
            {
                line_with_hidden(push, start + at(index), &line[at(index)..at(end + 1)], Token::Link);
                index = end + 1;
                continue;
            }
        }
        if c == '*' || c == '_' {
            let run = chars[index..].iter().take_while(|(_, other)| *other == c).count();
            let before = index.checked_sub(1).map(|k| chars[k].1);
            let after = chars.get(index + run).map(|(_, c)| *c);
            let bullet = c == '*' && index == lead && run == 1 && after == Some(' ');
            let inside_word = c == '_'
                && before.is_some_and(char::is_alphanumeric)
                && after.is_some_and(char::is_alphanumeric);
            let mark = if base == Token::Quote { Token::Quote } else { Token::Mark };
            let token = if bullet || inside_word || run > 3 { base } else { mark };
            push(start + at(index)..start + at(index + run), token);
            index += run;
            continue;
        }
        push(start + at(index)..start + at(index + 1), if hidden(c) { Token::Hidden } else { base });
        index += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::{Token, source_tokens, visible};

    fn tokens(text: &str) -> Vec<(&str, Token)> {
        source_tokens(text).into_iter().map(|(range, token)| (&text[range], token)).collect()
    }

    #[test]
    fn markdown_source_is_coloured_as_jupyter_colours_it() {
        let text = "# Intro ✨\n> **See** [it](https://x)\nA **bold** `code` [link](u) snake_case\n* item\n```\nlet a\n```\n";
        let spans = source_tokens(text);
        // Every byte once, in order, so the editor's cursor is unchanged.
        let mut next = 0;
        for (range, _) in &spans {
            assert_eq!(range.start, next);
            next = range.end;
        }
        assert_eq!(next, text.len());
        let found = tokens(text);
        assert!(found.contains(&("# Intro ✨\n", Token::Heading)));
        assert!(found.contains(&("> **See** ", Token::Quote)));
        assert!(found.contains(&("[it](https://x)", Token::Link)));
        assert!(found.contains(&("**", Token::Mark)));
        assert!(found.contains(&("`code`", Token::Code)));
        assert!(found.contains(&("[link](u)", Token::Link)));
        assert!(found.iter().any(|(text, token)| text.contains("snake_case") && *token == Token::Plain));
        assert!(found.iter().any(|(text, token)| text.contains("* item") && *token == Token::Plain));
        assert!(found.contains(&("```\nlet a\n```\n", Token::Code)));
    }

    /// Catches emoji drawn as "?": every face Studio writes in, bold
    /// headings and code included, has newer emoji such as the test tube.
    #[test]
    fn every_face_draws_emoji() {
        let ctx = egui::Context::default();
        crate::toolbar::install_fonts(&ctx);
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        let faces = [
            egui::FontFamily::Proportional,
            egui::FontFamily::Monospace,
            egui::FontFamily::Name(crate::icons::INTER_BOLD.into()),
            egui::FontFamily::Name(crate::icons::INTER_ITALIC.into()),
            egui::FontFamily::Name(crate::widgets::SEMIBOLD.into()),
        ];
        // egui's own `has_glyph` is wrong for a character in the face that
        // also draws its "missing" mark, so compare the drawn glyphs.
        let glyph = |font: &egui::FontId, c: char| {
            let galley = ctx.fonts_mut(|fonts| fonts.layout_no_wrap(c.to_string(), font.clone(), Color32::BLACK));
            galley.rows[0].glyphs[0].uv_rect
        };
        for family in faces {
            let font = egui::FontId::new(14.0, family.clone());
            let missing = glyph(&font, '\u{10FFFD}');
            for emoji in ['⚠', '✨', '📓', '🧪', '🫠'] {
                assert_ne!(glyph(&font, emoji), missing, "{emoji} is missing in {family:?}");
            }
        }
    }

    #[test]
    fn an_emoji_style_selector_draws_nothing() {
        assert_eq!(visible("⚠\u{FE0F} Experimental"), "⚠ Experimental");
        assert!(tokens("⚠\u{FE0F}").contains(&("\u{FE0F}", Token::Hidden)));
    }

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
