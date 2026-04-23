use ratatui::{
	style::{Color, Style},
	widgets::{Block, Borders, List, ListItem, ListState},
	DefaultTerminal, Frame,
};
use std::fs::DirEntry;

use crate::config::Config;
use crossterm::event::{self, Event, KeyCode};
use std::io;

mod apis;
mod config;

fn main() -> Result<(), Box<dyn std::error::Error>> {
	let mut terminal = ratatui::init();
	App::new().run(&mut terminal)?;
	ratatui::restore();
	Ok(())
}

struct App {
	exit: bool,
	state: ListState,
	apis: Vec<(String, Vec<DirEntry>)>,
	config: Config,
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
		}
	}
	pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
		// TODO: to be safe
		self.config = config::get_config().unwrap();
		self.apis = apis::get_api_names(self.config.clone()).unwrap();

		while !self.exit {
			terminal.draw(|frame| self.draw(frame))?;

			if let Event::Key(key) = event::read()? {
				match key.code {
					KeyCode::Down => {
						let i = match self.state.selected() {
							Some(i) => (i + 1) % self.apis.len(),
							None => 0,
						};
						self.state.select(Some(i));
					}
					KeyCode::Up => {
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

							for x in self.apis.iter() {
								if x.0 == self.apis[i].0 {
									let _ = apis::apply_api(&self.config, &x.1);
								}
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
		let items: Vec<ListItem> = self
			.apis
			.iter()
			.map(|i| ListItem::new(i.0.as_str()))
			.collect();

		let list = List::new(items)
			.block(Block::default().borders(Borders::ALL))
			.highlight_style(Style::default().bg(Color::Blue))
			.highlight_symbol(">> ");

		frame.render_stateful_widget(list, frame.area(), &mut self.state);
	}
}
