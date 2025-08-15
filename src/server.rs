use crate::backend::state;
use crate::config::Config;
use crate::connection;
use anyhow::Result;
use maxminddb::Reader;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{error, info};

pub async fn run(config: Arc<Config>) -> Result<()> {
    info!("Loading GeoIP database from: {}", &config.geoip_db_path);
    let geo_reader = Arc::new(Reader::open_readfile(&config.geoip_db_path)?);

    let server_state = state::new_shared_state();

    let listener = TcpListener::bind(&config.listen_addr).await?;
    info!("Proxy server starting on {}...", config.listen_addr);

    loop {
        let (client_socket, client_addr) = listener.accept().await?;
        info!("Accepted connection from {}", client_addr);

        let state_clone = Arc::clone(&server_state);
        let config_clone = Arc::clone(&config);
        let geo_reader_clone = geo_reader.clone();

        tokio::spawn(async move {
            if let Err(e) = connection::handle(
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
