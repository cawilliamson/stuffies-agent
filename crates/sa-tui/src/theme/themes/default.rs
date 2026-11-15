//! the shipped layout: tabs, transcript pane, composer, status bar.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "default"
    }

    fn blurb(&self) -> &'static str {
        "tabs on top, transcript pane, composer and powerline status bar"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(6)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(frame.area());

        frame.render_widget(Paragraph::new(parts::tab_bar(app, rows[0].width)), rows[0]);

        let border = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(parts::RULE));
        let inner = border.inner(rows[1]);
        frame.render_widget(border, rows[1]);
        frame.render_widget(parts::transcript(app, inner), inner);

        let cursor = parts::composer(frame, rows[2], app);
        frame.render_widget(
            Paragraph::new(parts::status_bar(app, rows[3].width)),
            rows[3],
        );
        cursor
    }
}
