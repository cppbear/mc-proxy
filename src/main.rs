mod backend;
mod config;
mod connection;
mod filter;
mod protocol;
mod server;

use anyhow::Result;
use std::sync::Arc;
use tracing::info;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    let subscriber = FmtSubscriber::builder()
        .with_max_level(LevelFilter::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    // 加载配置
    info!("Loading configuration...");
    let config = Arc::new(config::load()?);
    info!("Configuration loaded: {:?}", config);

    // 启动服务器
    server::run(config).await
}
