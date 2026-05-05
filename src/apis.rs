use std::{
	fs::{self, DirEntry},
	path::{Path, PathBuf},
	sync::mpsc::{self, Receiver},
	thread,
	time::Duration,
};

use crate::config::Config;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Local, Utc};
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
	Unavailable(Option<String>),
}

pub enum ApiUsageTone {
	Default,
	Success,
	Warning,
	Error,
}

pub struct ApiUsage {
	email: Option<String>,
	plan_type: Option<String>,
	primary_label: String,
	primary_used_percent: Option<f64>,
	primary_reset_after_seconds: Option<u64>,
	primary_reset_at: Option<i64>,
	secondary_label: String,
	secondary_used_percent: Option<f64>,
	secondary_reset_after_seconds: Option<u64>,
	secondary_reset_at: Option<i64>,
}

pub struct ApiUsageUpdate {
	pub account_name: String,
	pub usage: Option<ApiUsage>,
	pub email: Option<String>,
}

#[derive(Deserialize)]
struct AuthJson {
	email: Option<String>,
	tokens: Option<AuthTokens>,
}

#[derive(Deserialize)]
struct AuthTokens {
	access_token: Option<String>,
	account_id: Option<String>,
	id_token: Option<String>,
}

#[derive(Deserialize)]
struct UsageResponse {
	email: Option<String>,
	plan_type: Option<String>,
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
	reset_after_seconds: Option<u64>,
	reset_at: Option<i64>,
}

impl ApiAccount {
	pub fn usage_text(&self) -> String {
		match &self.usage {
			ApiUsageState::Loading => format_usage("5h", "...", "7d", "..."),
			ApiUsageState::Loaded(usage) => usage.display_text(),
			ApiUsageState::Unavailable(_) => format_usage("5h", "--%", "7d", "--%"),
		}
	}

	pub fn usage_tone(&self) -> ApiUsageTone {
		match &self.usage {
			ApiUsageState::Loading => ApiUsageTone::Default,
			ApiUsageState::Unavailable(_) => ApiUsageTone::Error,
			ApiUsageState::Loaded(usage) => {
				if usage.is_low_remaining() {
					ApiUsageTone::Warning
				} else if usage.has_remaining_percents() {
					ApiUsageTone::Success
				} else {
					ApiUsageTone::Error
				}
			}
		}
	}

	pub fn usage_sort_score(&self) -> f64 {
		match &self.usage {
			ApiUsageState::Loaded(usage) => usage.primary_remaining_percent().unwrap_or(0.0),
			ApiUsageState::Loading | ApiUsageState::Unavailable(_) => 0.0,
		}
	}

	pub fn usage_secondary_sort_score(&self) -> f64 {
		match &self.usage {
			ApiUsageState::Loaded(usage) => usage.secondary_remaining_percent().unwrap_or(0.0),
			ApiUsageState::Loading | ApiUsageState::Unavailable(_) => 0.0,
		}
	}

	pub fn usage_sort_rank(&self) -> u8 {
		match self.usage_tone() {
			ApiUsageTone::Error => 0,
			ApiUsageTone::Default => 1,
			ApiUsageTone::Warning => 2,
			ApiUsageTone::Success => 3,
		}
	}

	pub fn detail_lines(&self) -> Vec<String> {
		match &self.usage {
			ApiUsageState::Loading => vec![
				String::from("email: loading..."),
				String::from("plan_type: loading..."),
				String::from("5h ... reset in ..."),
				String::from("7d ... reset in ..."),
			],
			ApiUsageState::Unavailable(email) => vec![
				format!("email: {}", email.as_deref().unwrap_or("--")),
				String::from("plan_type: --"),
				String::from("5h --% reset --"),
				String::from("7d --% reset --"),
			],
			ApiUsageState::Loaded(usage) => usage.detail_lines(),
		}
	}
}

impl ApiUsage {
	fn display_text(&self) -> String {
		format_usage(
			&self.primary_label,
			&format_percent(self.primary_remaining_percent()),
			&self.secondary_label,
			&format_percent(self.secondary_remaining_percent()),
		)
	}

	fn primary_remaining_percent(&self) -> Option<f64> {
		remaining_percent(self.primary_used_percent)
	}

	fn secondary_remaining_percent(&self) -> Option<f64> {
		remaining_percent(self.secondary_used_percent)
	}

	fn has_remaining_percents(&self) -> bool {
		self.primary_remaining_percent().is_some() && self.secondary_remaining_percent().is_some()
	}

	fn is_low_remaining(&self) -> bool {
		self.primary_remaining_percent()
			.is_some_and(|remaining| remaining < 1.0)
			|| self
				.secondary_remaining_percent()
				.is_some_and(|remaining| remaining < 1.0)
	}

	fn detail_lines(&self) -> Vec<String> {
		vec![
			format!("email: {}", self.email.as_deref().unwrap_or("--")),
			format!("plan_type: {}", self.plan_type.as_deref().unwrap_or("--")),
			format_window_detail(
				&self.primary_label,
				self.primary_remaining_percent(),
				self.primary_reset_after_seconds,
				self.primary_reset_at,
			),
			format_window_detail(
				&self.secondary_label,
				self.secondary_remaining_percent(),
				self.secondary_reset_after_seconds,
				self.secondary_reset_at,
			),
		]
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

	for account in accounts {
		let tx = tx.clone();
		let account_name = account.name.clone();
		let auth_path = auth_path(&account.entries);

		thread::spawn(move || {
			let email = auth_path.as_deref().and_then(auth_email);
			let usage = auth_path
				.as_deref()
				.and_then(|path| fetch_usage(path, email.clone()).ok());
			let _ = tx.send(ApiUsageUpdate {
				account_name,
				usage,
				email,
			});
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

fn fetch_usage(
	auth_path: &Path,
	fallback_email: Option<String>,
) -> Result<ApiUsage, Box<dyn std::error::Error>> {
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
		email: usage.email.or(fallback_email),
		plan_type: usage.plan_type,
		primary_label: window_label(
			primary_window
				.as_ref()
				.and_then(|window| window.limit_window_seconds),
			"5h",
		),
		primary_used_percent: primary_window
			.as_ref()
			.and_then(|window| window.used_percent),
		primary_reset_after_seconds: primary_window
			.as_ref()
			.and_then(|window| window.reset_after_seconds),
		primary_reset_at: primary_window.as_ref().and_then(|window| window.reset_at),
		secondary_label: window_label(
			secondary_window
				.as_ref()
				.and_then(|window| window.limit_window_seconds),
			"7d",
		),
		secondary_used_percent: secondary_window
			.as_ref()
			.and_then(|window| window.used_percent),
		secondary_reset_after_seconds: secondary_window
			.as_ref()
			.and_then(|window| window.reset_after_seconds),
		secondary_reset_at: secondary_window.as_ref().and_then(|window| window.reset_at),
	})
}

fn auth_email(auth_path: &Path) -> Option<String> {
	let auth_content = fs::read_to_string(auth_path).ok()?;
	let auth: AuthJson = serde_json::from_str(&auth_content).ok()?;

	if let Some(email) = auth.email.filter(|email| !email.trim().is_empty()) {
		return Some(email);
	}

	let id_token = auth.tokens?.id_token?;
	email_from_id_token(&id_token)
}

fn email_from_id_token(id_token: &str) -> Option<String> {
	let payload = id_token.split('.').nth(1)?;
	let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
	let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
	claims
		.get("email")
		.and_then(|email| email.as_str())
		.filter(|email| !email.trim().is_empty())
		.map(str::to_string)
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

fn remaining_percent(used_percent: Option<f64>) -> Option<f64> {
	used_percent.map(|used| (100.0 - used).clamp(0.0, 100.0))
}

fn format_window_detail(
	label: &str,
	remaining_percent: Option<f64>,
	reset_after_seconds: Option<u64>,
	reset_at: Option<i64>,
) -> String {
	format!(
		"{} {} {}",
		label,
		format_percent(remaining_percent),
		format_reset(reset_after_seconds, reset_at),
	)
}

fn format_reset(reset_after_seconds: Option<u64>, reset_at: Option<i64>) -> String {
	let Some(reset_after_seconds) = reset_after_seconds else {
		return String::from("reset --");
	};

	if reset_after_seconds <= 86_400 {
		let hours = reset_after_seconds / 3_600;
		let minutes = (reset_after_seconds % 3_600) / 60;
		return format!("reset in {hours}h {minutes}min");
	}

	let Some(reset_at) = reset_at else {
		return String::from("reset --");
	};

	let Some(reset_at) = DateTime::<Utc>::from_timestamp(reset_at, 0) else {
		return String::from("reset --");
	};

	let reset_at = reset_at.with_timezone(&Local);
	format!("reset at {}", reset_at.format("%m.%d %H:%M"))
}

fn format_usage(
	primary_label: &str,
	primary_percent: &str,
	secondary_label: &str,
	secondary_percent: &str,
) -> String {
	format!("{primary_label:>2} {primary_percent:>4} {secondary_label:>2} {secondary_percent:>4}")
}
