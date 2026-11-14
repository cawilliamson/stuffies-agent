//! a large footer dashboard of boxed cells under a transcript pane.

use crate::theme::prelude::*;

pub struct Theme;

fn cell(title: &str, value: String, accent: Color) -> Paragraph<'static> {
    Paragraph::new(Line::from(Span::styled(value, Style::default().fg(accent))))
        .block(Block::default().borders(Borders::ALL).title(format!(" {title} ")))
}

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "footer-grid"
    }

    fn blurb(&self) -> &'static str {
        "dashboard footer of boxed metrics cells, transcript above"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(4),
                Constraint::Length(3),
            ])
            .split(frame.area());

        frame.render_widget(
            Paragraph::new(parts::transcript(app))
                .wrap(Wrap { trim: false })
                .scroll((parts::scroll(app), 0)),
            rows[0],
        );

        let grid = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Ratio(1, 4),
                Constraint::Ratio(1, 4),
                Constraint::Ratio(1, 4),
                Constraint::Ratio(1, 4),
            ])
            .split(rows[1]);
        frame.render_widget(cell("session", parts::session(app), Color::Cyan), grid[0]);
        frame.render_widget(cell("model", parts::model(app), Color::Blue), grid[1]);
        frame.render_widget(cell("rate", parts::rate(app), Color::Red), grid[2]);
        frame.render_widget(cell("tokens", parts::tokens(app), Color::White), grid[3]);

        let width = usize::from(rows[2].width.saturating_sub(2));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(parts::input(app).to_owned())
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX)))
                .block(Block::default().borders(Borders::ALL).title(" prompt ")),
            rows[2],
        );
        Some((rows[2].x + 1 + u16::try_from(column).unwrap_or(0), rows[2].y + 1))
    }
}
