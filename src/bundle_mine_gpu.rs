use std::{
    collections::HashMap,
    sync::{atomic::AtomicUsize, Arc},
    time::{Duration, Instant},
};

use clap::Parser;
use itertools::Itertools;
use ore::state::Bus;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    clock::Slot,
    hash::Hash,
    pubkey::Pubkey,
    signature::{Keypair, Signature},
    signer::Signer,
    transaction::Transaction,
};
use tokio::sync::{
    mpsc::{channel, Sender},
    RwLock,
};
use tracing::{debug, error, info, warn};

use crate::{
    constant,
    format_duration,
    format_reward,
    jito,
    jito::{subscribe_jito_tips, JitoTips},
    utils,
    wait_return,
    Miner,
};

#[derive(Debug, Clone, Parser)]
pub struct BundleMineGpuArgs {
    #[arg(long, help = "The folder that contains all the keys used to claim $ORE")]
    pub key_folder: String,

    #[arg(
        long,
        default_value = "0",
        help = "The maximum tip to pay for jito. Set to 0 to disable adaptive tip"
    )]
    pub max_adaptive_tip: u64,

    #[arg(long, default_value = "2", help = "The maximum number of buses to use for mining")]
    pub max_buses: usize,
}

impl Miner {
    pub async fn bundle_mine_gpu(&self, args: &BundleMineGpuArgs) {
        if args.max_buses == 0 {
            panic!("max buses must be greater than 0");
        }

        let client = Miner::get_client_confirmed(&self.rpc);

        let all_signers = Self::read_keys(&args.key_folder)
            .into_iter()
            .map(Box::new)
            .collect::<Vec<_>>();

        if all_signers.len() % Accounts::size() != 0 {
            panic!("number of keys must be a multiple of {}", Accounts::size());
        }

        info!("{} keys loaded", all_signers.len());

        let idle_accounts_counter = Arc::new(AtomicUsize::new(all_signers.len()));

        // Setup channels
        let (ch_accounts, mut ch_accounts_receiver) = channel::<Accounts>(all_signers.len() / Accounts::size());

        let batches = all_signers
            .into_iter()
            .chunks(Accounts::size())
            .into_iter()
            .enumerate()
            .map(|(i, signers)| {
                let signers = signers.collect::<Vec<_>>();

                Accounts {
                    id: i,
                    pubkey: signers.iter().map(|k| k.pubkey()).collect(),
                    proof_pda: signers
                        .iter()
                        .map(|k| utils::get_proof_pda_no_cache(k.pubkey()))
                        .collect(),
                    signers,
                    release_stuff: (ch_accounts.clone(), idle_accounts_counter.clone()),
                }
            })
            .collect::<Vec<_>>();

        for signers in batches {
            ch_accounts.send(signers).await.unwrap();
        }

        info!("splitted signers into batches");

        // Subscribe tip stream
        let tips = Arc::new(RwLock::new(JitoTips::default()));
        subscribe_jito_tips(tips.clone()).await;
        info!("subscribed to jito tip stream");

        loop {
            let mut batch = Vec::new();

            while let Ok(accounts) = ch_accounts_receiver.try_recv() {
                batch.push(accounts);
                if batch.len() >= 4 {
                    break;
                }
            }

            if batch.is_empty() {
                debug!("no more batches, waiting for more signers");
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }

            let idle_accounts = idle_accounts_counter
                .fetch_sub(batch.len() * Accounts::size(), std::sync::atomic::Ordering::Relaxed) -
                batch.len() * Accounts::size();

            loop {
                let result = self
                    .mine_with_accounts(args, client.clone(), tips.clone(), batch, idle_accounts)
                    .await;

                batch = match result {
                    Some(batch_to_retry) => batch_to_retry,
                    None => break,
                }
            }
        }
    }

    async fn mine_with_accounts(
        &self,
        args: &BundleMineGpuArgs,
        client: Arc<RpcClient>,
        tips: Arc<RwLock<JitoTips>>,
        batch: Vec<Accounts>,
        idle_accounts: usize,
    ) -> Option<Vec<Accounts>> {
        let (treasury, clock, buses) = match Self::get_system_accounts(&client).await {
            Ok(accounts) => accounts,
            Err(err) => {
                error!("fail to fetch system accounts: {err:#}");
                wait_return!(500, Some(batch));
            }
        };

        let all_pubkey = batch
            .iter()
            .flat_map(|accounts| accounts.pubkey.clone())
            .collect::<Vec<_>>();

        let proof_pda = batch
            .iter()
            .flat_map(|accounts| accounts.proof_pda.clone())
            .collect::<Vec<_>>();

        let signer_balances = match Self::get_balances(&client, &all_pubkey).await {
            Ok(b) => b,
            Err(err) => {
                error!("fail to get signers balances: {err:#}");
                wait_return!(500, Some(batch));
            }
        };

        let proofs = match Self::get_proof_accounts(&client, &proof_pda).await {
            Ok(proofs) => proofs,
            Err(err) => {
                error!("fail to fetch proof accounts: {err:#}");
                wait_return!(500, Some(batch));
            }
        };

        let reset_threshold = treasury.last_reset_at.saturating_add(ore::EPOCH_DURATION);
        let time_to_next_epoch = Self::get_time_to_next_epoch(&treasury, &clock, reset_threshold);

        let hash_and_pubkey = all_pubkey
            .iter()
            .zip(proofs.iter())
            .map(|(signer, proof)| (solana_sdk::keccak::Hash::new_from_array(proof.hash.0), *signer))
            .collect::<Vec<_>>();
        let (mining_duration, mining_results) = self
            .mine_hashes_gpu(&treasury.difficulty.into(), &hash_and_pubkey)
            .await;

        if mining_duration > time_to_next_epoch {
            warn!("mining took too long, waiting for next epoch");
            wait_return!(time_to_next_epoch.as_millis() as u64, Some(batch));
        } else {
            info!(
                accounts = Accounts::size() * batch.len(),
                accounts.idle = idle_accounts,
                mining = format_duration!(mining_duration),
                "mining done"
            );
        }

        let available_bus = Self::find_buses(buses, treasury.reward_rate.saturating_mul(all_pubkey.len() as u64 + 20))
            .into_iter()
            .take(args.max_buses)
            .collect_vec();
        if available_bus.is_empty() {
            warn!("no bus available for mining, waiting for next epoch",);
            wait_return!(time_to_next_epoch.as_millis() as u64, Some(batch));
        }

        let rewards = treasury.reward_rate.saturating_mul(25);
        let tip = self.priority_fee.expect("priority fee should be set");

        let (send_at_slot, blockhash) = match Self::get_latest_blockhash_and_slot(&client).await {
            Ok(value) => value,
            Err(err) => {
                error!("fail to get latest blockhash: {err:#}");
                wait_return!(time_to_next_epoch.as_millis() as u64, Some(batch));
            }
        };

        let task = SendBundleTask {
            client,
            tips,
            batch,
            available_bus,
            signer_balances,
            mining_duration,
            mining_results,
            rewards,
            tip,
            max_tip: args.max_adaptive_tip,
            slot: send_at_slot,
            blockhash,
        };

        tokio::spawn(task.work());

        None
    }
}

struct Accounts {
    pub id: usize,
    #[allow(clippy::vec_box)]
    pub signers: Vec<Box<Keypair>>,
    pub pubkey: Vec<Pubkey>,
    pub proof_pda: Vec<Pubkey>,
    release_stuff: (Sender<Accounts>, Arc<AtomicUsize>),
}

impl Accounts {
    pub async fn release(self) {
        self.release_stuff
            .1
            .fetch_add(Self::size(), std::sync::atomic::Ordering::Relaxed);

        self.release_stuff
            .0
            .clone()
            .send(self)
            .await
            .expect("failed to release accounts");
    }

    pub const fn size() -> usize {
        25
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn watch_signatures(
        self,
        client: Arc<RpcClient>,
        signatures: Vec<Signature>,
        tip: u64,
        tips: Arc<RwLock<JitoTips>>,
        send_at_slot: Slot,
        sent_at_time: Instant,
        rewards: u64,
    ) {
        let mut latest_slot = send_at_slot;
        let mut landed_tx = vec![];

        while landed_tx.is_empty() && latest_slot < send_at_slot + constant::SLOT_EXPIRATION {
            tokio::time::sleep(Duration::from_secs(2)).await;
            debug!(
                acc.id = self.id,
                slot.current = latest_slot,
                slot.send = send_at_slot,
                ?signatures,
                "checking bundle status"
            );

            let (statuses, slot) = match Miner::get_signature_statuses(&client, &signatures).await {
                Ok(value) => value,
                Err(err) => {
                    error!(
                        acc.id = self.id,
                        slot.current = latest_slot,
                        slot.send = send_at_slot,
                        "fail to get bundle status: {err:#}"
                    );

                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            };

            latest_slot = slot;
            landed_tx = utils::find_landed_txs(&signatures, statuses);
        }

        if !landed_tx.is_empty() {
            let cost = 25 * constant::FEE_PER_SIGNER + tip;

            info!(
                acc.id = self.id,
                confirm = format_duration!(sent_at_time.elapsed()),
                rewards = format_reward!(rewards),
                cost = format_reward!(cost),
                tip,
                tx.first = ?landed_tx.first().unwrap(),
                "bundle mined",
            );
        } else {
            let tips = *tips.read().await;

            warn!(
                acc.id = self.id,
                confirm = format_duration!(sent_at_time.elapsed()),
                tip,
                tips.p25 = tips.p25(),
                tips.p50 = tips.p50(),
                "bundle dropped"
            );
        }

        self.release().await;
    }
}

struct SendBundleTask {
    client: Arc<RpcClient>,
    tips: Arc<RwLock<JitoTips>>,
    batch: Vec<Accounts>,
    available_bus: Vec<Bus>,
    signer_balances: HashMap<Pubkey, u64>,
    mining_duration: Duration,
    mining_results: Vec<(solana_sdk::keccak::Hash, u64)>,
    rewards: u64,
    tip: u64,
    max_tip: u64,

    slot: Slot,
    blockhash: Hash,
}

impl SendBundleTask {
    async fn work(self) {
        let tips_now = *self.tips.read().await;

        let tip = if self.max_tip > 0 {
            let p50 = tips_now.p50();
            if p50 == 0 {
                self.tip
            } else {
                let tip = p50 + 1;
                tip.max(50000).min(self.max_tip)
            }
        } else {
            self.tip
        };

        let signer_and_mining_results = self
            .mining_results
            .into_iter()
            .chunks(Accounts::size())
            .into_iter()
            .map(|c| c.collect_vec())
            .collect_vec()
            .into_iter()
            .zip(self.batch.into_iter())
            .collect::<Vec<_>>();

        // Bundle limit
        for (mining_results, accounts) in signer_and_mining_results {
            let mut signatures = vec![];

            let tipper = utils::pick_richest_account(&self.signer_balances, &accounts.pubkey);
            let material_to_build_bundle = mining_results.chunks(5).zip(accounts.signers.chunks(5));
            let send_bundle_time = Instant::now();

            debug!(accounts = ?accounts.pubkey, %tipper, "building bundle");

            for bus in &self.available_bus {
                let mut bundle = Vec::with_capacity(5);

                for (hash_and_nonce, signers) in material_to_build_bundle.clone() {
                    let fee_payer_this_batch = signers
                        .iter()
                        .map(|s| s.pubkey())
                        .max_by_key(|pubkey| self.signer_balances.get(pubkey).unwrap())
                        .expect("signers balances should not be empty");

                    let mut tx_signers = Vec::with_capacity(5);
                    let mut ixs = Vec::with_capacity(6);

                    for ((hash, nonce), signer) in hash_and_nonce.iter().zip(signers.iter()) {
                        debug!(%tipper, signer = %signer.pubkey(), "adding mine instruction");

                        ixs.push(ore::instruction::mine(
                            signer.pubkey(),
                            ore::BUS_ADDRESSES[bus.id as usize],
                            ore::state::Hash(hash.to_bytes()),
                            *nonce,
                        ));

                        tx_signers.push(signer);

                        if tipper == signer.pubkey() {
                            ixs.push(jito::build_bribe_ix(&tipper, tip));
                        }
                    }

                    let tx = Transaction::new_signed_with_payer(
                        &ixs,
                        Some(&fee_payer_this_batch),
                        &tx_signers,
                        self.blockhash,
                    );

                    bundle.push(tx);
                }

                let sig = bundle[0].signatures[0];

                match jito::send_bundle(bundle).await {
                    Ok((_, bundle_id)) => debug!(acc.id = accounts.id, %sig, bundle = %bundle_id, "bundle sent"),
                    Err(err) => error!(acc.id = accounts.id, %sig, "fail to send bundle: {err:#}"),
                }

                signatures.push(sig);
            }

            info!(
                acc.id = accounts.id,
                mining = format_duration!(self.mining_duration),
                tip,
                tip.p25 = tips_now.p25(),
                tip.p50 = tips_now.p50(),
                slot = self.slot,
                "bundles sent"
            );

            tokio::spawn({
                let client = self.client.clone();
                let tips = self.tips.clone();

                async move {
                    accounts
                        .watch_signatures(client, signatures, tip, tips, self.slot, send_bundle_time, self.rewards)
                        .await;
                }
            });
        }
    }
}
