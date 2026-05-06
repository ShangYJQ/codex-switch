use std::{
    fs,
    path::Path,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use crate::profile::{ApiAccount, ApiProfile};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Local, Utc};
use reqwest::blocking::Client;
use serde::Deserialize;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

pub enum ApiUsageState {
    Loading,
    Loaded(ApiUsage),
    Api(ApiProfile),
    Unavailable(Option<String>),
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

impl ApiUsage {
    pub(crate) fn display_text(&self) -> String {
        format_usage(
            &self.primary_label,
            &format_percent(self.primary_remaining_percent()),
            &self.secondary_label,
            &format_percent(self.secondary_remaining_percent()),
        )
    }

    pub(crate) fn primary_remaining_percent(&self) -> Option<f64> {
        remaining_percent(self.primary_used_percent)
    }

    pub(crate) fn secondary_remaining_percent(&self) -> Option<f64> {
        remaining_percent(self.secondary_used_percent)
    }

    pub(crate) fn has_remaining_percents(&self) -> bool {
        self.primary_remaining_percent().is_some() && self.secondary_remaining_percent().is_some()
    }

    pub(crate) fn is_low_remaining(&self) -> bool {
        self.primary_remaining_percent()
            .is_some_and(|remaining| remaining < 1.0)
            || self
                .secondary_remaining_percent()
                .is_some_and(|remaining| remaining < 1.0)
    }

    pub(crate) fn detail_lines(&self) -> Vec<String> {
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

pub fn spawn_usage_tasks(accounts: &[ApiAccount]) -> Receiver<ApiUsageUpdate> {
    let (tx, rx) = mpsc::channel();

    for account in accounts
        .iter()
        .filter(|account| account.needs_usage_fetch())
    {
        let tx = tx.clone();
        let account_name = account.name.clone();
        let auth_path = account.auth_path.clone();

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

pub(crate) fn auth_uses_api_key(auth_path: &Path) -> bool {
    read_auth_json(auth_path)
        .as_ref()
        .and_then(auth_api_key_from_json)
        .is_some()
}

pub(crate) fn auth_account_id(auth_path: &Path) -> Option<String> {
    let auth = read_auth_json(auth_path)?;
    auth_account_id_from_json(&auth).map(str::to_string)
}

pub(crate) fn auth_api_key(auth_path: &Path) -> Option<String> {
    let auth = read_auth_json(auth_path)?;
    auth_api_key_from_json(&auth).map(str::to_string)
}

pub(crate) fn format_usage(
    primary_label: &str,
    primary_percent: &str,
    secondary_label: &str,
    secondary_percent: &str,
) -> String {
    format!("{primary_label:>2} {primary_percent:>4} {secondary_label:>2} {secondary_percent:>4}")
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

fn read_auth_json(auth_path: &Path) -> Option<AuthJson> {
    let auth_content = fs::read_to_string(auth_path).ok()?;
    serde_json::from_str(&auth_content).ok()
}

fn auth_account_id_from_json(auth: &AuthJson) -> Option<&str> {
    auth.tokens
        .as_ref()?
        .account_id
        .as_deref()
        .map(str::trim)
        .filter(|account_id| !account_id.is_empty())
}

fn auth_api_key_from_json(auth: &AuthJson) -> Option<&str> {
    auth.openai_api_key
        .as_deref()
        .map(str::trim)
        .filter(|api_key| !api_key.is_empty())
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
