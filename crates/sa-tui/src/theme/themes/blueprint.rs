//! drafting table: white lines on blue, coordinate ticks, dashed frames.

use crate::theme::prelude::*;

pub struct Theme;

const INK: Color = Color::Rgb(0xdc, 0xe8, 0xff);
const PAPER: Color = Color::Rgb(0x10, 0x28, 0x50);

fn ticks(width: u16) -> String {
    let mut out = String::new();
    for x in 0..width {
        out.push(if x % 10 == 0 { '+' } else { '-' });
    }
    out
}

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "blueprint"
    }

    fn blurb(&self) -> &'static str {
        "drafting-table blueprint: dashed frames and coordinate ticks"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        frame.render_widget(
            Block::default().style(Style::default().bg(PAPER)),
            frame.area(),
        );

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(frame.area());

        frame.render_widget(
            Paragraph::new(ticks(rows[0].width)).style(Style::default().fg(INK).bg(PAPER)),
            rows[0],
        );
        frame.render_widget(
            Paragraph::new(parts::transcript(app))
                .wrap(Wrap { trim: false })
                .scroll((parts::scroll(app), 0))
                .style(Style::default().fg(INK).bg(PAPER))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(INK))
                        .title(" sheet 1 "),
                ),
            rows[1],
        );

        let width = usize::from(rows[2].width.saturating_sub(2));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(parts::input(app).to_owned())
                .style(Style::default().fg(INK).bg(PAPER))
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX)))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(INK))
                        .title(" spec "),
                ),
            rows[2],
        );
        frame.render_widget(
            Paragraph::new(format!(
                "rev {} · {} · {} · {}",
                parts::session(app),
                parts::model(app),
                parts::rate(app),
                parts::tokens(app)
            ))
            .style(Style::default().fg(INK).bg(PAPER)),
            rows[3],
        );
        Some((rows[2].x + 1 + u16::try_from(column).unwrap_or(0), rows[2].y + 1))
    }
}
