// A Markdown preview for the text editor: pulldown-cmark parses, and each block becomes an egui
// label laid out from the palette. Code blocks are highlighted like the editor. It covers what
// notes use (headings, emphasis, lists, tasks, quotes, code, tables, rules); links are styled
// but not clickable, and images show their description.

use std::path::Path;

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{self, Color32, FontFamily, FontId, Margin, Stroke, Ui};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::theme::Palette;

/// Inline style in effect while walking the text.
#[derive(Default, Clone, Copy)]
struct Style {
    strong: bool,
    emphasis: bool,
    strike: bool,
    link: bool,
    quote: bool,
    heading: Option<HeadingLevel>,
}

struct Renderer<'a> {
    p: &'a Palette,
    theme: &'a syntect::highlighting::Theme,
    size: f32,
    style: Style,
    job: LayoutJob,
    /// Open lists: the next number for ordered ones, None for bullets.
    lists: Vec<Option<u64>>,
    code: Option<(String, String)>,
    table: Option<Vec<Vec<LayoutJob>>>,
}

impl Renderer<'_> {
    fn format(&self, code: bool) -> TextFormat {
        let s = self.style;
        let size = match s.heading {
            Some(HeadingLevel::H1) => self.size * 1.8,
            Some(HeadingLevel::H2) => self.size * 1.5,
            Some(HeadingLevel::H3) => self.size * 1.25,
            Some(_) => self.size * 1.1,
            None => self.size,
        };
        let family = if code {
            FontFamily::Monospace
        } else {
            FontFamily::Proportional
        };
        let color = if s.link {
            self.p.cyan
        } else if s.heading.is_some() {
            self.p.accent
        } else if s.strong {
            self.p.bright
        } else if s.quote {
            self.p.dim
        } else {
            self.p.fg
        };
        TextFormat {
            font_id: FontId::new(size, family),
            color,
            italics: s.emphasis || s.quote,
            background: if code {
                self.p.surface
            } else {
                Color32::TRANSPARENT
            },
            underline: if s.link {
                Stroke::new(1.0, self.p.cyan)
            } else {
                Stroke::NONE
            },
            strikethrough: if s.strike {
                Stroke::new(1.0, color)
            } else {
                Stroke::NONE
            },
            ..Default::default()
        }
    }

    fn text(&mut self, text: &str, code: bool) {
        let format = self.format(code);
        self.job.append(text, 0.0, format);
    }

    /// Ends the current block: shows what was gathered, indented for lists and quotes.
    fn flush(&mut self, ui: &mut Ui) {
        if self.job.text.is_empty() {
            return;
        }
        let mut job = std::mem::take(&mut self.job);
        job.wrap.max_width = ui.available_width();
        if let Some(rows) = &mut self.table {
            if let Some(row) = rows.last_mut() {
                row.push(job);
            }
            return;
        }
        let indent = self.lists.len().saturating_sub(1) as f32 * 18.0
            + if self.style.quote { 14.0 } else { 0.0 };
        ui.horizontal_wrapped(|ui| {
            ui.add_space(indent);
            ui.label(job);
        });
    }

    fn start(&mut self, tag: Tag, ui: &mut Ui) {
        match tag {
            Tag::Heading { level, .. } => {
                self.flush(ui);
                ui.add_space(self.size * 0.6);
                self.style.heading = Some(level);
            }
            Tag::Paragraph => self.flush(ui),
            Tag::BlockQuote(_) => {
                self.flush(ui);
                self.style.quote = true;
            }
            Tag::CodeBlock(kind) => {
                self.flush(ui);
                let lang = match kind {
                    CodeBlockKind::Fenced(lang) => {
                        lang.split([' ', ',']).next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::List(start) => {
                self.flush(ui);
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush(ui);
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}. ", *n - 1)
                    }
                    _ => "• ".into(),
                };
                let mut format = self.format(false);
                format.color = self.p.magenta;
                self.job.append(&marker, 0.0, format);
            }
            Tag::Emphasis => self.style.emphasis = true,
            Tag::Strong => self.style.strong = true,
            Tag::Strikethrough => self.style.strike = true,
            Tag::Link { .. } => self.style.link = true,
            Tag::Image { .. } => {
                self.style.link = true;
                self.text("\u{f03e} ", false);
            }
            Tag::Table(_) => {
                self.flush(ui);
                self.table = Some(Vec::new());
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(rows) = &mut self.table {
                    rows.push(Vec::new());
                }
                self.style.strong = matches!(tag, Tag::TableHead);
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd, ui: &mut Ui) {
        match tag {
            TagEnd::Heading(_) => {
                self.flush(ui);
                self.style.heading = None;
                ui.add_space(self.size * 0.2);
            }
            TagEnd::Paragraph => {
                self.flush(ui);
                if self.lists.is_empty() {
                    ui.add_space(self.size * 0.5);
                }
            }
            TagEnd::BlockQuote(_) => {
                self.flush(ui);
                self.style.quote = false;
            }
            TagEnd::CodeBlock => {
                if let Some((lang, code)) = self.code.take() {
                    let font = FontId::monospace(self.size * 0.95);
                    let path = Path::new("code").with_extension(&lang);
                    let mut job = crate::highlight::layout(
                        code.trim_end(),
                        &path,
                        self.theme,
                        self.p,
                        &font,
                        &[],
                        None,
                    );
                    job.wrap.max_width = f32::INFINITY;
                    egui::Frame::new()
                        .fill(self.p.surface)
                        .inner_margin(Margin::same(8))
                        .corner_radius(4)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            egui::ScrollArea::horizontal()
                                .id_salt(ui.next_auto_id())
                                .show(ui, |ui| ui.label(job));
                        });
                    ui.add_space(self.size * 0.5);
                }
            }
            TagEnd::List(_) => {
                self.flush(ui);
                self.lists.pop();
                if self.lists.is_empty() {
                    ui.add_space(self.size * 0.5);
                }
            }
            TagEnd::Item => self.flush(ui),
            TagEnd::Emphasis => self.style.emphasis = false,
            TagEnd::Strong => self.style.strong = false,
            TagEnd::Strikethrough => self.style.strike = false,
            TagEnd::Link | TagEnd::Image => self.style.link = false,
            TagEnd::TableCell => {
                self.flush(ui);
                if let Some(row) = self.table.as_mut().and_then(|r| r.last_mut())
                    && self.job.text.is_empty()
                    && row.is_empty()
                {
                    // An empty first cell still takes its column.
                    row.push(LayoutJob::default());
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => self.style.strong = false,
            TagEnd::Table => {
                if let Some(rows) = self.table.take() {
                    egui::Grid::new(ui.next_auto_id())
                        .spacing([18.0, 6.0])
                        .show(ui, |ui| {
                            for row in rows {
                                for cell in row {
                                    ui.label(cell);
                                }
                                ui.end_row();
                            }
                        });
                    ui.add_space(self.size * 0.5);
                }
            }
            _ => {}
        }
    }
}

/// Shows `text` as rendered Markdown at base font size `size`.
pub fn show(ui: &mut Ui, text: &str, p: &Palette, theme: &syntect::highlighting::Theme, size: f32) {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut r = Renderer {
        p,
        theme,
        size,
        style: Style::default(),
        job: LayoutJob::default(),
        lists: Vec::new(),
        code: None,
        table: None,
    };
    for event in Parser::new_ext(text, options) {
        match event {
            Event::Start(tag) => r.start(tag, ui),
            Event::End(tag) => r.end(tag, ui),
            Event::Text(t) => match &mut r.code {
                Some((_, code)) => code.push_str(&t),
                None => r.text(&t, false),
            },
            Event::Code(t) => r.text(&t, true),
            Event::SoftBreak => r.text(" ", false),
            Event::HardBreak => r.text("\n", false),
            Event::Rule => {
                r.flush(ui);
                ui.separator();
            }
            Event::TaskListMarker(done) => {
                let mut format = r.format(false);
                format.color = if done { p.green } else { p.dim };
                r.job
                    .append(if done { "\u{f14a} " } else { "\u{f096} " }, 0.0, format);
            }
            Event::Html(t) | Event::InlineHtml(t) => r.text(&t, true),
            _ => {}
        }
    }
    r.flush(ui);
}

/// Whether a file is Markdown, so the editor offers the preview.
pub fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "md" | "markdown" | "mdown" | "mkd"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_every_kind_of_block_without_panicking() {
        let p = Palette::parse("");
        let theme = crate::highlight::theme(&p);
        let text = "# Title\n\nSome *em* and **strong** and `code` and [a link](x) ~~old~~.\n\n\
                    - one\n- two\n  1. nested\n\n- [ ] todo\n- [x] done\n\n> quoted\n\n\
                    ```rust\nfn main() {}\n```\n\n| a | b |\n|---|---|\n|  | 2 |\n\n---\n\n![pic](p.png)\n";
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            show(ui, text, &p, &theme, 14.0)
        });
        out.textures_delta.clear();
        assert!(is_markdown(Path::new("README.MD")) && !is_markdown(Path::new("a.txt")));
    }
}
