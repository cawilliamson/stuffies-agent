//! transcript with a docked live-tail strip above the composer.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "inspector"
    }

    fn blurb(&self) -> &'static str {
        "transcript plus a docked live-tail strip and composer bar"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(12)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(6),
                Constraint::Length(3),
            ])
            .split(frame.area());

        frame.render_widget(Paragraph::new(parts::tab_bar(app, rows[0].width)), rows[0]);
        let border = parts::pane("conversation", parts::RULE);
        let inner = border.inner(rows[1]);
        frame.render_widget(border, rows[1]);
        parts::transcript(frame, inner, app);

        let tail: Vec<Line<'static>> = app
            .tab()
            .transcript
            .iter()
            .rev()
            .take(4)
            .rev()
            .map(|entry| {
                Line::from(Span::styled(
                    entry.text.lines().next().unwrap_or_default().to_owned(),
                    Style::default().fg(app::DIM),
                ))
            })
            .collect();
        frame.render_widget(
            Paragraph::new(tail).block(parts::pane("live tail", parts::RULE)),
            rows[2],
        );
        parts::composer(frame, rows[3], app)
    }
}
