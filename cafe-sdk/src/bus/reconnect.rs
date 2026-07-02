use std::time::Duration;
use tracing::{info, warn};

/// Wraps an async operation with an exponential-backoff reconnect loop.
///
/// The `operation` closure is called repeatedly. If it returns an error the
/// loop waits with backoff and retries. If it returns `Ok(())` the loop
/// exits cleanly.
pub async fn run_with_reconnect<F, Fut>(label: &'static str, operation: F)
where
    F: Fn() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<(), anyhow::Error>> + Send,
{
    let mut delay = Duration::from_secs(1);
    loop {
        match operation().await {
            Ok(()) => {
                info!("{}: clean shutdown", label);
                return;
            }
            Err(e) => {
                warn!("{}: error (retrying in {delay:?}): {e}", label);
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(30));
            }
        }
    }
}
