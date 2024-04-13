use std::{fmt::Formatter, sync::Arc};

use futures_util::stream::StreamExt;
use serde::{de, Deserialize};
use serde_json::{json, Value};
use solana_sdk::{pubkey::Pubkey, signature::Signature, transaction::Transaction};
use solana_transaction_status::{Encodable, EncodedTransaction, UiTransactionEncoding};
use tokio::{sync::RwLock, task::JoinHandle};
use tracing::info;

use crate::{constant, Miner};

#[derive(Debug, Deserialize)]
pub struct JitoResponse<T> {
    pub result: T,
}

async fn make_jito_request<T>(method: &'static str, params: Value) -> eyre::Result<T>
where
    T: de::DeserializeOwned,
{
    let response = reqwest::Client::new()
        .post("https://ny.mainnet.block-engine.jito.wtf/api/v1/bundles")
        .header("Content-Type", "application/json")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
        .send()
        .await;

    let response = match response {
        Ok(response) => response,
        Err(err) => eyre::bail!("fail to send request: {err}"),
    };

    let status = response.status();
    let text = match response.text().await {
        Ok(text) => text,
        Err(err) => eyre::bail!("fail to read response content: {err:#}"),
    };

    if !status.is_success() {
        eyre::bail!("status code: {status}, response: {text}");
    }

    let response: T = match serde_json::from_str(&text) {
        Ok(response) => response,
        Err(err) => eyre::bail!("fail to deserialize response: {err:#}, response: {text}, status: {status}"),
    };

    Ok(response)
}

pub async fn send_bundle(bundle: Vec<Transaction>) -> eyre::Result<(Signature, String)> {
    let signature = *bundle
        .first()
        .expect("empty bundle")
        .signatures
        .first()
        .expect("empty transaction");

    let bundle = bundle
        .into_iter()
        .map(|tx| match tx.encode(UiTransactionEncoding::Binary) {
            EncodedTransaction::LegacyBinary(b) => b,
            _ => panic!("impossible"),
        })
        .collect::<Vec<_>>();

    let response: JitoResponse<String> = make_jito_request("sendBundle", json!([bundle])).await?;

    Ok((signature, response.result))
}

pub fn build_bribe_ix(pubkey: &Pubkey, value: u64) -> solana_sdk::instruction::Instruction {
    solana_sdk::system_instruction::transfer(pubkey, constant::pick_jito_recipient(), value)
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct JitoTips {
    #[serde(rename = "landed_tips_25th_percentile")]
    pub p25_landed: f64,

    #[serde(rename = "landed_tips_50th_percentile")]
    pub p50_landed: f64,

    #[serde(rename = "landed_tips_75th_percentile")]
    pub p75_landed: f64,

    #[serde(rename = "landed_tips_95th_percentile")]
    pub p95_landed: f64,

    #[serde(rename = "landed_tips_99th_percentile")]
    pub p99_landed: f64,
}

impl JitoTips {
    pub fn p50(&self) -> u64 {
        (self.p50_landed * 1e9f64) as u64
    }

    pub fn p25(&self) -> u64 {
        (self.p25_landed * 1e9f64) as u64
    }
}

impl std::fmt::Display for JitoTips {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tips(p25={},p50={},p75={},p95={},p99={})",
            (self.p25_landed * 1e9f64) as u64,
            (self.p50_landed * 1e9f64) as u64,
            (self.p75_landed * 1e9f64) as u64,
            (self.p95_landed * 1e9f64) as u64,
            (self.p99_landed * 1e9f64) as u64
        )
    }
}

pub async fn subscribe_jito_tips(tips: Arc<RwLock<JitoTips>>) -> JoinHandle<()> {
    tokio::spawn({
        let tips = tips.clone();
        async move {
            let url = "ws://bundles-api-rest.jito.wtf/api/v1/bundles/tip_stream";

            loop {
                let stream = match tokio_tungstenite::connect_async(url).await {
                    Ok((ws_stream, _)) => ws_stream,
                    Err(err) => {
                        tracing::error!("fail to connect to jito tip stream: {err:#}");
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                        continue;
                    }
                };

                let (_, read) = stream.split();

                read.for_each(|message| async {
                    let data = match message {
                        Ok(data) => data.into_data(),
                        Err(err) => {
                            tracing::error!("fail to read jito tips message: {err:#}");
                            return;
                        }
                    };

                    let data = match serde_json::from_slice::<Vec<JitoTips>>(&data) {
                        Ok(t) => t,
                        Err(err) => {
                            tracing::error!("fail to parse jito tips: {err:#}");
                            return;
                        }
                    };

                    if data.is_empty() {
                        return;
                    }

                    *tips.write().await = *data.first().unwrap();
                })
                .await;

                tracing::info!("jito tip stream disconnected, retries in 5 seconds");
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        }
    })
}

impl Miner {
    pub async fn jito_tip_stream(&self) {
        let tips = Arc::new(RwLock::new(JitoTips::default()));

        subscribe_jito_tips(tips.clone()).await;

        loop {
            let tips = *tips.read().await;

            info!(%tips);

            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
    }
}
