//! shared pieces themes compose. no layout decisions here.

use ratatui::text::Line;

use crate::app::App;

pub use crate::app::{Entry, Role};

/// the structured transcript, for themes that style entries themselves.
#[must_use]
pub fn entries(app: &App) -> &[Entry] {
    &app.transcript
}

/// the transcript as renderable lines.
#[must_use]
pub fn transcript(app: &App) -> Vec<Line<'static>> {
    app.lines()
}

/// the transcript scroll offset.
#[must_use]
pub fn scroll(app: &App) -> u16 {
    app.scroll
}

/// the text currently in the input field.
#[must_use]
pub fn input(app: &App) -> &str {
    app.input_text()
}

/// the horizontal scroll and cursor column for an input of the given width.
#[must_use]
pub fn input_view(app: &App, width: usize) -> (usize, usize) {
    app.input_view(width)
}

/// the short session id.
#[must_use]
pub fn session(app: &App) -> String {
    app.status.session.chars().take(8).collect()
}

/// the workspace branch, if any.
#[must_use]
pub fn branch(app: &App) -> Option<String> {
    app.branch.clone()
}

/// the workspace directory, if known.
#[must_use]
pub fn workspace(app: &App) -> Option<String> {
    app.workspace.clone()
}

/// the model id, or empty when it is not known yet.
#[must_use]
pub fn model(app: &App) -> String {
    app.status.model.clone()
}

/// throughput as a short string.
#[must_use]
pub fn rate(app: &App) -> String {
    format!("{:.1} t/s", app.tok_per_sec)
}

/// whether the throughput figure is on the lit half of its flash.
#[must_use]
pub fn rate_lit(app: &App) -> bool {
    app.flash_highlight
}

/// prompt and completion tokens, abbreviated.
#[must_use]
pub fn tokens(app: &App) -> String {
    format!(
        "{}/{}",
        compact(app.status.prompt_tokens),
        compact(app.status.completion_tokens)
    )
}

/// whether a turn is in flight.
#[must_use]
pub fn busy(app: &App) -> bool {
    app.status.turn_running
}

/// abbreviates large counts.
#[must_use]
pub fn compact(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{}M", count / 1_000_000)
    } else if count >= 1_000 {
        format!("{}k", count / 1_000)
    } else {
        count.to_string()
    }
}

/// a paragraph of the transcript, scrolled to the app's offset.
#[must_use]
pub fn transcript_paragraph(app: &App) -> ratatui::widgets::Paragraph<'static> {
    ratatui::widgets::Paragraph::new(transcript(app))
        .wrap(ratatui::widgets::Wrap { trim: false })
        .scroll((scroll(app), 0))
}

/// the colours a theme uses for transcript roles.
pub struct Ink {
    pub user: ratatui::style::Color,
    pub assistant: ratatui::style::Color,
    pub tool: ratatui::style::Color,
    pub error: ratatui::style::Color,
    pub system: ratatui::style::Color,
    pub dim: ratatui::style::Color,
}

impl Default for Ink {
    fn default() -> Self {
        Self {
            user: ratatui::style::Color::Rgb(0x8a, 0xc8, 0xff),
            assistant: ratatui::style::Color::Reset,
            tool: ratatui::style::Color::Rgb(0x7a, 0x84, 0x94),
            error: ratatui::style::Color::Rgb(0xe8, 0x7c, 0x7c),
            system: ratatui::style::Color::Rgb(0x6c, 0x76, 0x86),
            dim: ratatui::style::Color::Rgb(0x5a, 0x62, 0x70),
        }
    }
}

/// renders the transcript as readable message blocks: no role words, no gutters.
///
/// the operator's turns get a prompt glyph, the agent's turns read as plain prose, and tool
/// activity is a compact dim line with its output beneath it.
#[must_use]
pub fn messages(app: &App, ink: &Ink, width: u16) -> Vec<Line<'static>> {
    use ratatui::style::Style;
    use ratatui::text::Span;

    let mut out: Vec<Line<'static>> = Vec::new();
    for entry in entries(app) {
        match entry.role {
            Role::User => {
                if !out.is_empty() {
                    out.push(Line::from(""));
                }
                for (index, raw) in entry.text.lines().enumerate() {
                    let head = if index == 0 {
                        Span::styled("\u{276f} ", Style::default().fg(ink.user))
                    } else {
                        Span::styled("  ", Style::default())
                    };
                    out.push(Line::from(vec![
                        head,
                        Span::styled(
                            raw.to_owned(),
                            Style::default()
                                .fg(ink.user)
                                .add_modifier(ratatui::style::Modifier::BOLD),
                        ),
                    ]));
                }
            }
            Role::Assistant => {
                if !out.is_empty() {
                    out.push(Line::from(""));
                }
                for raw in entry.text.lines() {
                    out.push(Line::from(Span::styled(
                        raw.to_owned(),
                        Style::default().fg(ink.assistant),
                    )));
                }
            }
            Role::Tool => {
                out.push(Line::from(vec![
                    Span::styled("\u{25be} ", Style::default().fg(ink.dim)),
                    Span::styled(entry.text.clone(), Style::default().fg(ink.tool)),
                ]));
            }
            Role::ToolOutput => {
                // output sits in a bordered well, like a shell result in a transcript.
                let inner = usize::from(width).saturating_sub(6).clamp(8, 120);
                out.push(Line::from(Span::styled(
                    format!("\u{256d}{}\u{256e}", "\u{2500}".repeat(inner + 2)),
                    Style::default().fg(ink.dim),
                )));
                for raw in entry.text.lines() {
                    let mut text = raw.to_owned();
                    if text.chars().count() > inner {
                        text = text
                            .chars()
                            .take(inner.saturating_sub(1))
                            .collect::<String>();
                        text.push('\u{2026}');
                    }
                    let pad = inner.saturating_sub(text.chars().count());
                    out.push(Line::from(vec![
                        Span::styled("\u{2502} ", Style::default().fg(ink.dim)),
                        Span::styled(text, Style::default().fg(ink.dim)),
                        Span::styled(
                            format!("{} \u{2502}", " ".repeat(pad)),
                            Style::default().fg(ink.dim),
                        ),
                    ]));
                }
                out.push(Line::from(Span::styled(
                    format!("\u{2570}{}\u{256f}", "\u{2500}".repeat(inner + 2)),
                    Style::default().fg(ink.dim),
                )));
            }
            Role::System => {
                out.push(Line::from(vec![
                    Span::styled("  \u{00b7} ", Style::default().fg(ink.dim)),
                    Span::styled(entry.text.clone(), Style::default().fg(ink.system)),
                ]));
            }
            Role::Error => {
                out.push(Line::from(vec![
                    Span::styled("  \u{2717} ", Style::default().fg(ink.error)),
                    Span::styled(entry.text.clone(), Style::default().fg(ink.error)),
                ]));
            }
        }
    }
    out
}
