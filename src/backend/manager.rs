use crate::backend::state::{ServerStatus, SharedServerState};
use crate::config::Config;
use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::process::Command;
use tracing::{info, warn};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// 尝试连接到后端服务器，如果失败，则触发唤醒逻辑并等待其上线。
pub async fn connect_or_wake_server(
    state: SharedServerState,
    config: Arc<Config>,
) -> Result<TcpStream> {
    loop {
        info!(
            "Attempting to connect to backend server: {} (timeout: {:?})",
            config.server_addr, CONNECT_TIMEOUT
        );
        let connect_future = TcpStream::connect(&config.server_addr);
        match tokio::time::timeout(CONNECT_TIMEOUT, connect_future).await {
            Ok(Ok(socket)) => {
                info!("Successfully connected to backend server.");
                let mut state_guard = state.lock().await;
                if *state_guard != ServerStatus::Online {
                    info!("Updating shared state to Online.");
                    *state_guard = ServerStatus::Online;
                }
                return Ok(socket);
            }
            Ok(Err(e)) => {
                warn!(
                    "Failed to connect to backend (connection error): {}. Assuming it's offline.",
                    e
                );
                wait_for_server_after_wakeup(state.clone(), config.clone()).await?;
            }
            Err(_) => {
                warn!("Failed to connect to backend (timed out). Assuming it's offline.");
                wait_for_server_after_wakeup(state.clone(), config.clone()).await?;
            }
        }
    }
}

/// 封装了唤醒服务器并等待其准备就绪的完整流程。
async fn wait_for_server_after_wakeup(state: SharedServerState, config: Arc<Config>) -> Result<()> {
    // 在调用唤醒流程前，立即将状态重置为 Offline
    {
        let mut state_guard = state.lock().await;
        if *state_guard != ServerStatus::Offline {
            info!("Resetting server state to Offline due to connection failure.");
            *state_guard = ServerStatus::Offline;
        }
    }

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

                execute_wake_command(config.clone()).await?;

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

/// 执行在配置文件中定义的唤醒命令。
async fn execute_wake_command(config: Arc<Config>) -> Result<()> {
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
    Ok(())
}
