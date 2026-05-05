use ratatui::{
	DefaultTerminal, Frame,
	style::{Color, Style},
	widgets::{Block, Borders, List, ListItem, ListState},
};

use crate::{
	apis::{ApiAccount, ApiUsageState, ApiUsageUpdate},
	config::Config,
};
use crossterm::event::{self, Event, KeyCode};
use std::{io, sync::mpsc::Receiver, time::Duration};

mod apis;
mod config;

const HIGHLIGHT_SYMBOL: &str = ">> ";

fn main() -> Result<(), Box<dyn std::error::Error>> {
	let mut terminal = ratatui::init();
	App::new().run(&mut terminal)?;
	ratatui::restore();
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

			if event::poll(Duration::from_millis(100))? {
				if let Event::Key(key) = event::read()? {
					match key.code {
						KeyCode::Down => {
							if self.apis.is_empty() {
								continue;
							}

							let i = match self.state.selected() {
								Some(i) => (i + 1) % self.apis.len(),
								None => 0,
							};
							self.state.select(Some(i));
						}
						KeyCode::Up => {
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
						KeyCode::Enter => {
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
		}

		Ok(())
	}
	fn draw(&mut self, frame: &mut Frame) {
		let item_width = usize::from(frame.area().width.saturating_sub(2))
			.saturating_sub(HIGHLIGHT_SYMBOL.chars().count());
		let items: Vec<ListItem> = self
			.apis
			.iter()
			.map(|api| ListItem::new(format_account_row(api, item_width)))
			.collect();

		let list = List::new(items)
			.block(Block::default().borders(Borders::ALL))
			.highlight_style(Style::default().bg(Color::Blue))
			.highlight_symbol(HIGHLIGHT_SYMBOL);

		frame.render_stateful_widget(list, frame.area(), &mut self.state);
	}
	fn receive_usage_updates(&mut self) {
		let Some(rx) = &self.usage_rx else {
			return;
		};

		while let Ok(update) = rx.try_recv() {
			if let Some(account) = self.apis.get_mut(update.index) {
				account.usage = match update.usage {
					Some(usage) => ApiUsageState::Loaded(usage),
					None => ApiUsageState::Unavailable,
				};
			}
		}
	}
}

fn format_account_row(account: &ApiAccount, width: usize) -> String {
	let usage = account.usage_text();
	let name_width = account.name.chars().count();
	let usage_width = usage.chars().count();

	if width <= name_width + usage_width {
		return format!("{} {}", account.name, usage);
	}

	let spaces = width - name_width - usage_width;
	format!("{}{}{}", account.name, " ".repeat(spaces), usage)
}
