//! green phosphor terminal with a double bezel.

use crate::theme::prelude::*;

pub struct Theme;

const GREEN: Color = Color::Rgb(0x33, 0xff, 0x66);

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "crt"
    }

    fn blurb(&self) -> &'static str {
        "green phosphor CRT with a double bezel and block prompt"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(6)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let bezel = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(GREEN))
            .title(Line::from(Span::styled(
                " SA/CRT ",
                Style::default().fg(Color::Black).bg(GREEN),
            )));
        let inner = bezel.inner(frame.area());
        frame.render_widget(bezel, frame.area());

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(2), Constraint::Length(1)])
            .split(inner);
        frame.render_widget(parts::transcript(app, rows[0]), rows[0]);

        let tab = app.tab();
        let field = usize::from(rows[1].width.saturating_sub(3));
        let (hscroll, column) = tab.input_view(field);
        frame.render_widget(
            Paragraph::new(format!("\u{2588} {}", tab.input_text()))
                .style(Style::default().fg(GREEN))
                .scroll((0, u16::try_from(hscroll).unwrap_or(u16::MAX))),
            rows[1],
        );
        Some((
            rows[1].x + 2 + u16::try_from(column).unwrap_or(0),
            rows[1].y,
        ))
    }
}
