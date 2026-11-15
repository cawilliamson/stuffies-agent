//! narrow centred document column with wide margins.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "centred"
    }

    fn blurb(&self) -> &'static str {
        "narrow centred document column with wide margins"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(5)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let area = frame.area();
        let width = area.width.min(76);
        let column = Rect {
            x: area.x + (area.width - width) / 2,
            y: area.y,
            width,
            height: area.height,
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(column);

        frame.render_widget(
            Paragraph::new(parts::tab_bar(app, rows[0].width)).alignment(Alignment::Center),
            rows[0],
        );
        frame.render_widget(parts::transcript(app, rows[1]), rows[1]);
        let cursor = parts::composer(frame, rows[2], app);
        frame.render_widget(
            Paragraph::new(parts::status_bar(app, rows[3].width)),
            rows[3],
        );
        cursor
    }
}
