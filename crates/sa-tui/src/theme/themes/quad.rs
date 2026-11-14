//! four quadrants: transcript, prompt, facts and keys each walled off.

use crate::theme::prelude::*;

pub struct Theme;

fn walled(title: &'static str, color: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .border_style(Style::default().fg(color))
}

impl crate::theme::Theme for Theme {
    fn name(&self) -> &'static str {
        "quad"
    }

    fn blurb(&self) -> &'static str {
        "four walled quadrants: transcript, prompt, facts, keys"
    }

    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)> {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(frame.area());
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(rows[0]);
        let bottom = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(rows[1]);

        frame.render_widget(
            Paragraph::new(parts::transcript(app))
                .wrap(Wrap { trim: false })
                .scroll((parts::scroll(app), 0))
                .block(walled("transcript", Color::Rgb(0x44, 0x66, 0x88))),
            top[0],
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(parts::session(app)),
                Line::from(parts::model(app)),
                Line::from(parts::rate(app)),
                Line::from(parts::tokens(app)),
            ])
            .block(walled("facts", Color::Rgb(0x88, 0x66, 0x44))),
            top[1],
        );

        let width = usize::from(bottom[0].width.saturating_sub(2));
        let (scroll, column) = parts::input_view(app, width);
        frame.render_widget(
            Paragraph::new(parts::input(app).to_owned())
                .scroll((0, u16::try_from(scroll).unwrap_or(u16::MAX)))
                .block(walled("prompt", Color::Rgb(0x44, 0x88, 0x66))),
            bottom[0],
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::from("esc interrupt"),
                Line::from("ctrl-c quit"),
                Line::from("/theme list"),
            ])
            .block(walled("keys", Color::Rgb(0x66, 0x44, 0x88))),
            bottom[1],
        );
        Some((bottom[0].x + 1 + u16::try_from(column).unwrap_or(0), bottom[0].y + 1))
    }
}
