//! cyberpunk magenta and cyan with a glowing frame.

use crate::theme::prelude::*;

pub struct Theme;

const MAGENTA: Color = Color::Rgb(0xff, 0x2d, 0x95);
const CYAN: Color = Color::Rgb(0x00, 0xff, 0xe1);

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "neon"
    }

    fn blurb(&self) -> &'static str {
        "cyberpunk magenta and cyan with a glowing frame"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(7)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)> {
        let border = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(MAGENTA).add_modifier(Modifier::BOLD))
            .title(Line::from(vec![
                Span::styled("\u{25c8} ", Style::default().fg(CYAN)),
                Span::styled(
                    app.tab().session.chars().take(8).collect::<String>(),
                    Style::default().fg(MAGENTA),
                ),
                Span::styled(" \u{25c8} ", Style::default().fg(CYAN)),
            ]));
        let inner = border.inner(frame.area());
        frame.render_widget(border, frame.area());

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(2),
                Constraint::Length(1),
            ])
            .split(inner);
        frame.render_widget(
            Paragraph::new(parts::status_bar(app, rows[0].width)),
            rows[0],
        );
        frame.render_widget(parts::transcript(app, rows[1]), rows[1]);

        let tab = app.tab();
        let field = usize::from(rows[2].width.saturating_sub(3));
        let (hscroll, column) = tab.input_view(field);
        frame.render_widget(
            Paragraph::new(format!("\u{203a}\u{203a} {}", tab.input_text()))
                .style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD))
                .scroll((0, u16::try_from(hscroll).unwrap_or(u16::MAX))),
            rows[2],
        );
        Some((
            rows[2].x + 3 + u16::try_from(column).unwrap_or(0),
            rows[2].y,
        ))
    }
}
