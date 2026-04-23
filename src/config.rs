use dirs::{self};
use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf};
use toml;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Config {
	pub codex_config_dir: String,
	pub key_dir: String,
}

impl Config {
	pub fn new() -> Self {
		Self {
			codex_config_dir: String::new(),
			key_dir: String::new(),
		}
	}
}

pub fn get_config() -> Result<Config, Box<dyn std::error::Error>> {
	let home_dir = dirs::home_dir()
		.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "找不到家目录！"))?;
	// println!("{:?}", home_dir);

	let mut config_path = PathBuf::from(home_dir);
	config_path.push(".config");
	config_path.push("codexswitch");
	config_path.push("config.toml");

	// println!("{:?}", config_path);

	if let Some(parent) = config_path.parent() {
		fs::create_dir_all(parent)?;
	}

	if !config_path.exists() {
		let default_config = Config {
			codex_config_dir: String::new(),
			key_dir: String::new(),
		};
		let toml_str = toml::to_string(&default_config)?;

		fs::write(&config_path, toml_str)?;
	}

	let content = fs::read_to_string(config_path)?;

	let config: Config = toml::from_str(&content)?;

	// println!("{:?}", config.key_dir);

	Ok(config)
}
