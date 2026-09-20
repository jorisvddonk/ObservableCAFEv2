//! Cron ticks for background JS agents.
//!
//! Mirrors `cafe-agent-runtime/src/scheduler.rs`: each registered cron
//! expression publishes a `cafe.flow.signal = "tick"` null chunk into the
//! agent's background session, which [`classify_event`] turns into a `tick`
//! event for the agent's `for await` loop.
//!
//! [`classify_event`]: crate::bridge::classify_event

use anyhow::Result;
use cafe_sdk::bus::BusClient;
use cafe_sdk::{keys, Chunk};
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info};

pub struct JsScheduler {
    scheduler: JobScheduler,
}

impl JsScheduler {
    pub async fn new() -> Result<Self> {
        Ok(Self {
            scheduler: JobScheduler::new().await?,
        })
    }

    /// Register `cron_expr` (5-field or 7-field with seconds) ticking
    /// `agent_name`'s session. An invalid expression is an error — callers
    /// log it against the agent and continue with the rest.
    pub async fn schedule(
        &self,
        agent_name: String,
        cron_expr: &str,
        socket_path: String,
    ) -> Result<()> {
        let name = agent_name.clone();
        let sp = socket_path.clone();
        self.scheduler
            .add(Job::new_async(cron_expr, move |_, _| {
                let name = name.clone();
                let sp = sp.clone();
                Box::pin(async move {
                    info!("scheduler: ticking JS agent {name}");
                    if let Err(e) = tick_agent(&sp, &name).await {
                        error!("scheduler: failed to tick JS agent {name}: {e}");
                    }
                })
            })?)
            .await?;
        info!("scheduler: registered cron '{cron_expr}' for JS agent {agent_name}");
        Ok(())
    }

    pub async fn start(&self) -> Result<()> {
        self.scheduler.start().await?;
        Ok(())
    }
}

/// Publish a flow tick into an agent's background session (fire-and-forget;
/// the one-shot `publish` deprecation does not apply — no reply is expected).
pub async fn tick_agent(socket_path: &str, agent_name: &str) -> Result<()> {
    let client = BusClient::unix(socket_path);
    let chunk = Chunk::new_null("com.nominal.cafe-agent-js")
        .with_annotation(keys::CAFE_FLOW_SIGNAL, "tick");
    #[allow(deprecated)]
    client.publish(agent_name, chunk).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_field_cron_rejected_documents_seven_field_requirement() {
        // tokio-cron-scheduler 0.10 (cron 0.12) requires 7-field expressions
        // with seconds. NOTE: the legacy `rss-summarizer.toml` 5-field
        // schedule hits this too — JS agents must use 7-field.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let sched = JsScheduler::new().await.unwrap();
            assert!(sched
                .schedule("x".into(), "0 7 * * *", "/tmp/nope.sock".into())
                .await
                .is_err());
        });
    }

    #[test]
    fn seven_field_cron_with_seconds_accepted() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let sched = JsScheduler::new().await.unwrap();
            sched
                .schedule("x".into(), "*/10 * * * * * *", "/tmp/nope.sock".into())
                .await
                .unwrap();
        });
    }

    #[test]
    fn invalid_cron_rejected() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let sched = JsScheduler::new().await.unwrap();
            assert!(sched
                .schedule("x".into(), "not a cron", "/tmp/nope.sock".into())
                .await
                .is_err());
        });
    }
}
