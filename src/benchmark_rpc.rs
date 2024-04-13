use std::time::{Duration, Instant};

use clap::Parser;
use solana_client::nonblocking::rpc_client::RpcClient;

use crate::Miner;

#[derive(Parser, Debug, Clone)]
pub struct BenchmarkRpcArgs {
    #[clap(long, default_value = "500")]
    pub timeout_ms: u64,

    #[arg(long, value_delimiter = ',')]
    pub endpoints: Vec<String>,
}

impl Miner {
    pub async fn benchmark_rpc(&self, args: &BenchmarkRpcArgs) {
        let mut tasks = vec![];
        let timeout = Duration::from_millis(args.timeout_ms);

        for cluster in &args.endpoints {
            let cluster = cluster.clone();
            let client = RpcClient::new_with_timeout(cluster.clone(), timeout);

            tasks.push(tokio::spawn(async move {
                (cluster.to_string(), Self::test_cluster(client).await)
            }));
        }

        let mut result = Vec::with_capacity(tasks.len());

        for task_result in tasks {
            let (rpc, metric) = task_result.await.unwrap();

            match metric {
                Some((slot, latency)) => {
                    tracing::info!(rpc = %rpc, slot = slot, latency = ?latency, "    rpc benchmark result");
                }
                None => {
                    tracing::info!(rpc = %rpc, "    rpc benchmark failed");
                }
            }

            result.push((rpc, metric));
        }

        let mut result = result
            .into_iter()
            .filter(|result| result.1.is_some())
            .collect::<Vec<_>>();

        // sort by rule: largest slot first and lowest latency first
        result.sort_by(|a, b| match (a.1, b.1) {
            (Some((slot_a, latency_a)), Some((slot_b, latency_b))) => {
                if slot_a == slot_b {
                    latency_a.cmp(&latency_b)
                } else {
                    slot_b.cmp(&slot_a)
                }
            }

            _ => std::cmp::Ordering::Equal,
        });

        tracing::info!("ordered result:");

        for (rpc, metric) in result {
            let (slot, latency) = metric.unwrap();
            tracing::info!(rpc = %rpc, slot = slot, latency = ?latency, "    rpc benchmark result");
        }
    }

    pub async fn test_cluster(client: RpcClient) -> Option<(u64, Duration)> {
        let start = Instant::now();
        let slot = client.get_slot().await.ok()?;
        Some((slot, start.elapsed()))
    }
}
