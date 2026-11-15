//! left rail of session facts, transcript and composer on the right.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "sidebar-left"
    }

    fn blurb(&self) -> &'static str {
        "left rail of session facts, transcript and composer on the right"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(5)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(26), Constraint::Min(20)])
            .split(frame.area());
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(3),
            ])
            .split(cols[1]);

        let mut rail: Vec<Line<'static>> = Vec::new();
        rail.push(Line::from(Span::styled(
            " FACTS",
            Style::default().fg(app::DIM).add_modifier(Modifier::BOLD),
        )));
        for (key, value) in parts::facts(app) {
            rail.push(Line::from(vec![
                Span::styled(format!(" {key:<8}"), Style::default().fg(app::FAINT)),
                Span::styled(value, Style::default().fg(app::INK)),
            ]));
        }
        frame.render_widget(
            Paragraph::new(rail).block(
                Block::default()
                    .borders(Borders::RIGHT)
                    .border_style(Style::default().fg(parts::RULE)),
            ),
            cols[0],
        );

        frame.render_widget(Paragraph::new(parts::tab_bar(app, right[0].width)), right[0]);
        frame.render_widget(parts::transcript(app, right[1]), right[1]);
        
        parts::composer(frame, right[2], app)
    }
}
