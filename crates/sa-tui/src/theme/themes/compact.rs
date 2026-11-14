//! dense single-line chrome: metrics strip on top, bare input at the bottom.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "compact"
    }

    fn blurb(&self) -> &'static str {
        "maximum density: one-line metrics strip, borderless prompt"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(2),
                Constraint::Length(1),
            ])
            .split(frame.area());

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(parts::session(app), Style::default().fg(Color::Cyan)),
                Span::raw(" "),
                Span::styled(parts::model(app), Style::default().fg(Color::Blue)),
                Span::raw(" "),
                Span::styled(parts::rate(app), Style::default().fg(Color::Red)),
                Span::raw(" "),
                Span::styled(parts::tokens(app), Style::default().fg(Color::DarkGray)),
                Span::raw(" "),
                Span::styled(
                    if parts::busy(app) { "..." } else { "" },
                    Style::default().fg(Color::Yellow),
                ),
            ])),
            rows[0],
        );
        frame.render_widget(parts::transcript_paragraph(app), rows[1]);

        let width = usize::from(rows[2].width.saturating_sub(2));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(format!(":{}", parts::input(app)))
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX))),
            rows[2],
        );
        Some((rows[2].x + 1 + u16::try_from(column).unwrap_or(0), rows[2].y))
    }
}
