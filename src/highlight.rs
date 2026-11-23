// Syntax highlighting for the text editor, colored from the Omarchy palette rather than a
// stock theme, so code follows the desktop's theme like the rest of Ferry. Grammars are
// syntect's (Sublime Text's), loaded once on first use.

use std::ops::Range;
use std::path::Path;
use std::str::FromStr;
use std::sync::OnceLock;

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{Color32, FontId, Stroke};
use eframe::epaint::text::ByteIndex;
use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color, FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::theme::Palette;

/// Above this size the editor shows plain text: highlighting re-runs on every keystroke.
pub const MAX_BYTES: usize = 256 * 1024;

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn color(c: Color32) -> Color {
    Color {
        r: c.r(),
        g: c.g(),
        b: c.b(),
        a: 255,
    }
}

/// A syntect theme made from the palette: scope selectors mapped to the theme's colors.
pub fn theme(p: &Palette) -> Theme {
    let item = |scopes: &str, fg: Color32, style: Option<FontStyle>| ThemeItem {
        scope: ScopeSelectors::from_str(scopes).unwrap_or_default(),
        style: StyleModifier {
            foreground: Some(color(fg)),
            background: None,
            font_style: style,
        },
    };
    let italic = Some(FontStyle::ITALIC);
    let bold = Some(FontStyle::BOLD);
    Theme {
        name: Some("Omarchy".into()),
        author: None,
        settings: ThemeSettings {
            foreground: Some(color(p.fg)),
            ..Default::default()
        },
        scopes: vec![
            item("comment, punctuation.definition.comment", p.dim, italic),
            item("string, markup.raw, markup.inline.raw", p.green, None),
            item(
                "constant.numeric, constant.language, constant.character, constant.other",
                p.yellow,
                None,
            ),
            item("constant.character.escape, string.regexp", p.cyan, None),
            item("keyword, storage, keyword.operator.word", p.magenta, None),
            item(
                "storage.type, entity.name.type, support.type, support.class, entity.other.inherited-class",
                p.cyan,
                None,
            ),
            item(
                "entity.name.function, support.function, meta.function-call, variable.function",
                p.accent,
                None,
            ),
            item("entity.name.tag, meta.tag", p.red, None),
            item(
                "entity.other.attribute-name, variable.parameter",
                p.yellow,
                None,
            ),
            item("invalid", p.red, None),
            item("markup.heading, entity.name.section", p.accent, bold),
            item("markup.bold", p.bright, bold),
            item("markup.italic", p.bright, italic),
            item(
                "markup.underline.link, string.other.link, markup.link",
                p.cyan,
                None,
            ),
            item("markup.list, punctuation.definition.list", p.magenta, None),
            item("markup.quote", p.dim, italic),
        ],
    }
}

/// Lays out `text` for the editor: highlighted by the grammar for `path` when there is one
/// and the file is small enough, with the find bar's matches (byte ranges) marked.
pub fn layout(
    text: &str,
    path: &Path,
    theme: &Theme,
    p: &Palette,
    font: &FontId,
    found: &[Range<usize>],
    current: Option<usize>,
) -> LayoutJob {
    let plain = |fg: Color32| TextFormat {
        font_id: font.clone(),
        color: fg,
        ..Default::default()
    };
    let mut job = LayoutJob::default();
    let ss = syntaxes();
    let syntax = (text.len() <= MAX_BYTES)
        .then(|| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_default();
            ss.find_syntax_by_token(&ext)
                .or_else(|| ss.find_syntax_by_extension(&name))
                .or_else(|| ss.find_syntax_by_first_line(text.lines().next()?))
        })
        .flatten();
    match syntax {
        Some(syntax) => {
            let mut lines = HighlightLines::new(syntax, theme);
            for line in LinesWithEndings::from(text) {
                match lines.highlight_line(line, ss) {
                    Ok(spans) => {
                        for (style, piece) in spans {
                            let c = style.foreground;
                            let mut format = plain(Color32::from_rgb(c.r, c.g, c.b));
                            format.italics = style.font_style.contains(FontStyle::ITALIC);
                            job.append(piece, 0.0, format);
                        }
                    }
                    Err(_) => job.append(line, 0.0, plain(p.fg)),
                }
            }
        }
        None => job.append(text, 0.0, plain(p.fg)),
    }
    mark(&mut job, found, current, p);
    job
}

/// Gives the find bar's matches a background, splitting sections where a match starts or ends.
fn mark(job: &mut LayoutJob, found: &[Range<usize>], current: Option<usize>, p: &Palette) {
    if found.is_empty() {
        return;
    }
    let mut cuts: Vec<usize> = found.iter().flat_map(|r| [r.start, r.end]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut sections = Vec::with_capacity(job.sections.len() + cuts.len());
    for section in job.sections.drain(..) {
        let (from, to) = (section.byte_range.start.0, section.byte_range.end.0);
        let mut start = from;
        for &cut in cuts.iter().filter(|&&c| c > from && c < to) {
            sections.push(egui_section(&section, start..cut));
            start = cut;
        }
        sections.push(egui_section(&section, start..to));
    }
    for section in &mut sections {
        if let Some(i) = found.iter().position(|r| {
            r.start <= section.byte_range.start.0 && section.byte_range.end.0 <= r.end
        }) {
            let strong = current == Some(i);
            section.format.background = if strong {
                p.accent
            } else {
                p.accent.gamma_multiply(0.35)
            };
            if strong {
                section.format.color = crate::theme::on(p.accent);
                section.format.underline = Stroke::NONE;
            }
        }
    }
    job.sections = sections;
}

fn egui_section(
    s: &eframe::egui::text::LayoutSection,
    range: Range<usize>,
) -> eframe::egui::text::LayoutSection {
    eframe::egui::text::LayoutSection {
        leading_space: 0.0,
        byte_range: ByteIndex(range.start)..ByteIndex(range.end),
        format: s.format.clone(),
    }
}

/// Byte ranges where `needle` occurs in `text`, ignoring case unless the needle has capitals
/// (the same "smart case" the folder search uses).
pub fn find_all(text: &str, needle: &str) -> Vec<Range<usize>> {
    if needle.is_empty() {
        return Vec::new();
    }
    if needle.chars().any(char::is_uppercase) {
        return text
            .match_indices(needle)
            .map(|(i, m)| i..i + m.len())
            .collect();
    }
    // Lowercasing can change byte lengths, so match char by char against the original text.
    let needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i + needle.len() <= chars.len() {
        let hit = needle
            .iter()
            .zip(&chars[i..])
            .all(|(n, (_, c))| c.to_lowercase().eq(std::iter::once(*n)));
        if hit {
            let end = chars.get(i + needle.len()).map_or(text.len(), |(b, _)| *b);
            found.push(chars[i].0..end);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_with_smart_case_and_unicode() {
        assert_eq!(
            find_all("Lisbon lisbon LISBON", "lisbon"),
            vec![0..6, 7..13, 14..20]
        );
        assert_eq!(find_all("Lisbon lisbon", "Lisbon"), vec![0..6]);
        assert_eq!(find_all("Café CAFÉ", "café"), vec![0..5, 6..11]);
        assert!(find_all("abc", "").is_empty());
    }

    #[test]
    fn highlights_rust_and_marks_matches() {
        let p = Palette::parse("");
        let font = FontId::monospace(14.0);
        let text = "fn main() { let x = 1; }\n";
        let job = layout(
            text,
            Path::new("a.rs"),
            &theme(&p),
            &p,
            &font,
            std::slice::from_ref(&(3..7)),
            Some(0),
        );
        // Highlighting produces several colored sections, and they still cover the whole text.
        assert!(job.sections.len() > 3);
        let covered: usize = job
            .sections
            .iter()
            .map(|s| s.byte_range.end.0 - s.byte_range.start.0)
            .sum();
        assert_eq!(covered, text.len());
        let main = job
            .sections
            .iter()
            .find(|s| s.byte_range.start.0 == 3 && s.byte_range.end.0 == 7)
            .expect("the match is its own section");
        assert_eq!(main.format.background, p.accent);
    }

    #[test]
    fn unknown_files_stay_plain() {
        let p = Palette::parse("");
        let job = layout(
            "just words",
            Path::new("notes"),
            &theme(&p),
            &p,
            &FontId::monospace(14.0),
            &[],
            None,
        );
        assert_eq!(job.sections.len(), 1);
    }
}
