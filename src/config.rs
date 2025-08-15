use anyhow::Result;
use clap::Parser;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

/// 接收命令行参数
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    /// 设置配置文件的路径
    #[clap(short, long, value_name = "FILE_PATH", default_value = "config.toml")]
    config: PathBuf,
}

#[derive(Deserialize, Debug)]
pub struct Config {
    pub listen_addr: String,
    pub server_addr: String,
    pub wake_command: String,
    pub geoip_db_path: String,
}

pub fn load() -> Result<Config> {
    let cli = Cli::parse();
    let config_content = fs::read_to_string(&cli.config)?;
    let config: Config = toml::from_str(&config_content)?;
    Ok(config)
}
