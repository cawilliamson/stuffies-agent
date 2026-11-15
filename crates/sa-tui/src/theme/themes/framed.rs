//! one heavy outer frame containing tabs, transcript and composer.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "framed"
    }

    fn blurb(&self) -> &'static str {
        "one thick outer frame around the whole workspace"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(8)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let outer = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Thick)
            .border_style(Style::default().fg(Color::Rgb(0x6a, 0x74, 0x88)))
            .title(Line::from(vec![
                Span::styled(
                    " sa ",
                    Style::default().fg(Color::Black).bg(app::ACCENT),
                ),
                Span::styled(
                    format!(" {} ", app.tab().session.chars().take(8).collect::<String>()),
                    Style::default().fg(app::DIM),
                ),
            ]));
        let inner = outer.inner(frame.area());
        frame.render_widget(outer, frame.area());

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(inner);
        frame.render_widget(Paragraph::new(parts::tab_bar(app, rows[0].width)), rows[0]);
        parts::transcript(frame, rows[1], app);
        let cursor = parts::composer(frame, rows[2], app);
        frame.render_widget(Paragraph::new(parts::status_bar(app, rows[3].width)), rows[3]);
        cursor
    }
}
