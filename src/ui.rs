use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Alignment},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, ListState},
    Terminal,
};
use std::io::{self, stdout, Write};

pub struct UserInterface {
    stdout: io::Stdout,
}

impl UserInterface {
    pub fn new() -> Self {
        Self {
            stdout: io::stdout(),
        }
    }

    pub fn clear_screen(&mut self) {
        print!("\x1B[2J\x1B[1;1H");
        let _ = self.stdout.flush();
    }

    pub fn print_header(&mut self, text: &str) {
        println!("\x1B[36m{}\x1B[0m", "=".repeat(60));
        println!("\x1B[36m  {}\x1B[0m", text);
        println!("\x1B[36m{}\x1B[0m\n", "=".repeat(60));
    }

    pub fn print_section(&mut self, text: &str) {
        println!("\n\x1B[33m>>> {}\x1B[0m", text);
    }

    pub fn print_info(&mut self, text: &str) {
        println!("\x1B[34m[INFO]\x1B[0m {}", text);
    }

    pub fn print_success(&mut self, text: &str) {
        println!("\x1B[32m[OK]\x1B[0m {}", text);
    }

    pub fn print_warning(&mut self, text: &str) {
        println!("\x1B[33m[WARN]\x1B[0m {}", text);
    }

    pub fn print_error(&mut self, text: &str) {
        println!("\x1B[31m[ERROR]\x1B[0m {}", text);
    }

    pub fn select_from_list(&mut self, prompt: &str, options: &[String]) -> Result<String> {
        // 设置终端
        enable_raw_mode()?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let mut selected = 0;
        let result = loop {
            terminal.draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(1)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Min(5),
                        Constraint::Length(2),
                    ])
                    .split(f.area());

                // 标题
                let title = Paragraph::new(prompt)
                    .style(Style::default().fg(Color::Cyan))
                    .block(Block::default().borders(Borders::NONE));
                f.render_widget(title, chunks[0]);

                // 选项列表
                let items: Vec<ListItem> = options
                    .iter()
                    .enumerate()
                    .map(|(i, option)| {
                        let style = if i == selected {
                            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        };
                        ListItem::new(option.as_str()).style(style)
                    })
                    .collect();

                let list = List::new(items)
                    .block(Block::default().borders(Borders::ALL).title("Options"))
                    .highlight_style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
                    .highlight_symbol("> ");

                let mut state = ListState::default();
                state.select(Some(selected));
                f.render_stateful_widget(list, chunks[1], &mut state);

                // 帮助信息
                let help = Paragraph::new("↑/↓: Navigate | Enter: Select | Esc: Cancel")
                    .style(Style::default().fg(Color::Gray))
                    .alignment(Alignment::Center);
                f.render_widget(help, chunks[2]);
            })?;

            // 处理按键
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Up => {
                        if selected > 0 {
                            selected -= 1;
                        } else {
                            selected = options.len() - 1;
                        }
                    }
                    KeyCode::Down => {
                        if selected < options.len() - 1 {
                            selected += 1;
                        } else {
                            selected = 0;
                        }
                    }
                    KeyCode::Enter => {
                        break Ok(options[selected].clone());
                    }
                    KeyCode::Esc | KeyCode::Char('q') => {
                        break Err(anyhow::anyhow!("Selection cancelled"));
                    }
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        break Err(anyhow::anyhow!("Interrupted"));
                    }
                    _ => {}
                }
            }
        };

        // 恢复终端
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

        match &result {
            Ok(selection) => println!("Selected: {}\n", selection),
            Err(_) => println!("Selection cancelled\n"),
        }

        result
    }

    pub fn select_number(&mut self, prompt: &str, min: i32, max: i32, default: i32) -> Result<i32> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let mut value = default;

        let result = loop {
            terminal.draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(2)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Length(5),
                        Constraint::Length(3),
                    ])
                    .split(f.area());

                // 标题
                let title = Paragraph::new(vec![
                    Line::from(prompt),
                    Line::from(format!("Range: {} to {} | Default: {}", min, max, default)),
                ])
                    .style(Style::default().fg(Color::Cyan))
                    .block(Block::default().borders(Borders::NONE));
                f.render_widget(title, chunks[0]);

                // 当前值
                let value_display = Paragraph::new(format!("{}", value))
                    .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
                    .block(Block::default().borders(Borders::ALL).title("Current Value"))
                    .alignment(Alignment::Center);
                f.render_widget(value_display, chunks[1]);

                // 帮助
                let help = Paragraph::new(vec![
                    Line::from("↑/↓: ±1 | ←/→: ±10 | PgUp/PgDn: ±100"),
                    Line::from("Home: Min | End: Max | Enter: Confirm | Esc: Default"),
                ])
                    .style(Style::default().fg(Color::Gray))
                    .alignment(Alignment::Center);
                f.render_widget(help, chunks[2]);
            })?;

            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Up => value = (value + 1).min(max),
                    KeyCode::Down => value = (value - 1).max(min),
                    KeyCode::Left => value = (value - 10).max(min),
                    KeyCode::Right => value = (value + 10).min(max),
                    KeyCode::PageUp => value = (value + 100).min(max),
                    KeyCode::PageDown => value = (value - 100).max(min),
                    KeyCode::Home => value = min,
                    KeyCode::End => value = max,
                    KeyCode::Enter => break Ok(value),
                    KeyCode::Esc => break Ok(default),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        break Err(anyhow::anyhow!("Interrupted"));
                    }
                    _ => {}
                }
            }
        };

        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

        match result {
            Ok(v) => {
                println!("Selected: {}\n", v);
                Ok(v)
            }
            Err(e) => Err(e),
        }
    }

    pub fn input_number(&mut self, prompt: &str, min: i32, max: i32, default: i32) -> Result<i32> {
        print!("{} [{}-{}] (default: {}): ", prompt, min, max, default);
        self.stdout.flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            println!("Using default: {}\n", default);
            return Ok(default);
        }

        match input.parse::<i32>() {
            Ok(val) if val >= min && val <= max => {
                println!("Set to: {}\n", val);
                Ok(val)
            }
            _ => {
                println!("Invalid input, using default: {}\n", default);
                Ok(default)
            }
        }
    }

    pub fn input_number_float(&mut self, prompt: &str, min: f32, max: f32, default: f32) -> Result<f32> {
        print!("{} [{:.1}-{:.1}] (default: {:.1}): ", prompt, min, max, default);
        self.stdout.flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            println!("Using default: {:.1}\n", default);
            return Ok(default);
        }

        match input.parse::<f32>() {
            Ok(val) if val >= min && val <= max => {
                println!("Set to: {:.1}\n", val);
                Ok(val)
            }
            _ => {
                println!("Invalid input, using default: {:.1}\n", default);
                Ok(default)
            }
        }
    }

    pub fn input_hex(&mut self, prompt: &str, default: u8) -> Result<u8> {
        print!("{} (default: 0x{:02X}): ", prompt, default);
        self.stdout.flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            println!("Using default: 0x{:02X}\n", default);
            return Ok(default);
        }

        let input = input.trim_start_matches("0x").trim_start_matches("0X");
        match u8::from_str_radix(input, 16) {
            Ok(val) => {
                println!("Set to: 0x{:02X}\n", val);
                Ok(val)
            }
            _ => {
                println!("Invalid hex input, using default: 0x{:02X}\n", default);
                Ok(default)
            }
        }
    }
}