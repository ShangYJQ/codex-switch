use std::{env, error::Error};

use crate::usage::ApiUsageState;

mod config;
mod profile;
mod ui;
mod usage;

fn main() -> Result<(), Box<dyn Error>> {
    match env::args().nth(1).as_deref() {
        Some("auto") => run_auto(),
        Some("--version" | "-V") => {
            print_version();
            Ok(())
        }
        _ => ui::run(),
    }
}

fn print_version() {
    println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
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
