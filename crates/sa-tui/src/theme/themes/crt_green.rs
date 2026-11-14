//! green phosphor terminal: heavy border, dim scanlines, blocky prompt.

use crate::theme::prelude::*;

pub struct Theme;

const GREEN: Color = Color::Rgb(0x33, 0xff, 0x66);

fn scanline(line: Line<'static>, index: usize) -> Line<'static> {
    if index % 2 == 1 {
        let spans: Vec<Span<'static>> = line
            .spans
            .into_iter()
            .map(|s| Span::styled(s.content, s.style.fg(Color::Rgb(0x1a, 0x8a, 0x38))))
            .collect();
        Line::from(spans)
    } else {
        line
    }
}

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "crt-green"
    }

    fn blurb(&self) -> &'static str {
        "green phosphor CRT with scanlines and a heavy bezel"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        frame.render_widget(Clear, frame.area());
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
            .constraints([Constraint::Min(2), Constraint::Length(1), Constraint::Length(1)])
            .split(inner);

        let body: Vec<Line<'static>> = parts::transcript(app)
            .into_iter()
            .enumerate()
            .map(|(i, l)| scanline(l, i))
            .collect();
        frame.render_widget(
            Paragraph::new(body)
                .wrap(Wrap { trim: false })
                .scroll((parts::scroll(app), 0))
                .style(Style::default().fg(GREEN)),
            rows[0],
        );

        let width = usize::from(rows[1].width.saturating_sub(3));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(format!("\u{2588} {}", parts::input(app)))
                .style(Style::default().fg(GREEN))
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX))),
            rows[1],
        );
        frame.render_widget(
            Paragraph::new(format!(
                "{}  {}  {}",
                parts::session(app),
                parts::rate(app),
                parts::tokens(app)
            ))
            .style(Style::default().fg(Color::Rgb(0x1a, 0x8a, 0x38))),
            rows[2],
        );
        Some((rows[1].x + 2 + u16::try_from(column).unwrap_or(0), rows[1].y))
    }
}
