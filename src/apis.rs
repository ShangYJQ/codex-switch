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
const EXCLUDED_ACCOUNT_DIRS: &[&str] = &[".git"];
const ACCOUNT_CONFIG_FILE: &str = ".account-config.toml";
const API_CONFIG_FILE: &str = ".api-config.toml";

pub struct ApiAccount {
    pub name: String,
    pub entries: Vec<DirEntry>,
    pub kind: ApiAccountKind,
    pub usage: ApiUsageState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiAccountKind {
    Account,
    Api,
}

pub enum ApiUsageState {
    Loading,
    Loaded(ApiUsage),
    Api(ApiProfile),
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

#[derive(Clone)]
pub struct ApiProfile {
    model_provider: Option<String>,
    model: Option<String>,
    auth_method: Option<String>,
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
    #[serde(rename = "OPENAI_API_KEY")]
    openai_api_key: Option<String>,
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
    pub fn needs_usage_fetch(&self) -> bool {
        matches!(self.kind, ApiAccountKind::Account)
    }

    pub fn usage_text(&self) -> String {
        match &self.usage {
            ApiUsageState::Loading => format_usage("5h", "...", "7d", "..."),
            ApiUsageState::Loaded(usage) => usage.display_text(),
            ApiUsageState::Api(profile) => profile.display_text(),
            ApiUsageState::Unavailable(_) => format_usage("5h", "--%", "7d", "--%"),
        }
    }

    pub fn usage_tone(&self) -> ApiUsageTone {
        match &self.usage {
            ApiUsageState::Loading => ApiUsageTone::Default,
            ApiUsageState::Api(_) => ApiUsageTone::Default,
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
            ApiUsageState::Loading | ApiUsageState::Api(_) | ApiUsageState::Unavailable(_) => 0.0,
        }
    }

    pub fn usage_secondary_sort_score(&self) -> f64 {
        match &self.usage {
            ApiUsageState::Loaded(usage) => usage.secondary_remaining_percent().unwrap_or(0.0),
            ApiUsageState::Loading | ApiUsageState::Api(_) | ApiUsageState::Unavailable(_) => 0.0,
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
            ApiUsageState::Api(profile) => profile.detail_lines(),
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

impl ApiProfile {
    fn display_text(&self) -> String {
        let provider = self.model_provider.as_deref().unwrap_or("api");
        let model = self.model.as_deref().unwrap_or("--");
        format!("API {provider} {model}")
    }

    fn detail_lines(&self) -> Vec<String> {
        vec![
            String::from("type: api key"),
            format!(
                "model_provider: {}",
                self.model_provider.as_deref().unwrap_or("--")
            ),
            format!("model: {}", self.model.as_deref().unwrap_or("--")),
            format!(
                "preferred_auth_method: {}",
                self.auth_method.as_deref().unwrap_or("apikey")
            ),
        ]
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
    let key_dir = configured_dir(&config.key_dir, "key_dir")?;
    let api_profile = load_api_profile(&key_dir);

    for entry in fs::read_dir(&key_dir)? {
        let entry = entry?;

        // println!("{:?}", entry.file_type());

        if entry.file_type()?.is_dir() {
            let file_name = entry.file_name();
            if EXCLUDED_ACCOUNT_DIRS
                .iter()
                .any(|excluded| file_name == *excluded)
            {
                continue;
            }

            let dir_name = file_name.to_string_lossy().to_string();

            let sub_dir_full_path = key_dir.join(&dir_name);

            let mut item = (dir_name, Vec::<DirEntry>::new());

            // println!("{:?}", sub_dir_full_path);

            for entry in fs::read_dir(sub_dir_full_path)? {
                let entry = entry?;
                item.1.push(entry);
            }

            let kind = account_kind(&item.0, &item.1);
            let usage = match kind {
                ApiAccountKind::Account => ApiUsageState::Loading,
                ApiAccountKind::Api => ApiUsageState::Api(api_profile.clone()),
            };

            result.push(ApiAccount {
                name: item.0,
                entries: item.1,
                kind,
                usage,
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

pub fn apply_api(config: &Config, account: &ApiAccount) -> Result<(), Box<dyn std::error::Error>> {
    let target_path = configured_dir(&config.codex_config_dir, "codex_config_dir")?;
    let key_dir = configured_dir(&config.key_dir, "key_dir")?;
    let auth_path =
        auth_path(&account.entries).ok_or_else(|| format!("{} 缺少 auth.json", account.name))?;

    let source_config = match account.kind {
        ApiAccountKind::Account => key_dir.join(ACCOUNT_CONFIG_FILE),
        ApiAccountKind::Api => key_dir.join(API_CONFIG_FILE),
    };
    if !source_config.exists() {
        return Err(format!("缺少默认配置模板: {}", source_config.display()).into());
    }

    fs::copy(auth_path, target_path.join("auth.json"))?;
    fs::copy(source_config, target_path.join("config.toml"))?;

    Ok(())
}

pub fn active_profile_name(config: &Config, accounts: &[ApiAccount]) -> Option<String> {
    let target_path = configured_dir(&config.codex_config_dir, "codex_config_dir").ok()?;
    let active_auth = read_auth_json(&target_path.join("auth.json"))?;

    accounts
        .iter()
        .find(|account| auth_matches_account(&active_auth, account))
        .map(|account| account.name.clone())
}

fn configured_dir(value: &str, field_name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let value = value.trim();
    if value.is_empty() {
        return Err(
            format!("{field_name} 未配置，请编辑 ~/.config/codexswitch/config.toml").into(),
        );
    }

    let path = PathBuf::from(value);
    if !path.exists() {
        return Err(format!("{field_name} 不存在: {}", path.display()).into());
    }

    if !path.is_dir() {
        return Err(format!("{field_name} 不是目录: {}", path.display()).into());
    }

    Ok(path)
}

pub fn spawn_usage_tasks(accounts: &[ApiAccount]) -> Receiver<ApiUsageUpdate> {
    let (tx, rx) = mpsc::channel();

    for account in accounts
        .iter()
        .filter(|account| account.needs_usage_fetch())
    {
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

fn account_kind(dir_name: &str, entries: &[DirEntry]) -> ApiAccountKind {
    if dir_name.eq_ignore_ascii_case("api") {
        return ApiAccountKind::Api;
    }

    if auth_path(entries).as_deref().is_some_and(auth_uses_api_key) {
        return ApiAccountKind::Api;
    }

    ApiAccountKind::Account
}

fn auth_path(entries: &[DirEntry]) -> Option<PathBuf> {
    entries
        .iter()
        .find(|entry| entry.file_name() == "auth.json")
        .map(DirEntry::path)
}

fn auth_uses_api_key(auth_path: &Path) -> bool {
    read_auth_json(auth_path)
        .as_ref()
        .and_then(auth_api_key)
        .is_some()
}

fn auth_matches_account(active_auth: &AuthJson, account: &ApiAccount) -> bool {
    let Some(profile_auth) = auth_path(&account.entries)
        .as_deref()
        .and_then(read_auth_json)
    else {
        return false;
    };

    match account.kind {
        ApiAccountKind::Account => auth_account_id(active_auth)
            .zip(auth_account_id(&profile_auth))
            .is_some_and(|(active_id, profile_id)| active_id == profile_id),
        ApiAccountKind::Api => auth_api_key(active_auth)
            .zip(auth_api_key(&profile_auth))
            .is_some_and(|(active_key, profile_key)| active_key == profile_key),
    }
}

fn read_auth_json(auth_path: &Path) -> Option<AuthJson> {
    let auth_content = fs::read_to_string(auth_path).ok()?;
    serde_json::from_str(&auth_content).ok()
}

fn auth_account_id(auth: &AuthJson) -> Option<&str> {
    auth.tokens
        .as_ref()?
        .account_id
        .as_deref()
        .map(str::trim)
        .filter(|account_id| !account_id.is_empty())
}

fn auth_api_key(auth: &AuthJson) -> Option<&str> {
    auth.openai_api_key
        .as_deref()
        .map(str::trim)
        .filter(|api_key| !api_key.is_empty())
}

fn load_api_profile(key_dir: &Path) -> ApiProfile {
    let config_path = key_dir.join(API_CONFIG_FILE);
    let config = fs::read_to_string(config_path)
        .ok()
        .and_then(|content| toml::from_str::<toml::Value>(&content).ok());

    ApiProfile {
        model_provider: toml_string(&config, "model_provider"),
        model: toml_string(&config, "model"),
        auth_method: toml_string(&config, "preferred_auth_method"),
    }
}

fn toml_string(config: &Option<toml::Value>, key: &str) -> Option<String> {
    config
        .as_ref()?
        .get(key)?
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
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
