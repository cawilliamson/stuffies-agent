//! three stacked numbered panels.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "panels"
    }

    fn blurb(&self) -> &'static str {
        "stacked numbered panels: transcript, prompt, metrics"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(7)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(3),
            ])
            .split(frame.area());

        let inner = parts::pane("1 transcript", parts::RULE).inner(rows[0]);
        frame.render_widget(parts::pane("1 transcript", parts::RULE), rows[0]);
        frame.render_widget(parts::transcript(app, inner), inner);
        let cursor = parts::composer(frame, rows[1], app);

        let bar = parts::status_bar(app, rows[2].width.saturating_sub(2));
        frame.render_widget(
            Paragraph::new(bar).block(parts::pane("3 metrics", parts::RULE)),
            rows[2],
        );
        cursor
    }
}
