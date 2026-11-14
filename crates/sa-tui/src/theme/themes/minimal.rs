//! nothing but text: no borders, no bars, a bare prompt line at the bottom.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "minimal"
    }

    fn blurb(&self) -> &'static str {
        "no chrome at all: plain transcript and a bare prompt line"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(2), Constraint::Length(1)])
            .split(frame.area());

        frame.render_widget(parts::transcript_paragraph(app), rows[0]);

        let width = usize::from(rows[1].width.saturating_sub(2));
        let (scroll, column) = parts::input_view(app, width);
        let text = format!("> {}", parts::input(app));
        frame.render_widget(
            Paragraph::new(text).scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX))),
            rows[1],
        );
        Some((rows[1].x + 2 + u16::try_from(column).unwrap_or(0), rows[1].y))
    }
}
