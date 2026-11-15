//! vertical tab rail on the left instead of a horizontal strip.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "stack"
    }

    fn blurb(&self) -> &'static str {
        "vertical tab rail down the left edge, transcript and composer right"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(5)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(24), Constraint::Min(20)])
            .split(frame.area());
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(3)])
            .split(cols[1]);

        let mut rail: Vec<Line<'static>> = vec![Line::from(Span::styled(
            " TABS",
            Style::default().fg(app::DIM).add_modifier(Modifier::BOLD),
        ))];
        for (index, tab) in app.tabs.iter().enumerate() {
            let active = index == app.active;
            rail.push(Line::from(Span::styled(
                format!(
                    " {} {}",
                    index + 1,
                    tab.title.chars().take(16).collect::<String>()
                ),
                if active {
                    Style::default()
                        .fg(Color::Rgb(0x10, 0x14, 0x1a))
                        .bg(app::ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(app::DIM)
                },
            )));
        }
        frame.render_widget(
            Paragraph::new(rail).block(
                Block::default()
                    .borders(Borders::RIGHT)
                    .border_style(Style::default().fg(parts::RULE)),
            ),
            cols[0],
        );

        frame.render_widget(parts::transcript(app, right[0]), right[0]);
        parts::composer(frame, right[1], app)
    }
}
