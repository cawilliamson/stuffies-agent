//! digital rain: dim glyph gutter beside the transcript.

use crate::theme::prelude::*;

pub struct Theme;

const BRIGHT: Color = Color::Rgb(0x8f, 0xff, 0x8f);
const DIM: Color = Color::Rgb(0x1f, 0x5f, 0x2f);

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "matrix"
    }

    fn blurb(&self) -> &'static str {
        "digital rain: dim glyph gutter and a block prompt"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(6)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let border = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(DIM))
            .title(Line::from(Span::styled(
                " \u{30c6}\u{30fc}\u{30de} ",
                Style::default().fg(BRIGHT),
            )));
        let inner = border.inner(frame.area());
        frame.render_widget(border, frame.area());

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(2), Constraint::Length(1)])
            .split(inner);
        parts::transcript(frame, rows[0], app);

        let tab = app.tab();
        let field = usize::from(rows[1].width.saturating_sub(3));
        let (hscroll, column) = tab.input_view(field);
        frame.render_widget(
            Paragraph::new(format!("\u{25b6} {}", tab.input_text()))
                .style(Style::default().fg(BRIGHT))
                .scroll((0, u16::try_from(hscroll).unwrap_or(u16::MAX))),
            rows[1],
        );
        Some((
            rows[1].x + 2 + u16::try_from(column).unwrap_or(0),
            rows[1].y,
        ))
    }
}
