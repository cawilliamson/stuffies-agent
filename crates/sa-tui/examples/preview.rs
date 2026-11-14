//! renders a theme to stdout so layouts can be compared without a terminal.
//!
//! an example is a program: it is allowed to write to the terminal.
#![allow(clippy::print_stdout, clippy::print_stderr)]

//!
//!   cargo run -p sa-tui --example preview -- studio

use ratatui::backend::TestBackend;
use ratatui::Terminal;
use sa_tui::app::{App, Role};

fn sample(theme: usize) -> App {
    let mut app = App::new(
        "9f3c1a7e-2b44-4d19-8a5e-77c0d1e2f3a4",
        "deepseek-v4-1-flash",
    );
    app.theme = theme;
    app.branch = Some("feat/theme-engine".to_owned());
    app.status.prompt_tokens = 41_200;
    app.status.completion_tokens = 3_180;
    app.tok_per_sec = 48.6;
    app.push(
        Role::User,
        "the login test is flaky on CI, can you find out why?",
    );
    app.push(
        Role::Assistant,
        "It looks like a timing assumption in the fixture. Let me check the setup.",
    );
    app.push(Role::Tool, "bash cargo test login -- --nocapture");
    app.push(
        Role::ToolOutput,
        "running 3 tests\ntest login::rejects_bad_password ... FAILED\n  left: 200, right: 401\n",
    );
    app.push(
        Role::Assistant,
        "The fixture sleeps 50ms before asserting; on CI the handler has not\nfinished. Raising it to 500ms fixes the race.",
    );
    app.push(Role::User, "do that then, and run the suite");
    app.set_input("and check the audit log while you are there");
    app
}

fn main() {
    let wanted = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "studio".to_owned());
    let index = sa_tui::theme::index_of(&wanted).unwrap_or_else(|| {
        eprintln!("unknown theme {wanted}; available:");
        for theme in sa_tui::theme::THEMES {
            eprintln!("  {:<14} {}", theme.name(), theme.blurb());
        }
        std::process::exit(1);
    });

    let mut app = sample(index);
    let (width, height) = (100u16, 28u16);
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");

    let buffer = terminal.backend().buffer();
    println!("\u{250c}{}\u{2510}", "\u{2500}".repeat(usize::from(width)));
    for y in 0..height {
        let row: String = (0..width)
            .map(|x| buffer[(x, y)].symbol().to_owned())
            .collect();
        println!("\u{2502}{row}\u{2502}");
    }
    println!("\u{2514}{}\u{2518}", "\u{2500}".repeat(usize::from(width)));
}
