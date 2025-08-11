use anyhow::Result;
use clap::Parser;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::{error, info, instrument, level_filters::LevelFilter, warn};
use tracing_subscriber::FmtSubscriber;

// 接收命令行参数
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    /// 设置配置文件的路径
    #[clap(short, long, value_name = "FILE_PATH", default_value = "config.toml")]
    config: PathBuf,
}

// 匹配 TOML 配置文件的结构
#[derive(Deserialize, Debug)]
struct Config {
    listen_addr: String,
    mc_server_addr: String,
    wake_command: String,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ServerStatus {
    Offline,
    WakingUp,
    Online,
}

type SharedServerState = Arc<Mutex<ServerStatus>>;

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志记录器
    let subscriber = FmtSubscriber::builder()
        .with_max_level(LevelFilter::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    // 解析命令行参数并读取配置文件
    let cli = Cli::parse();
    info!("Loading configuration from: {:?}", cli.config);
    let config_content = fs::read_to_string(&cli.config)?;
    let config: Config = toml::from_str(&config_content)?;
    info!("Configuration loaded: {:?}", config);

    // 将配置包装在 Arc 中以便在任务间高效共享
    let config = Arc::new(config);

    let server_state = Arc::new(Mutex::new(ServerStatus::Offline));

    // 使用配置中的监听地址
    let listener = TcpListener::bind(&config.listen_addr).await?;
    info!("Proxy server starting on {}...", config.listen_addr);

    loop {
        let (client_socket, client_addr) = listener.accept().await?;
        info!("Accepted connection from {}", client_addr);

        let state_clone = Arc::clone(&server_state);
        // 克隆配置的 Arc 引用
        let config_clone = Arc::clone(&config);

        tokio::spawn(async move {
            if let Err(e) = handle_connection(client_socket, state_clone, config_clone).await {
                error!("Error handling connection from {}: {}", client_addr, e);
            }
        });
    }
}

#[instrument(skip(client_socket, state, config))]
async fn handle_connection(
    mut client_socket: TcpStream,
    state: SharedServerState,
    config: Arc<Config>,
) -> Result<()> {
    // 检查并触发唤醒流程（如果需要）
    wait_for_server_online(state.clone(), config.clone()).await?;

    // 确定服务器已经在线
    info!(
        "Connecting to the backend MC server: {}",
        config.mc_server_addr
    );
    let mut mc_server_socket = TcpStream::connect(&config.mc_server_addr).await?;
    info!("Connection successful. Forwarding traffic.");

    let (bytes_sent, bytes_received) =
        match io::copy_bidirectional(&mut client_socket, &mut mc_server_socket).await {
            Ok((sent, received)) => (sent, received),
            Err(e) => {
                error!("Error during traffic forwarding: {}", e);
                return Err(e.into());
            }
        };

    info!(
        "Connection closed. Sent {} bytes, received {} bytes.",
        bytes_sent, bytes_received
    );

    Ok(())
}

async fn wait_for_server_online(state: SharedServerState, config: Arc<Config>) -> Result<()> {
    loop {
        let mut state_guard = state.lock().await;
        match *state_guard {
            ServerStatus::Online => {
                info!("Server is online. Proceeding.");
                return Ok(());
            }
            ServerStatus::WakingUp => {
                info!("Server is waking up. Waiting...");
                drop(state_guard);
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
            ServerStatus::Offline => {
                info!("Server is offline. Initiating wake-up sequence...");
                *state_guard = ServerStatus::WakingUp;
                drop(state_guard);

                info!("Executing wake-up command: '{}'", config.wake_command);
                let mut cmd = Command::new("sh");
                cmd.arg("-c")
                    .arg(&config.wake_command)
                    .env_remove("http_proxy")
                    .env_remove("HTTP_PROXY")
                    .env_remove("https_proxy")
                    .env_remove("HTTPS_PROXY")
                    .env_remove("socks_proxy")
                    .env_remove("SOCKS_PROXY")
                    .env_remove("all_proxy")
                    .env_remove("ALL_PROXY");

                let mut child = cmd.spawn()?;
                let status = child.wait().await?;
                if status.success() {
                    info!("Wake-up command executed successfully.");
                } else {
                    warn!(
                        "Wake-up command failed with status: {}. Will still attempt to connect.",
                        status
                    );
                }

                info!(
                    "Waiting for MC Server to respond at {}...",
                    config.mc_server_addr
                );
                loop {
                    if TcpStream::connect(&config.mc_server_addr).await.is_ok() {
                        let mut final_state_guard = state.lock().await;
                        *final_state_guard = ServerStatus::Online;
                        info!("Server is now online!");
                        return Ok(());
                    }
                    info!("Server not ready yet. Retrying in 10 seconds...");
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
            }
        }
    }
}
