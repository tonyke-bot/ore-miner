use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

use clap::{Parser, Subcommand};
use eyre::{bail, ContextCompat};
use ore::{
    state::{Bus, Proof, Treasury},
    utils::AccountDeserialize,
};
use serde_json::json;
use solana_client::{
    nonblocking::rpc_client::RpcClient,
    rpc_request::RpcRequest,
    rpc_response::{Response, RpcBlockhash},
};
use solana_sdk::{
    account::{Account, ReadableAccount},
    clock::{Clock, Slot},
    commitment_config::CommitmentConfig,
    keccak::Hash,
    pubkey::Pubkey,
    signature::{Keypair, Signature},
    signer::EncodableKey,
    sysvar,
};
use solana_transaction_status::TransactionStatus;
use tokio::io::AsyncWriteExt;
use tracing::{error, log};

mod batch_transfer;
mod benchmark_rpc;
mod bundle_mine;
mod bundle_mine_gpu;
mod collect;
mod claim;
mod constant;
mod generate_wallet;
mod jito;
mod register;
mod utils;
mod init_claim;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    Miner::init_pretty_env_logger();
    let miner = Miner::parse();

    match &miner.command {
        Command::Claim(args) => miner.claim(args).await,
        Command::BundleMine(args) => miner.bundle_mine(args).await,
        Command::BundleMineGpu(args) => miner.bundle_mine_gpu(args).await,
        Command::Register(args) => miner.register(args).await,
        Command::BenchmarkRpc(args) => miner.benchmark_rpc(args).await,
        Command::BatchTransfer(args) => miner.batch_transfer(args).await,
        Command::JitoTipStream => miner.jito_tip_stream().await,
        Command::GenerateWallet(args) => miner.generate_wallet(args),
        Command::Collect(args) => miner.collect(args).await,
        Command::InitClaim(args) => miner.init_claim(args).await,
    }
}

#[derive(Parser, Debug, Clone)]
pub struct Miner {
    #[arg(long, default_value = "https://api.mainnet-beta.solana.com")]
    pub rpc: String,

    #[arg(long)]
    pub priority_fee: Option<u64>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    Claim(crate::claim::ClaimArgs),
    BundleMine(crate::bundle_mine::BundleMineArgs),
    BundleMineGpu(crate::bundle_mine_gpu::BundleMineGpuArgs),
    Register(crate::register::RegisterArgs),
    BenchmarkRpc(crate::benchmark_rpc::BenchmarkRpcArgs),
    JitoTipStream,
    GenerateWallet(crate::generate_wallet::GenerateWalletArgs),
    BatchTransfer(crate::batch_transfer::BatchTransferArgs),
    Collect(crate::collect::CollectArgs),
    InitClaim(crate::init_claim::InitClaimArgs),
}

impl Miner {
    pub fn init_pretty_env_logger() {
        env_logger::Builder::new()
            .filter_level(log::LevelFilter::Info)
            .parse_default_env()
            .init();
    }

    pub fn get_client_confirmed(rpc: &str) -> Arc<RpcClient> {
        Arc::new(RpcClient::new_with_commitment(
            rpc.to_string(),
            CommitmentConfig::confirmed(),
        ))
    }

    pub fn read_keys(key_folder: &str) -> Vec<Keypair> {
        fs::read_dir(key_folder)
            .expect("Failed to read key folder")
            .map(|entry| {
                let path = entry.expect("Failed to read entry").path();

                Keypair::read_from_file(&path).unwrap_or_else(|_| panic!("Failed to read keypair from {:?}", path))
            })
            .collect::<Vec<_>>()
    }

    pub async fn get_latest_blockhash_and_slot(client: &RpcClient) -> eyre::Result<(Slot, solana_sdk::hash::Hash)> {
        let (blockhash, send_at_slot) = match client
            .send::<Response<RpcBlockhash>>(RpcRequest::GetLatestBlockhash, json!([{"commitment": "confirmed"}]))
            .await
        {
            Ok(r) => (r.value.blockhash, r.context.slot),
            Err(err) => eyre::bail!("failed to get latest blockhash: {err:#}"),
        };

        let blockhash = match solana_sdk::hash::Hash::from_str(&blockhash) {
            Ok(b) => b,
            Err(err) => eyre::bail!("fail to parse blockhash: {err:#}"),
        };

        Ok((send_at_slot, blockhash))
    }

    pub async fn mine_hashes_cpu(
        &self,
        threads: usize,
        difficulty: &Hash,
        hash_and_pubkey: &[(Hash, Pubkey)],
    ) -> (Duration, Vec<(Hash, u64)>) {
        self.mine_hashes(utils::get_nonce_worker_path(), threads, difficulty, hash_and_pubkey)
            .await
    }

    pub async fn mine_hashes_gpu(
        &self,
        difficulty: &Hash,
        hash_and_pubkey: &[(Hash, Pubkey)],
    ) -> (Duration, Vec<(Hash, u64)>) {
        self.mine_hashes(utils::get_gpu_nonce_worker_path(), 0, difficulty, hash_and_pubkey)
            .await
    }

    pub async fn mine_hashes(
        &self,
        worker: PathBuf,
        threads: usize,
        difficulty: &Hash,
        hash_and_pubkey: &[(Hash, Pubkey)],
    ) -> (Duration, Vec<(Hash, u64)>) {
        let mining_start = Instant::now();
        println!("difficulty: {difficulty}", difficulty = difficulty);
        let mut child = tokio::process::Command::new(worker)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("nonce_worker failed to spawn");

        {
            let stdin = child.stdin.as_mut().unwrap();

            stdin.write_u8(threads as u8).await.unwrap();
            stdin.write_all(difficulty.as_ref()).await.unwrap();

            for (hash, pubkey) in hash_and_pubkey {
                stdin.write_all(hash.as_ref()).await.unwrap();
                stdin.write_all(pubkey.as_ref()).await.unwrap();
            }
        }

        let output = child.wait_with_output().await.unwrap().stdout;
        let mut results = vec![];

        for item in output.chunks(40) {
            let hash = Hash(item[..32].try_into().unwrap());
            let nonce = u64::from_le_bytes(item[32..40].try_into().unwrap());

            results.push((hash, nonce));
        }

        let mining_duration = mining_start.elapsed();
        (mining_duration, results)
    }

    pub fn find_buses(buses: [Bus; ore::BUS_COUNT], required_reward: u64) -> Vec<Bus> {
        let mut available_bus = buses
            .into_iter()
            .filter(|bus| bus.rewards >= required_reward)
            .collect::<Vec<_>>();

        available_bus.sort_by(|a, b| b.rewards.cmp(&a.rewards));

        available_bus
    }

    pub async fn get_accounts(
        id: usize,
        client: &RpcClient,
        accounts: &[Pubkey],
    ) -> Option<(Treasury, Clock, [Bus; ore::BUS_COUNT], Vec<Proof>)> {
        let proof_count = accounts.len() - (2 + ore::BUS_COUNT);

        let accounts = match client
            .get_multiple_accounts_with_commitment(accounts, CommitmentConfig::processed())
            .await
        {
            Ok(accounts) => accounts.value,
            Err(err) => {
                error!(miner = id, "failed to get proof and treasury accounts: {err}",);
                return None;
            }
        };

        let mut accounts = accounts.into_iter();
        let treasury: Treasury = parse_account("treasury", accounts.next())?;
        let clock: Clock = match accounts.next() {
            Some(Some(account)) => match bincode::deserialize::<Clock>(account.data()) {
                Ok(account) => account,
                Err(err) => {
                    error!(miner = id, "failed to deserialize clock account: {err:#}",);
                    return None;
                }
            },
            _ => {
                error!(miner = id, "clock account doesn't exist");
                return None;
            }
        };

        let mut buses = [Bus { id: 0, rewards: 0 }; ore::BUS_COUNT];
        let mut proofs = Vec::with_capacity(proof_count);

        for bus in buses.iter_mut() {
            *bus = parse_account("bus", accounts.next())?;
        }

        for _ in 0..proof_count {
            proofs.push(parse_account("proof", accounts.next())?);
        }

        Some((treasury, clock, buses, proofs))
    }

    pub fn get_time_to_next_epoch(treasury: &Treasury, clock: &Clock, reset_threshold: i64) -> Duration {
        Duration::from_secs(if clock.unix_timestamp < reset_threshold {
            reset_threshold - clock.unix_timestamp
        } else {
            treasury.last_reset_at + ore::EPOCH_DURATION - clock.unix_timestamp
        } as u64)
    }

    async fn get_system_accounts(client: &RpcClient) -> eyre::Result<(Treasury, Clock, [Bus; ore::BUS_COUNT])> {
        pub const SYSTEM_ACCOUNTS: &[Pubkey] = &[
            ore::TREASURY_ADDRESS,
            sysvar::clock::ID,
            ore::BUS_ADDRESSES[0],
            ore::BUS_ADDRESSES[1],
            ore::BUS_ADDRESSES[2],
            ore::BUS_ADDRESSES[3],
            ore::BUS_ADDRESSES[4],
            ore::BUS_ADDRESSES[5],
            ore::BUS_ADDRESSES[6],
            ore::BUS_ADDRESSES[7],
        ];

        let accounts = match client
            .get_multiple_accounts_with_commitment(SYSTEM_ACCOUNTS, CommitmentConfig::processed())
            .await
        {
            Ok(accounts) => accounts.value,
            Err(err) => bail!("failed to fetch accounts: {err}"),
        };

        let mut accounts = accounts.into_iter();
        let treasury: Treasury =
            parse_account("treasury", accounts.next()).context("failed to parse treasury account")?;

        let clock: Clock = match accounts.next() {
            Some(Some(account)) => match bincode::deserialize::<Clock>(account.data()) {
                Ok(account) => account,
                Err(err) => bail!("failed to deserialize clock account: {err:#}"),
            },
            _ => bail!("clock account doesn't exist"),
        };

        let mut buses = [Bus { id: 0, rewards: 0 }; ore::BUS_COUNT];
        for bus in buses.iter_mut() {
            *bus = parse_account("bus", accounts.next()).context("failed to parse bus account")?;
        }

        Ok((treasury, clock, buses))
    }

    async fn get_proof_accounts(client: &RpcClient, accounts: &[Pubkey]) -> eyre::Result<Vec<Proof>> {
        let account_data = match client
            .get_multiple_accounts_with_commitment(accounts, CommitmentConfig::processed())
            .await
        {
            Ok(accounts) => accounts.value,
            Err(err) => bail!("failed to get proof accounts: {err}"),
        };

        let mut proofs = vec![];

        for (i, account) in account_data.into_iter().enumerate() {
            let account = match account {
                None => bail!("account {} not registered", accounts[i]),
                Some(a) => a,
            };

            let proof = match Proof::try_from_bytes(account.data()) {
                Ok(proof) => proof,
                Err(err) => bail!("failed to deserialize proof account {}: {err:#}", accounts[i]),
            };

            proofs.push(*proof);
        }

        Ok(proofs)
    }

    pub async fn get_balances(client: &RpcClient, accounts: &[Pubkey]) -> eyre::Result<HashMap<Pubkey, u64>> {
        let account_data = match client.get_multiple_accounts(accounts).await {
            Ok(a) => a,
            Err(err) => eyre::bail!("fail to get accounts: {err:#}"),
        };

        let result = account_data
            .into_iter()
            .zip(accounts.iter())
            .filter(|(account, _)| account.is_some())
            .map(|(account, pubkey)| (*pubkey, account.unwrap().lamports))
            .collect();

        Ok(result)
    }

    pub async fn get_signature_statuses(
        client: &RpcClient,
        signatures: &[Signature],
    ) -> eyre::Result<(Vec<Option<TransactionStatus>>, Slot)> {
        let signatures_params = signatures.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        let (statuses, slot) = match client
            .send::<Response<Vec<Option<TransactionStatus>>>>(
                RpcRequest::GetSignatureStatuses,
                json!([signatures_params]),
            )
            .await
        {
            Ok(result) => (result.value, result.context.slot),
            Err(err) => eyre::bail!("fail to get bundle status: {err}"),
        };

        Ok((statuses, slot))
    }
}

pub fn parse_account<S: AccountDeserialize + Copy>(name: &str, account: Option<Option<Account>>) -> Option<S> {
    match account {
        Some(Some(account)) => match S::try_from_bytes(account.data()) {
            Ok(account) => Some(*account),
            Err(err) => {
                error!("failed to deserialize {name} account: {err:#}",);
                None
            }
        },
        _ => {
            error!("{name} account doesn't exist");
            None
        }
    }
}
