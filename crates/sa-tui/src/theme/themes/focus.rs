//! distraction-free: one centred column, no chrome, a calm composer.

use crate::theme::prelude::*;

pub struct Theme;

const MUTED: Color = Color::Rgb(0x6c, 0x76, 0x86);
const RULE: Color = Color::Rgb(0x2e, 0x33, 0x3c);
const ACCENT: Color = Color::Rgb(0x8a, 0xc8, 0xff);

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "focus"
    }

    fn blurb(&self) -> &'static str {
        "distraction-free centred column with a calm composer"
    }

    fn transcript_rows(&self, area: Rect) -> u16 {
        area.height.saturating_sub(8)
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let area = frame.area();
        let width = area.width.min(88);
        let column = Rect {
            x: area.x + (area.width - width) / 2,
            y: area.y,
            width,
            height: area.height,
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(column);

        frame.render_widget(
            Paragraph::new(Span::styled(
                if parts::busy(app) { "working" } else { "" },
                Style::default().fg(MUTED),
            ))
            .alignment(Alignment::Center),
            rows[0],
        );

        frame.render_widget(
            Paragraph::new(parts::messages(app, &parts::Ink::default(), rows[1].width))
                .wrap(Wrap { trim: false })
                .scroll((parts::scroll(app), 0)),
            rows[1],
        );

        let width = usize::from(rows[2].width.saturating_sub(4));
        let (scroll, cursor) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("\u{276f} ", Style::default().fg(ACCENT)),
                Span::raw(parts::input(app).to_owned()),
            ]))
            .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX)))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(RULE))
                    .padding(ratatui::widgets::Padding::new(1, 1, 0, 0)),
            ),
            rows[2],
        );

        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(
                    "{}  \u{00b7}  {}  \u{00b7}  {}",
                    parts::session(app),
                    parts::rate(app),
                    parts::tokens(app)
                ),
                Style::default().fg(RULE),
            ))
            .alignment(Alignment::Center),
            rows[3],
        );

        Some((
            rows[2].x + 3 + u16::try_from(cursor).unwrap_or(0),
            rows[2].y + 1,
        ))
    }
}
