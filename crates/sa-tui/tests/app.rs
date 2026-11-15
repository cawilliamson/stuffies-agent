//! the tabbed tui: key handling, tab lifecycle, transcript framing and rate accounting.
#![allow(clippy::expect_used, clippy::panic)]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use sa_tui::app::{transcript_lines, Action, App, PickerItem, Role};
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
fn the_status_bar_shows_the_session_rate_and_tokens() {
    let mut app = App::new("session-abcdef", "deepseek");
    app.tab_mut().status.context_tokens = 32_000;
    app.tab_mut().status.context_limit = 128_000;
    let screen = screen(&mut app, 160, 24);
    assert!(screen.contains("session-"), "{screen}");
    assert!(screen.contains("0.0 t/s"), "{screen}");
    // the token counts gave way to a context meter.
    assert!(screen.contains("25%"), "{screen}");
    assert!(
        screen.contains("ctrl+t"),
        "key hints must be present:\n{screen}"
    );
}

#[test]
fn thinking_tokens_count_toward_the_rate() {
    use std::time::{Duration, Instant};
    let mut app = App::new("s", "m");
    let t0 = Instant::now();
    app.tab_mut()
        .on_event_at("turn.started", &json!({"prompt": "go"}), t0);
    app.tab_mut()
        .on_event_at("llm.reasoning", &json!({"text": "y".repeat(400)}), t0);
    app.tab_mut()
        .on_event_at("llm.completed", &json!({}), t0 + Duration::from_secs(2));
    app.tab_mut().on_event_at(
        "turn.completed",
        &json!({"usage": {"prompt_tokens": 0, "completion_tokens": 100}}),
        t0 + Duration::from_secs(2),
    );
    app.tab_mut().tick_at(t0 + Duration::from_secs(2));
    assert!(
        (app.tab().tok_per_sec - 50.0).abs() < 1.0,
        "got {}",
        app.tab().tok_per_sec
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
        app.branch = Some("main".to_owned());
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
    let widths: Vec<usize> = lines.iter().map(ratatui::prelude::Line::width).collect();
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

    // the rule belongs to the assistant frame, so it carries that frame's colour.
    assert_eq!(
        fg_of("\u{251c}"),
        Some(sa_tui::app::OK),
        "the divider must match the frame it splits"
    );

    // reasoning is neutral grey, never tinted.
    assert_eq!(
        fg_of("w"),
        Some(Color::Rgb(0x8a, 0x93, 0xa0)),
        "reasoning text must be grey"
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
        .find(|c| c.symbol() == "\u{256d}" && c.fg == sa_tui::app::DIM)
        .map(|c| c.fg);
    assert_eq!(
        edge,
        Some(sa_tui::app::DIM),
        "a thinking frame must be grey"
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
    app.tab_mut().tick_at(t0 + Duration::from_millis(100));
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
    // a braille frame from the spinner set sits beside the tab label.
    assert!(
        screen
            .chars()
            .any(|c| ('\u{2800}'..='\u{28ff}').contains(&c)),
        "expected a spinner glyph:\n{screen}"
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
            .any(|c| ('\u{2800}'..='\u{28ff}').contains(&c)),
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

    let code = buffer
        .content()
        .iter()
        .any(|c| c.symbol() == "-" && c.fg == sa_tui::app::CODE_INK);
    assert!(code, "the code run must use the code colour");
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
    // a rebuild must stay well under the frame interval, or the loop starves.
    assert!(
        once < std::time::Duration::from_millis(50),
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
    let text = app.tab().input_text().to_owned();
    assert!(
        text.starts_with("~/"),
        "the operator's home form must survive completion: {text}"
    );
    assert!(!text.contains("//"), "no doubled slashes: {text}");
}
