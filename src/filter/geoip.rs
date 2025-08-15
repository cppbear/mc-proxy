use maxminddb::{Reader, geoip2};
use std::net::IpAddr;
use tracing::{info, warn};

pub fn check_ip(ip: IpAddr, geo_reader: &Reader<Vec<u8>>) -> bool {
    match geo_reader.lookup::<geoip2::Country>(ip) {
        Ok(country_data) => {
            if let Some(country_code) = country_data
                .and_then(|data| data.country)
                .and_then(|c| c.iso_code)
            {
                info!("Client IP {} resolved to country: {}", ip, country_code);
                if country_code != "CN" && country_code != "PRIVATE" {
                    warn!(
                        "Blocking connection from {} ({}) as it's not from China.",
                        ip, country_code
                    );
                    return false; // 阻止
                }
            } else {
                warn!(
                    "Could not determine country for IP {}. Allowing connection by default.",
                    ip
                );
            }
        }
        Err(e) => {
            // IP 地址在数据库中未找到，可能是私有地址或数据库不完整
            warn!(
                "IP lookup failed for {}: {}. Allowing connection by default.",
                ip, e
            );
        }
    }
    true // 默认允许
}
