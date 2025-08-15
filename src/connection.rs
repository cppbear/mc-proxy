use crate::backend::{manager, state::SharedServerState};
use crate::config::Config;
use crate::filter::{geoip, handshake};
use anyhow::Result;
use maxminddb::Reader;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io;
use tokio::net::TcpStream;
use tracing::{error, info, instrument, warn};

const FIRST_PACKET_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PACKET_SIZE: usize = 1024;

#[instrument(skip(client_socket, client_addr, state, config, geo_reader))]
pub async fn handle(
    mut client_socket: TcpStream,
    client_addr: SocketAddr,
    state: SharedServerState,
    config: Arc<Config>,
    geo_reader: Arc<Reader<Vec<u8>>>,
) -> Result<()> {
    // GeoIP 过滤
    if !geoip::check_ip(client_addr.ip(), &geo_reader) {
        return Ok(()); // 连接被阻止
    }

    // 握手包验证
    let mut buffer = vec![0u8; MAX_PACKET_SIZE];
    let bytes_read =
        match tokio::time::timeout(FIRST_PACKET_TIMEOUT, client_socket.peek(&mut buffer)).await {
            Ok(Ok(n)) if n > 0 => n,
            Ok(Ok(_)) => {
                info!(
                    "Client {} closed connection before sending data.",
                    client_addr
                );
                return Ok(());
            }
            Ok(Err(e)) => {
                error!("Failed to peek first packet from {}: {}", client_addr, e);
                return Err(e.into());
            }
            Err(_) => {
                warn!(
                    "Client {} timed out waiting for first packet. Dropping.",
                    client_addr
                );
                return Ok(());
            }
        };

    let packet_data = &buffer[..bytes_read];
    if !handshake::is_valid_handshake(packet_data) {
        warn!(
            "Invalid first packet from {}. Dropping connection.",
            client_addr
        );
        return Ok(());
    }
    info!(
        "First packet validation successful for {}. Proceeding to connect backend.",
        client_addr
    );

    // 连接或唤醒后端服务器
    let mut server_socket = manager::connect_or_wake_server(state.clone(), config.clone()).await?;

    // 流量转发
    info!("Connection established. Forwarding traffic.");
    let (bytes_sent, bytes_received) =
        match io::copy_bidirectional(&mut client_socket, &mut server_socket).await {
            Ok((sent, received)) => (sent, received),
            Err(e) => {
                error!("Error during traffic forwarding: {}", e);
                // 如果转发时出错，重置状态
                let mut state_guard = state.lock().await;
                *state_guard = crate::backend::state::ServerStatus::Offline;
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
