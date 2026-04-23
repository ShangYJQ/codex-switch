use std::{
	fs::{self, DirEntry},
	path::PathBuf,
};

use crate::config::Config;

pub fn get_api_names(
	config: Config,
) -> Result<Vec<(String, Vec<DirEntry>)>, Box<dyn std::error::Error>> {
	let mut result = Vec::<(String, Vec<DirEntry>)>::new();

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

			result.push(item);
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
