use crate::error::SdkError;
use std::time::Duration;
use tokio::net::UnixStream;
use tracing::info;

/// Poll for the bus socket to become available.
///
/// Checks every `interval` for up to `max_retries` attempts. Returns an
/// error if the bus is not reachable after all retries.
pub async fn wait_for_bus(
    socket_path: &str,
    interval: Duration,
    max_retries: u32,
) -> Result<(), SdkError> {
    for attempt in 1..=max_retries {
        match UnixStream::connect(socket_path).await {
            Ok(_) => {
                info!("cafe-sdk: bus ready after {attempt} attempt(s)");
                return Ok(());
            }
            Err(_) if attempt < max_retries => {
                tokio::time::sleep(interval).await;
            }
            Err(_) => {
                return Err(SdkError::BusNotReady {
                    retries: max_retries,
                });
            }
        }
    }
    Err(SdkError::BusNotReady {
        retries: max_retries,
    })
}
