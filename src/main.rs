use std::{error::Error, process::ExitCode};

use crate::usage::ApiUsageState;
use clap::{Parser, Subcommand};

mod config;
mod profile;
mod ui;
mod usage;

#[derive(Parser)]
#[command(version, about = "Switch local Codex account profiles")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Automatically switch to the regular account with the most remaining usage.
    Auto,
    /// Check whether regular accounts can return usage API data.
    Ping,
}

fn main() -> Result<ExitCode, Box<dyn Error>> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Auto) => {
            run_auto()?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Ping) => run_ping(),
        None => {
            ui::run()?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn run_auto() -> Result<(), Box<dyn Error>> {
    let config = config::get_config()?;
    let mut accounts = profile::load_accounts(&config)?;

    if accounts.is_empty() {
        return Err("没有可用账号".into());
    }

    let usage_task_count = accounts
        .iter()
        .filter(|account| account.needs_usage_fetch())
        .count();
    let rx = usage::spawn_usage_tasks(&accounts);
    for _ in 0..usage_task_count {
        let update = rx.recv()?;
        profile::apply_usage_update(&mut accounts, update);
    }
    profile::sort_accounts(&mut accounts);

    let account = accounts
        .iter()
        .find(|account| matches!(account.usage, ApiUsageState::Loaded(_)))
        .ok_or("没有可自动切换的账号（API profile 不参与 auto）")?;

    profile::apply_profile(&config, account)?;
    println!("switched to {}", account.name);
    Ok(())
}

fn run_ping() -> Result<ExitCode, Box<dyn Error>> {
    let config = config::get_config()?;
    let accounts = profile::load_accounts(&config)?;
    let usage_task_count = accounts
        .iter()
        .filter(|account| account.needs_usage_fetch())
        .count();

    if usage_task_count == 0 {
        return Err("没有可检测账号（API profile 不参与 ping）".into());
    }

    let rx = usage::spawn_usage_tasks(&accounts);
    let mut failed_accounts = Vec::new();

    for _ in 0..usage_task_count {
        let update = rx.recv()?;
        if update.usage.is_none() {
            failed_accounts.push(ping_failure_label(&accounts, &update));
        }
    }

    if failed_accounts.is_empty() {
        println!("all accounts ok");
        return Ok(ExitCode::SUCCESS);
    }

    for account in failed_accounts {
        println!("{account}");
    }

    Ok(ExitCode::FAILURE)
}

fn ping_failure_label(accounts: &[profile::ApiAccount], update: &usage::ApiUsageUpdate) -> String {
    if let Some(email) = update
        .email
        .as_deref()
        .filter(|email| !email.trim().is_empty())
    {
        return email.to_string();
    }

    accounts
        .iter()
        .find(|account| account.name == update.account_name)
        .map(|account| format!("{} ({})", account.name, account.profile_dir.display()))
        .unwrap_or_else(|| update.account_name.clone())
}
