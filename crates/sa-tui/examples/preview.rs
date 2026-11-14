//! renders a theme in the terminal, with real colours, so layouts can be judged.
//!
//! an example is a program: it is allowed to write to the terminal.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use ratatui::backend::TestBackend;
use ratatui::buffer::Cell;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;
use sa_tui::app::{App, Role};

fn sample(theme: usize) -> App {
    let mut app = App::new(
        "9f3c1a7e-2b44-4d19-8a5e-77c0d1e2f3a4",
        "deepseek-v4-1-flash",
    );
    app.theme = theme;
    app.branch = Some("feat/theme-engine".to_owned());
    app.workspace = Some("/home/chrisaw/Projects/stuffies-agent".to_owned());
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

/// an ansi escape for one colour channel.
fn ansi(color: Color, foreground: bool) -> String {
    let base = if foreground { 38 } else { 48 };
    match color {
        Color::Reset => (if foreground { 39 } else { 49 }).to_string(),
        Color::Black => format!("{};5;0", base),
        Color::Red => format!("{};5;1", base),
        Color::Green => format!("{};5;2", base),
        Color::Yellow => format!("{};5;3", base),
        Color::Blue => format!("{};5;4", base),
        Color::Magenta => format!("{};5;5", base),
        Color::Cyan => format!("{};5;6", base),
        Color::Gray => format!("{};5;7", base),
        Color::DarkGray => format!("{};5;8", base),
        Color::LightRed => format!("{};5;9", base),
        Color::LightGreen => format!("{};5;10", base),
        Color::LightYellow => format!("{};5;11", base),
        Color::LightBlue => format!("{};5;12", base),
        Color::LightMagenta => format!("{};5;13", base),
        Color::LightCyan => format!("{};5;14", base),
        Color::White => format!("{};5;15", base),
        Color::Indexed(index) => format!("{};5;{index}", base),
        Color::Rgb(r, g, b) => format!("{};2;{r};{g};{b}", base),
    }
}

fn modifier_code(modifier: Modifier) -> String {
    let mut codes = Vec::new();
    for (flag, code) in [
        (Modifier::BOLD, 1),
        (Modifier::DIM, 2),
        (Modifier::ITALIC, 3),
        (Modifier::UNDERLINED, 4),
        (Modifier::REVERSED, 7),
    ] {
        if modifier.contains(flag) {
            codes.push(code.to_string());
        }
    }
    codes.join(";")
}

/// emits the style only when it changes, so the output stays small and fast.
fn paint(cell: &Cell, out: &mut String, last: &mut String) {
    let mut codes = vec![ansi(cell.fg, true), ansi(cell.bg, false)];
    let modifiers = modifier_code(cell.modifier);
    if !modifiers.is_empty() {
        codes.push(modifiers);
    }
    let style = codes.join(";");
    if style != *last {
        out.push_str(&format!("\u{1b}[{style}m"));
        last.clone_from(&style);
    }
    out.push_str(cell.symbol());
}

fn show(name: &str) {
    let Some(index) = sa_tui::theme::index_of(name) else {
        eprintln!("unknown theme {name}; available:");
        for theme in sa_tui::theme::THEMES {
            eprintln!("  {:<14} {}", theme.name(), theme.blurb());
        }
        std::process::exit(1);
    };

    let mut app = sample(index);
    let (width, height) = (100u16, 28u16);
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");

    println!(
        "\u{1b}[2m  {} \u{2014} {}\u{1b}[0m",
        name,
        sa_tui::theme::at(index).blurb()
    );
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    let mut last = String::new();
    for y in 0..height {
        for x in 0..width {
            paint(&buffer[(x, y)], &mut out, &mut last);
        }
        out.push_str("\u{1b}[0m\n");
        last.clear();
    }
    print!("{out}");
    println!();
}

fn main() {
    let wanted = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "studio".to_owned());
    if wanted == "all" {
        for name in ["studio", "workspace", "focus"] {
            show(name);
        }
    } else {
        show(&wanted);
    }
}
