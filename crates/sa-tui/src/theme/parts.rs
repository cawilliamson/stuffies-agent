//! shared pieces themes compose. no layout decisions here.

use ratatui::text::Line;

use crate::app::App;

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
