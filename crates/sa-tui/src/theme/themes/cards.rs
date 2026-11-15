//! each exchange rendered as a card, with a card composer.

use crate::theme::prelude::*;

pub struct Theme;

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "cards"
    }

    fn blurb(&self) -> &'static str {
        "transcript split into cards with a card composer"
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
                Constraint::Length(4),
            ])
            .split(frame.area());

        frame.render_widget(Paragraph::new(parts::tab_bar(app, rows[0].width)), rows[0]);
        let border = parts::pane("conversation", Color::Rgb(0x55, 0x55, 0x66));
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
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(app::ACCENT))
                        .title(" compose ")
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
