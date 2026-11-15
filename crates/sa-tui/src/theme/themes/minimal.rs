//! no chrome: transcript and a bare prompt line.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "minimal"
    }

    fn blurb(&self) -> &'static str {
        "no chrome at all: plain transcript and a bare prompt"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(1)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(2), Constraint::Length(1)])
            .split(frame.area());
        frame.render_widget(parts::transcript(app, rows[0]), rows[0]);

        let tab = app.tab();
        let field = usize::from(rows[1].width.saturating_sub(2));
        let (hscroll, column) = tab.input_view(field);
        frame.render_widget(
            Paragraph::new(tab.input_text().to_owned())
                .scroll((0, u16::try_from(hscroll).unwrap_or(u16::MAX))),
            rows[1],
        );
        Some((rows[1].x + u16::try_from(column).unwrap_or(0), rows[1].y))
    }
}
