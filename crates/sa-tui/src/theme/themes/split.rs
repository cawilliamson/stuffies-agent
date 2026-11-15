//! vertical split: transcript left, composer and facts right.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "split"
    }

    fn blurb(&self) -> &'static str {
        "side-by-side: transcript left, composer and facts right"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(4)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(frame.area());
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Min(4),
            ])
            .split(cols[1]);

        frame.render_widget(
            parts::transcript(app, cols[0]).block(parts::pane("transcript", parts::RULE)),
            cols[0],
        );
        frame.render_widget(Paragraph::new(parts::tab_bar(app, right[0].width)), right[0]);
        let cursor = parts::composer(frame, right[1], app);

        let facts: Vec<Line<'static>> = parts::facts(app)
            .into_iter()
            .map(|(k, v)| {
                Line::from(vec![
                    Span::styled(format!("{k:<9}"), Style::default().fg(app::FAINT)),
                    Span::styled(v, Style::default().fg(app::INK)),
                ])
            })
            .collect();
        frame.render_widget(
            Paragraph::new(facts).block(parts::pane("facts", parts::RULE)),
            right[2],
        );
        cursor
    }
}
