use std::collections::VecDeque;
use std::path::Path;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table, TableState};
use ratatui::Frame;
use ratatui::Terminal;

use crate::docker_client::DockerClient;

#[derive(Clone)]
struct ContainerRow {
    id: String,
    name: String,
    image: String,
    status: String,
    created: i64,
}

struct App {
    containers: Vec<ContainerRow>,
    selected: usize,
    logs: VecDeque<String>,
    should_quit: bool,
}

pub async fn run_dashboard(speck_home: &Path) -> anyhow::Result<()> {
    let sock_path = speck_home.join("speck.sock");
    let _client = DockerClient::new(&sock_path);

    terminal::enable_raw_mode()?;
    let mut stderr = std::io::stderr();
    crossterm::execute!(stderr, terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stderr);
    let mut terminal = Terminal::new(backend)?;

    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), terminal::LeaveAlternateScreen);
        prev_hook(info);
    }));

    let (tx, mut rx) = tokio::sync::watch::channel(Vec::new());
    let poll_sock = sock_path.to_path_buf();

    tokio::spawn(async move {
        let poll_client = DockerClient::new(&poll_sock);
        loop {
            match poll_client.get("/containers/json?all=true").await {
                Ok(value) => {
                    let rows = parse_containers(&value);
                    let _ = tx.send(rows);
                }
                Err(_) => {
                    let _ = tx.send(Vec::new());
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        }
    });

    let (log_tx, mut log_rx) = tokio::sync::watch::channel(VecDeque::new());
    let log_sock = sock_path.to_path_buf();
    let rx_for_logs = rx.clone();

    tokio::spawn(async move {
        let log_client = DockerClient::new(&log_sock);
        let mut last_id = String::new();
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            let current_id = {
                let containers = rx_for_logs.borrow().clone();
                containers
                    .get(0)
                    .map(|c| c.id.clone())
                    .unwrap_or_default()
            };
            if current_id.is_empty() || current_id == last_id {
                continue;
            }
            last_id = current_id.clone();

            let path = format!("/containers/{current_id}/logs?tail=50&stdout=true&stderr=true");
            match log_client.get(&path).await {
                Ok(value) => {
                    let text = value.as_str().unwrap_or("");
                    let mut logs = VecDeque::new();
                    for line in text.lines() {
                        if logs.len() >= 200 {
                            logs.pop_front();
                        }
                        logs.push_back(line.to_string());
                    }
                    let _ = log_tx.send(logs);
                }
                Err(_) => {}
            }
        }
    });

    let mut app = App {
        containers: Vec::new(),
        selected: 0,
        logs: VecDeque::new(),
        should_quit: false,
    };

    loop {
        tokio::select! {
            _ = rx.changed() => {
                app.containers = rx.borrow().clone();
                if !app.containers.is_empty() && app.selected >= app.containers.len() {
                    app.selected = app.containers.len().saturating_sub(1);
                }
            }
            _ = log_rx.changed() => {
                app.logs = log_rx.borrow().clone();
            }
            event_result = tokio::task::spawn_blocking(event::read) => {
                match event_result {
                    Ok(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => {
                                app.should_quit = true;
                            }
                            KeyCode::Up => {
                                app.selected = app.selected.saturating_sub(1);
                            }
                            KeyCode::Down => {
                                app.selected = (app.selected + 1).min(app.containers.len().saturating_sub(1));
                            }
                            _ => {}
                        }
                    }
                    Ok(Ok(_)) | Ok(Err(_)) | Err(_) => {}
                }
            }
        }

        if app.should_quit {
            break;
        }

        terminal.draw(|f| ui(f, &app))?;
    }

    let _ = terminal::disable_raw_mode();
    let _ = crossterm::execute!(std::io::stdout(), terminal::LeaveAlternateScreen);

    Ok(())
}

fn parse_containers(value: &serde_json::Value) -> Vec<ContainerRow> {
    let arr = match value.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };

    arr.iter()
        .filter_map(|c| {
            let id = c["Id"].as_str().unwrap_or("");
            let short_id = if id.len() > 12 { &id[..12] } else { id };
            let name = c["Names"]
                .as_array()
                .and_then(|n| n.first())
                .and_then(|n| n.as_str())
                .map(|n| n.trim_start_matches('/').to_string())
                .unwrap_or_default();
            let image = c["Image"].as_str().unwrap_or("").to_string();
            let status = c["State"].as_str().unwrap_or("").to_string();
            let created = c["Created"].as_i64().unwrap_or(0);

            Some(ContainerRow {
                id: short_id.to_string(),
                name,
                image,
                status,
                created,
            })
        })
        .collect()
}

fn ui(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(area);

    let upper = chunks[0];
    let lower = chunks[1];

    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(upper);

    render_container_list(frame, panels[0], app);
    render_log_tail(frame, panels[1], app);
    render_status_bar(frame, lower, app);
}

fn render_container_list(frame: &mut Frame, area: Rect, app: &App) {
    let header_cells = ["ID", "NAME", "IMAGE", "STATUS"]
        .iter()
        .map(|h| {
            Line::from(Span::styled(
                *h,
                Style::default()
                    .fg(Color::Rgb(180, 0, 255))
                    .add_modifier(Modifier::BOLD),
            ))
        })
        .collect::<Vec<_>>();
    let header = Row::new(header_cells).height(1);

    let rows: Vec<Row> = app
        .containers
        .iter()
        .map(|c| {
            let cells = vec![
                Line::from(Span::raw(&c.id)),
                Line::from(Span::raw(&c.name)),
                Line::from(Span::raw(&c.image)),
                Line::from(Span::raw(&c.status)),
            ];
            Row::new(cells).height(1)
        })
        .collect();

    let mut table_state = TableState::new()
        .with_offset(0)
        .with_selected(Some(app.selected));

    let selected_style = Style::default()
        .bg(Color::Rgb(20, 0, 40))
        .fg(Color::Rgb(0, 255, 255));

    let table = Table::new(
        rows,
        [
            Constraint::Length(14),
            Constraint::Length(20),
            Constraint::Length(20),
            Constraint::Length(12),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(" Containers "))
    .row_highlight_style(selected_style)
    .highlight_symbol("> ");

    frame.render_stateful_widget(table, area, &mut table_state);
}

fn render_log_tail(frame: &mut Frame, area: Rect, app: &App) {
    let log_lines: Vec<Line> = app
        .logs
        .iter()
        .map(|l| Line::from(Span::raw(l.as_str())))
        .collect();

    let paragraph = Paragraph::new(Text::from(log_lines))
        .block(Block::default().borders(Borders::ALL).title(" Logs "))
        .wrap(ratatui::widgets::Wrap { trim: true });

    frame.render_widget(paragraph, area);
}

fn render_status_bar(frame: &mut Frame, area: Rect, app: &App) {
    let container_count = app.containers.len();
    let status = format!(
        " ↑↓ navigate  q quit  spk dashboard | {} container(s)",
        container_count
    );

    let paragraph = Paragraph::new(Line::from(Span::styled(
        status,
        Style::default().fg(Color::DarkGray),
    )))
    .block(Block::default().borders(Borders::ALL));

    frame.render_widget(paragraph, area);
}
