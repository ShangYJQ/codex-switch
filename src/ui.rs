use ratatui::{
    DefaultTerminal, Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};

use crate::{
    config::{self, Config},
    profile::{self, ApiAccount, ApiUsageTone},
    usage::{self, ApiUsageUpdate},
};
use crossterm::event::{self, Event, KeyCode};
use std::{error::Error, sync::mpsc::Receiver, time::Duration};

const HIGHLIGHT_SYMBOL: &str = ">> ";

pub fn run() -> Result<(), Box<dyn Error>> {
    let mut terminal = ratatui::init();
    let result = App::new().run(&mut terminal);
    ratatui::restore();
    result
}

struct App {
    exit: bool,
    state: ListState,
    accounts: Vec<ApiAccount>,
    config: Config,
    usage_rx: Option<Receiver<ApiUsageUpdate>>,
    last_error: Option<String>,
    active_profile_name: Option<String>,
}

impl App {
    fn new() -> Self {
        let mut state = ListState::default();
        state.select(Some(0));

        Self {
            exit: false,
            state,
            accounts: Vec::new(),
            config: Config::new(),
            usage_rx: None,
            last_error: None,
            active_profile_name: None,
        }
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<(), Box<dyn Error>> {
        self.config = config::get_config()?;
        self.accounts = profile::load_accounts(&self.config)?;
        self.active_profile_name = profile::active_profile_name(&self.config, &self.accounts);
        if self.accounts.is_empty() {
            self.state.select(None);
        } else {
            self.state.select(Some(0));
        }
        self.usage_rx = Some(usage::spawn_usage_tasks(&self.accounts));

        while !self.exit {
            self.receive_usage_updates();
            terminal.draw(|frame| self.draw(frame))?;

            if event::poll(Duration::from_millis(100))?
                && let Event::Key(key) = event::read()?
            {
                match key.code {
                    KeyCode::Down | KeyCode::Char('j') => {
                        if self.accounts.is_empty() {
                            continue;
                        }

                        let i = match self.state.selected() {
                            Some(i) => (i + 1) % self.accounts.len(),
                            None => 0,
                        };
                        self.state.select(Some(i));
                        self.last_error = None;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if self.accounts.is_empty() {
                            continue;
                        }

                        let i = match self.state.selected() {
                            Some(i) => {
                                if i == 0 {
                                    self.accounts.len() - 1
                                } else {
                                    i - 1
                                }
                            }
                            None => 0,
                        };
                        self.state.select(Some(i));
                        self.last_error = None;
                    }
                    KeyCode::Enter | KeyCode::Char('l') => {
                        if let Some(i) = self.state.selected()
                            && let Some(account) = self.accounts.get(i)
                        {
                            match profile::apply_profile(&self.config, account) {
                                Ok(()) => self.exit = true,
                                Err(error) => self.last_error = Some(error.to_string()),
                            }
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
        let header_height = inner_area.height.min(2);
        let detail_height = inner_area.height.saturating_sub(header_height).min(4);
        let list_height = inner_area
            .height
            .saturating_sub(header_height)
            .saturating_sub(detail_height);
        let header_area = Rect {
            x: inner_area.x,
            y: inner_area.y,
            width: inner_area.width,
            height: header_height,
        };
        let list_area = Rect {
            x: inner_area.x,
            y: inner_area.y + header_height,
            width: inner_area.width,
            height: list_height,
        };
        let detail_area = Rect {
            x: inner_area.x,
            y: inner_area.y + header_height + list_height,
            width: inner_area.width,
            height: detail_height,
        };

        let item_width =
            usize::from(list_area.width).saturating_sub(HIGHLIGHT_SYMBOL.chars().count());
        let selected_index = self.state.selected();
        let items: Vec<ListItem> = self
            .accounts
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
        self.draw_active_profile(frame, header_area);
        if list_area.height > 0 {
            frame.render_stateful_widget(list, list_area, &mut self.state);
        }
        self.draw_account_detail(frame, detail_area);
    }

    fn draw_active_profile(&self, frame: &mut Frame, area: Rect) {
        if area.height == 0 {
            return;
        }

        let name = self.active_profile_name.as_deref().unwrap_or("--");
        let line = Line::from(vec![
            Span::styled("current: ", Style::default().fg(Color::Gray)),
            Span::styled(name.to_string(), Style::default().fg(Color::Cyan)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    fn draw_account_detail(&self, frame: &mut Frame, area: Rect) {
        if area.height == 0 {
            return;
        }

        let lines = self
            .last_error
            .as_ref()
            .map(|error| {
                vec![
                    format!("error: {error}"),
                    String::from("按 q 退出，或选择其他 profile"),
                ]
            })
            .or_else(|| self.selected_account().map(ApiAccount::detail_lines))
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
        self.state
            .selected()
            .and_then(|index| self.accounts.get(index))
    }

    fn receive_usage_updates(&mut self) {
        let Some(rx) = &self.usage_rx else {
            return;
        };

        let selected_index = self.state.selected();
        let mut updated = false;

        while let Ok(update) = rx.try_recv() {
            updated |= profile::apply_usage_update(&mut self.accounts, update);
        }

        if updated {
            self.sort_accounts(selected_index);
        }
    }

    fn sort_accounts(&mut self, selected_index: Option<usize>) {
        profile::sort_accounts(&mut self.accounts);

        if self.accounts.is_empty() {
            self.state.select(None);
        } else {
            let index = selected_index.unwrap_or(0).min(self.accounts.len() - 1);
            self.state.select(Some(index));
        }
    }
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
