//! transcript on the left, a wide inspector rail on the right.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "sidebar-right"
    }

    fn blurb(&self) -> &'static str {
        "transcript left, inspector rail right, composer under the transcript"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(6)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(20), Constraint::Length(30)])
            .split(frame.area());
        let left = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(cols[0]);

        frame.render_widget(Paragraph::new(parts::tab_bar(app, left[0].width)), left[0]);
        frame.render_widget(parts::transcript(app, left[1]), left[1]);
        let cursor = parts::composer(frame, left[2], app);
        frame.render_widget(Paragraph::new(parts::status_bar(app, left[3].width)), left[3]);

        let mut rail: Vec<Line<'static>> = Vec::new();
        for (key, value) in parts::facts(app) {
            rail.push(Line::from(vec![
                Span::styled(format!(" {key:<9}"), Style::default().fg(app::FAINT)),
                Span::styled(value, Style::default().fg(app::INK)),
            ]));
        }
        rail.push(Line::from(""));
        rail.push(Line::from(Span::styled(
            " KEYS",
            Style::default().fg(app::DIM).add_modifier(Modifier::BOLD),
        )));
        for (key, action) in [
            ("ctrl+t", "new tab"),
            ("ctrl+w", "close tab"),
            ("ctrl+1..9", "jump"),
            ("esc", "interrupt"),
            ("/theme", "reskin"),
        ] {
            rail.push(Line::from(vec![
                Span::styled(format!(" {key:<10}"), Style::default().fg(app::FAINT)),
                Span::styled(action.to_owned(), Style::default().fg(app::DIM)),
            ]));
        }
        frame.render_widget(
            Paragraph::new(rail).block(
                Block::default()
                    .borders(Borders::LEFT)
                    .title(" inspector ")
                    .border_style(Style::default().fg(parts::RULE)),
            ),
            cols[1],
        );
        cursor
    }
}
