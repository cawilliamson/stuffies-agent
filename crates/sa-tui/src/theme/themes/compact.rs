//! maximum density: one-line metrics strip, borderless prompt.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "compact"
    }

    fn blurb(&self) -> &'static str {
        "maximum density: one-line metrics strip and a borderless prompt"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(2)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(2),
                Constraint::Length(1),
            ])
            .split(frame.area());

        frame.render_widget(Paragraph::new(parts::tab_bar(app, rows[0].width)), rows[0]);
        parts::transcript(frame, rows[1], app);

        let tab = app.tab();
        let field = usize::from(rows[2].width.saturating_sub(2));
        let (hscroll, column) = tab.input_view(field);
        frame.render_widget(
            Paragraph::new(format!(":{}", tab.input_text()))
                .scroll((0, u16::try_from(hscroll).unwrap_or(u16::MAX))),
            rows[2],
        );
        Some((
            rows[2].x + 1 + u16::try_from(column).unwrap_or(0),
            rows[2].y,
        ))
    }
}
