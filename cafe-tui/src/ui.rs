use crate::app::{App, AppMode};
use cafe_types::ContentType;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};

pub fn draw(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(0),    // messages
            Constraint::Length(3), // input
        ])
        .split(f.size());

    draw_header(f, app, chunks[0]);
    draw_messages(f, app, chunks[1]);
    draw_input(f, app, chunks[2]);

    if app.mode == AppMode::SessionPicker {
        draw_session_picker(f, app);
    }
    if app.mode == AppMode::ModelPicker {
        draw_model_picker(f, app);
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let session_name = app
        .active_session()
        .map(|s| {
            s.display_name
                .clone()
                .unwrap_or_else(|| s.session_id.clone())
        })
        .unwrap_or_else(|| "No session".into());

    let agent = app
        .active_session()
        .map(|s| s.agent_id.as_str())
        .unwrap_or("—");

    let title = format!(" ObservableCAFE  │  {}  [{}] ", session_name, agent);
    let status = app.status_msg.as_deref().unwrap_or("");

    let header = Paragraph::new(format!("{}{}", title, status))
        .block(Block::default().borders(Borders::ALL))
        .style(Style::default().fg(Color::Cyan));
    f.render_widget(header, area);
}

fn draw_messages(f: &mut Frame, app: &mut App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    for chunk in &app.messages {
        match chunk.content_type {
            ContentType::Text => {
                let role = chunk.role().unwrap_or("system");
                let content = chunk.content.as_deref().unwrap_or("");
                let (label, color) = match role {
                    "user" => ("You", Color::Green),
                    "assistant" => ("Assistant", Color::Blue),
                    _ => ("System", Color::Yellow),
                };

                lines.push(Line::from(Span::styled(
                    format!("{}:", label),
                    Style::default()
                        .fg(color)
                        .add_modifier(Modifier::BOLD),
                )));

                for text_line in content.lines() {
                    lines.push(Line::from(Span::raw(format!("  {}", text_line))));
                }
                lines.push(Line::from(""));
            }
            ContentType::Binary => {
                let mime = chunk.mime_type.as_deref().unwrap_or("binary");
                lines.push(Line::from(Span::styled(
                    format!("[Binary: {}]", mime),
                    Style::default().fg(Color::Magenta),
                )));
                lines.push(Line::from(""));
            }
            ContentType::Null => {
                if chunk
                    .get_annotation::<bool>("chat.is_streaming")
                    .unwrap_or(false)
                {
                    lines.push(Line::from(Span::styled(
                        "  ▋",
                        Style::default()
                            .fg(Color::Blue)
                            .add_modifier(Modifier::SLOW_BLINK),
                    )));
                }
            }
        }
    }

    if app.streaming {
        lines.push(Line::from(Span::styled(
            "  ▋",
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::SLOW_BLINK),
        )));
    }

    let total = lines.len();
    let visible = area.height.saturating_sub(2) as usize;

    let max_scroll = total.saturating_sub(visible);
    if app.scroll_offset > max_scroll {
        app.scroll_offset = max_scroll;
    }

    let skip = total.saturating_sub(visible).saturating_sub(app.scroll_offset);
    let visible_lines: Vec<Line> = lines.into_iter().skip(skip).collect();

    let messages = Paragraph::new(visible_lines)
        .block(Block::default().borders(Borders::ALL).title(" Messages "))
        .wrap(Wrap { trim: false });
    f.render_widget(messages, area);
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    let prompt = if app.streaming { "  (streaming…)" } else { "> " };
    let input_text = format!("{}{}", prompt, app.input);
    let input = Paragraph::new(input_text)
        .block(Block::default().borders(Borders::ALL).title(" Input "))
        .style(Style::default().fg(Color::White));
    f.render_widget(input, area);
}

fn draw_session_picker(f: &mut Frame, app: &App) {
    let area = centered_rect(60, 50, f.size());

    let items: Vec<ListItem> = app
        .sessions
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let name = s
                .display_name
                .clone()
                .unwrap_or_else(|| s.session_id.clone());
            let style = if i == app.active_session_idx {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(format!("  {}  [{}]", name, s.agent_id)).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Sessions (↑↓ to select, Enter to switch, Esc to close) "),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    // Clear background
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(list, area);
}

fn draw_model_picker(f: &mut Frame, app: &App) {
    let area = centered_rect(60, 50, f.size());

    let items: Vec<ListItem> = app
        .model_picker_items
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let style = if i == app.model_picker_idx {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(format!("  {}", m)).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Models (↑↓ to select, Enter to choose, Esc to close) "),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(list, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
