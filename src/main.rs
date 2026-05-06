use ratatui::{
    DefaultTerminal, Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};

use crate::{
    apis::{ApiAccount, ApiUsageState, ApiUsageTone, ApiUsageUpdate},
    config::Config,
};
use crossterm::event::{self, Event, KeyCode};
use std::{env, io, sync::mpsc::Receiver, time::Duration};

mod apis;
mod config;

const HIGHLIGHT_SYMBOL: &str = ">> ";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if env::args().nth(1).as_deref() == Some("auto") {
        run_auto()?;
        return Ok(());
    }

    let mut terminal = ratatui::init();
    App::new().run(&mut terminal)?;
    ratatui::restore();
    Ok(())
}

fn run_auto() -> Result<(), Box<dyn std::error::Error>> {
    let config = config::get_config()?;
    let mut accounts = apis::get_api_names(&config)?;

    if accounts.is_empty() {
        return Err("没有可用账号".into());
    }

    let rx = apis::spawn_usage_tasks(&accounts);
    for _ in 0..accounts.len() {
        let update = rx.recv()?;
        apply_usage_update(&mut accounts, update);
    }
    sort_accounts(&mut accounts);

    let account = accounts
        .iter()
        .find(|account| matches!(account.usage, ApiUsageState::Loaded(_)))
        .ok_or("没有成功获取 API 用量的账号")?;

    apis::apply_api(&config, &account.entries)?;
    println!("switched to {}", account.name);
    Ok(())
}

struct App {
    exit: bool,
    state: ListState,
    apis: Vec<ApiAccount>,
    config: Config,
    usage_rx: Option<Receiver<ApiUsageUpdate>>,
}

impl App {
    pub fn new() -> Self {
        let mut state = ListState::default();
        state.select(Some(0));

        Self {
            exit: false,
            state,
            apis: Vec::new(),
            config: Config::new(),
            usage_rx: None,
        }
    }
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        // TODO: to be safe
        self.config = config::get_config().unwrap();
        self.apis = apis::get_api_names(&self.config).unwrap();
        if self.apis.is_empty() {
            self.state.select(None);
        } else {
            self.state.select(Some(0));
        }
        self.usage_rx = Some(apis::spawn_usage_tasks(&self.apis));

        while !self.exit {
            self.receive_usage_updates();
            terminal.draw(|frame| self.draw(frame))?;

            if event::poll(Duration::from_millis(100))?
                && let Event::Key(key) = event::read()?
            {
                match key.code {
                    KeyCode::Down | KeyCode::Char('j') => {
                        if self.apis.is_empty() {
                            continue;
                        }

                        let i = match self.state.selected() {
                            Some(i) => (i + 1) % self.apis.len(),
                            None => 0,
                        };
                        self.state.select(Some(i));
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if self.apis.is_empty() {
                            continue;
                        }

                        let i = match self.state.selected() {
                            Some(i) => {
                                if i == 0 {
                                    self.apis.len() - 1
                                } else {
                                    i - 1
                                }
                            }
                            None => 0,
                        };
                        self.state.select(Some(i));
                    }
                    KeyCode::Enter | KeyCode::Char('l') => {
                        if let Some(i) = self.state.selected() {
                            // self.selected_api_name = self.api_names[i].to_string();

                            if let Some(account) = self.apis.get(i) {
                                let _ = apis::apply_api(&self.config, &account.entries);
                            }

                            self.exit = true;
                        }
                    }
                    KeyCode::Char('q') => self.exit = true,
                    _ => {}
                }
            }
        }

        Ok(())
    }
    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let inner_area = Rect {
            x: area.x.saturating_add(1),
            y: area.y.saturating_add(1),
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };
        let detail_height = inner_area.height.min(4);
        let list_height = inner_area.height.saturating_sub(detail_height);
        let list_area = Rect {
            x: inner_area.x,
            y: inner_area.y,
            width: inner_area.width,
            height: list_height,
        };
        let detail_area = Rect {
            x: inner_area.x,
            y: inner_area.y + list_height,
            width: inner_area.width,
            height: detail_height,
        };

        let item_width =
            usize::from(list_area.width).saturating_sub(HIGHLIGHT_SYMBOL.chars().count());
        let selected_index = self.state.selected();
        let items: Vec<ListItem> = self
            .apis
            .iter()
            .enumerate()
            .map(|(index, api)| {
                ListItem::new(format_account_row(
                    api,
                    item_width,
                    selected_index == Some(index),
                ))
            })
            .collect();

        let block = Block::default().borders(Borders::ALL);
        let list = List::new(items)
            .highlight_style(Style::default().bg(Color::Blue))
            .highlight_symbol(HIGHLIGHT_SYMBOL);

        frame.render_widget(block, area);
        if list_area.height > 0 {
            frame.render_stateful_widget(list, list_area, &mut self.state);
        }
        self.draw_account_detail(frame, detail_area);
    }
    fn draw_account_detail(&self, frame: &mut Frame, area: Rect) {
        if area.height == 0 {
            return;
        }

        let lines = self
            .selected_account()
            .map(ApiAccount::detail_lines)
            .unwrap_or_else(|| {
                vec![
                    String::from("email: --"),
                    String::from("plan_type: --"),
                    String::from("5h --% reset --"),
                    String::from("7d --% reset --"),
                ]
            })
            .into_iter()
            .map(Line::from)
            .collect::<Vec<_>>();

        frame.render_widget(Paragraph::new(lines), area);
    }
    fn selected_account(&self) -> Option<&ApiAccount> {
        self.state.selected().and_then(|index| self.apis.get(index))
    }
    fn receive_usage_updates(&mut self) {
        let Some(rx) = &self.usage_rx else {
            return;
        };

        let selected_name = self.selected_account().map(|account| account.name.clone());
        let mut updated = false;

        while let Ok(update) = rx.try_recv() {
            if let Some(account) = self
                .apis
                .iter_mut()
                .find(|account| account.name == update.account_name)
            {
                account.usage = usage_state_from_update(update);
                updated = true;
            }
        }

        if updated {
            self.sort_apis(selected_name);
        }
    }
    fn sort_apis(&mut self, selected_name: Option<String>) {
        sort_accounts(&mut self.apis);

        if let Some(selected_name) = selected_name
            && let Some(index) = self
                .apis
                .iter()
                .position(|account| account.name == selected_name)
        {
            self.state.select(Some(index));
            return;
        }

        if self.apis.is_empty() {
            self.state.select(None);
        } else {
            self.state.select(Some(0));
        }
    }
}

fn apply_usage_update(accounts: &mut [ApiAccount], update: ApiUsageUpdate) -> bool {
    if let Some(account) = accounts
        .iter_mut()
        .find(|account| account.name == update.account_name)
    {
        account.usage = usage_state_from_update(update);
        true
    } else {
        false
    }
}

fn usage_state_from_update(update: ApiUsageUpdate) -> ApiUsageState {
    match update.usage {
        Some(usage) => ApiUsageState::Loaded(usage),
        None => ApiUsageState::Unavailable(update.email),
    }
}

fn sort_accounts(accounts: &mut [ApiAccount]) {
    accounts.sort_by(|a, b| {
        b.usage_sort_rank()
            .cmp(&a.usage_sort_rank())
            .then_with(|| b.usage_sort_score().total_cmp(&a.usage_sort_score()))
            .then_with(|| {
                b.usage_secondary_sort_score()
                    .total_cmp(&a.usage_secondary_sort_score())
            })
            .then_with(|| a.name.cmp(&b.name))
    });
}

fn format_account_row(account: &ApiAccount, width: usize, is_selected: bool) -> Line<'static> {
    let usage = account.usage_text();
    let name_width = account.name.chars().count();
    let usage_width = usage.chars().count();
    let name_style = if is_selected {
        Style::default().fg(Color::Rgb(0, 0, 0))
    } else {
        Style::default()
    };
    let usage_style = Style::default().fg(usage_color(account.usage_tone()));

    if width <= name_width + usage_width {
        return Line::from(vec![
            Span::styled(account.name.clone(), name_style),
            Span::raw(" "),
            Span::styled(usage, usage_style),
        ]);
    }

    let spaces = width - name_width - usage_width;
    Line::from(vec![
        Span::styled(account.name.clone(), name_style),
        Span::raw(" ".repeat(spaces)),
        Span::styled(usage, usage_style),
    ])
}

fn usage_color(tone: ApiUsageTone) -> Color {
    match tone {
        ApiUsageTone::Default => Color::White,
        ApiUsageTone::Success => Color::Green,
        ApiUsageTone::Warning => Color::Rgb(255, 165, 0),
        ApiUsageTone::Error => Color::Red,
    }
}
