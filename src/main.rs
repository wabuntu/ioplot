mod scan;

use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Sparkline, Table, TableState};
use scan::{ProcRate, Sampler, human_rate};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// TUI that ranks processes by live disk I/O, like `iotop` with a
/// scrolling read/write history graph on top.
#[derive(Parser, Debug)]
#[clap(
    name = env!("CARGO_PKG_NAME"),
    version = env!("CARGO_PKG_VERSION"),
    about = env!("CARGO_PKG_DESCRIPTION"),
)]
struct Args {
    /// How often to resample /proc, in milliseconds.
    #[arg(long, default_value_t = 1000)]
    interval: u64,

    /// How many of the busiest processes to show.
    #[arg(long, default_value_t = 50)]
    top: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Total,
    Read,
    Write,
    Pid,
    Name,
}

impl SortKey {
    fn next(self) -> SortKey {
        match self {
            SortKey::Total => SortKey::Read,
            SortKey::Read => SortKey::Write,
            SortKey::Write => SortKey::Pid,
            SortKey::Pid => SortKey::Name,
            SortKey::Name => SortKey::Total,
        }
    }

    fn label(self) -> &'static str {
        match self {
            SortKey::Total => "Total",
            SortKey::Read => "Read",
            SortKey::Write => "Write",
            SortKey::Pid => "PID",
            SortKey::Name => "Name",
        }
    }
}

struct App {
    sampler: Sampler,
    processes: Vec<ProcRate>,
    restricted: bool,
    read_history: VecDeque<u64>,
    write_history: VecDeque<u64>,
    history_cap: usize,
    table_state: TableState,
    sort_key: SortKey,
    sort_desc: bool,
    filter: String,
    filter_mode: bool,
    paused: bool,
    interval: Duration,
    show_detail: Option<usize>,
    confirm_kill: Option<usize>,
    status: Option<String>,
    last_sample: Instant,
}

impl App {
    fn new(interval: Duration) -> App {
        App {
            sampler: Sampler::new(),
            processes: Vec::new(),
            restricted: false,
            read_history: VecDeque::new(),
            write_history: VecDeque::new(),
            history_cap: 120,
            table_state: TableState::default(),
            sort_key: SortKey::Total,
            sort_desc: true,
            filter: String::new(),
            filter_mode: false,
            paused: false,
            interval,
            show_detail: None,
            confirm_kill: None,
            status: None,
            last_sample: Instant::now(),
        }
    }

    fn resample(&mut self) {
        let sample = self.sampler.sample();
        self.processes = sample.processes;
        self.restricted = sample.restricted;
        self.last_sample = Instant::now();

        self.read_history
            .push_back(sample.total_read_bps.round() as u64);
        self.write_history
            .push_back(sample.total_write_bps.round() as u64);
        while self.read_history.len() > self.history_cap {
            self.read_history.pop_front();
        }
        while self.write_history.len() > self.history_cap {
            self.write_history.pop_front();
        }
        self.sort();
        if self.table_state.selected().is_none() && !self.visible_indices().is_empty() {
            self.table_state.select(Some(0));
        }
    }

    fn sort(&mut self) {
        let key = self.sort_key;
        self.processes.sort_by(|a, b| {
            let ord = match key {
                SortKey::Total => (a.read_bps + a.write_bps).total_cmp(&(b.read_bps + b.write_bps)),
                SortKey::Read => a.read_bps.total_cmp(&b.read_bps),
                SortKey::Write => a.write_bps.total_cmp(&b.write_bps),
                SortKey::Pid => a.pid.cmp(&b.pid),
                SortKey::Name => b.name.cmp(&a.name), // reversed so `desc` reads A-Z by default
            };
            if self.sort_desc { ord.reverse() } else { ord }
        });
    }

    /// Indices into `self.processes` that match the current filter, in
    /// already-sorted order.
    fn visible_indices(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            (0..self.processes.len()).collect()
        } else {
            let needle = self.filter.to_lowercase();
            self.processes
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    p.name.to_lowercase().contains(&needle) || p.pid.to_string().contains(&needle)
                })
                .map(|(i, _)| i)
                .collect()
        }
    }

    fn move_selection(&mut self, forward: bool) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            self.table_state.select(None);
            return;
        }
        let cur_pos = self
            .table_state
            .selected()
            .and_then(|sel| visible.iter().position(|&i| i == sel))
            .unwrap_or(0);
        let len = visible.len();
        let next_pos = if forward {
            (cur_pos + 1) % len
        } else {
            (cur_pos + len - 1) % len
        };
        self.table_state.select(Some(visible[next_pos]));
    }

    fn kill_selected(&mut self, signal: &str) {
        let Some(i) = self.confirm_kill.take() else {
            return;
        };
        let Some(proc) = self.processes.get(i) else {
            return;
        };
        let result = std::process::Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(proc.pid.to_string())
            .status();
        self.status = Some(match result {
            Ok(s) if s.success() => format!("Sent SIG{signal} to {} ({})", proc.pid, proc.name),
            Ok(_) => format!("kill exited non-zero for {} ({})", proc.pid, proc.name),
            Err(e) => format!("Failed to signal {}: {}", proc.pid, e),
        });
    }
}

fn main() {
    let args = Args::parse();
    let interval = Duration::from_millis(args.interval);

    let mut app = App::new(interval);
    app.resample(); // establish a baseline sample so the first real rates aren't zero
    std::thread::sleep(interval);
    app.resample();

    let mut terminal = ratatui::init();
    let res = run(&mut terminal, &mut app, args.top);
    ratatui::restore();

    if let Err(e) = res {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    top_n: usize,
) -> std::io::Result<()> {
    loop {
        terminal.draw(|frame| draw(frame, app, top_n))?;

        let poll_timeout = Duration::from_millis(80);
        if event::poll(poll_timeout)?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if handle_key(app, key.code) {
                return Ok(());
            }
        }

        if !app.paused && app.last_sample.elapsed() >= app.interval {
            app.resample();
        }
    }
}

/// Returns true if the app should quit.
fn handle_key(app: &mut App, code: KeyCode) -> bool {
    if app.filter_mode {
        match code {
            KeyCode::Enter | KeyCode::Esc => app.filter_mode = false,
            KeyCode::Backspace => {
                app.filter.pop();
            }
            KeyCode::Char(c) => app.filter.push(c),
            _ => {}
        }
        return false;
    }

    if app.confirm_kill.is_some() {
        match code {
            KeyCode::Char('t') | KeyCode::Char('T') => app.kill_selected("TERM"),
            KeyCode::Char('k') | KeyCode::Char('K') => app.kill_selected("KILL"),
            _ => app.confirm_kill = None,
        }
        return false;
    }

    if app.show_detail.is_some() {
        app.show_detail = None;
        return false;
    }

    match code {
        KeyCode::Char('q') | KeyCode::Esc => return true,
        KeyCode::Down => app.move_selection(true),
        KeyCode::Up => app.move_selection(false),
        KeyCode::Enter => app.show_detail = app.table_state.selected(),
        KeyCode::Char('k') if app.table_state.selected().is_some() => {
            app.confirm_kill = app.table_state.selected();
        }
        KeyCode::Char('s') => {
            app.sort_key = app.sort_key.next();
            app.sort();
        }
        KeyCode::Char('r') => {
            app.sort_desc = !app.sort_desc;
            app.sort();
        }
        KeyCode::Char('/') => app.filter_mode = true,
        KeyCode::Char(' ') | KeyCode::Char('p') => app.paused = !app.paused,
        KeyCode::Char('+') | KeyCode::Char('=') => {
            app.interval = (app.interval - Duration::from_millis(100).min(app.interval))
                .max(Duration::from_millis(200));
        }
        KeyCode::Char('-') => {
            app.interval = (app.interval + Duration::from_millis(100)).min(Duration::from_secs(10));
        }
        _ => {}
    }
    false
}

/// Color scales with I/O intensity — a small heatmap, not just a flat list.
fn heat_color(bytes_per_sec: f64) -> Color {
    if bytes_per_sec >= 50_000_000.0 {
        Color::Rgb(255, 85, 85) // hot: red
    } else if bytes_per_sec >= 5_000_000.0 {
        Color::Rgb(255, 184, 108) // warm: orange
    } else if bytes_per_sec >= 500_000.0 {
        Color::Rgb(241, 250, 140) // busy: yellow
    } else if bytes_per_sec > 0.0 {
        Color::Rgb(139, 233, 253) // light: cyan
    } else {
        Color::Rgb(98, 114, 164) // idle: muted blue-gray
    }
}

fn draw(frame: &mut Frame, app: &mut App, top_n: usize) {
    let area = frame.area();
    let layout = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(7),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    draw_header(frame, app, layout[0]);
    draw_history(frame, app, layout[1]);
    draw_table(frame, app, layout[2], top_n);
    draw_status(frame, app, layout[3]);

    if let Some(i) = app.show_detail
        && let Some(p) = app.processes.get(i)
    {
        draw_detail_popup(frame, p);
    }
    if let Some(i) = app.confirm_kill
        && let Some(p) = app.processes.get(i)
    {
        draw_kill_popup(frame, p);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let pause_note = if app.paused { " [PAUSED]" } else { "" };
    let restricted_note = if app.restricted {
        "  (run as root to see all processes)"
    } else {
        ""
    };
    let title = format!(
        "ioplot — {} tracked, sort: {} {}{}{}",
        app.processes.len(),
        app.sort_key.label(),
        if app.sort_desc { "↓" } else { "↑" },
        pause_note,
        restricted_note,
    );
    let style = if app.restricted {
        Style::default().fg(Color::Rgb(255, 184, 108))
    } else {
        Style::default()
            .fg(Color::Rgb(189, 147, 249))
            .add_modifier(Modifier::BOLD)
    };
    frame.render_widget(Line::from(title).style(style), area);
}

fn draw_history(frame: &mut Frame, app: &App, area: Rect) {
    let cols =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);

    let read_now = app.read_history.back().copied().unwrap_or(0);
    let write_now = app.write_history.back().copied().unwrap_or(0);

    let read_data: Vec<u64> = app.read_history.iter().copied().collect();
    let read = Sparkline::default()
        .block(
            Block::bordered()
                .border_style(Style::default().fg(Color::Rgb(139, 233, 253)))
                .title(format!(" Read  {} ", human_rate(read_now as f64))),
        )
        .data(&read_data)
        .style(Style::default().fg(Color::Rgb(139, 233, 253)))
        .bar_set(symbols::bar::NINE_LEVELS);
    frame.render_widget(read, cols[0]);

    let write_data: Vec<u64> = app.write_history.iter().copied().collect();
    let write = Sparkline::default()
        .block(
            Block::bordered()
                .border_style(Style::default().fg(Color::Rgb(255, 121, 198)))
                .title(format!(" Write  {} ", human_rate(write_now as f64))),
        )
        .data(&write_data)
        .style(Style::default().fg(Color::Rgb(255, 121, 198)))
        .bar_set(symbols::bar::NINE_LEVELS);
    frame.render_widget(write, cols[1]);
}

fn draw_table(frame: &mut Frame, app: &mut App, area: Rect, top_n: usize) {
    let visible = app.visible_indices();
    let shown: Vec<usize> = visible.into_iter().take(top_n).collect();

    let rows: Vec<Row> = shown
        .iter()
        .map(|&i| {
            let p = &app.processes[i];
            let total = p.read_bps + p.write_bps;
            let color = heat_color(total);
            Row::new(vec![
                Cell::from(p.pid.to_string()),
                Cell::from(p.name.clone()),
                Cell::from(human_rate(p.read_bps))
                    .style(Style::default().fg(Color::Rgb(139, 233, 253))),
                Cell::from(human_rate(p.write_bps))
                    .style(Style::default().fg(Color::Rgb(255, 121, 198))),
                Cell::from(human_rate(total)),
            ])
            .style(Style::default().fg(color))
        })
        .collect();

    let widths = [
        Constraint::Length(8),
        Constraint::Min(20),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(12),
    ];
    let filter_title = if app.filter_mode {
        format!(" filter: {}_ ", app.filter)
    } else if !app.filter.is_empty() {
        format!(" filter: {} (Esc in filter box to clear) ", app.filter)
    } else {
        String::new()
    };
    let table = Table::new(rows, widths)
        .header(
            Row::new(vec!["PID", "Process", "Read/s", "Write/s", "Total/s"]).style(
                Style::default()
                    .fg(Color::Rgb(248, 248, 242))
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .block(
            Block::bordered()
                .border_style(Style::default().fg(Color::Rgb(98, 114, 164)))
                .title(" Processes, busiest first ")
                .title_bottom(filter_title),
        )
        .row_highlight_style(
            Style::default()
                .bg(Color::Rgb(68, 71, 90))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(table, area, &mut app.table_state);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let text = app.status.clone().unwrap_or_else(|| {
        "↑/↓ select  Enter details  k kill  s sort  r reverse  / filter  space pause  +/- speed  q quit".to_string()
    });
    frame.render_widget(
        Line::from(text).style(Style::default().fg(Color::Rgb(98, 114, 164))),
        area,
    );
}

fn draw_detail_popup(frame: &mut Frame, p: &ProcRate) {
    let area = centered_rect(60, 10, frame.area());
    frame.render_widget(Clear, area);
    let text = format!(
        "PID:    {}\nName:   {}\n\nRead:   {}  ({} total)\nWrite:  {}  ({} total)\n\nany key: close",
        p.pid,
        p.name,
        human_rate(p.read_bps),
        human_bytes(p.read_bytes_total),
        human_rate(p.write_bps),
        human_bytes(p.write_bytes_total),
    );
    let popup = Paragraph::new(text)
        .style(Style::default().fg(Color::Rgb(248, 248, 242)))
        .block(
            Block::bordered()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(189, 147, 249)))
                .title(" Process details "),
        );
    frame.render_widget(popup, area);
}

fn draw_kill_popup(frame: &mut Frame, p: &ProcRate) {
    let area = centered_rect(60, 9, frame.area());
    frame.render_widget(Clear, area);
    let text = format!(
        "{} (PID {})\n\nt: SIGTERM (ask it to exit)\nk: SIGKILL (force, no cleanup)\nany other key: cancel",
        p.name, p.pid
    );
    let popup = Paragraph::new(text)
        .style(Style::default().fg(Color::Rgb(255, 85, 85)))
        .block(
            Block::bordered()
                .border_style(Style::default().fg(Color::Rgb(255, 85, 85)))
                .title(" Send signal? "),
        );
    frame.render_widget(popup, area);
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.1} {}", v, UNITS[unit])
    }
}

fn centered_rect(pct_x: u16, rows: u16, area: Rect) -> Rect {
    let rows = rows.min(area.height);
    let vertical = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(rows),
        Constraint::Fill(1),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - pct_x) / 2),
        Constraint::Percentage(pct_x),
        Constraint::Percentage((100 - pct_x) / 2),
    ])
    .split(vertical[1])[1]
}
