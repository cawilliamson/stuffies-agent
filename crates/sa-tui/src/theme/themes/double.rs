//! double-ruled boxes: header strip, transcript frame, prompt frame.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "double"
    }

    fn blurb(&self) -> &'static str {
        "double-ruled frames around transcript and prompt with a header rule"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(9)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Length(3),
            ])
            .split(frame.area());

        frame.render_widget(
            Paragraph::new(parts::status_bar(app, rows[0].width.saturating_sub(2))).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Double)
                    .border_style(Style::default().fg(parts::RULE)),
            ),
            rows[0],
        );

        let border = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(parts::RULE))
            .title(" transcript ");
        let inner = border.inner(rows[1]);
        frame.render_widget(border, rows[1]);
        parts::transcript(frame, inner, app);

        let tab = app.tab();
        let field = usize::from(rows[2].width.saturating_sub(4));
        let (hscroll, column) = tab.input_view(field);
        frame.render_widget(
            Paragraph::new(tab.input_text().to_owned())
                .scroll((0, u16::try_from(hscroll).unwrap_or(u16::MAX)))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Double)
                        .border_style(Style::default().fg(parts::RULE))
                        .title(" prompt ")
                        .padding(Padding::new(1, 1, 0, 0)),
                ),
            rows[2],
        );
        Some((
            rows[2].x + 2 + u16::try_from(column).unwrap_or(0),
            rows[2].y + 1,
        ))
    }
}
