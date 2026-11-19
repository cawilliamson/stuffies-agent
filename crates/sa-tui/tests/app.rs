//! the tabbed tui: key handling, tab lifecycle, transcript framing and rate accounting.
#![allow(clippy::expect_used, clippy::panic)]

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, ModifierKeyCode};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use sa_tui::app::{transcript_lines, Action, App, PickerItem, PickerKind, Role, ServedModel};
use serde_json::json;

const fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

const fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn ctrl_shift(c: char) -> KeyEvent {
    KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    )
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

/// the screen as rows, so a layout that collapsed into one line is visible.
fn rows(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, app).expect("draw");
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

fn screen(app: &mut App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, app).expect("draw");
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect()
}

#[test]
fn typing_and_enter_submits_the_prompt() {
    let mut app = App::new("session-abcdef", "m");
    typed(&mut app, "hello");
    assert_eq!(app.tab().input_text(), "hello");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit("hello".to_owned())
    );
    assert_eq!(app.tab().input_text(), "");
}

#[test]
fn backspace_edits_the_input() {
    let mut app = App::new("s", "m");
    typed(&mut app, "abc");
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(app.tab().input_text(), "ab");
}

#[test]
fn escape_interrupts_and_ctrl_c_quits() {
    let mut app = App::new("s", "m");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Cancel);
    assert_eq!(app.on_key(ctrl('c')), Action::Quit);
}

#[test]
fn shift_tab_requests_the_next_tab() {
    let mut app = App::new("s", "m");
    assert_eq!(
        app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
        Action::NextTab
    );
}

#[test]
fn ctrl_t_opens_and_ctrl_w_closes() {
    let mut app = App::new("s", "m");
    assert_eq!(app.on_key(ctrl('t')), Action::NewTab);
    assert_eq!(app.on_key(ctrl('w')), Action::CloseTab);
    // the shifted forms stay accepted as aliases.
    assert_eq!(app.on_key(ctrl_shift('T')), Action::NewTab);
}

#[test]
fn ctrl_digits_jump_to_a_tab() {
    let mut app = App::new("first", "m");
    app.add_tab("second", "m");
    app.add_tab("third", "m");

    // on_key reports the intent; the caller applies it.
    assert_eq!(app.on_key(ctrl('1')), Action::SwitchTab(0));
    app.switch_tab(0);
    assert_eq!(app.active, 0);

    assert_eq!(app.on_key(ctrl('3')), Action::SwitchTab(2));
    app.switch_tab(2);
    assert_eq!(app.active, 2);

    // out of range presses are ignored.
    app.switch_tab(8);
    assert_eq!(app.active, 2);
}

#[test]
fn replacing_the_last_tab_keeps_the_strip_alive() {
    let mut app = App::new("only", "m");
    app.replace_active("fresh", "m");
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.tab().session, "fresh");
    assert!(app.tab().transcript.is_empty());
}

#[test]
fn tabs_can_be_added_cycled_and_closed() {
    let mut app = App::new("first", "m");
    app.add_tab("second", "m");
    app.add_tab("third", "m");
    assert_eq!(app.tabs.len(), 3);
    assert_eq!(app.active, 2);

    app.next_tab();
    assert_eq!(app.active, 0, "cycling wraps");
    app.next_tab();
    assert_eq!(app.active, 1);

    let closed = app.close_tab().expect("closes");
    assert_eq!(closed, "second");
    assert_eq!(app.tabs.len(), 2);
}

#[test]
fn the_last_tab_is_never_closed() {
    let mut app = App::new("only", "m");
    assert!(app.close_tab().is_none());
    assert_eq!(app.tabs.len(), 1);
}

#[test]
fn each_tab_keeps_its_own_transcript_and_input() {
    let mut app = App::new("first", "m");
    typed(&mut app, "one");
    app.add_tab("second", "m");
    typed(&mut app, "two");

    assert_eq!(app.tabs[0].input_text(), "one");
    assert_eq!(app.tabs[1].input_text(), "two");

    app.tab_mut().push(Role::User, "hello from second");
    assert_eq!(app.tabs[0].transcript.len(), 0);
    assert_eq!(app.tabs[1].transcript.len(), 1);
}

#[test]
fn events_route_to_the_tab_that_owns_the_session() {
    let mut app = App::new("first", "m");
    app.add_tab("second", "m");

    let tab = app.tab_for("first").expect("first tab");
    tab.on_event("llm.delta", &json!({"text": "hello"}));
    assert_eq!(app.tabs[0].transcript.len(), 1);
    assert_eq!(app.tabs[1].transcript.len(), 0);
}

#[test]
fn the_tab_bar_is_one_row_of_lit_and_dim_tabs() {
    let mut app = App::new("session-one", "m");
    app.add_tab("session-two", "m");

    let backend = TestBackend::new(100, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();

    let row0: String = (0..100).map(|x| buffer[(x, 0)].symbol()).collect();
    assert!(
        row0.contains("1 (session"),
        "the tab must carry its id:\n{row0}"
    );
    assert!(row0.contains("2 (session"), "second tab label:\n{row0}");
    assert!(row0.contains('\u{2502}'), "tabs need a divider:\n{row0}");

    // the active tab is a lit chip; the inactive one is not filled.
    let lit = buffer.content().iter().any(|c| c.bg == sa_tui::app::ACCENT);
    assert!(lit, "the active tab must be highlighted");

    // one row only: the pane border starts on row 1.
    let row1: String = (0..100).map(|x| buffer[(x, 1)].symbol()).collect();
    assert!(
        row1.contains('\u{256d}'),
        "the pane must start immediately below:\n{row1}"
    );
}

#[test]
fn every_transcript_entry_is_framed_with_its_role() {
    let mut app = App::new("s", "deepseek-v4-flash");
    for (role, text) in [
        (Role::User, "a question"),
        (Role::Assistant, "an answer"),
        (Role::Thinking, "reasoning here"),
        (Role::Tool, "bash ls"),
        (Role::ToolOutput, "file.txt"),
        (Role::Error, "it broke"),
    ] {
        app.tab_mut().push(role, text);
    }

    let screen = screen(&mut app, 100, 40);
    for label in [
        "\u{256d}\u{2500} you",
        "\u{256d}\u{2500} deepseek-v4-flash",
        "\u{256d}\u{2500} thinking",
        "\u{256d}\u{2500} tool",
        "\u{251c}\u{2500}\u{2500}",
        "\u{256d}\u{2500} error",
    ] {
        assert!(screen.contains(label), "missing frame {label:?}:\n{screen}");
    }
    for text in [
        "a question",
        "an answer",
        "reasoning here",
        "file.txt",
        "it broke",
    ] {
        assert!(screen.contains(text), "missing body {text:?}");
    }
}

#[test]
fn each_role_gets_a_distinct_frame_colour() {
    use ratatui::style::Color;
    let mut app = App::new("s", "m");
    for (role, text) in [
        (Role::User, "q"),
        (Role::Assistant, "a"),
        (Role::Thinking, "t"),
        (Role::Tool, "c"),
        (Role::Error, "e"),
    ] {
        app.tab_mut().push(role, text);
    }

    let backend = TestBackend::new(100, 40);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();

    let mut edges: Vec<Color> = buffer
        .content()
        .iter()
        .filter(|c| c.symbol() == "\u{256d}")
        .map(|c| c.fg)
        .collect();
    edges.sort_by_key(|c| format!("{c:?}"));
    edges.dedup();
    assert!(
        edges.len() >= 5,
        "expected a distinct frame colour per role, got {edges:?}"
    );
}

#[test]
fn thinking_text_is_rendered_and_boxed() {
    let mut app = App::new("s", "m");
    app.tab_mut()
        .on_event("llm.reasoning", &json!({"text": "considering options"}));
    let screen = screen(&mut app, 100, 30);
    assert!(screen.contains("thinking"), "{screen}");
    assert!(screen.contains("considering options"), "{screen}");
}

#[test]
fn the_transcript_shows_no_raw_role_gutters() {
    let mut app = App::new("s", "m");
    app.tab_mut().push(Role::User, "q");
    app.tab_mut().push(Role::Assistant, "a");
    let screen = screen(&mut app, 100, 30);
    for gutter in [
        "you \u{2502}",
        "sa  \u{2502}",
        "tool\u{2502}",
        "sys \u{2502}",
    ] {
        assert!(!screen.contains(gutter), "raw gutter {gutter:?}:\n{screen}");
    }
}

#[test]
fn the_status_bar_shows_the_working_directory_rate_and_tokens() {
    let mut app = App::new("session-abcdef", "deepseek");
    app.tab_mut().cwd = "/work/project".to_owned();
    app.tab_mut().status.context_tokens = 32_000;
    app.tab_mut().status.context_limit = 128_000;
    let screen = screen(&mut app, 160, 24);
    assert!(screen.contains("/work/project"), "{screen}");
    assert!(screen.contains("0.0 t/s"), "{screen}");
    // the token counts gave way to a context meter.
    assert!(screen.contains("25%"), "{screen}");
    assert!(
        screen.contains("ctrl+t"),
        "key hints must be present:\n{screen}"
    );
}

#[test]
fn thinking_text_counts_as_generation() {
    use std::time::{Duration, Instant};
    let mut app = App::new("s", "m");
    let t0 = Instant::now();
    let done = t0 + Duration::from_secs(2);
    app.tab_mut()
        .on_event_at("turn.started", &json!({"prompt": "go"}), t0);
    // reasoning is the model producing tokens, so it is generation like any other.
    app.tab_mut()
        .on_event_at("llm.reasoning", &json!({"text": "y".repeat(400)}), t0);
    app.tab_mut().on_event_at("llm.completed", &json!({}), done);
    app.tab_mut().on_event_at(
        "turn.completed",
        &json!({"usage": {"prompt_tokens": 10, "completion_tokens": 100}}),
        done,
    );
    app.tab_mut().tick_at(done);

    assert!(
        (app.tab().tok_per_sec - 50.0).abs() < 1.0,
        "thinking streams tokens: got {}",
        app.tab().tok_per_sec
    );
}

#[test]
fn silence_before_the_first_token_is_not_counted() {
    use std::time::{Duration, Instant};
    let mut app = App::new("s", "m");
    let t0 = Instant::now();
    // the prompt goes out and nothing comes back for five seconds.
    let first = t0 + Duration::from_secs(5);
    let done = first + Duration::from_secs(2);
    app.tab_mut()
        .on_event_at("turn.started", &json!({"prompt": "hello"}), t0);
    app.tab_mut()
        .on_event_at("llm.delta", &json!({"text": "x".repeat(400)}), first);
    app.tab_mut()
        .on_event_at("llm.delta", &json!({"text": "x".repeat(400)}), done);
    app.tab_mut().on_event_at("llm.completed", &json!({}), done);
    app.tab_mut().on_event_at(
        "turn.completed",
        &json!({"usage": {"prompt_tokens": 10, "completion_tokens": 200}}),
        done,
    );
    app.tab_mut().tick_at(done);

    let rate = app.tab().tok_per_sec;
    assert!(
        (rate - 100.0).abs() < 1.0,
        "200 tokens over the 2s of streaming, not the 7s including the wait: got {rate}"
    );
}

#[test]
fn replaying_a_session_never_produces_a_rate() {
    let mut app = App::new("s", "m");

    // a whole session coming back from disk, as open_tab does it.
    app.tab_mut()
        .on_replay("turn.started", &json!({"prompt": "go"}));
    for _ in 0..200 {
        app.tab_mut()
            .on_replay("llm.delta", &json!({"text": "x".repeat(500)}));
    }
    app.tab_mut().on_replay("llm.completed", &json!({}));
    app.tab_mut().on_replay(
        "turn.completed",
        &json!({"usage": {"prompt_tokens": 10, "completion_tokens": 25_000}}),
    );
    app.tab_mut().tick();

    assert_eq!(
        app.tab().tok_per_sec,
        0.0,
        "a replay is history, not generation: got {}",
        app.tab().tok_per_sec
    );
    assert!(
        app.tab().transcript.len() >= 2,
        "the transcript is still rebuilt from the replay"
    );
}

#[test]
fn a_stall_between_deltas_is_not_counted_as_generation_time() {
    use std::time::{Duration, Instant};
    let mut app = App::new("s", "m");
    let t0 = Instant::now();
    let later = t0 + Duration::from_secs(10);
    app.tab_mut()
        .on_event_at("turn.started", &json!({"prompt": "go"}), t0);
    app.tab_mut()
        .on_event_at("llm.delta", &json!({"text": "x".repeat(400)}), t0);
    // ten seconds of nothing, then the rest of the same message.
    app.tab_mut()
        .on_event_at("llm.delta", &json!({"text": "x".repeat(400)}), later);
    app.tab_mut()
        .on_event_at("llm.completed", &json!({}), later);
    app.tab_mut().on_event_at(
        "turn.completed",
        &json!({"usage": {"prompt_tokens": 10, "completion_tokens": 200}}),
        later,
    );
    app.tab_mut().tick_at(later);

    let rate = app.tab().tok_per_sec;
    assert!(
        (rate - 100.0).abs() < 1.0,
        "only a capped gap counts, so 200 tokens over 2s of streaming: got {rate}"
    );
}

#[test]
fn the_providers_own_count_is_preferred_over_the_estimate() {
    use std::time::{Duration, Instant};
    let mut app = App::new("s", "m");
    let t0 = Instant::now();
    let done = t0 + Duration::from_secs(2);
    app.tab_mut()
        .on_event_at("turn.started", &json!({"prompt": "go"}), t0);
    app.tab_mut()
        .on_event_at("llm.delta", &json!({"text": "x".repeat(400)}), t0);
    app.tab_mut().on_event_at("llm.completed", &json!({}), done);
    app.tab_mut().on_event_at(
        "turn.completed",
        &json!({"usage": {"prompt_tokens": 10, "completion_tokens": 300}}),
        done,
    );
    app.tab_mut().tick_at(done);

    let rate = app.tab().tok_per_sec;
    assert!(
        (rate - 150.0).abs() < 1.0,
        "300 counted tokens over 2s is 150 t/s: got {rate}"
    );
}

#[test]
fn the_average_counts_only_generation_time() {
    use std::time::{Duration, Instant};
    let mut app = App::new("s", "m");
    let t0 = Instant::now();
    let done = t0 + Duration::from_secs(2);
    let after = done + Duration::from_secs(28);
    app.tab_mut()
        .on_event_at("turn.started", &json!({"prompt": "go"}), t0);
    app.tab_mut()
        .on_event_at("llm.delta", &json!({"text": "x".repeat(400)}), t0);
    app.tab_mut().on_event_at("llm.completed", &json!({}), done);
    app.tab_mut()
        .on_event_at("tool.requested", &json!({"tool": "bash"}), done);
    app.tab_mut()
        .on_event_at("tool.completed", &json!({"output": "slow"}), after);
    app.tab_mut().on_event_at(
        "turn.completed",
        &json!({"usage": {"prompt_tokens": 1, "completion_tokens": 100}}),
        after,
    );
    app.tab_mut().tick_at(after);
    assert!(
        (app.tab().tok_per_sec - 50.0).abs() < 1.0,
        "tool time must not count: got {}",
        app.tab().tok_per_sec
    );
}

#[test]
fn the_average_holds_until_the_next_message_starts() {
    use std::time::{Duration, Instant};
    let mut app = App::new("s", "m");
    let t0 = Instant::now();
    let done = t0 + Duration::from_secs(2);
    app.tab_mut()
        .on_event_at("turn.started", &json!({"prompt": "go"}), t0);
    app.tab_mut()
        .on_event_at("llm.delta", &json!({"text": "x".repeat(400)}), t0);
    app.tab_mut().on_event_at("llm.completed", &json!({}), done);
    app.tab_mut().on_event_at(
        "turn.completed",
        &json!({"usage": {"prompt_tokens": 1, "completion_tokens": 100}}),
        done,
    );

    app.tab_mut().tick_at(done + Duration::from_secs(30));
    assert!(app.tab().tok_per_sec > 0.0, "the figure holds");

    app.tab_mut().on_event_at(
        "turn.started",
        &json!({"prompt": "next"}),
        done + Duration::from_secs(31),
    );
    app.tab_mut().tick_at(done + Duration::from_secs(31));
    assert_eq!(app.tab().tok_per_sec, 0.0, "a new message resets it");
}

#[test]
fn scroll_clamps_to_the_rendered_transcript() {
    let mut app = App::new("s", "m");
    for n in 0..12 {
        app.tab_mut().push(Role::Assistant, format!("line {n}"));
    }
    let _ = screen(&mut app, 100, 24);
    let ceiling = app.tab().max_scroll;
    assert!(ceiling > 0, "a long transcript must be scrollable");

    for _ in 0..500 {
        app.on_key(key(KeyCode::Up));
    }
    assert_eq!(app.tab().scroll, 0);

    for _ in 0..500 {
        app.on_key(key(KeyCode::Down));
    }
    assert_eq!(app.tab().scroll, ceiling);
}

#[test]
fn the_layout_renders_at_several_sizes() {
    for (width, height) in [(20u16, 6u16), (40, 10), (100, 30), (200, 60)] {
        let mut app = App::new("session-abcdef", "deepseek");
        app.tab_mut().branch = Some("main".to_owned());
        app.add_tab("second-session", "deepseek");
        app.tab_mut().push(Role::User, "hello there");
        app.tab_mut().push(
            Role::Assistant,
            "a reply long enough to wrap in a narrow pane",
        );
        app.tab_mut().push(Role::ToolOutput, "output line");
        let _ = screen(&mut app, width, height);
    }
}

#[test]
fn the_composer_is_just_a_box_with_the_text() {
    let mut app = App::new("session-abcdef", "m");
    typed(&mut app, "hello");

    let height = 24u16;
    let backend = TestBackend::new(100, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();
    let row = |y: u16| -> String { (0..100).map(|x| buffer[(x, y)].symbol()).collect() };

    let composer = row(height - 3);
    assert!(
        composer.contains("hello"),
        "the text must render:\n{composer}"
    );
    // the caret is the terminal cursor, not a drawn glyph.
    assert!(!composer.contains('|'), "no drawn caret:\n{composer}");
    assert!(
        !composer.contains('\u{276f}'),
        "no prompt glyph:\n{composer}"
    );

    let border = row(height - 4);
    assert!(
        border.contains('\u{256d}') && border.contains('\u{256e}'),
        "the box must remain:\n{border}"
    );
}

#[test]
fn every_frame_row_is_the_same_width() {
    let mut app = App::new("s", "m");
    app.tab_mut().push(Role::User, "a question");
    app.tab_mut().push(Role::Assistant, "an answer");
    app.tab_mut().push(Role::Thinking, "thinking");

    let lines = sa_tui::render(&app, 98);
    // the blank rows between frames are spacing, not frame rows.
    let widths: Vec<usize> = lines
        .iter()
        .map(ratatui::prelude::Line::width)
        .filter(|width| *width > 0)
        .collect();
    let first = widths.first().copied().unwrap_or(0);
    assert_eq!(first, 98, "frames must fill the pane exactly");
    for (index, width) in widths.iter().enumerate() {
        assert_eq!(
            *width, first,
            "row {index} is {width} wide, expected {first}"
        );
    }
}

#[test]
fn the_assistant_frame_is_labelled_with_the_model_not_the_agent() {
    let mut app = App::new("s", "deepseek-v4-flash");
    app.tab_mut().push(Role::Assistant, "an answer");
    let screen = screen(&mut app, 100, 20);

    assert!(
        screen.contains("\u{256d}\u{2500} deepseek-v4-flash"),
        "the model must label its own output:\n{screen}"
    );
    assert!(
        !screen.contains("\u{256d}\u{2500} sa "),
        "the agent's own name must not appear:\n{screen}"
    );
}

#[test]
fn an_unknown_model_falls_back_to_a_generic_label() {
    let mut app = App::new("s", "");
    app.tab_mut().push(Role::Assistant, "an answer");
    let screen = screen(&mut app, 100, 20);
    assert!(screen.contains("\u{256d}\u{2500} assistant"), "{screen}");
}

#[test]
fn reasoning_and_its_reply_share_one_split_frame() {
    let mut app = App::new("s", "deepseek-v4-flash");
    app.tab_mut().push(Role::Thinking, "weighing the options");
    app.tab_mut().push(Role::Assistant, "here is the answer");

    let screen = screen(&mut app, 100, 30);
    // one titled frame, not two.
    // the pane border also starts with a corner, so match the frame header's space.
    assert_eq!(
        screen.matches("\u{256d}\u{2500} ").count(),
        1,
        "the turn must be a single frame:\n{screen}"
    );
    assert!(
        screen.contains("\u{256d}\u{2500} deepseek-v4-flash"),
        "{screen}"
    );
    assert!(screen.contains("weighing the options"), "{screen}");
    assert!(screen.contains("here is the answer"), "{screen}");
    assert!(
        screen.contains('\u{251c}'),
        "the two sections must be split by a rule:\n{screen}"
    );

    // reasoning sits above the reply.
    let thinking_at = screen.find("weighing the options").expect("reasoning");
    let reply_at = screen.find("here is the answer").expect("reply");
    assert!(
        thinking_at < reply_at,
        "reasoning must come first:\n{screen}"
    );
}

#[test]
fn reasoning_without_a_reply_keeps_its_own_frame() {
    let mut app = App::new("s", "deepseek-v4-flash");
    app.tab_mut().push(Role::Thinking, "still thinking");
    let screen = screen(&mut app, 100, 20);
    assert!(screen.contains("\u{256d}\u{2500} thinking"), "{screen}");
    assert!(screen.contains("still thinking"), "{screen}");
    assert!(
        !screen.contains('\u{251c}'),
        "no divider without a reply:\n{screen}"
    );
}

#[test]
fn reasoning_is_always_grey_and_the_divider_matches_its_frame() {
    use ratatui::style::Color;

    let mut app = App::new("s", "deepseek-v4");
    app.tab_mut().push(Role::Thinking, "weighing");
    app.tab_mut().push(Role::Assistant, "answer");

    let backend = TestBackend::new(100, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();

    let fg_of = |symbol: &str| -> Option<Color> {
        buffer
            .content()
            .iter()
            .find(|c| c.symbol() == symbol)
            .map(|c| c.fg)
    };

    // the rule belongs to the assistant frame, so it is that frame's colour.
    assert_eq!(
        fg_of("\u{251c}"),
        Some(sa_tui::app::OK),
        "the divider must match the frame it splits"
    );

    // reasoning is neutral grey, never tinted.
    let Color::Rgb(r, g, b) = fg_of("w").expect("reasoning text") else {
        panic!("rgb");
    };
    assert!(
        r.max(g).max(b) - r.min(g).min(b) <= 24,
        "reasoning text must be grey, not tinted: {:?}",
        fg_of("w")
    );
    assert_ne!(
        fg_of("w"),
        Some(Color::Rgb(0xc7, 0x9b, 0xff)),
        "reasoning text must not be violet"
    );
}

#[test]
fn a_standalone_reasoning_frame_is_grey() {
    let mut app = App::new("s", "deepseek-v4");
    app.tab_mut().push(Role::Thinking, "pondering");

    let backend = TestBackend::new(100, 20);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();

    let edge = buffer
        .content()
        .iter()
        .find(|c| c.symbol() == "\u{256d}")
        .map(|c| c.fg)
        .expect("a frame corner");
    // grey means the channels sit together: no tint from a role colour.
    let ratatui::style::Color::Rgb(r, g, b) = edge else {
        panic!("rgb");
    };
    let spread = r.max(g).max(b) - r.min(g).min(b);
    assert!(
        spread <= 20,
        "a thinking frame must be grey, not tinted: {edge:?}"
    );
}

#[test]
fn a_tool_call_and_its_output_share_one_split_frame() {
    let mut app = App::new("s", "m");
    app.tab_mut().on_event(
        "tool.requested",
        &json!({"tool": "bash", "args": {"command": "ls -la"}}),
    );
    app.tab_mut().on_event(
        "tool.completed",
        &json!({"output": "file.txt", "exit_code": 1}),
    );

    let screen = screen(&mut app, 100, 30);

    // one frame, titled with the tool, not two.
    assert_eq!(
        screen.matches("\u{256d}\u{2500} ").count(),
        1,
        "the call and its output must share a frame:\n{screen}"
    );
    assert!(
        screen.contains("\u{256d}\u{2500} bash (exit 1)"),
        "the exit code belongs in the title:\n{screen}"
    );
    assert!(screen.contains("$ ls -la"), "bash gets a prompt:\n{screen}");
    assert!(
        !screen.contains("\u{251c}\u{2500} "),
        "the rule must carry no text:\n{screen}"
    );
    assert!(screen.contains("file.txt"), "{screen}");

    // the invocation sits above the rule, the output below it.
    let call_at = screen.find("$ ls -la").expect("call");
    let rule_at = screen.find("\u{251c}").expect("rule");
    let out_at = screen.find("file.txt").expect("output");
    assert!(call_at < rule_at && rule_at < out_at, "{screen}");
}

#[test]
fn a_tool_without_output_yet_is_its_own_frame() {
    let mut app = App::new("s", "m");
    app.tab_mut().on_event(
        "tool.requested",
        &json!({"tool": "bash", "args": {"command": "sleep 10"}}),
    );
    let screen = screen(&mut app, 100, 20);
    assert!(screen.contains("\u{256d}\u{2500} bash"), "{screen}");
    assert!(screen.contains("$ sleep 10"), "{screen}");
    assert!(
        !screen.contains("\u{251c}"),
        "no split without a result:\n{screen}"
    );
}

#[test]
fn the_rate_freezes_while_a_tool_runs() {
    use std::time::{Duration, Instant};
    let mut app = App::new("s", "m");
    let t0 = Instant::now();
    app.tab_mut().status.turn_running = true;
    app.tab_mut()
        .on_event_at("turn.started", &json!({"prompt": "go"}), t0);
    app.tab_mut()
        .on_event_at("llm.delta", &json!({"text": "x".repeat(400)}), t0);
    // a rate needs two samples: one delta alone has no span to measure over.
    app.tab_mut().on_event_at(
        "llm.delta",
        &json!({"text": "x".repeat(400)}),
        t0 + Duration::from_millis(200),
    );
    app.tab_mut().tick_at(t0 + Duration::from_millis(300));
    let live = app.tab().tok_per_sec;
    assert!(live > 0.0, "a live rate first");

    // the window ages out during a long tool call, but the figure must hold.
    app.tab_mut().on_event_at(
        "tool.requested",
        &json!({"tool": "bash"}),
        t0 + Duration::from_millis(200),
    );
    app.tab_mut().tick_at(t0 + Duration::from_secs(30));
    assert!(
        app.tab().tok_per_sec > 0.0,
        "the rate must freeze during tools, got {}",
        app.tab().tok_per_sec
    );
}

#[test]
fn the_status_bar_shows_the_effort_level() {
    let mut app = App::new("session-abcdef", "deepseek-v4");
    app.tab_mut().status.effort = "high".to_owned();
    let screen = screen(&mut app, 160, 24);
    assert!(screen.contains("deepseek-v4:high"), "{screen}");
}

#[test]
fn frames_fill_whatever_width_is_available() {
    let mut app = App::new("s", "m");
    app.tab_mut().push(Role::Assistant, "x");
    assert_eq!(sa_tui::render(&app, 60)[0].width(), 60);
    assert_eq!(
        sa_tui::render(&app, 120)[0].width(),
        120,
        "a wide terminal must not leave a dead margin"
    );
}

#[test]
fn scrolling_moves_several_lines_per_key() {
    let mut app = App::new("s", "m");
    for n in 0..40 {
        app.tab_mut().push(Role::Assistant, format!("line {n}"));
    }
    let _ = screen(&mut app, 100, 24);
    let ceiling = app.tab().max_scroll;
    assert!(ceiling > 3, "need a scrollable transcript");

    app.on_key(key(KeyCode::Up));
    assert!(
        app.tab().scroll <= ceiling - 3,
        "an arrow key must move more than one line"
    );
}

#[test]
fn markdown_bold_is_rendered_as_bold_without_markers() {
    use ratatui::style::Modifier;

    let mut app = App::new("s", "m");
    app.tab_mut()
        .push(Role::Assistant, "It is **Sunday** today");

    let backend = TestBackend::new(100, 20);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();
    let screen: String = buffer
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect();

    assert!(
        screen.contains("It is Sunday today"),
        "the markers must not be drawn:\n{screen}"
    );
    assert!(!screen.contains("**"), "no raw markers:\n{screen}");

    let bold = buffer
        .content()
        .iter()
        .any(|c| c.symbol() == "S" && c.modifier.contains(Modifier::BOLD));
    assert!(bold, "the wrapped run must actually be bold");
}

#[test]
fn long_text_is_truncated_inside_the_frame_not_wrapped_outside_it() {
    let mut app = App::new("s", "m");
    app.tab_mut().push(
        Role::Assistant,
        "The user just says hello. I should respond briefly. Maybe check the workspace? \
         A simple greeting response is fine, but perhaps I should be a bit useful. Keep concise.",
    );

    let width = 60u16;
    let backend = TestBackend::new(width, 20);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();

    // every drawn row must start and end with a frame border, at the frame width.
    let mut rows = 0;
    for y in 0..20u16 {
        let row: String = (0..width).map(|x| buffer[(x, y)].symbol()).collect();
        if row.starts_with('\u{2502}') || row.starts_with('\u{256d}') {
            rows += 1;
            assert!(
                row.ends_with('\u{2502}') || row.ends_with('\u{256e}'),
                "row {y} lost its right border: {row:?}"
            );
        }
    }
    assert!(rows >= 3, "expected a framed message, got {rows} rows");
}

#[test]
fn a_busy_tab_shows_a_spinner() {
    let mut app = App::new("session-abcdef", "m");
    app.tab_mut().status.turn_running = true;
    app.spinner = 3;
    let screen = screen(&mut app, 100, 20);
    // a star sits beside the tab label.
    assert!(
        screen
            .chars()
            .any(|c| ('\u{2736}'..='\u{273a}').contains(&c)),
        "expected a busy marker on the tab:\n{screen}"
    );
}

#[test]
fn an_idle_tab_has_no_spinner() {
    let app = App::new("session-abcdef", "m");
    let mut app = app;
    let screen = screen(&mut app, 100, 20);
    assert!(
        !screen
            .chars()
            .any(|c| ('\u{2736}'..='\u{273a}').contains(&c)),
        "no spinner when idle:\n{screen}"
    );
}

#[test]
fn the_tab_title_keeps_its_session_id_after_a_rename() {
    let mut app = App::new("abcdef12-3456", "m");
    let tab = app.tab_for("abcdef12-3456").expect("tab");
    let short: String = "abcdef12-3456".chars().take(8).collect();
    tab.title = format!("Fixing rust bugs ({short})");
    let screen = screen(&mut app, 100, 20);
    // the summary may be shortened to fit, but the id must always survive.
    assert!(screen.contains("Fixing rust"), "{screen}");
    assert!(screen.contains("(abcdef12"), "{screen}");
}

#[test]
fn applying_a_rename_sets_the_tab_title_with_its_id() {
    let mut app = App::new("abcdef12-3456-7890", "m");
    app.apply_rename("abcdef12-3456-7890", "Fixing rust bugs");

    assert_eq!(app.tab().title, "Fixing rust bugs (abcdef12)");

    let screen = screen(&mut app, 120, 20);
    assert!(screen.contains("Fixing rust bugs"), "{screen}");
    assert!(screen.contains("(abcdef12)"), "{screen}");
}

#[test]
fn a_rename_for_an_unknown_session_is_ignored() {
    let mut app = App::new("abcdef12-3456", "m");
    app.apply_rename("other-session-id", "Somewhere else");
    assert!(
        app.tab().title.contains("abcdef12"),
        "{:?}",
        app.tab().title
    );
}

#[test]
fn long_tool_output_is_not_truncated() {
    let mut app = App::new("s", "m");
    let output: String = (0..40)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.tab_mut().on_event(
        "tool.requested",
        &json!({"tool": "bash", "args": {"command": "seq 40"}}),
    );
    app.tab_mut()
        .on_event("tool.completed", &json!({"output": output, "exit_code": 0}));

    // every line must survive, not just the first dozen.
    let screen = screen(&mut app, 100, 60);
    assert!(screen.contains("line 0"), "{screen}");
    assert!(
        screen.contains("line 39"),
        "the tail must survive:\n{screen}"
    );
}

#[test]
fn markdown_italics_and_code_render_without_markers() {
    use ratatui::style::Modifier;

    let mut app = App::new("s", "m");
    app.tab_mut()
        .push(Role::Assistant, "use *cargo* with `--release` now");

    let backend = TestBackend::new(120, 20);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();
    let screen: String = buffer
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect();

    assert!(
        screen.contains("use cargo with --release now"),
        "markers must be stripped:\n{screen}"
    );
    for marker in ["*", "`"] {
        assert!(!screen.contains(marker), "no {marker:?} left:\n{screen}");
    }

    let italic = buffer
        .content()
        .iter()
        .any(|c| c.symbol() == "c" && c.modifier.contains(Modifier::ITALIC));
    assert!(italic, "the italic run must be italic");

    // the renderer styles a code run apart from the prose around it.
    let prose = buffer
        .content()
        .iter()
        .find(|c| c.symbol() == "c")
        .map(|c| c.fg);
    let code = buffer
        .content()
        .iter()
        .any(|c| c.symbol() == "-" && Some(c.fg) != prose);
    assert!(code, "the code run must be styled apart from the prose");
}

#[test]
fn the_hint_offers_interrupt_while_a_turn_runs() {
    let mut app = App::new("session-abcdef", "m");
    let idle = screen(&mut app, 160, 24);
    assert!(idle.contains("ctrl+1..9 jump"), "idle hints:\n{idle}");

    app.tab_mut().status.turn_running = true;
    let busy = screen(&mut app, 160, 24);
    assert!(busy.contains("esc interrupt"), "busy hints:\n{busy}");
    // the interrupt takes the whole hint rather than sitting beside the others.
    assert!(
        !busy.contains("ctrl+1..9 jump"),
        "jump hint yields:\n{busy}"
    );
    assert!(!busy.contains("ctrl+t new"), "new-tab hint yields:\n{busy}");

    // and it is plain text, not a highlighted chip: no accent background on the status row.
    let backend = TestBackend::new(160, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let width = 160;
    let status_row = 23 * width;
    let highlighted = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .skip(status_row)
        .take(width)
        .any(|cell| cell.bg == sa_tui::app::ACCENT);
    assert!(!highlighted, "the interrupt hint must not be highlighted");
}

#[test]
fn scrolling_does_not_invalidate_the_render_cache() {
    let mut app = App::new("s", "m");
    for n in 0..30 {
        app.tab_mut().push(Role::Assistant, format!("line {n}"));
    }
    let _ = screen(&mut app, 100, 24);
    let before = app.tab().content_rev;

    for _ in 0..10 {
        app.on_key(key(KeyCode::Up));
    }
    assert_eq!(
        app.tab().content_rev,
        before,
        "scrolling must reuse the cached render"
    );

    app.tab_mut().push(Role::Assistant, "new line");
    assert_ne!(
        app.tab().content_rev,
        before,
        "new content must invalidate the cache"
    );
}

#[test]
fn tool_output_is_never_treated_as_markdown() {
    let mut app = App::new("s", "m");
    app.tab_mut().on_event(
        "tool.requested",
        &json!({"tool": "bash", "args": {"command": "echo **not bold**"}}),
    );
    app.tab_mut().on_event(
        "tool.completed",
        &json!({"output": "**literal** and *also literal*", "exit_code": 0}),
    );

    let screen = screen(&mut app, 120, 30);
    assert!(
        screen.contains("**literal**"),
        "tool output must keep its markers:\n{screen}"
    );
    assert!(
        screen.contains("$ echo **not bold**"),
        "the command is literal too:\n{screen}"
    );
}

#[test]
fn ansi_colour_in_command_output_is_honoured() {
    use ratatui::style::Color;

    let mut app = App::new("s", "m");
    app.tab_mut().on_event(
        "tool.requested",
        &json!({"tool": "bash", "args": {"command": "ls --color"}}),
    );
    app.tab_mut().on_event(
        "tool.completed",
        &json!({"output": "\u{1b}[31mred\u{1b}[0m plain", "exit_code": 0}),
    );

    let backend = TestBackend::new(120, 20);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();
    let screen: String = buffer
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect();

    assert!(screen.contains("red plain"), "{screen}");
    assert!(
        !screen.contains('\u{1b}'),
        "escapes must not leak:\n{screen}"
    );

    let tinted = buffer
        .content()
        .iter()
        .any(|c| c.symbol() == "r" && c.fg != sa_tui::app::DIM && c.fg != Color::Reset);
    assert!(tinted, "the coloured run must keep its colour");
}

#[test]
fn patch_lines_are_coloured_by_their_marker() {
    let mut app = App::new("s", "m");
    app.tab_mut().on_event(
        "tool.requested",
        &json!({"tool": "bash", "args": {"command": "git diff"}}),
    );
    app.tab_mut().on_event(
        "tool.completed",
        &json!({"output": "+added\n-removed", "exit_code": 0}),
    );

    let backend = TestBackend::new(120, 20);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();

    let ink = |symbol: &str| {
        buffer
            .content()
            .iter()
            .find(|c| c.symbol() == symbol)
            .map(|c| c.fg)
    };
    let added = ink("+");
    let removed = ink("-");
    assert!(
        added.is_some() && removed.is_some(),
        "both lines must render"
    );
    assert_ne!(added, removed, "additions and removals differ in colour");
}

#[test]
fn rebuilding_a_large_transcript_stays_within_a_frame() {
    let mut app = App::new("s", "m");
    for n in 0..400 {
        app.tab_mut().push(
            Role::Assistant,
            format!("line {n} with some prose to pad it out"),
        );
    }

    let started = std::time::Instant::now();
    let lines = sa_tui::render(&app, 120);
    let once = started.elapsed();

    assert!(lines.len() > 1000, "expected a large transcript");
    // a rebuild must stay well under the frame interval, or the loop starves. the budget is
    // loose on purpose: this catches a pathological regression, not a busy machine.
    assert!(
        once < std::time::Duration::from_millis(250),
        "rebuilding {} lines took {once:?}",
        lines.len()
    );
}

#[test]
fn the_status_bar_shows_context_pressure() {
    let mut app = App::new("session-abcdef", "m");
    app.tab_mut().status.context_tokens = 64_000;
    app.tab_mut().status.context_limit = 128_000;

    let screen = screen(&mut app, 160, 24);
    assert!(screen.contains("50%"), "expected a usage figure:\n{screen}");
    assert!(
        screen.contains('\u{2588}'),
        "expected filled meter cells:\n{screen}"
    );
}

#[test]
fn context_pressure_turns_red_near_the_limit() {
    use ratatui::style::Color;

    let mut app = App::new("session-abcdef", "m");
    app.tab_mut().status.context_tokens = 120_000;
    app.tab_mut().status.context_limit = 128_000;

    let backend = TestBackend::new(160, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();

    let hot = buffer
        .content()
        .iter()
        .any(|c| c.symbol() == "\u{2588}" && c.fg == sa_tui::app::BAD);
    assert!(hot, "a nearly full window must read as danger");
    let _ = Color::Reset;
}

fn flatten(lines: &[ratatui::text::Line<'static>]) -> String {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_working_header_rides_the_composer_border() {
    let mut app = App::new("session-abcdef", "m");
    app.tab_mut().status.turn_running = true;
    app.tab_mut().working = Some("cogitating".to_owned());

    let screen = screen(&mut app, 100, 20);
    assert!(screen.contains("cogitating"), "{screen}");

    // it belongs to the composer, not the transcript at the bottom of the chat.
    let transcript = flatten(&transcript_lines(app.tab(), 100));
    assert!(!transcript.contains("cogitating"), "{transcript}");
}

#[test]
fn the_working_header_disappears_when_the_turn_ends() {
    let mut app = App::new("session-abcdef", "m");
    app.tab_mut().status.turn_running = true;
    app.tab_mut().working = Some("cogitating".to_owned());
    assert!(screen(&mut app, 100, 20).contains("cogitating"));

    // what turn.completed and turn.interrupted do.
    app.tab_mut().status.turn_running = false;
    app.tab_mut().working = None;
    let screen = screen(&mut app, 100, 20);
    assert!(!screen.contains("cogitating"), "{screen}");
}

#[test]
fn every_working_word_is_distinct_and_non_empty() {
    let mut words: Vec<&str> = sa_tui::app::WORKING_WORDS.to_vec();
    assert_ne!(words.len(), 0);
    words.sort_unstable();
    words.dedup();
    assert_eq!(words.len(), sa_tui::app::WORKING_WORDS.len());
    assert!(sa_tui::app::WORKING_WORDS
        .iter()
        .all(|w| !w.trim().is_empty()));
    assert_ne!(sa_tui::app::working_word().len(), 0);
}

#[test]
fn the_scrollbar_appears_only_while_scrolling() {
    let mut app = App::new("session-abcdef", "m");
    for line in 0..60 {
        app.tab_mut().push(Role::System, format!("line {line}"));
    }

    // nothing to show until the operator actually scrolls.
    let idle = screen(&mut app, 80, 12);
    assert!(!idle.contains('\u{2503}'), "no bar when idle:\n{idle}");

    app.tab_mut().scroll_touched = Some(std::time::Instant::now());
    let scrolled = screen(&mut app, 80, 12);
    assert!(
        scrolled.contains('\u{2503}'),
        "bar while scrolling:\n{scrolled}"
    );

    // the tick clears it once the hold has passed, so it is not permanent furniture.
    app.tab_mut()
        .tick_at(std::time::Instant::now() + std::time::Duration::from_secs(2));
    let after = screen(&mut app, 80, 12);
    assert!(!after.contains('\u{2503}'), "bar clears:\n{after}");
}

#[test]
fn a_short_transcript_never_shows_a_scrollbar() {
    let mut app = App::new("session-abcdef", "m");
    app.tab_mut().push(Role::System, "only one line".to_owned());
    app.tab_mut().scroll_touched = Some(std::time::Instant::now());
    let screen = screen(&mut app, 80, 20);
    assert!(
        !screen.contains('\u{2503}'),
        "no bar without overflow:\n{screen}"
    );
}

#[test]
fn the_picker_routes_keys_and_returns_the_chosen_session() {
    let mut app = App::new("session-abcdef", "m");
    app.open_picker(
        PickerKind::Resume,
        "resume",
        vec![
            PickerItem {
                id: "aaa".to_owned(),
                label: "first".to_owned(),
            },
            PickerItem {
                id: "bbb".to_owned(),
                label: "second".to_owned(),
            },
        ],
    );
    let shown = screen(&mut app, 100, 20);
    assert!(shown.contains("resume"), "{shown}");
    assert!(shown.contains("first"), "{shown}");

    assert_eq!(app.on_key(key(KeyCode::Down)), Action::None);
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::ResumePicked("bbb".to_owned())
    );
    assert!(app.picker.is_none(), "the picker closes on choice");
}

#[test]
fn escape_closes_the_picker_without_choosing() {
    let mut app = App::new("session-abcdef", "m");
    app.open_picker(
        PickerKind::Resume,
        "resume",
        vec![PickerItem {
            id: "aaa".to_owned(),
            label: "first".to_owned(),
        }],
    );
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert!(app.picker.is_none());
}

#[test]
fn keys_go_to_the_picker_not_the_composer() {
    let mut app = App::new("session-abcdef", "m");
    app.open_picker(
        PickerKind::Resume,
        "resume",
        vec![PickerItem {
            id: "aaa".to_owned(),
            label: "first".to_owned(),
        }],
    );
    app.on_key(key(KeyCode::Char('x')));
    assert_eq!(app.tab().input_text(), "", "the composer must not see keys");
}

#[test]
fn typing_an_image_path_attaches_it() {
    let dir = std::env::temp_dir().join("sa-tui-attach-test");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("shot.png");
    std::fs::write(&path, [0x89, b'P', b'N', b'G']).expect("write png");

    let mut app = App::new("session-abcdef", "m");
    for character in path.display().to_string().chars() {
        app.on_key(key(KeyCode::Char(character)));
    }

    let attachments = &app.tab().attachments;
    assert_eq!(attachments.len(), 1, "the path is picked up as it is typed");
    assert_eq!(attachments[0].media_type, "image/png");
    assert_eq!(attachments[0].name, "shot.png");
    assert!(!attachments[0].data.is_empty(), "the bytes are carried");
}

#[test]
fn deleting_the_path_drops_the_attachment() {
    let dir = std::env::temp_dir().join("sa-tui-attach-test");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("shot2.png");
    std::fs::write(&path, [0x89, b'P', b'N', b'G']).expect("write png");

    let mut app = App::new("session-abcdef", "m");
    let text = path.display().to_string();
    for character in text.chars() {
        app.on_key(key(KeyCode::Char(character)));
    }
    assert_eq!(app.tab().attachments.len(), 1);

    for _ in 0..text.chars().count() {
        app.on_key(key(KeyCode::Backspace));
    }
    assert!(
        app.tab().attachments.is_empty(),
        "attachment follows the text"
    );
}

/// types `text` into the composer, one key at a time.
fn type_text(app: &mut App, text: &str) {
    for character in text.chars() {
        app.on_key(key(KeyCode::Char(character)));
    }
}

#[test]
fn tab_completes_a_directory_path() {
    let dir = std::env::temp_dir().join("sa-tui-complete");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Downloads")).expect("dirs");

    let mut app = App::new("session-abcdef", "m");
    type_text(&mut app, &format!("{}/Down", dir.display()));
    app.on_key(key(KeyCode::Tab));

    assert_eq!(
        app.tab().input_text(),
        format!("{}/Downloads/", dir.display()),
        "a unique match completes, with the trailing slash"
    );
}

#[test]
fn tab_extends_to_the_shared_prefix_when_ambiguous() {
    let dir = std::env::temp_dir().join("sa-tui-complete-ambiguous");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dirs");
    std::fs::write(dir.join("ab-one.txt"), b"x").expect("write");
    std::fs::write(dir.join("ab-two.txt"), b"x").expect("write");

    let mut app = App::new("session-abcdef", "m");
    type_text(&mut app, &format!("{}/a", dir.display()));
    app.on_key(key(KeyCode::Tab));

    assert_eq!(
        app.tab().input_text(),
        format!("{}/ab-", dir.display()),
        "ambiguous input extends to the shared prefix"
    );
}

#[test]
fn tab_without_a_match_leaves_the_text_alone() {
    let mut app = App::new("session-abcdef", "m");
    type_text(&mut app, "hello");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.tab().input_text(), "hello");
}

#[test]
fn tab_completion_keeps_the_home_form() {
    // home is process-global, so this asserts the form is preserved rather than which
    // entry wins. the absolute-path tests cover the matching itself.
    let mut app = App::new("session-abcdef", "m");
    type_text(&mut app, "~/Down");
    app.on_key(key(KeyCode::Tab));
    let text = app.tab().input_text();
    assert!(
        text.starts_with("~/"),
        "the operator's home form must survive completion: {text}"
    );
    assert!(!text.contains("//"), "no doubled slashes: {text}");
}

#[test]
fn ctrl_k_cuts_to_the_end_of_the_line() {
    let mut app = App::new("session-abcdef", "m");
    type_text(&mut app, "hello world");
    for _ in 0..6 {
        app.on_key(key(KeyCode::Left));
    }
    app.on_key(ctrl('k'));
    assert_eq!(app.tab().input_text(), "hello", "cut to end of line");

    // ctrl+y is the editor's paste, which reads the system clipboard rather than a kill
    // ring, so it is only asserted to leave the text intact when there is nothing to paste.
    app.on_key(ctrl('y'));
    assert!(
        app.tab().input_text().starts_with("hello"),
        "paste must not lose the line"
    );
}

#[test]
fn ctrl_u_deletes_to_the_start_of_the_line() {
    let mut app = App::new("session-abcdef", "m");
    type_text(&mut app, "hello world");
    for _ in 0..5 {
        app.on_key(key(KeyCode::Left));
    }
    app.on_key(ctrl('u'));
    assert_eq!(
        app.tab().input_text(),
        "world",
        "only the part before the cursor goes"
    );
}

#[test]
fn ctrl_a_and_ctrl_e_move_to_the_line_ends() {
    let mut app = App::new("session-abcdef", "m");
    type_text(&mut app, "abcdef");
    app.on_key(ctrl('a'));
    app.on_key(ctrl('k'));
    assert_eq!(app.tab().input_text(), "", "ctrl+a reached the start");

    type_text(&mut app, "xyz");
    app.on_key(ctrl('a'));
    app.on_key(ctrl('e'));
    type_text(&mut app, "!");
    assert_eq!(app.tab().input_text(), "xyz!", "ctrl+e reached the end");
}

#[test]
fn undo_steps_back_what_was_typed() {
    let mut app = App::new("session-abcdef", "m");
    type_text(&mut app, "hello");

    // the editor owns its undo history: one press steps back one edit, and repeated
    // presses walk back to empty.
    app.on_key(ctrl('-'));
    let after_one = app.tab().input_text();
    assert!(
        after_one.len() < "hello".len(),
        "undo must step back: {after_one:?}"
    );

    for _ in 0..16 {
        app.on_key(ctrl('-'));
    }
    assert_eq!(
        app.tab().input_text(),
        "",
        "enough undo empties the composer"
    );
}

#[test]
fn ctrl_z_also_undoes() {
    let mut app = App::new("session-abcdef", "m");
    type_text(&mut app, "abc");
    app.on_key(ctrl('z'));
    assert!(
        app.tab().input_text().len() < 3,
        "ctrl+z must undo, got {:?}",
        app.tab().input_text()
    );
}

#[test]
fn undo_restores_text_removed_by_a_cut() {
    let mut app = App::new("session-abcdef", "m");
    type_text(&mut app, "hello world");
    for _ in 0..6 {
        app.on_key(key(KeyCode::Left));
    }
    app.on_key(ctrl('k'));
    assert_eq!(app.tab().input_text(), "hello");

    app.on_key(ctrl('-'));
    assert_eq!(app.tab().input_text(), "hello world", "the cut is undone");
}

#[test]
fn undo_with_nothing_recorded_is_harmless() {
    let mut app = App::new("session-abcdef", "m");
    app.on_key(ctrl('-'));
    assert_eq!(app.tab().input_text(), "");
}

#[test]
fn the_status_bar_shows_the_working_directory_not_the_session_id() {
    let mut app = App::new("session-abcdef", "m");
    app.tab_mut().cwd = "/work/project".to_owned();

    let backend = TestBackend::new(160, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let width = 160;
    let status: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .skip(23 * width)
        .map(ratatui::buffer::Cell::symbol)
        .collect();
    assert!(status.contains("/work/project"), "status row:\n{status}");
    assert!(
        !status.contains("session-"),
        "the session id belongs to the tab strip, not the status row:\n{status}"
    );
}

#[test]
fn a_cwd_event_repoints_the_tab() {
    let mut app = App::new("session-abcdef", "m");
    app.tab_mut()
        .on_event("session.cwd", &json!({"cwd": "/work/elsewhere"}));
    assert_eq!(app.tab().cwd, "/work/elsewhere");
}

#[test]
fn tab_completes_a_directory_argument_to_cwd_and_suggests_the_rest() {
    let dir = std::env::temp_dir().join("sa-tui-cwd-complete");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Downloads")).expect("dirs");

    let mut app = App::new("session-abcdef", "m");
    type_text(&mut app, &format!("/cwd {}/Down", dir.display()));
    assert_eq!(
        app.tab().suggestion().as_deref(),
        Some("loads/"),
        "the rest of the completion is offered before it is accepted"
    );

    app.on_key(key(KeyCode::Tab));
    assert_eq!(
        app.tab().input_text(),
        format!("/cwd {}/Downloads/", dir.display()),
        "the argument completes to a directory with a trailing slash"
    );
}

#[test]
fn cwd_completion_never_offers_a_file() {
    let dir = std::env::temp_dir().join("sa-tui-cwd-files");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dirs");
    std::fs::write(dir.join("notes.txt"), b"x").expect("file");

    let mut app = App::new("session-abcdef", "m");
    let typed = format!("/cwd {}/no", dir.display());
    type_text(&mut app, &typed);
    app.on_key(key(KeyCode::Tab));

    assert_eq!(app.tab().input_text(), typed, "a file is not a directory");
}

#[test]
fn a_sent_image_is_drawn_inside_the_user_message() {
    // a 1x1 png, which is enough to prove the picture lands in the frame.
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    let mut app = App::new("session-abcdef", "m");
    app.tab_mut().on_event(
        "turn.started",
        &json!({
            "prompt": "look at this",
            "images": [{"media_type": "image/png", "data": PNG, "name": "shot.png"}],
        }),
    );

    let rendered = flatten(&transcript_lines(app.tab(), 80));
    assert!(rendered.contains("look at this"), "{rendered}");
    assert!(
        rendered.contains("shot.png"),
        "the picture is labelled:\n{rendered}"
    );
    assert!(
        rendered.contains('\u{2580}'),
        "the picture is drawn in the message:\n{rendered}"
    );
}

#[test]
fn the_strip_keeps_a_tab_for_every_live_session() {
    let mut app = App::new("one", "m");
    app.add_tab("two", "m");
    app.add_tab("three", "m");
    app.switch_tab(1);

    // a session deleted elsewhere loses its tab; the rest keep theirs.
    let survived = app.retain_tabs(&["one", "three"]);
    assert!(!survived, "the active tab was the deleted one");
    let live: Vec<&str> = app.tabs.iter().map(|tab| tab.session.as_str()).collect();
    assert_eq!(live, ["one", "three"]);

    // and the tab that stayed active is still the one the operator was on.
    app.switch_tab(1);
    assert!(
        app.retain_tabs(&["one", "three"]),
        "an untouched tab survives"
    );
    assert_eq!(app.tab().session, "three");
}

#[test]
fn a_tab_label_always_keeps_the_session_id() {
    assert_eq!(
        sa_tui::app::tab_label("session-abcdef", "fix the parser"),
        "fix the parser (session-)"
    );
}

/// the tab strip row, as drawn.
fn strip(app: &mut App, width: u16) -> String {
    let backend = TestBackend::new(width, 6);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, app).expect("draw");
    let buffer = terminal.backend().buffer();
    (0..width).map(|x| buffer[(x, 0)].symbol()).collect()
}

fn many_tabs() -> App {
    let mut app = App::new("s0000001", "m");
    for n in 2..=30 {
        app.add_tab(format!("s{n:07}"), "m");
    }
    app
}

#[test]
fn the_tab_strip_slides_to_the_active_tab_at_the_end() {
    let mut app = many_tabs();
    app.switch_tab(29);
    let row = strip(&mut app, 80);

    assert!(
        row.contains("30 (s0000030)"),
        "active tab must be shown:\n{row}"
    );
    assert!(
        row.contains('\u{2039}'),
        "a left marker must say tabs are hidden before it:\n{row}"
    );
    assert!(
        !row.contains('\u{203a}'),
        "nothing is hidden to the right:\n{row}"
    );
    assert!(
        !row.contains("1 (s0000001)"),
        "the first tab scrolled off:\n{row}"
    );
}

#[test]
fn the_tab_strip_marks_hidden_tabs_on_the_right() {
    let mut app = many_tabs();
    app.switch_tab(0);
    let row = strip(&mut app, 80);

    assert!(
        row.contains("1 (s0000001)"),
        "active tab must be shown:\n{row}"
    );
    assert!(
        row.contains('\u{203a}'),
        "a right marker must say tabs are hidden after it:\n{row}"
    );
    assert!(
        !row.contains('\u{2039}'),
        "nothing is hidden to the left:\n{row}"
    );
}

#[test]
fn the_tab_strip_marks_hidden_tabs_on_both_sides() {
    let mut app = many_tabs();
    app.switch_tab(14);
    let row = strip(&mut app, 80);

    assert!(
        row.contains("15 (s0000015)"),
        "active tab must be shown:\n{row}"
    );
    assert!(row.contains('\u{2039}'), "hidden to the left:\n{row}");
    assert!(row.contains('\u{203a}'), "hidden to the right:\n{row}");
}

#[test]
fn the_active_tab_is_on_screen_at_every_position_and_width() {
    let mut app = many_tabs();
    for active in 0..30 {
        app.switch_tab(active);
        let needle = format!("{} (s{:07})", active + 1, active + 1);
        for width in [40u16, 60, 80, 120] {
            let row = strip(&mut app, width);
            assert!(
                row.contains(&needle),
                "tab {} must be visible at width {width}:\n{row}",
                active + 1
            );
        }
    }
}

#[test]
fn the_tab_strip_never_draws_wider_than_its_room() {
    let mut app = many_tabs();
    for active in [0usize, 1, 14, 28, 29] {
        app.switch_tab(active);
        for width in [20u16, 40, 60, 80, 120, 200] {
            let drawn = sa_tui::ui::tab_bar(&app, width).width();
            assert!(
                drawn <= usize::from(width),
                "the strip drew {drawn} cells into {width}"
            );
        }
    }
}

#[test]
fn the_tab_strip_sits_flush_against_the_end_it_has_reached() {
    let mut app = many_tabs();

    // at the last tab the strip ends against the right edge, with no dead margin after it.
    app.switch_tab(29);
    let row = strip(&mut app, 80);
    assert!(
        row.trim_end().ends_with("30 (s0000030)"),
        "the strip must end on the last tab, not leave a gap:\n[{row}]"
    );

    // and at the first tab it starts against the left edge.
    app.switch_tab(0);
    let row = strip(&mut app, 80);
    assert!(
        row.starts_with(" 1 (s0000001)"),
        "the strip must start on the first tab:\n[{row}]"
    );
}

#[test]
fn the_tab_strip_has_no_new_tab_button() {
    let mut app = many_tabs();
    for active in [0usize, 14, 29] {
        app.switch_tab(active);
        let row = strip(&mut app, 80);
        assert!(
            !row.contains('+'),
            "no dead affordance in the strip:\n{row}"
        );
    }
}

#[test]
fn the_overflow_markers_carry_no_numbers() {
    let mut app = many_tabs();
    app.switch_tab(14);
    let row = strip(&mut app, 80);

    // a bare marker flush against the chip it borders, so a count can never read as
    // another tab number.
    assert!(
        row.starts_with("\u{2039}14 (s0000014)"),
        "the left marker must be bare and flush:\n{row}"
    );
    assert!(
        row.trim_end().ends_with("17 (s0000017)\u{203a}"),
        "the right marker must be bare and flush:\n{row}"
    );
}

#[test]
fn closing_a_tab_with_work_asks_before_throwing_it_away() {
    let mut app = App::new("session-abcdef", "m");
    app.add_tab("session-second", "m");
    app.tab_mut().push(Role::User, "hello");

    assert_eq!(app.on_key(ctrl('w')), Action::None, "it must ask first");
    assert!(app.confirm.is_some(), "a question must be on screen");

    let shown = screen(&mut app, 100, 20);
    assert!(shown.contains("close"), "the question is drawn:\n{shown}");
    assert!(shown.contains("y close"), "the answers are drawn:\n{shown}");

    // anything that is not yes or no leaves the question up.
    assert_eq!(app.on_key(key(KeyCode::Char('x'))), Action::None);
    assert!(app.confirm.is_some());

    assert_eq!(app.on_key(key(KeyCode::Char('y'))), Action::CloseTab);
    assert!(app.confirm.is_none(), "the question clears on answer");
}

#[test]
fn answering_no_keeps_the_tab_and_its_work() {
    let mut app = App::new("session-abcdef", "m");
    app.tab_mut().push(Role::Assistant, "some work");
    app.on_key(ctrl('w'));
    assert!(app.confirm.is_some());

    assert_eq!(app.on_key(key(KeyCode::Char('n'))), Action::None);
    assert!(app.confirm.is_none());
    assert_eq!(app.tabs.len(), 1, "the tab is still there");
    assert_eq!(app.tab().transcript.len(), 1, "and so is its work");

    // escape is the same answer.
    app.on_key(ctrl('w'));
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert!(app.confirm.is_none());
}

#[test]
fn closing_an_untouched_tab_does_not_ask() {
    let mut app = App::new("session-abcdef", "m");
    app.add_tab("session-second", "m");
    assert_eq!(app.on_key(ctrl('w')), Action::CloseTab);
    assert!(app.confirm.is_none());
}

#[test]
fn only_work_counts_as_content() {
    let mut app = App::new("s", "m");
    assert!(!app.tab().has_content(), "a fresh tab holds nothing");

    app.tab_mut().push(Role::System, "title: something");
    assert!(
        !app.tab().has_content(),
        "a notice the app wrote is not work"
    );

    typed(&mut app, "half a thought");
    assert!(app.tab().has_content(), "a typed prompt is work");
}

/// the status bar row, as drawn.
fn status_row(app: &mut App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, app).expect("draw");
    let buffer = terminal.backend().buffer();
    let start = usize::from(height - 1) * usize::from(width);
    buffer
        .content()
        .iter()
        .skip(start)
        .take(usize::from(width))
        .map(ratatui::buffer::Cell::symbol)
        .collect()
}

#[test]
fn the_meter_uses_the_window_the_model_advertises() {
    let mut app = App::new("session-abcdef", "big-model");
    app.served.insert(
        "big-model".to_owned(),
        ServedModel {
            context_limit: Some(1_000_000),
            effort_levels: Vec::new(),
        },
    );
    app.tab_mut().status.context_tokens = 90_000;

    let status = status_row(&mut app, 160, 24);
    assert!(
        status.contains("\u{2591} 9%"),
        "90k of a 1m window is 9%, one space after the bar:\n{status}"
    );
}

#[test]
fn the_meter_invents_no_percentage_without_a_window() {
    let mut app = App::new("session-abcdef", "silent-model");
    app.tab_mut().status.context_tokens = 90_000;

    let status = status_row(&mut app, 160, 24);
    assert!(
        !status.contains('%'),
        "no advertised window means no percentage:\n{status}"
    );
    assert!(
        status.contains("90k tokens"),
        "the measured figure is still shown:\n{status}"
    );
}

#[test]
fn the_effort_levels_shown_are_the_endpoints_own() {
    let mut app = App::new("session-abcdef", "deepseek");
    app.served.insert(
        "deepseek".to_owned(),
        ServedModel {
            context_limit: None,
            effort_levels: vec!["none".to_owned(), "high".to_owned()],
        },
    );

    assert_eq!(
        app.effort_levels(app.tab()),
        vec!["none".to_owned(), "high".to_owned()],
        "the endpoint's own values, in its own order"
    );

    // a model the endpoint said nothing about gets no invented list.
    app.tab_mut().status.model = "silent".to_owned();
    let levels = app.effort_levels(app.tab());
    assert!(
        levels.is_empty(),
        "no advertised levels means none: {levels:?}"
    );
}

#[test]
fn a_tab_standing_in_the_strip_still_needs_hydrating() {
    let mut app = App::new("session-abcdef", "m");

    // the app's own first slot: a tab object exists, but nothing has been replayed into it,
    // so it must still be hydrated or it stays deaf for the whole run.
    assert!(
        app.needs_hydration("session-abcdef"),
        "a tab with no history is not hydrated"
    );

    app.tab_mut().hydrated = true;
    assert!(!app.needs_hydration("session-abcdef"));
    assert!(app.needs_hydration("never-seen"));
}

#[test]
fn an_error_alone_is_not_work() {
    let mut app = App::new("session-abcdef", "m");
    app.add_tab("session-second", "m");
    app.tab_mut()
        .push(Role::Error, "rpc: session has no turn to steer");

    assert!(!app.tab().has_content(), "an error is not work");
    assert_eq!(
        app.on_key(ctrl('w')),
        Action::CloseTab,
        "closing a tab that only holds an error must not ask"
    );
}

#[test]
fn a_turn_started_anywhere_shows_as_running_but_a_replayed_one_does_not() {
    let mut app = App::new("s", "m");

    // history: a turn that finished long ago is not running now.
    app.tab_mut()
        .on_replay("turn.started", &json!({"prompt": "old"}));
    assert!(!app.tab().status.turn_running, "a replayed turn is history");

    // live: another window starting a turn is a running turn here too.
    app.tab_mut()
        .on_event("turn.started", &json!({"prompt": "now"}));
    assert!(app.tab().status.turn_running, "a live turn is running");
}

const fn shift_enter() -> KeyEvent {
    KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)
}

#[test]
fn shift_enter_makes_a_new_line_instead_of_sending() {
    let mut app = App::new("session-abcdef", "m");
    typed(&mut app, "first");

    assert_eq!(
        app.on_key(shift_enter()),
        Action::None,
        "shift+enter must not send"
    );
    assert_eq!(app.tab().input_text(), "first\n");

    typed(&mut app, "second");
    assert_eq!(app.tab().input_text(), "first\nsecond");

    // a plain enter sends the whole thing, line break and all.
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit("first\nsecond".to_owned())
    );
    assert_eq!(app.tab().input_text(), "");
}

#[test]
fn the_cursor_tracks_a_multi_line_prompt() {
    let mut app = App::new("session-abcdef", "m");
    typed(&mut app, "one");
    app.on_key(shift_enter());
    typed(&mut app, "two");

    let (lines, line, column) = app.tab().input_lines();
    assert_eq!(lines, ["one", "two"]);
    assert_eq!(
        (line, column),
        (1, 3),
        "the cursor is at the end of line two"
    );

    // back to the start of the second line: a break there opens a blank line above it.
    for _ in 0..3 {
        app.on_key(key(KeyCode::Left));
    }
    app.on_key(shift_enter());
    let (lines, line, column) = app.tab().input_lines();
    assert_eq!(lines, ["one", "", "two"]);
    assert_eq!(
        (line, column),
        (2, 0),
        "the line that was pushed down keeps the cursor below the new break"
    );
}

#[test]
fn a_multi_line_prompt_is_drawn_on_separate_rows() {
    let mut app = App::new("session-abcdef", "m");
    typed(&mut app, "qqq");
    app.on_key(shift_enter());
    typed(&mut app, "www");
    app.on_key(shift_enter());
    typed(&mut app, "eee");

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();

    // the transcript is empty and the strip and status bar are outside this band, so any
    // of these letters inside it can only be composer text.
    let row_of =
        |target: &str| (2..22u16).find(|&y| (0..80u16).any(|x| buffer[(x, y)].symbol() == target));
    let (q, w, e) = (
        row_of("q").expect("first line"),
        row_of("w").expect("second line"),
        row_of("e").expect("third line"),
    );
    assert!(
        q < w && w < e,
        "each line must get its own row: {q} {w} {e}"
    );
}

#[test]
fn the_effort_picker_returns_the_level_that_was_chosen() {
    let mut app = App::new("session-abcdef", "deepseek");
    app.tab_mut().status.effort = "none".to_owned();
    app.served.insert(
        "deepseek".to_owned(),
        ServedModel {
            context_limit: None,
            effort_levels: vec!["none".to_owned(), "high".to_owned()],
        },
    );

    let levels = app.effort_levels(app.tab());
    let current = app.tab().status.effort.clone();
    let items: Vec<PickerItem> = levels
        .into_iter()
        .map(|level| PickerItem {
            label: if level == current {
                format!("{level} (current)")
            } else {
                level.clone()
            },
            id: level,
        })
        .collect();
    app.open_picker(PickerKind::Effort, "effort", items);

    let shown = screen(&mut app, 100, 20);
    assert!(
        shown.contains("none (current)"),
        "the current level is marked:\n{shown}"
    );

    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::EffortPicked("high".to_owned())
    );
    assert!(app.picker.is_none(), "the picker closes on choice");
}

#[test]
fn a_newline_never_needs_a_terminal_protocol() {
    let mut app = App::new("session-abcdef", "m");
    typed(&mut app, "a");

    // ctrl+j arrives as a bare newline: it must not send.
    assert_eq!(app.on_key(key(KeyCode::Char('\n'))), Action::None);
    typed(&mut app, "b");

    // and as a key event, for terminals that report it that way.
    assert_eq!(
        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL)),
        Action::None
    );
    typed(&mut app, "c");

    // shift+enter too, where the terminal can report it.
    assert_eq!(app.on_key(shift_enter()), Action::None);
    typed(&mut app, "d");

    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit("a\nb\nc\nd".to_owned()),
        "enter alone sends the whole thing"
    );
}

#[test]
fn messages_are_separated_by_a_blank_row() {
    let mut app = App::new("s", "m");
    app.tab_mut().push(Role::User, "a question");
    app.tab_mut().push(Role::Assistant, "an answer");

    let lines = sa_tui::render(&app, 80);
    let widths: Vec<usize> = lines.iter().map(ratatui::prelude::Line::width).collect();
    let blank = widths.iter().position(|width| *width == 0);
    assert!(
        blank.is_some(),
        "frames must not sit against each other: {widths:?}"
    );

    // and it is between them, not trailing at the end.
    let at = blank.expect("a blank row");
    assert!(
        at > 0 && at < widths.len() - 1,
        "the gap belongs between frames"
    );
}

#[test]
fn a_tiny_share_is_shown_to_a_tenth_so_the_meter_can_move() {
    let mut app = App::new("session-abcdef", "big-model");
    app.served.insert(
        "big-model".to_owned(),
        ServedModel {
            context_limit: Some(1_048_576),
            effort_levels: Vec::new(),
        },
    );
    app.tab_mut().status.context_tokens = 918;

    let status = status_row(&mut app, 160, 24);
    assert!(
        status.contains("0.1%"),
        "918 of a million is 0.1%, not a bare 0%:\n{status}"
    );
    assert!(
        status.contains("\u{2591} 0.1%"),
        "one space between the bar and the figure, no padding:\n{status}"
    );
}

#[test]
fn a_tool_call_appears_without_waiting_for_its_result() {
    let mut app = App::new("session-abcdef", "m");
    app.tab_mut().push(Role::User, "run it");
    // draw once so the transcript render cache is populated
    let before = screen(&mut app, 100, 24);
    assert!(!before.contains("sleep 5"), "nothing yet:\n{before}");

    app.tab_mut().on_event(
        "tool.requested",
        &json!({"tool": "bash", "args": {"command": "sleep 5"}}),
    );
    let after = screen(&mut app, 100, 24);
    assert!(
        after.contains("sleep 5"),
        "the invocation must appear while the command runs, not after it:\n{after}"
    );
}

#[test]
fn repeated_effort_levels_are_offered_once() {
    let mut app = App::new("session-abcdef", "m");
    app.served.insert(
        "m".to_owned(),
        ServedModel {
            context_limit: None,
            effort_levels: vec![
                "none".to_owned(),
                "low".to_owned(),
                "low".to_owned(),
                "high".to_owned(),
                "high".to_owned(),
                "max".to_owned(),
            ],
        },
    );

    assert_eq!(
        app.effort_levels(app.tab()),
        ["none", "low", "high", "max"],
        "a value repeated by the server is not offered twice"
    );
}

#[test]
fn escape_reverts_the_view_at_once() {
    let mut app = App::new("session-abcdef", "m");
    app.tab_mut().status.turn_running = true;
    app.tab_mut().working = Some("pondering".to_owned());
    app.tab_mut().tool_running = true;

    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Cancel);
    assert!(!app.tab().status.turn_running, "the spinner stops at once");
    assert!(app.tab().working.is_none(), "the header clears at once");
    assert!(!app.tab().tool_running, "a running tool is not still shown");
}

#[test]
fn the_working_label_hides_while_a_tool_runs() {
    let mut app = App::new("session-abcdef", "m");
    app.tab_mut().status.turn_running = true;
    app.tab_mut().working = Some("pondering".to_owned());

    let thinking = screen(&mut app, 100, 24);
    assert!(thinking.contains("pondering"), "{thinking}");

    // the model is not thinking, it is waiting for a command.
    app.tab_mut().on_event(
        "tool.requested",
        &json!({"tool": "bash", "args": {"command": "ls"}}),
    );
    let waiting = screen(&mut app, 100, 24);
    assert!(
        !waiting.contains("pondering"),
        "the label hides:\n{waiting}"
    );

    app.tab_mut()
        .on_event("tool.completed", &json!({"output": "ok", "exit_code": 0}));
    let after = screen(&mut app, 100, 24);
    assert!(
        after.contains("pondering"),
        "and comes back after it:\n{after}"
    );
}

#[test]
fn the_composer_grows_when_a_newline_is_added() {
    let mut app = App::new("session-abcdef", "m");
    typed(&mut app, "one line");

    let single = screen(&mut app, 80, 24);
    assert!(single.contains("one line"), "{single}");
    assert!(!single.contains("second"), "not typed yet:\n{single}");

    app.on_key(shift_enter());
    typed(&mut app, "second");
    let (lines, _, _) = app.tab().input_lines();
    assert_eq!(lines.len(), 2, "the newline is in the composer");

    // the box grows to show both lines: the second is only visible if it does.
    let grown = screen(&mut app, 80, 24);
    assert!(grown.contains("one line"), "{grown}");
    assert!(
        grown.contains("second"),
        "the composer must grow to show the new line:\n{grown}"
    );
}

#[test]
fn typing_a_cwd_argument_lists_every_directory_that_matches() {
    let dir = std::env::temp_dir().join("sa-tui-cwd-list");
    let _ = std::fs::remove_dir_all(&dir);
    for name in ["android_device_phh_treble", "api", "beta"] {
        std::fs::create_dir_all(dir.join(name)).expect("dirs");
    }

    let mut app = App::new("session-abcdef", "m");
    type_text(&mut app, &format!("/cwd {}/a", dir.display()));

    let listed = app.tab().cwd_suggestions();
    assert_eq!(
        listed,
        [
            format!("{}/android_device_phh_treble/", dir.display()),
            format!("{}/api/", dir.display()),
        ],
        "every a* directory, in the form typed"
    );

    // and they are shown while typing, not only on tab.
    let shown = screen(&mut app, 120, 24);
    assert!(shown.contains("android_device_phh_treble"), "{shown}");
    assert!(shown.contains("api"), "{shown}");
    assert!(
        !shown.contains("beta"),
        "only matches are offered:\n{shown}"
    );
}

#[test]
fn a_cwd_prefix_that_matches_one_directory_still_completes_on_tab() {
    let dir = std::env::temp_dir().join("sa-tui-cwd-single");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Projects")).expect("dirs");

    let mut app = App::new("session-abcdef", "m");
    type_text(&mut app, &format!("/cwd {}/Pro", dir.display()));
    assert_eq!(
        app.tab().cwd_suggestions(),
        [format!("{}/Projects/", dir.display())]
    );

    app.on_key(key(KeyCode::Tab));
    assert_eq!(
        app.tab().input_text(),
        format!("/cwd {}/Projects/", dir.display())
    );
}

/// a terminal that sends a bare enter for shift+enter still reports the shift key itself.
#[test]
fn a_bare_enter_while_shift_is_held_makes_a_new_line() {
    let mut app = App::new("session-abcdef", "m");
    typed(&mut app, "one");

    let shift = |kind| {
        KeyEvent::new_with_kind(
            KeyCode::Modifier(ModifierKeyCode::LeftShift),
            KeyModifiers::SHIFT,
            kind,
        )
    };

    // shift goes down, then the terminal sends a plain enter.
    app.on_key(shift(KeyEventKind::Press));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::None,
        "enter while shift is held must not send"
    );
    typed(&mut app, "two");
    assert_eq!(app.tab().input_text(), "one\ntwo");

    // shift goes up: enter sends again.
    app.on_key(shift(KeyEventKind::Release));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit("one\ntwo".to_owned())
    );
}

/// with the event types reported, a release must not type the key a second time.
#[test]
fn key_releases_are_ignored() {
    let mut app = App::new("session-abcdef", "m");
    typed(&mut app, "ab");
    let release = KeyEvent::new_with_kind(
        KeyCode::Char('c'),
        KeyModifiers::NONE,
        KeyEventKind::Release,
    );
    assert_eq!(app.on_key(release), Action::None);
    assert_eq!(app.tab().input_text(), "ab", "a release types nothing");
}

#[test]
fn a_stall_drops_the_rate_to_zero_instead_of_holding_it() {
    use std::time::{Duration, Instant};
    let mut app = App::new("s", "m");
    let t0 = Instant::now();
    app.tab_mut().status.turn_running = true;
    app.tab_mut()
        .on_event_at("turn.started", &json!({"prompt": "go"}), t0);
    for step in 0..2 {
        app.tab_mut().on_event_at(
            "llm.delta",
            &json!({"text": "x".repeat(400)}),
            t0 + Duration::from_millis(step * 200),
        );
    }
    app.tab_mut().tick_at(t0 + Duration::from_millis(300));
    assert!(app.tab().tok_per_sec > 0.0, "a rate while text arrives");

    // a full window with nothing: the model is not generating, so the meter says so.
    app.tab_mut().tick_at(t0 + Duration::from_secs(5));
    assert_eq!(
        app.tab().tok_per_sec,
        0.0,
        "a stall is zero, not the last figure held forever"
    );
}

#[test]
fn the_rate_estimate_counts_full_width_text_as_one_token_a_character() {
    use std::time::{Duration, Instant};
    let mut app = App::new("s", "m");
    let t0 = Instant::now();
    app.tab_mut().status.turn_running = true;
    app.tab_mut()
        .on_event_at("turn.started", &json!({"prompt": "go"}), t0);
    // one wide character is the baseline; 99 more arrive over the next second. counted as
    // four characters to the token that would read 25, so this is what tells them apart.
    app.tab_mut()
        .on_event_at("llm.delta", &json!({"text": "漢"}), t0);
    app.tab_mut().on_event_at(
        "llm.delta",
        &json!({"text": "漢".repeat(99)}),
        t0 + Duration::from_millis(1000),
    );
    app.tab_mut().tick_at(t0 + Duration::from_millis(1000));
    let rate = app.tab().tok_per_sec;
    assert!(
        (80.0..=110.0).contains(&rate),
        "99 wide tokens in a second, got {rate}"
    );
}

#[test]
fn the_context_count_follows_the_turn_as_it_grows() {
    let mut app = App::new("s", "m");
    app.tab_mut()
        .on_event("turn.context", &json!({"tokens": 12_345}));
    assert_eq!(
        app.tab().status.context_tokens,
        12_345,
        "the meter must move while the turn runs, not only when it ends"
    );
}

#[test]
fn typing_an_effort_lists_and_completes_the_levels_the_endpoint_offers() {
    let mut app = App::new("s", "m");
    app.served.insert(
        "m".to_owned(),
        ServedModel {
            context_limit: Some(1000),
            effort_levels: vec!["none".to_owned(), "low".to_owned(), "high".to_owned()],
        },
    );

    typed(&mut app, "/effort h");
    let levels = app.effort_levels(app.tab());
    assert_eq!(
        app.tab().effort_suggestions(&levels),
        ["high"],
        "only the matching level is offered"
    );

    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.tab().input_text(), "/effort high");

    typed(&mut app, "x");
    assert_eq!(app.tab().input_text(), "/effort highx");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(
        app.tab().input_text(),
        "/effort highx",
        "nothing matches, so tab leaves the line alone"
    );
}

#[test]
fn a_busy_tab_animates_the_composer_edge() {
    use ratatui::style::Color;
    let mut app = App::new("session-abcdef", "m");
    app.tab_mut().status.turn_running = true;
    app.spinner = 2;

    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();

    let accent = Color::Rgb(0x8a, 0xc8, 0xff);
    let lit = buffer
        .content()
        .iter()
        .filter(|cell| cell.fg == accent && cell.symbol() == "\u{2500}")
        .count();
    assert!(
        lit > 0,
        "the loading run must be drawn on the composer edge"
    );
}

#[test]
fn the_context_listing_is_grouped_by_root_and_sorted() {
    let home = std::env::var("HOME").expect("home");
    let groups = json!([
        {"root": home, "files": [
            {"path": format!("{home}/b/AGENTS.md"), "relative": "b/AGENTS.md", "tokens": 12},
            {"path": format!("{home}/a/PERSONALITY.md"), "relative": "a/PERSONALITY.md", "tokens": 34},
            {"path": format!("{home}/c/AGENTS.md"), "relative": "c/AGENTS.md", "tokens": 7},
            {"path": format!("{home}/c/RULES.md"), "relative": "c/RULES.md", "tokens": 3},
        ]},
        {"root": format!("{home}/.dotfiles"), "files": [
            {"path": format!("{home}/.dotfiles/AGENTS.md"), "relative": "AGENTS.md", "tokens": 5},
        ]},
    ]);
    let text = sa_tui::app::context_summary(&home, groups.as_array().expect("groups"));

    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "Files loaded into context:", "{text}");
    assert_eq!(lines[1], "", "a blank line before the tree");
    assert_eq!(lines[2], "\u{f07b} ~", "the scope, as a folder");
    // a directory holding one file is that file's path on one line.
    assert_eq!(
        lines[3], "\u{251c}\u{2500} \u{f15c} a/PERSONALITY.md (34 tokens)",
        "one file folds into its directory:\n{text}"
    );
    assert_eq!(
        lines[4], "\u{251c}\u{2500} \u{f15c} b/AGENTS.md (12 tokens)",
        "{text}"
    );
    // a directory holding more than one keeps its own line.
    assert_eq!(lines[5], "\u{251c}\u{2500} \u{f07b} c", "{text}");
    assert_eq!(
        lines[6], "\u{2502}  \u{251c}\u{2500} \u{f15c} AGENTS.md (7 tokens)",
        "{text}"
    );
    assert_eq!(
        lines[7], "\u{2502}  \u{2514}\u{2500} \u{f15c} RULES.md (3 tokens)",
        "{text}"
    );
    assert_eq!(
        lines[8], "\u{2514}\u{2500} \u{f15c} .dotfiles/AGENTS.md (5 tokens)",
        "{text}"
    );
    assert_eq!(lines[9], "", "{text}");
    assert_eq!(lines[10], "Total context added: 61 tokens", "{text}");
}

#[test]
fn the_context_listing_is_shown_when_a_session_starts() {
    let home = std::env::var("HOME").expect("home");
    let mut app = App::new("session-abcdef", "m");
    app.tab_mut().on_replay(
        "context.loaded",
        &json!({
            "cwd": home,
            "groups": [{"root": home, "files": [
                {"path": format!("{home}/AGENTS.md"), "relative": "AGENTS.md", "tokens": 9},
                {"path": format!("{home}/PERSONALITY.md"), "relative": "PERSONALITY.md", "tokens": 4},
            ]}],
        }),
    );
    let shown = screen(&mut app, 100, 24);
    assert!(shown.contains("Files loaded into context:"), "{shown}");
    assert!(shown.contains("Total context added: 13 tokens"), "{shown}");
    assert!(shown.contains("\u{f15c} AGENTS.md (9 tokens)"), "{shown}");

    // the listing is laid out by us, so each of its lines stays on its own row: markdown
    // folding them into one paragraph is the regression this catches.
    let rows = rows(&mut app, 100, 24);
    let row_of = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle} is not on screen:\n{}", rows.join("\n")))
    };
    assert_ne!(
        row_of("AGENTS.md"),
        row_of("PERSONALITY.md"),
        "two entries must not share a row:\n{}",
        rows.join("\n")
    );
    assert!(
        row_of("Total context added:") > row_of("PERSONALITY.md"),
        "the total sits below the listing:\n{}",
        rows.join("\n")
    );
}

#[test]
fn cd_offers_the_same_completion_as_cwd() {
    let dir = std::env::temp_dir().join("sa-tui-cd-completion");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Projects")).expect("dirs");

    let mut app = App::new("session-abcdef", "m");
    type_text(&mut app, &format!("/cd {}/Pro", dir.display()));
    assert_eq!(
        app.tab().cwd_suggestions(),
        [format!("{}/Projects/", dir.display())],
        "the list appears for /cd as well"
    );

    app.on_key(key(KeyCode::Tab));
    assert_eq!(
        app.tab().input_text(),
        format!("/cd {}/Projects/", dir.display()),
        "and it completes without rewriting the command to /cwd"
    );
}

#[test]
fn a_chain_of_single_directories_is_folded_into_one_line() {
    let home = std::env::var("HOME").expect("home");
    let deep = format!("{home}/.config/stuffies-agent/workspaces/home-chrisaw");
    let groups = json!([
        {"root": home, "files": [
            {"path": format!("{deep}/AGENTS.md"), "relative": "config/stuffies-agent/workspaces/home-chrisaw/AGENTS.md", "tokens": 10},
            {"path": format!("{deep}/skills/ponytail/SKILL.md"), "relative": "config/stuffies-agent/workspaces/home-chrisaw/skills/ponytail/SKILL.md", "tokens": 20},
        ]},
    ]);
    let text = sa_tui::app::context_summary(&home, groups.as_array().expect("groups"));
    let lines: Vec<&str> = text.lines().collect();

    assert_eq!(lines[2], "\u{f07b} ~", "{text}");
    assert_eq!(
        lines[3], "\u{2514}\u{2500} \u{f07b} config/stuffies-agent/workspaces/home-chrisaw",
        "one directory per line, not one per level:\n{text}"
    );
    assert_eq!(
        lines[4], "   \u{251c}\u{2500} \u{f15c} AGENTS.md (10 tokens)",
        "{text}"
    );
    // and a directory holding one file is that file's path, with the file's own icon.
    assert_eq!(
        lines[5], "   \u{2514}\u{2500} \u{f15c} skills/ponytail/SKILL.md (20 tokens)",
        "{text}"
    );
}

#[test]
fn a_directory_with_several_entries_is_still_expanded() {
    let home = std::env::var("HOME").expect("home");
    let groups = json!([
        {"root": home, "files": [
            {"path": format!("{home}/a/AGENTS.md"), "relative": "a/AGENTS.md", "tokens": 1},
            {"path": format!("{home}/a/RULES.md"), "relative": "a/RULES.md", "tokens": 2},
        ]},
    ]);
    let text = sa_tui::app::context_summary(&home, groups.as_array().expect("groups"));
    assert!(
        text.contains("a\n"),
        "the folder stays on its own line:\n{text}"
    );
    assert!(text.contains("AGENTS.md (1 tokens)"), "{text}");
    assert!(text.contains("RULES.md (2 tokens)"), "{text}");
}

#[test]
fn a_file_deployed_elsewhere_names_the_directory_it_lives_in() {
    let home = std::env::var("HOME").expect("home");
    let deployed = format!("{home}/.config/stuffies-agent/workspaces/home-chrisaw-.dotfiles");
    let groups = json!([
        {"root": format!("{home}/.dotfiles"), "files": [
            {
                "path": format!("{deployed}/AGENTS.md"),
                "relative": "AGENTS.md",
                "provider": deployed,
                "tokens": 7,
            },
        ]},
    ]);
    let text = sa_tui::app::context_summary(&home, groups.as_array().expect("groups"));
    let lines: Vec<&str> = text.lines().collect();

    assert_eq!(lines[2], "\u{f07b} ~/.dotfiles", "{text}");
    assert_eq!(
        lines[3],
        "\u{2514}\u{2500} \u{f15c} ~/.config/stuffies-agent/workspaces/home-chrisaw-.dotfiles/AGENTS.md (7 tokens)",
        "the deployed path is named in full, on its own line:\n{text}"
    );
}

#[test]
fn a_markdown_table_is_drawn_as_a_table() {
    let mut app = App::new("s", "m");
    app.tab_mut().push(
        Role::Assistant,
        "before\n| name | tokens |\n| --- | ---: |\n| AGENTS.md | 1074 |\n| SOUL.md | 240 |\nafter",
    );
    let shown = screen(&mut app, 100, 30);

    assert!(shown.contains("before"), "{shown}");
    assert!(
        shown.contains("after"),
        "text after the table survives:\n{shown}"
    );
    assert!(
        !shown.contains("| --- |"),
        "the separator is not shown raw:\n{shown}"
    );
    assert!(
        shown.contains('\u{250c}'),
        "a top border is drawn:\n{shown}"
    );
    assert!(
        shown.contains('\u{2518}'),
        "a bottom border is drawn:\n{shown}"
    );
    assert!(shown.contains("AGENTS.md"), "{shown}");
    assert!(shown.contains("1074"), "{shown}");
}

#[test]
fn pipes_in_ordinary_text_are_left_alone() {
    let mut app = App::new("s", "m");
    app.tab_mut()
        .push(Role::Assistant, "run a | b to pipe, not a table");
    let shown = screen(&mut app, 100, 20);
    assert!(shown.contains("run a | b to pipe, not a table"), "{shown}");
    assert!(!shown.contains('\u{250c}'), "{shown}");
}

#[test]
fn the_activity_bar_never_strikes_through_the_working_label() {
    let mut app = App::new("session-abcdef", "m");
    app.tab_mut().status.turn_running = true;
    app.tab_mut().working = Some("working".to_owned());
    for spinner in 0..40 {
        app.spinner = spinner;
        let screen = screen(&mut app, 100, 20);
        assert!(
            screen.contains("working\u{2026}"),
            "the moving run overwrote the label at spinner {spinner}:\n{screen}"
        );
    }
}

#[test]
fn a_message_names_the_model_and_effort_that_produced_it() {
    let mut app = App::new("session-abcdef", "m");
    app.tab_mut().apply_session_row(&json!({
        "id": "session-abcdef",
        "title": "work",
        "state": "active",
        "cwd": "/tmp",
        "model": "deepseek-v4-1-flash-1017-ablit",
        "effort": "high",
        "created_at": 1,
        "updated_at": 2,
    }));
    app.tab_mut().on_event(
        "turn.started",
        &json!({
            "prompt": "hello",
            "model": "deepseek-v4-1-flash-1017-ablit",
            "effort": "low",
        }),
    );
    app.tab_mut().on_event("llm.delta", &json!({"text": "hi"}));

    let shown = screen(&mut app, 120, 24);
    assert!(
        shown.contains("deepseek-v4-1-flash-1017-ablit:low"),
        "the message names the effort it ran at:\n{shown}"
    );
}

#[test]
fn a_replayed_message_keeps_the_effort_it_ran_at() {
    let mut app = App::new("session-abcdef", "m");
    app.tab_mut().on_replay(
        "turn.started",
        &json!({"prompt": "hello", "model": "m1", "effort": "none"}),
    );
    app.tab_mut().on_replay("llm.delta", &json!({"text": "hi"}));
    // the session has since moved to a different effort.
    app.tab_mut().apply_session_row(&json!({
        "id": "session-abcdef",
        "title": "work",
        "state": "active",
        "cwd": "/tmp",
        "model": "m1",
        "effort": "max",
        "created_at": 1,
        "updated_at": 2,
    }));

    let shown = screen(&mut app, 120, 24);
    assert!(
        shown.contains("m1:none"),
        "history keeps the effort it ran at, not the session's current one:\n{shown}"
    );
}

#[test]
fn a_message_carries_its_role_colour_in_the_text_and_the_frame() {
    let mut app = App::new("session-abcdef", "m");
    app.tab_mut().push(Role::User, "refactor the parser");
    app.tab_mut()
        .push(Role::Assistant, "the parser splits on ; at line 40");

    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    sa_tui::draw(&mut terminal, &mut app).expect("draw");
    let buffer = terminal.backend().buffer();

    // the first cell of each message's text, and the frame it sits in.
    let colour_of = |needle: &str| -> (ratatui::style::Color, ratatui::style::Color) {
        for y in 0..24 {
            let row: String = (0..120).map(|x| buffer[(x, y)].symbol()).collect();
            if let Some(at) = row.find(needle) {
                #[allow(clippy::cast_possible_truncation)]
                let cell = &buffer[(at as u16, y)];
                return (cell.fg, cell.bg);
            }
        }
        panic!("{needle} is not on screen");
    };

    let (user_text, user_bg) = colour_of("refactor");
    let (reply_text, reply_bg) = colour_of("the parser splits");
    assert_ne!(user_text, reply_text, "each role has its own colour");
    assert_ne!(user_text, ratatui::style::Color::Rgb(0xff, 0xff, 0xff));
    assert_eq!(
        user_bg,
        ratatui::style::Color::Reset,
        "the terminal background shows through"
    );
    assert_eq!(reply_bg, ratatui::style::Color::Reset);

    // the frame around a message is the same colour as its text.
    let wall = buffer
        .content()
        .iter()
        .find(|c| c.symbol() == "\u{256d}")
        .map(|c| c.fg)
        .expect("a frame corner");
    let tinted = [user_text, reply_text];
    assert!(
        tinted.iter().any(|colour| *colour != wall),
        "the frame keeps the role's own colour, not the lightened text tone"
    );
}

#[test]
fn a_long_tool_call_is_shown_in_full_across_rows() {
    let mut app = App::new("s", "m");
    let command = "cd /home/chrisaw/.dotfiles && ls modules/home-automation modules/pico-dsv41 2>/dev/null; git status --short";
    app.tab_mut().on_event(
        "tool.requested",
        &json!({"tool": "bash", "args": {"command": command}}),
    );
    app.tab_mut()
        .on_event("tool.completed", &json!({"output": "", "exit_code": 0}));

    let shown = screen(&mut app, 60, 30);
    let flat: String = shown.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        flat.contains("gitstatus--short"),
        "the whole command is drawn, not cut at a fixed length:\n{shown}"
    );
    assert!(!shown.contains('\u{2026}'), "nothing is elided:\n{shown}");
}

#[test]
fn a_long_tool_output_is_shown_in_full_across_rows() {
    let mut app = App::new("s", "m");
    app.tab_mut().on_event(
        "tool.requested",
        &json!({"tool": "bash", "args": {"command": "ls"}}),
    );
    let line = "modules/home-automation/README.md modules/pico-dsv41/default.nix modules/pi-contexts/index.ts";
    app.tab_mut()
        .on_event("tool.completed", &json!({"output": line, "exit_code": 0}));

    let shown = screen(&mut app, 60, 30);
    let flat: String = shown.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        flat.contains("modules/pi-contexts/index.ts"),
        "the whole output is drawn:\n{shown}"
    );
    assert!(!shown.contains('\u{2026}'), "nothing is elided:\n{shown}");
}

#[test]
fn a_fenced_code_block_is_drawn_as_code_without_its_fences() {
    let mut app = App::new("s", "m");
    app.tab_mut().push(
        Role::Assistant,
        "Here is the file:\n\n```php\n<?php\necho 1;\n?>\n```\n",
    );

    let shown = screen(&mut app, 80, 20);
    assert!(shown.contains("<?php"), "the code is shown:\n{shown}");
    assert!(shown.contains("echo 1;"), "every line of it:\n{shown}");
    assert!(
        !shown.contains("```"),
        "the fence markers are the renderer's decoration, not the message:\n{shown}"
    );
}
