//! the tui state machine and its rendering (spec 14, v1.1).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use sa_tui::app::{Action, App, Role};
use serde_json::json;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

#[test]
fn typing_and_enter_submits_the_prompt() {
    let mut app = App::new("s", "m");
    typed(&mut app, "hello");
    assert_eq!(app.input_text(), "hello");
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::Submit("hello".to_owned()));
    assert!(
        app.input_text().is_empty(),
        "input must be cleared on submit"
    );
}

#[test]
fn enter_on_an_empty_input_does_nothing() {
    let mut app = App::new("s", "m");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
}

#[test]
fn backspace_edits_the_input() {
    let mut app = App::new("s", "m");
    typed(&mut app, "abc");
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(app.input_text(), "ab");
}

#[test]
fn escape_interrupts_the_turn_and_never_leaves() {
    let mut app = App::new("s", "m");
    app.status.turn_running = true;
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Cancel);
    app.status.turn_running = false;
    assert_eq!(
        app.on_key(key(KeyCode::Esc)),
        Action::Cancel,
        "escape is an interrupt, not an exit, even when idle"
    );
}

#[test]
fn ctrl_c_always_quits() {
    let mut app = App::new("s", "m");
    app.status.turn_running = true;
    assert_eq!(app.on_key(ctrl('c')), Action::Quit);

    let mut idle = App::new("s", "m");
    assert_eq!(idle.on_key(ctrl('c')), Action::Quit);
}

#[test]
fn slash_commands_are_not_sent_as_prompts() {
    for command in ["/exit", "/quit"] {
        let mut app = App::new("s", "m");
        typed(&mut app, command);
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::Command(command.to_owned()),
            "{command}"
        );
    }
}

#[test]
fn a_normal_prompt_is_still_a_prompt() {
    let mut app = App::new("s", "m");
    typed(&mut app, "read main.rs");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit("read main.rs".to_owned())
    );
}

#[test]
fn ctrl_d_quits() {
    let mut app = App::new("s", "m");
    assert_eq!(app.on_key(ctrl('d')), Action::Quit);
}

#[test]
fn scroll_clamps_to_the_rendered_transcript() {
    let mut app = App::new("s", "m");
    for n in 0..20 {
        app.push(Role::Tool, format!("line {n}"));
    }
    // the ceiling is derived from rendered lines and the viewport, so it needs a draw.
    let _ = screen_of(&mut app, 60, 12);
    let ceiling = app.max_scroll;
    assert!(ceiling > 0, "a long transcript must be scrollable");

    for _ in 0..500 {
        app.on_key(key(KeyCode::Up));
    }
    assert_eq!(app.scroll, 0, "scrolling up must stop at the top");

    for _ in 0..500 {
        app.on_key(key(KeyCode::Down));
    }
    assert_eq!(
        app.scroll, ceiling,
        "scrolling down must stop at the newest content"
    );
    assert!(app.follow, "reaching the bottom resumes following");
}

#[test]
fn tool_events_render_as_transcript_entries() {
    let mut app = App::new("s", "m");
    app.on_event(
        "tool.requested",
        &json!({"tool": "read", "args": {"path": "main.rs"}}),
    );
    app.on_event("tool.completed", &json!({"output": "fn main() {}"}));

    let text: String = app
        .lines()
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("→ read main.rs"), "{text}");
    assert!(text.contains("fn main() {}"), "{text}");
}

#[test]
fn streamed_text_accumulates_into_one_assistant_entry() {
    let mut app = App::new("s", "m");
    app.on_event("llm.delta", &json!({"text": "hel"}));
    app.on_event("llm.delta", &json!({"text": "lo"}));
    let assistant: Vec<&str> = app
        .transcript
        .iter()
        .filter(|e| e.role == Role::Assistant)
        .map(|e| e.text.as_str())
        .collect();
    assert_eq!(assistant, vec!["hello"]);
}

#[test]
fn usage_accumulates_across_turns() {
    let mut app = App::new("s", "m");
    app.status.turn_running = true;
    app.on_event(
        "turn.completed",
        &json!({"usage": {"prompt_tokens": 10, "completion_tokens": 5}}),
    );
    assert_eq!(app.status.prompt_tokens, 10);
    assert!(!app.status.turn_running);
}

#[test]
fn the_input_line_renders() {
    let mut app = App::new("session-abcdef", "deepseek");
    app.push(Role::User, "what is this repo");
    typed(&mut app, "next prompt");

    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");

    let buffer = terminal.backend().buffer();
    let screen: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
    assert!(screen.contains("what is this repo"), "{screen}");
    assert!(screen.contains("next prompt"), "{screen}");
    assert!(
        screen.contains("deepseek"),
        "status bar must show the model"
    );
}

fn user_entries(app: &App) -> usize {
    app.transcript
        .iter()
        .filter(|e| e.role == Role::User)
        .count()
}

#[test]
fn submitting_does_not_render_the_prompt_twice() {
    // the prompt is echoed locally when submitted, and the daemon also emits turn.started.
    // only the local echo may render, or the operator sees their message duplicated.
    let mut app = App::new("s", "m");
    app.push(Role::User, "hello");
    app.on_event("turn.started", &json!({"prompt": "hello"}));
    assert_eq!(user_entries(&app), 1, "{:?}", app.transcript);
}

#[test]
fn replay_still_reconstructs_prompts_from_the_log() {
    let mut app = App::new("s", "m");
    app.replaying = true;
    app.on_event("turn.started", &json!({"prompt": "first"}));
    app.on_event("turn.started", &json!({"prompt": "second"}));
    app.replaying = false;
    assert_eq!(user_entries(&app), 2);
}

#[test]
fn every_role_renders_a_distinct_gutter() {
    let mut app = App::new("s", "m");
    app.push(Role::User, "mine");
    app.push(Role::Assistant, "theirs");
    app.push(Role::Tool, "tool output");

    let screen: String = app
        .lines()
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(screen.contains("you │ mine"), "{screen}");
    assert!(screen.contains("sa  │ theirs"), "{screen}");
    assert!(screen.contains("tool│ tool output"), "{screen}");
}

#[test]
fn the_agent_and_the_operator_use_different_colours() {
    use ratatui::style::Color;
    let mut app = App::new("s", "m");
    app.push(Role::User, "mine");
    app.push(Role::Assistant, "theirs");
    let colours: Vec<Color> = app
        .lines()
        .iter()
        .filter_map(|line| line.spans.first().and_then(|s| s.style.fg))
        .collect();
    assert!(colours.contains(&Color::Cyan), "{colours:?}");
    assert!(colours.contains(&Color::Green), "{colours:?}");
}

fn screen_of(app: &mut App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, app).expect("draw");
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[test]
fn the_newest_entry_is_visible_after_a_long_transcript() {
    // scroll is a rendered-line offset. counting entries instead leaves the viewport far
    // above the newest content, so the operator cannot see what they just typed.
    let mut app = App::new("s", "m");
    for n in 0..40 {
        app.push(Role::Tool, format!("tool output line {n}"));
    }
    app.push(Role::User, "my newest message");

    let screen = screen_of(&mut app, 60, 12);
    assert!(
        screen.contains("my newest message"),
        "the newest entry must be on screen:\n{screen}"
    );
    assert!(
        !screen.contains("tool output line 0"),
        "the oldest entry must have scrolled away:\n{screen}"
    );
}

#[test]
fn scrolling_up_holds_the_view_against_new_output() {
    let mut app = App::new("s", "m");
    for n in 0..20 {
        app.push(Role::Tool, format!("line {n}"));
    }
    // establish a ceiling by drawing once
    let _ = screen_of(&mut app, 60, 12);
    app.on_key(key(KeyCode::PageUp));
    assert!(!app.follow, "manual scrolling must stop following");
    let held = app.scroll;

    app.push(Role::Assistant, "streamed while scrolled up");
    assert_eq!(app.scroll, held, "new output must not yank the viewport");
}

#[test]
fn a_slash_command_never_enters_the_transcript() {
    let mut app = App::new("s", "m");
    typed(&mut app, "/help");
    let action = app.on_key(key(KeyCode::Enter));
    assert!(matches!(action, Action::Command(_)), "{action:?}");
    assert_eq!(
        user_entries(&app),
        0,
        "a command is not chat: {:?}",
        app.transcript
    );
}

#[test]
fn streaming_produces_a_live_token_rate() {
    let mut app = App::new("s", "m");
    app.status.turn_running = true;
    app.on_event("llm.delta", &json!({"text": "x".repeat(400)}));
    app.tick();
    assert!(
        app.tok_per_sec > 0.0,
        "a delta must produce a rate, got {}",
        app.tok_per_sec
    );
}

#[test]
fn replayed_history_does_not_contribute_to_the_rate() {
    let mut app = App::new("s", "m");
    app.replaying = true;
    app.on_event("llm.delta", &json!({"text": "x".repeat(4000)}));
    app.tick();
    assert_eq!(app.tok_per_sec, 0.0, "history is not a live rate");
}

/// drives one delivered message: `generation` of streaming, then a long tool run.
fn delivered_message(app: &mut App, t0: std::time::Instant, generation: std::time::Duration) {
    let done = t0 + generation;
    let after_tools = done + std::time::Duration::from_secs(28);
    app.status.turn_running = true;
    app.on_event_at("turn.started", &json!({"prompt": "go"}), t0);
    app.on_event_at("llm.delta", &json!({"text": "x".repeat(400)}), t0);
    app.on_event_at("llm.completed", &json!({}), done);
    app.on_event_at("tool.requested", &json!({"tool": "bash"}), done);
    app.on_event_at("tool.completed", &json!({"output": "slow"}), after_tools);
    app.on_event_at(
        "turn.completed",
        &json!({"usage": {"prompt_tokens": 1, "completion_tokens": 100}}),
        after_tools,
    );
}

#[test]
fn the_average_counts_only_generation_time_not_tool_time() {
    use std::time::{Duration, Instant};

    let mut app = App::new("s", "m");
    let t0 = Instant::now();
    // two seconds of generation, then twenty-eight seconds of tool execution.
    delivered_message(&mut app, t0, Duration::from_secs(2));

    app.tick_at(t0 + Duration::from_secs(30));
    // 400 chars / 4 = 100 tokens over 2s of generation = 50 tok/s.
    // counting wall time would give 100/30 = 3.3.
    assert!(
        (app.tok_per_sec - 50.0).abs() < 1.0,
        "expected ~50 tok/s from generation time alone, got {}",
        app.tok_per_sec
    );
}

#[test]
fn thinking_tokens_count_toward_the_rate() {
    use std::time::{Duration, Instant};

    let mut app = App::new("s", "m");
    let t0 = Instant::now();
    app.status.turn_running = true;
    app.on_event_at("turn.started", &json!({"prompt": "go"}), t0);
    // reasoning only: no visible content at all.
    app.on_event_at("llm.reasoning", &json!({"text": "y".repeat(400)}), t0);
    app.on_event_at("llm.completed", &json!({}), t0 + Duration::from_secs(2));
    app.on_event_at(
        "turn.completed",
        &json!({"usage": {"prompt_tokens": 0, "completion_tokens": 100}}),
        t0 + Duration::from_secs(2),
    );

    app.tick_at(t0 + Duration::from_secs(2));
    assert!(
        (app.tok_per_sec - 50.0).abs() < 1.0,
        "thinking must count toward throughput, got {}",
        app.tok_per_sec
    );
}

#[test]
fn a_delivered_message_holds_its_average_while_the_highlight_flashes() {
    use std::time::{Duration, Instant};

    let mut app = App::new("s", "m");
    let t0 = Instant::now();
    delivered_message(&mut app, t0, Duration::from_secs(2));
    let settled = t0 + Duration::from_secs(30);

    let mut flashes = 0;
    let mut was_lit = false;
    for step in 0..20u32 {
        app.tick_at(settled + Duration::from_millis(250) * step);
        if step < 9 {
            assert!(
                app.tok_per_sec > 0.0,
                "the figure must stay visible during the flash (step {step})"
            );
        }
        if app.flash_highlight && !was_lit {
            flashes += 1;
        }
        was_lit = app.flash_highlight;
    }
    assert_eq!(
        flashes, 3,
        "expected three highlight flashes, got {flashes}"
    );

    app.tick_at(settled + Duration::from_secs(10));
    assert!(
        app.tok_per_sec > 0.0,
        "the figure holds after the flash, got {}",
        app.tok_per_sec
    );
    assert!(!app.flash_highlight, "but the highlight is off");

    // and it resets only when the next message starts.
    app.on_event_at(
        "turn.started",
        &json!({"prompt": "next"}),
        settled + Duration::from_secs(11),
    );
    app.tick_at(settled + Duration::from_secs(11));
    assert_eq!(app.tok_per_sec, 0.0, "a new message resets the figure");
}

#[test]
fn the_flash_is_a_red_background_behind_the_figure() {
    use ratatui::style::Color;
    use std::time::{Duration, Instant};

    let mut app = App::new("session-abcdef", "deepseek");
    let t0 = Instant::now();
    delivered_message(&mut app, t0, Duration::from_secs(2));
    let settled = t0 + Duration::from_secs(30);
    app.tick_at(settled);
    assert!(app.flash_highlight);

    let backend = TestBackend::new(120, 20);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();

    let lit: Vec<&ratatui::buffer::Cell> = buffer
        .content()
        .iter()
        .filter(|cell| {
            cell.bg == Color::Rgb(0xc0, 0x30, 0x30) && cell.fg == Color::Rgb(0xff, 0xff, 0xff)
        })
        .collect();
    assert!(!lit.is_empty(), "the figure must be highlighted");
    let text: String = lit.iter().map(|cell| cell.symbol()).collect();
    assert!(
        text.contains("tok/s"),
        "the highlighted run is the rate: {text}"
    );
}

#[test]
fn the_status_bar_shows_a_number_and_never_a_dash() {
    let mut app = App::new("session-abcdef", "deepseek");
    let screen = screen_of(&mut app, 100, 20);
    assert!(
        screen.contains("0.0 tok/s"),
        "idle must read 0.0:\n{screen}"
    );
    assert!(!screen.contains('—'), "no dash placeholder:\n{screen}");
}

#[test]
fn the_rate_appears_in_the_status_bar_while_streaming() {
    let mut app = App::new("session-abcdef", "deepseek");
    app.status.turn_running = true;
    app.on_event("llm.delta", &json!({"text": "x".repeat(400)}));
    app.tick();

    let screen = screen_of(&mut app, 100, 20);
    assert!(
        screen.contains("tok/s"),
        "status bar must show the rate:\n{screen}"
    );
}

#[test]
fn a_line_feed_from_a_pipe_submits_like_enter() {
    let mut app = App::new("s", "m");
    typed(&mut app, "hello");
    assert_eq!(
        app.on_key(key(KeyCode::Char('\n'))),
        Action::Submit("hello".to_owned())
    );
}

#[test]
fn the_status_bar_is_split_into_labelled_segments() {
    let mut app = App::new("session-abcdef", "deepseek");
    let screen = screen_of(&mut app, 120, 20);

    for expected in ["session-", "deepseek", "0.0 tok/s", "0 in / 0 out"] {
        assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
    }
    assert!(
        screen.matches('\u{e0b0}').count() >= 4,
        "segments must be joined by powerline separators:\n{screen}"
    );
}

#[test]
fn the_bar_is_solid_coloured_areas_not_bracketed_text() {
    use ratatui::style::Color;

    let mut app = App::new("session-abcdef", "deepseek");
    app.branch = Some("main".to_owned());

    let backend = TestBackend::new(140, 20);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();

    // each area carries its own background, so several distinct non-default fills appear.
    let mut fills: Vec<Color> = buffer
        .content()
        .iter()
        .map(|cell| cell.bg)
        .filter(|bg| *bg != Color::Reset)
        .collect();
    fills.sort_by_key(|c| format!("{c:?}"));
    fills.dedup();
    assert!(
        fills.len() >= 4,
        "expected several distinct segment fills, got {fills:?}"
    );

    let screen: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
    assert!(
        screen.contains("main"),
        "the branch must be shown:\n{screen}"
    );
    assert!(!screen.contains('['), "no bracketed text:\n{screen}");
}

#[test]
fn the_bar_has_no_idle_or_running_state_block() {
    for running in [false, true] {
        let mut app = App::new("session-abcdef", "deepseek");
        app.status.turn_running = running;
        let screen = screen_of(&mut app, 140, 20);
        assert!(!screen.contains("idle"), "{screen}");
        assert!(!screen.contains("running"), "{screen}");
    }
}

#[test]
fn the_session_area_names_the_session_instead_of_an_opaque_id() {
    let mut app = App::new("session-abcdef", "deepseek");
    app.title = "stuffies-agent".to_owned();
    let screen = screen_of(&mut app, 140, 20);
    assert!(screen.contains("stuffies-agent"), "{screen}");
    assert!(
        !screen.contains("session-"),
        "the raw id is not shown:\n{screen}"
    );
}

#[test]
fn a_long_session_title_is_truncated() {
    let mut app = App::new("session-abcdef", "deepseek");
    app.title = "a-very-long-session-title-that-would-eat-the-bar".to_owned();
    let screen = screen_of(&mut app, 140, 20);
    assert!(screen.contains("a-very-long-ses"), "{screen}");
    assert!(!screen.contains("that-would-eat"), "{screen}");
}

#[test]
fn large_token_counts_are_abbreviated() {
    let mut app = App::new("session-abcdef", "deepseek");
    app.status.prompt_tokens = 230_000;
    app.status.completion_tokens = 1_500;

    let screen = screen_of(&mut app, 140, 20);
    assert!(screen.contains("230k in"), "expected 230k:\n{screen}");
    assert!(screen.contains("1k out"), "expected 1k:\n{screen}");
    assert!(
        !screen.contains("230000"),
        "the raw count must not appear:\n{screen}"
    );
}

#[test]
fn small_token_counts_are_shown_in_full() {
    let mut app = App::new("session-abcdef", "deepseek");
    app.status.prompt_tokens = 999;
    let screen = screen_of(&mut app, 140, 20);
    assert!(screen.contains("999 in"), "{screen}");
}

#[test]
fn the_hints_sit_at_the_right_edge_of_the_bar() {
    let mut app = App::new("session-abcdef", "deepseek");
    let width = 140u16;
    let backend = TestBackend::new(width, 20);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();
    let cells = buffer.content();

    let row = 19usize;
    let start = row * usize::from(width);
    let line: String = cells[start..start + usize::from(width)]
        .iter()
        .map(|c| c.symbol())
        .collect();

    let column = line.find("esc interrupt").expect("hints must render");
    assert!(
        column > usize::from(width) / 2,
        "hints must be pushed to the right edge, found at column {column}:\n{line}"
    );
}
