//! everything at the top: status strip, tabs, composer, then the transcript.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "topbar"
    }

    fn blurb(&self) -> &'static str {
        "status strip and composer above the transcript"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(6)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Min(3),
            ])
            .split(frame.area());

        frame.render_widget(Paragraph::new(parts::status_bar(app, rows[0].width)), rows[0]);
        frame.render_widget(Paragraph::new(parts::tab_bar(app, rows[1].width)), rows[1]);
        let cursor = parts::composer(frame, rows[2], app);
        frame.render_widget(parts::transcript(app, rows[3]), rows[3]);
        cursor
    }
}
