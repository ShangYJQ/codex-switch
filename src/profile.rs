use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    config::Config,
    usage::{self, ApiUsageState, ApiUsageUpdate},
};

const EXCLUDED_ACCOUNT_DIRS: &[&str] = &[".git"];
const ACCOUNT_CONFIG_FILE: &str = ".account-config.toml";
const API_CONFIG_FILE: &str = ".api-config.toml";

pub struct ApiAccount {
    pub name: String,
    pub profile_dir: PathBuf,
    pub auth_path: Option<PathBuf>,
    pub kind: ApiAccountKind,
    pub usage: ApiUsageState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiAccountKind {
    Account,
    Api,
}

pub enum ApiUsageTone {
    Default,
    Success,
    Warning,
    Error,
}

#[derive(Clone)]
pub struct ApiProfile {
    model_provider: Option<String>,
    model: Option<String>,
    auth_method: Option<String>,
}

impl ApiAccount {
    pub fn needs_usage_fetch(&self) -> bool {
        matches!(self.kind, ApiAccountKind::Account)
    }

    pub fn usage_text(&self) -> String {
        match &self.usage {
            ApiUsageState::Loading => usage::format_usage("5h", "...", "7d", "..."),
            ApiUsageState::Loaded(usage) => usage.display_text(),
            ApiUsageState::Api(profile) => profile.display_text(),
            ApiUsageState::Unavailable(_) => usage::format_usage("5h", "--%", "7d", "--%"),
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

pub fn load_accounts(config: &Config) -> Result<Vec<ApiAccount>, Box<dyn std::error::Error>> {
    let mut result = Vec::<ApiAccount>::new();
    let key_dir = configured_dir(&config.key_dir, "key_dir")?;
    let api_profile = load_api_profile(&key_dir);

    for entry in fs::read_dir(&key_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();
        if EXCLUDED_ACCOUNT_DIRS
            .iter()
            .any(|excluded| dir_name == *excluded)
        {
            continue;
        }

        let profile_dir = key_dir.join(&dir_name);
        let auth_path = find_auth_path(&profile_dir);
        let kind = account_kind(&dir_name, auth_path.as_deref());
        let usage = match kind {
            ApiAccountKind::Account => ApiUsageState::Loading,
            ApiAccountKind::Api => ApiUsageState::Api(api_profile.clone()),
        };

        result.push(ApiAccount {
            name: dir_name,
            profile_dir,
            auth_path,
            kind,
            usage,
        });
    }

    Ok(result)
}

pub fn apply_profile(
    config: &Config,
    account: &ApiAccount,
) -> Result<(), Box<dyn std::error::Error>> {
    let target_path = configured_dir(&config.codex_config_dir, "codex_config_dir")?;
    let key_dir = configured_dir(&config.key_dir, "key_dir")?;
    let auth_path = account.auth_path.as_ref().ok_or_else(|| {
        format!(
            "{} 缺少 auth.json: {}",
            account.name,
            account.profile_dir.display()
        )
    })?;

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
    let active_auth_path = target_path.join("auth.json");

    accounts
        .iter()
        .find(|account| auth_matches_account(&active_auth_path, account))
        .map(|account| account.name.clone())
}

pub fn apply_usage_update(accounts: &mut [ApiAccount], update: ApiUsageUpdate) -> bool {
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

pub fn sort_accounts(accounts: &mut [ApiAccount]) {
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

fn usage_state_from_update(update: ApiUsageUpdate) -> ApiUsageState {
    match update.usage {
        Some(usage) => ApiUsageState::Loaded(usage),
        None => ApiUsageState::Unavailable(update.email),
    }
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

fn find_auth_path(profile_dir: &Path) -> Option<PathBuf> {
    let auth_path = profile_dir.join("auth.json");
    auth_path.exists().then_some(auth_path)
}

fn account_kind(dir_name: &str, auth_path: Option<&Path>) -> ApiAccountKind {
    if dir_name.eq_ignore_ascii_case("api") {
        return ApiAccountKind::Api;
    }

    if auth_path.is_some_and(usage::auth_uses_api_key) {
        return ApiAccountKind::Api;
    }

    ApiAccountKind::Account
}

fn auth_matches_account(active_auth_path: &Path, account: &ApiAccount) -> bool {
    let Some(profile_auth_path) = account.auth_path.as_deref() else {
        return false;
    };

    match account.kind {
        ApiAccountKind::Account => usage::auth_account_id(active_auth_path)
            .zip(usage::auth_account_id(profile_auth_path))
            .is_some_and(|(active_id, profile_id)| active_id == profile_id),
        ApiAccountKind::Api => usage::auth_api_key(active_auth_path)
            .zip(usage::auth_api_key(profile_auth_path))
            .is_some_and(|(active_key, profile_key)| active_key == profile_key),
    }
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
