//! the original: transcript, prompt box, powerline bar at the bottom.

use crate::theme::prelude::*;

pub struct Theme;

const SEP: &str = "\u{e0b0}";

fn seg(text: impl Into<String>, fg: Color, bg: Color) -> Span<'static> {
    Span::styled(format!(" {} ", text.into()), Style::default().fg(fg).bg(bg))
}

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "classic"
    }

    fn blurb(&self) -> &'static str {
        "transcript, prompt box and a powerline bar along the bottom"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(frame.area());

        frame.render_widget(parts::transcript_paragraph(app), chunks[0]);

        let width = usize::from(chunks[1].width.saturating_sub(2));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(parts::input(app).to_owned())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" prompt ")
                        .border_style(Style::default().fg(Color::DarkGray)),
                )
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX))),
            chunks[1],
        );

        let mut segments = vec![
            ("\u{f120}", parts::session(app), Color::Rgb(0xd8, 0xd8, 0xd8), Color::Rgb(0x2a, 0x2a, 0x2a)),
        ];
        if let Some(branch) = parts::branch(app) {
            segments.push(("\u{e725}", branch, Color::Rgb(0xd8, 0xd8, 0xf0), Color::Rgb(0x3b, 0x3b, 0x5c)));
        }
        if !parts::model(app).is_empty() {
            segments.push(("\u{f2db}", parts::model(app), Color::Rgb(0xe0, 0xe8, 0xff), Color::Rgb(0x2f, 0x45, 0x66)));
        }
        segments.push(("\u{f0e4}", parts::rate(app), if parts::rate_lit(app) { Color::Rgb(0xff, 0xff, 0xff) } else { Color::Rgb(0xff, 0xd8, 0xd8) }, if parts::rate_lit(app) { Color::Rgb(0xc0, 0x30, 0x30) } else { Color::Rgb(0x5c, 0x2f, 0x2f) }));
        segments.push(("\u{f0e7}", parts::tokens(app), Color::Rgb(0xd8, 0xd8, 0xd8), Color::Rgb(0x2a, 0x2a, 0x2a)));

        let mut spans: Vec<Span<'static>> = Vec::new();
        for (index, (icon, text, fg, bg)) in segments.iter().enumerate() {
            spans.push(seg(format!("{icon} {text}"), *fg, *bg));
            let next = segments.get(index + 1).map_or(Color::Reset, |s| s.3);
            spans.push(Span::styled(SEP, Style::default().fg(*bg).bg(next)));
        }
        let used = Line::from(spans.clone()).width();
        let hints = " esc interrupt · ctrl-c quit ";
        let pad = usize::from(chunks[2].width).saturating_sub(used + hints.chars().count());
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(hints, Style::default().fg(Color::DarkGray)));
        frame.render_widget(Paragraph::new(Line::from(spans)), chunks[2]);

        Some((chunks[1].x + 1 + u16::try_from(column).unwrap_or(0), chunks[1].y + 1))
    }
}
