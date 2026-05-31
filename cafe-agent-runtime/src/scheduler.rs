use anyhow::Result;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info};

pub struct AgentScheduler {
    scheduler: JobScheduler,
}

impl AgentScheduler {
    pub async fn new() -> Result<Self> {
        Ok(Self {
            scheduler: JobScheduler::new().await?,
        })
    }

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
                    info!("scheduler: ticking agent {}", name);
                    if let Err(e) = crate::lifecycle::tick_agent(&sp, &name).await {
                        error!("scheduler: failed to tick agent {}: {}", name, e);
                    }
                })
            })?)
            .await?;

        info!(
            "scheduler: registered cron '{}' for agent {}",
            cron_expr, agent_name
        );
        Ok(())
    }

    pub async fn start(&self) -> Result<()> {
        self.scheduler.start().await?;
        Ok(())
    }
}
