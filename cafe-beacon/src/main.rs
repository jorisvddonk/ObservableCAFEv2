use anyhow::Result;
use cafe_sdk::bus::{BusClient, IrohConfig};
use cafe_sdk::{Chunk, SessionConfig};
use std::time::Duration;

const PRODUCER: &str = "com.nominal.cafe-beacon";
const INTERVAL_SECS: u64 = 30;

fn get_hostname() -> Result<String> {
    let mut buf = vec![0u8; 256];
    let ret = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if ret != 0 {
        anyhow::bail!("gethostname failed: {}", std::io::Error::last_os_error());
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    Ok(String::from_utf8(buf[..len].to_vec())?)
}

fn get_loadavg() -> Result<String> {
    let mut load = [0.0f64; 3];
    let ret = unsafe { libc::getloadavg(load.as_mut_ptr(), 3) };
    if ret < 1 {
        anyhow::bail!("getloadavg failed");
    }
    Ok(format!("{:.2} {:.2} {:.2}", load[0], load[1], load[2]))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let hostname = get_hostname()?;
    let session_id = format!("loadavg.{}", hostname);

    let bus = if let Some(addr_file) = std::env::var("CAFE_BUS_IROH_ADDR_FILE").ok()
        .filter(|s| !s.is_empty())
    {
        let json = std::fs::read_to_string(&addr_file)?;
        let mut cfg = IrohConfig::from_bus_addr_json(&json)
            .ok_or_else(|| anyhow::anyhow!("failed to parse iroh addr file: {}", addr_file))?;
        if let Ok(v) = std::env::var("CAFE_BUS_IROH_DIRECT") {
            if v == "1" || v == "true" {
                cfg = cfg.with_direct();
            }
        }
        tracing::info!("connecting via iroh (addr file: {})", addr_file);
        BusClient::from_iroh_config(cfg).await?
    } else if let Some(cfg) = IrohConfig::from_cli(None, None, None) {
        tracing::info!("connecting via iroh");
        BusClient::from_iroh_config(cfg).await?
    } else {
        let socket_path =
            std::env::var("CAFE_BUS_SOCKET").unwrap_or_else(|_| "/tmp/cafe-bus.sock".into());
        cafe_sdk::bus::wait_for_bus(&socket_path, Duration::from_millis(500), 60).await?;
        tracing::info!("connecting via unix socket: {}", socket_path);
        BusClient::unix(socket_path)
    };

    match bus
        .create_session(&session_id, PRODUCER, SessionConfig { tags: Some(vec!["monitoring".into()]), ..Default::default() })
        .await
    {
        Ok(()) => tracing::info!("created session: {}", session_id),
        Err(e) => tracing::warn!("session may already exist: {}", e),
    }

    let mut sub = bus.subscribe_session(&session_id).await?;

    let mut tick = 0u64;
    loop {
        match get_loadavg() {
            Ok(load) => {
                let mut chunk = Chunk::new_text(&load, PRODUCER).as_transient().with_retain(60);
                if let Some(info) = bus.connection_info() {
                    chunk = chunk.with_annotation("iroh.connections", info);
                }
                if let Err(e) = sub.publish(chunk).await {
                    tracing::warn!("publish failed: {}", e);
                } else {
                    tracing::info!("published loadavg: {}", load);
                }
            }
            Err(e) => tracing::warn!("get_loadavg failed: {}", e),
        }

        tick += 1;
        if tick % 4 == 0 {
            bus.log_connection_paths();
        }

        tokio::time::sleep(Duration::from_secs(INTERVAL_SECS)).await;
    }
}
