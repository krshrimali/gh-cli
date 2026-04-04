//! Markdown → ratatui `Text` (pulldown-cmark 0.12 uses [`TagEnd`] for `Event::End`).

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span, Text};

const TEXT: Color = Color::Rgb(205, 214, 244);
const SUB: Color = Color::Rgb(166, 173, 200);
const ACCENT: Color = Color::Rgb(137, 180, 250);
const GREEN: Color = Color::Rgb(166, 227, 161);
const PEACH: Color = Color::Rgb(250, 179, 135);
const MAUVE: Color = Color::Rgb(203, 166, 247);
const CODE_BG: Color = Color::Rgb(40, 42, 58);

/// First image URL in markdown (`![](url)`).
pub fn first_image_url(markdown: &str) -> Option<String> {
    let opts = Options::all();
    let parser = Parser::new_ext(markdown, opts);
    for ev in parser {
        if let Event::Start(Tag::Image { dest_url, .. }) = ev {
            let u = dest_url.to_string();
            if !u.is_empty() {
                return Some(u);
            }
        }
    }
    None
}

pub fn markdown_to_text(md: &str, wrap_width: usize) -> Text<'static> {
    let opts = Options::all();
    let parser = Parser::new_ext(md, opts);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let code_style = Style::default().fg(SUB).bg(CODE_BG);
    let mut in_code = false;
    let mut code_buf = String::new();
    let mut in_table = false;
    let mut para = String::new();

    let flush_para = |para: &mut String, lines: &mut Vec<Line<'static>>| {
        let t = std::mem::take(para).trim().to_string();
        if t.is_empty() {
            return;
        }
        for wline in textwrap::wrap(&t, wrap_width) {
            lines.push(Line::from(vec![Span::styled(
                wline.to_string(),
                Style::default().fg(TEXT),
            )]));
        }
        lines.push(Line::default());
    };

    for ev in parser {
        match ev {
            Event::Start(Tag::CodeBlock(kind)) => {
                flush_para(&mut para, &mut lines);
                in_code = true;
                code_buf.clear();
                let label = match kind {
                    CodeBlockKind::Fenced(lang) => format!("┌── {lang} "),
                    CodeBlockKind::Indented => "┌── code ".to_string(),
                };
                lines.push(Line::from(vec![Span::styled(
                    label,
                    Style::default().fg(ACCENT),
                )]));
            }
            Event::End(TagEnd::CodeBlock) => {
                for l in code_buf.lines() {
                    lines.push(Line::from(vec![Span::styled(format!("│ {l}"), code_style)]));
                }
                if code_buf.trim().is_empty() && !code_buf.is_empty() {
                    lines.push(Line::from(vec![Span::styled("│ ", code_style)]));
                }
                lines.push(Line::from(vec![Span::styled(
                    "└────────",
                    Style::default().fg(ACCENT),
                )]));
                lines.push(Line::default());
                in_code = false;
                code_buf.clear();
            }
            Event::Start(Tag::Table(_)) => {
                flush_para(&mut para, &mut lines);
                in_table = true;
                lines.push(Line::from(vec![Span::styled(
                    "┌─ table ───────────────────",
                    Style::default().fg(PEACH).italic(),
                )]));
            }
            Event::End(TagEnd::Table) => {
                in_table = false;
                lines.push(Line::from(vec![Span::styled(
                    "└───────────────────────────",
                    Style::default().fg(PEACH).italic(),
                )]));
                lines.push(Line::default());
            }
            Event::Start(Tag::TableCell) => {}
            Event::End(TagEnd::TableCell) => {
                flush_para(&mut para, &mut lines);
            }
            Event::Start(Tag::Strong) | Event::End(TagEnd::Strong) => {}
            Event::Start(Tag::Emphasis) | Event::End(TagEnd::Emphasis) => {}
            Event::Start(Tag::Strikethrough) | Event::End(TagEnd::Strikethrough) => {}
            Event::Start(Tag::Link { dest_url, .. }) => {
                flush_para(&mut para, &mut lines);
                lines.push(Line::from(vec![
                    Span::styled("→ ", Style::default().fg(GREEN)),
                    Span::styled(
                        dest_url.to_string(),
                        Style::default().fg(ACCENT).underlined(),
                    ),
                ]));
            }
            Event::End(TagEnd::Link) => {
                lines.push(Line::default());
            }
            Event::Start(Tag::Image { dest_url, title, .. }) => {
                flush_para(&mut para, &mut lines);
                let tit = title.to_string();
                lines.push(Line::from(vec![Span::styled(
                    "🖼  image",
                    Style::default().fg(PEACH).bold(),
                )]));
                if !tit.is_empty() {
                    lines.push(Line::from(vec![Span::styled(
                        format!("   {tit}"),
                        Style::default().fg(SUB),
                    )]));
                }
                lines.push(Line::from(vec![Span::styled(
                    format!("   {}", dest_url),
                    Style::default().fg(ACCENT),
                )]));
                lines.push(Line::from(vec![Span::styled(
                    "   Kitty: press I — `kitten icat` URL",
                    Style::default().fg(SUB).italic(),
                )]));
                lines.push(Line::default());
            }
            Event::End(TagEnd::Image) => {}
            Event::Text(t) => {
                if in_code {
                    code_buf.push_str(&t);
                } else if in_table {
                    para.push_str(&t);
                    para.push_str(" │ ");
                } else {
                    para.push_str(&t);
                }
            }
            Event::Code(c) => {
                if in_code {
                    code_buf.push_str(&c);
                } else {
                    para.push('`');
                    para.push_str(&c);
                    para.push('`');
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if !in_code {
                    para.push('\n');
                } else {
                    code_buf.push('\n');
                }
            }
            Event::Rule => {
                flush_para(&mut para, &mut lines);
                lines.push(Line::from(vec![Span::styled(
                    "────────────────────────",
                    Style::default().fg(SUB),
                )]));
                lines.push(Line::default());
            }
            Event::Html(html) => {
                flush_para(&mut para, &mut lines);
                let s = html.to_string();
                for wline in textwrap::wrap(&s, wrap_width) {
                    lines.push(Line::from(vec![Span::styled(
                        wline.to_string(),
                        Style::default().fg(SUB),
                    )]));
                }
                lines.push(Line::default());
            }
            Event::InlineHtml(h) => {
                para.push_str(&h);
            }
            Event::FootnoteReference(_) | Event::TaskListMarker(_) => {}
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                flush_para(&mut para, &mut lines);
            }
            Event::Start(Tag::Heading { level, .. }) => {
                flush_para(&mut para, &mut lines);
                let n = heading_level_usize(level);
                lines.push(Line::from(vec![Span::styled(
                    format!("{} ", "█".repeat(n)),
                    Style::default().fg(MAUVE).bold(),
                )]));
            }
            Event::End(TagEnd::Heading(level)) => {
                let _ = level;
                flush_para(&mut para, &mut lines);
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush_para(&mut para, &mut lines);
                lines.push(Line::from(vec![Span::styled(
                    "│ ",
                    Style::default().fg(MAUVE),
                )]));
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush_para(&mut para, &mut lines);
            }
            Event::Start(Tag::Item) => {
                para.push_str("• ");
            }
            Event::End(TagEnd::Item) => {
                flush_para(&mut para, &mut lines);
            }
            Event::Start(Tag::List(_)) | Event::End(TagEnd::List(_)) => {}
            Event::Start(Tag::TableHead) | Event::End(TagEnd::TableHead) => {}
            Event::Start(Tag::TableRow) => {}
            Event::End(TagEnd::TableRow) => {
                flush_para(&mut para, &mut lines);
                lines.push(Line::from(vec![Span::styled(
                    "  ─────────────────",
                    Style::default().fg(SUB),
                )]));
            }
            Event::Start(Tag::FootnoteDefinition(_)) | Event::End(TagEnd::FootnoteDefinition) => {}
            Event::Start(Tag::HtmlBlock) | Event::End(TagEnd::HtmlBlock) => {}
            Event::Start(Tag::DefinitionList) | Event::End(TagEnd::DefinitionList) => {}
            Event::Start(Tag::DefinitionListTitle) | Event::End(TagEnd::DefinitionListTitle) => {}
            Event::Start(Tag::DefinitionListDefinition)
            | Event::End(TagEnd::DefinitionListDefinition) => {}
            Event::Start(Tag::MetadataBlock(_)) | Event::End(TagEnd::MetadataBlock(_)) => {}
            Event::InlineMath(_) | Event::DisplayMath(_) => {}
        }
    }
    flush_para(&mut para, &mut lines);
    while lines.last() == Some(&Line::default()) {
        lines.pop();
    }
    Text::from(lines)
}

fn heading_level_usize(level: HeadingLevel) -> usize {
    (level as u8) as usize
}
