use anyhow::Result;
use clap::Parser;
use maxminddb::{Reader, geoip2};
use serde::Deserialize;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::{error, info, instrument, level_filters::LevelFilter, warn};
use tracing_subscriber::FmtSubscriber;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

// 接收命令行参数
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    /// 设置配置文件的路径
    #[clap(short, long, value_name = "FILE_PATH", default_value = "config.toml")]
    config: PathBuf,
}

#[derive(Deserialize, Debug)]
struct Config {
    listen_addr: String,
    server_addr: String,
    wake_command: String,
    geoip_db_path: String,
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
    let subscriber = FmtSubscriber::builder()
        .with_max_level(LevelFilter::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let cli = Cli::parse();
    info!("Loading configuration from: {:?}", cli.config);
    let config_content = fs::read_to_string(&cli.config)?;
    let config: Config = toml::from_str(&config_content)?;
    info!("Configuration loaded: {:?}", config);
    let config = Arc::new(config);

    info!("Loading GeoIP database from: {}", &config.geoip_db_path);
    let geo_reader = Arc::new(Reader::open_readfile(&config.geoip_db_path)?);

    let server_state = Arc::new(Mutex::new(ServerStatus::Offline));

    let listener = TcpListener::bind(&config.listen_addr).await?;
    info!("Proxy server starting on {}...", config.listen_addr);

    loop {
        let (client_socket, client_addr) = listener.accept().await?;
        info!("Accepted connection from {}", client_addr);

        let state_clone = Arc::clone(&server_state);
        let config_clone = Arc::clone(&config);
        let geo_reader_clone = geo_reader.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_connection(
                client_socket,
                client_addr,
                state_clone,
                config_clone,
                geo_reader_clone,
            )
            .await
            {
                error!("Error handling connection from {}: {}", client_addr, e);
            }
        });
    }
}

#[instrument(skip(client_socket, client_addr, state, config, geo_reader))]
async fn handle_connection(
    mut client_socket: TcpStream,
    client_addr: SocketAddr,
    state: SharedServerState,
    config: Arc<Config>,
    geo_reader: Arc<Reader<Vec<u8>>>,
) -> Result<()> {
    let client_ip = client_addr.ip();
    match geo_reader.lookup::<geoip2::Country>(client_ip) {
        Ok(country_data) => {
            if let Some(country_code) = country_data
                .and_then(|data| data.country)
                .and_then(|c| c.iso_code)
            {
                info!(
                    "Client IP {} resolved to country: {}",
                    client_ip, country_code
                );
                if country_code != "CN" {
                    warn!(
                        "Blocking connection from {} ({}) as it's not from China.",
                        client_ip, country_code
                    );
                    // 直接返回，会隐式地关闭连接
                    return Ok(());
                }
            } else {
                warn!(
                    "Could not determine country for IP {}. Allowing connection by default.",
                    client_ip
                );
            }
        }
        Err(e) => {
            // IP 地址在数据库中未找到，可能是私有地址或数据库不完整
            warn!(
                "IP lookup failed for {}: {}. Allowing connection by default.",
                client_ip, e
            );
        }
    }

    let mut server_socket;

    // 使用循环来处理“连接 -> 失败则唤醒 -> 重连”的逻辑
    loop {
        // 直接尝试连接进行在线检查
        info!(
            "Attempting to connect to backend server: {} (timeout: {:?})",
            config.server_addr, CONNECT_TIMEOUT
        );
        let connect_future = TcpStream::connect(&config.server_addr);
        match tokio::time::timeout(CONNECT_TIMEOUT, connect_future).await {
            Ok(Ok(socket)) => {
                // 连接成功
                info!("Successfully connected to backend server.");
                server_socket = socket;

                // 确保共享状态是 Online，以便其他并发连接可以跳过唤醒流程
                let mut state_guard = state.lock().await;
                if *state_guard != ServerStatus::Online {
                    info!("Updating shared state to Online.");
                    *state_guard = ServerStatus::Online;
                }
                break; // 跳出循环，进行流量转发
            }
            Ok(Err(e)) => {
                // 连接操作本身返回了错误 (例如 Connection Refused)，服务器不在线
                warn!(
                    "Failed to connect to backend (connection error): {}. Assuming it's offline.",
                    e
                );
                trigger_wakeup_and_wait(state.clone(), config.clone()).await?;
                info!("Wake-up sequence finished. Retrying connection...");
            }
            Err(_) => {
                // 连接操作超时，服务器不在线
                warn!("Failed to connect to backend (timed out). Assuming it's offline.");
                trigger_wakeup_and_wait(state.clone(), config.clone()).await?;
                info!("Wake-up sequence finished. Retrying connection...");
            }
        }
    }

    // --- 流量转发 ---
    info!("Connection established. Forwarding traffic.");
    let (bytes_sent, bytes_received) =
        match io::copy_bidirectional(&mut client_socket, &mut server_socket).await {
            Ok((sent, received)) => (sent, received),
            Err(e) => {
                error!("Error during traffic forwarding: {}", e);
                // 如果转发时出错，也认为服务器可能掉线了，重置状态
                let mut state_guard = state.lock().await;
                *state_guard = ServerStatus::Offline;
                warn!("Resetting server state to Offline due to a forwarding error.");
                return Err(e.into());
            }
        };

    info!(
        "Connection closed. Sent {} bytes, received {} bytes.",
        bytes_sent, bytes_received
    );

    Ok(())
}

/// 一个辅助函数，用于封装“重置状态 -> 触发唤醒”的通用逻辑
#[instrument(skip(state, config))]
async fn trigger_wakeup_and_wait(state: SharedServerState, config: Arc<Config>) -> Result<()> {
    // 在调用唤醒流程前，立即将状态重置为 Offline
    {
        let mut state_guard = state.lock().await;
        if *state_guard != ServerStatus::Offline {
            info!("Resetting server state to Offline due to connection failure.");
            *state_guard = ServerStatus::Offline;
        }
    } // 锁在这里被释放

    wait_for_server_online(state, config).await
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
                    config.server_addr
                );
                loop {
                    if TcpStream::connect(&config.server_addr).await.is_ok() {
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
