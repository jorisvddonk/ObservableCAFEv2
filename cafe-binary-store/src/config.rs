use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(name = "cafe-binary-store")]
pub struct Config {
    /// Bus socket path
    #[arg(long, default_value = "/tmp/cafe-bus.sock")]
    pub bus_socket: String,

    /// HTTP listen port
    #[arg(long, default_value_t = 4001)]
    pub port: u16,

    /// Data directory for binary files + JWT key + GC DB
    #[arg(long, default_value = "data/binary-store")]
    pub data_dir: PathBuf,

    /// Write JWT TTL in seconds (default: 7 days)
    #[arg(long, default_value_t = 604800)]
    pub write_ttl: u64,

    /// GC interval in seconds (default: 1 hour)
    #[arg(long, default_value_t = 3600)]
    pub gc_interval: u64,

    /// GC TTL for transient assets in seconds (default: 30 days)
    #[arg(long, default_value_t = 2592000)]
    pub gc_ttl: u64,

    /// Max bytes per chunk (default: 1 GB)
    #[arg(long, default_value_t = 1073741824)]
    pub max_chunk_bytes: u64,

    /// Public hostname/IP for URLs advertised to clients (e.g., in write credentials).
    /// Clients connect here, so this must be reachable from their network.
    /// Default: auto-detect via hostname.
    #[arg(long)]
    pub public_host: Option<String>,
}
