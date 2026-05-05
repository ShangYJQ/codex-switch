use std::{
	fs::{self, DirEntry},
	path::{Path, PathBuf},
	sync::mpsc::{self, Receiver},
	thread,
	time::Duration,
};

use crate::config::Config;
use reqwest::blocking::Client;
use serde::Deserialize;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

pub struct ApiAccount {
	pub name: String,
	pub entries: Vec<DirEntry>,
	pub usage: ApiUsageState,
}

pub enum ApiUsageState {
	Loading,
	Loaded(ApiUsage),
	Unavailable,
}

pub struct ApiUsage {
	primary_label: String,
	primary_used_percent: Option<f64>,
	secondary_label: String,
	secondary_used_percent: Option<f64>,
}

pub struct ApiUsageUpdate {
	pub index: usize,
	pub usage: Option<ApiUsage>,
}

#[derive(Deserialize)]
struct AuthJson {
	tokens: Option<AuthTokens>,
}

#[derive(Deserialize)]
struct AuthTokens {
	access_token: Option<String>,
	account_id: Option<String>,
}

#[derive(Deserialize)]
struct UsageResponse {
	rate_limit: Option<RateLimit>,
}

#[derive(Deserialize)]
struct RateLimit {
	primary_window: Option<RateLimitWindow>,
	secondary_window: Option<RateLimitWindow>,
}

#[derive(Deserialize)]
struct RateLimitWindow {
	used_percent: Option<f64>,
	limit_window_seconds: Option<u64>,
}

impl ApiAccount {
	pub fn usage_text(&self) -> String {
		match &self.usage {
			ApiUsageState::Loading => format_usage("5h", "...", "7d", "..."),
			ApiUsageState::Loaded(usage) => usage.display_text(),
			ApiUsageState::Unavailable => format_usage("5h", "--%", "7d", "--%"),
		}
	}
}

impl ApiUsage {
	fn display_text(&self) -> String {
		format_usage(
			&self.primary_label,
			&format_percent(self.primary_used_percent),
			&self.secondary_label,
			&format_percent(self.secondary_used_percent),
		)
	}
}

pub fn get_api_names(config: &Config) -> Result<Vec<ApiAccount>, Box<dyn std::error::Error>> {
	let mut result = Vec::<ApiAccount>::new();

	for entry in fs::read_dir(&config.key_dir)? {
		let entry = entry?;

		// println!("{:?}", entry.file_type());

		if entry.file_type()?.is_dir() {
			let dir_name = entry.file_name().to_string_lossy().to_string();

			let sub_dir_full_path = PathBuf::from(&config.key_dir).join(&dir_name);

			let mut item = (dir_name, Vec::<DirEntry>::new());

			// println!("{:?}", sub_dir_full_path);

			for entry in fs::read_dir(sub_dir_full_path)? {
				let entry = entry?;
				item.1.push(entry);
			}

			result.push(ApiAccount {
				name: item.0,
				entries: item.1,
				usage: ApiUsageState::Loading,
			});
		}

		// let mut actived_item = (String::from("actived"), Vec::<DirEntry>::new());
		//
		// for entry in fs::read_dir(&config.codex_config_dir)? {
		// 	let entry = entry?;
		// 	if entry.file_name() == OsString::from("config.toml")
		// 		|| entry.file_name() == OsString::from("auth.json")
		// 	{
		// 		actived_item.1.push(entry);
		// 	}
		// }
		//
		// result.push(actived_item);
	}

	Ok(result)
}

pub fn apply_api(
	config: &Config,
	new_entry: &[DirEntry],
) -> Result<(), Box<dyn std::error::Error>> {
	let target_path = PathBuf::from(&config.codex_config_dir);

	if !fs::exists(target_path)? {
		return Err(format!("不存在 {} !", &config.codex_config_dir).into());
	}

	for entry in new_entry {
		let path = entry.path();

		if path.is_file() {
			// println!("{:?}", path);

			let target_entry = PathBuf::from(&config.codex_config_dir)
				.join(entry.file_name().to_string_lossy().to_string());

			// println!("{:?} \n {:?}", path, target_entry);

			fs::copy(path, target_entry)?;
		}
	}

	Ok(())
}

pub fn spawn_usage_tasks(accounts: &[ApiAccount]) -> Receiver<ApiUsageUpdate> {
	let (tx, rx) = mpsc::channel();

	for (index, account) in accounts.iter().enumerate() {
		let tx = tx.clone();
		let auth_path = auth_path(&account.entries);

		thread::spawn(move || {
			let usage = auth_path.and_then(|path| fetch_usage(&path).ok());
			let _ = tx.send(ApiUsageUpdate { index, usage });
		});
	}

	rx
}

fn auth_path(entries: &[DirEntry]) -> Option<PathBuf> {
	entries
		.iter()
		.find(|entry| entry.file_name() == "auth.json")
		.map(DirEntry::path)
}

fn fetch_usage(auth_path: &Path) -> Result<ApiUsage, Box<dyn std::error::Error>> {
	let client = Client::builder().timeout(Duration::from_secs(10)).build()?;

	let auth_content = fs::read_to_string(auth_path)?;
	let auth: AuthJson = serde_json::from_str(&auth_content)?;
	let tokens = auth.tokens.ok_or("auth.json 缺少 tokens")?;
	let access_token = required_field(tokens.access_token, "tokens.access_token")?;
	let account_id = required_field(tokens.account_id, "tokens.account_id")?;

	let response = client
		.get(USAGE_URL)
		.bearer_auth(access_token)
		.header("ChatGPT-Account-Id", account_id)
		.send()?;

	let status = response.status();
	if !status.is_success() {
		return Err(format!("usage 请求失败: {status}").into());
	}

	let usage: UsageResponse = response.json()?;
	let rate_limit = usage.rate_limit.ok_or("usage 响应缺少 rate_limit")?;
	let primary_window = rate_limit.primary_window;
	let secondary_window = rate_limit.secondary_window;

	Ok(ApiUsage {
		primary_label: window_label(
			primary_window
				.as_ref()
				.and_then(|window| window.limit_window_seconds),
			"5h",
		),
		primary_used_percent: primary_window.and_then(|window| window.used_percent),
		secondary_label: window_label(
			secondary_window
				.as_ref()
				.and_then(|window| window.limit_window_seconds),
			"7d",
		),
		secondary_used_percent: secondary_window.and_then(|window| window.used_percent),
	})
}

fn required_field(value: Option<String>, name: &str) -> Result<String, Box<dyn std::error::Error>> {
	let value = value.ok_or_else(|| format!("auth.json 缺少 {name}"))?;
	if value.trim().is_empty() {
		Err(format!("auth.json 中 {name} 为空").into())
	} else {
		Ok(value)
	}
}

fn window_label(seconds: Option<u64>, fallback: &str) -> String {
	let Some(seconds) = seconds else {
		return String::from(fallback);
	};

	if seconds % 86_400 == 0 {
		format!("{}d", seconds / 86_400)
	} else if seconds % 3_600 == 0 {
		format!("{}h", seconds / 3_600)
	} else if seconds % 60 == 0 {
		format!("{}m", seconds / 60)
	} else {
		format!("{seconds}s")
	}
}

fn format_percent(value: Option<f64>) -> String {
	let Some(value) = value else {
		return String::from("--%");
	};

	if value.fract() == 0.0 {
		format!("{value:.0}%")
	} else {
		format!("{value:.1}%")
	}
}

fn format_usage(
	primary_label: &str,
	primary_percent: &str,
	secondary_label: &str,
	secondary_percent: &str,
) -> String {
	format!("{primary_label:>2} {primary_percent:>4} {secondary_label:>2} {secondary_percent:>4}")
}
