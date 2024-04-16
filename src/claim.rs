use std::{sync::Arc, time::Duration};

use clap::Parser;
use ore::{state::Proof, utils::AccountDeserialize};
use rand::Rng;
use solana_client::rpc_config::RpcSimulateTransactionConfig;
use solana_sdk::{commitment_config::CommitmentConfig, pubkey::Pubkey, signature::Signer, transaction::Transaction};
use tracing::{debug, error, info};

use crate::{constant, format_reward, jito, utils, Miner};

const RECHECK_INTERVAL: Duration = Duration::from_secs(60 * 5);

#[derive(Parser, Debug, Clone)]
pub struct ClaimArgs {
    #[arg(long)]
    pub beneficiary: Pubkey,

    #[arg(long, help = "The folder that contains all the keys used to claim $ORE")]
    pub key_folder: String,

    #[arg(
        long,
        default_value = "false",
        help = "Automatically claim rewards when threshold is hit"
    )]
    pub auto: bool,

    #[arg(
        long = "threshold",
        default_value = "0",
        help = "Claim rewards when total rewards exceed this threshold"
    )]
    pub threshold_ui_amount: f64,
}

impl ClaimArgs {
    pub fn threshold(&self) -> u64 {
        (self.threshold_ui_amount * (10u64.pow(ore::TOKEN_DECIMALS as u32) as f64)) as u64
    }
}

impl Miner {
    pub async fn claim(&self, args: &ClaimArgs) {
        let client = Miner::get_client_confirmed(&self.rpc);
        let accounts = Self::read_keys(&args.key_folder);
        let jito_tip = self.priority_fee.expect("jito tip is required");

        let beneficiary_ata = utils::get_ore_ata(args.beneficiary);

        info!(ata = %beneficiary_ata, recipient = %args.beneficiary);
        if let Ok(Some(_ata)) = client.get_token_account(&beneficiary_ata).await {
            info!("Token account already exists: {:?}, continue to claim", beneficiary_ata);
        } else {
            error!("Token account does not exist: {:?}", beneficiary_ata);
            return;
        }

        let owner_proof_pdas = accounts
            .iter()
            .map(|key| (utils::get_proof_pda(key.pubkey())))
            .collect::<Vec<_>>();

        if owner_proof_pdas.is_empty() {
            info!("No claimable accounts found");
            return;
        }

        loop {
            let mut claimable = Vec::with_capacity(owner_proof_pdas.len());

            // Fetch claimable amount of each account
            for (batch_pda, batch_account) in owner_proof_pdas
                .chunks(constant::FETCH_ACCOUNT_LIMIT)
                .zip(accounts.chunks(constant::FETCH_ACCOUNT_LIMIT))
            {
                let batch_accounts = client
                    .get_multiple_accounts(batch_pda)
                    .await
                    .expect("Failed to get Proof accounts")
                    .into_iter()
                    .zip(batch_account.iter())
                    .filter_map(|(account, key)| {
                        let account_data = account?.data;
                        let proof = Proof::try_from_bytes(&account_data).ok()?;

                        if proof.claimable_rewards == 0 {
                            return None;
                        }

                        Some((
                            key.pubkey(),
                            Arc::new(key.insecure_clone()) as Arc<dyn Signer>,
                            proof.claimable_rewards,
                        ))
                    });

                claimable.extend(batch_accounts);
            }

            claimable.sort_by_key(|(_, _, amount)| *amount);
            claimable.reverse();

            let mut remaining = claimable.iter().map(|(_, _, amount)| amount).sum::<u64>();
            let mut batch_iter = claimable.chunks(5);

            info!("total rewards: {}", utils::ore_ui_amount(remaining));
            info!("total claimable accounts: {}", claimable.len());

            let mut txs = vec![];
            let mut total_rewards_in_this_batch = 0;
            let mut signers_for_txs = vec![];
            let mut accounts_in_this_batch = 0;

            loop {
                while txs.len() < 5 {
                    let batch = match batch_iter.next() {
                        Some(batch) => batch,
                        None => break,
                    };

                    let mut ixs = vec![];
                    let mut signers = vec![];

                    for (pubkey, signer, amount) in batch {
                        ixs.push(ore::instruction::claim(*pubkey, beneficiary_ata, *amount));
                        signers.push(signer.clone());
                        total_rewards_in_this_batch += amount;
                    }

                    let mut fee_payer = signers[rand::thread_rng().gen_range(0..signers.len())].pubkey();

                    match Self::get_balances(
                        &client,
                        &signers.iter().map(|signer| signer.pubkey()).collect::<Vec<_>>(),
                    )
                    .await
                    {
                        Ok(value) => {
                            // pick richest
                            fee_payer = value
                                .iter()
                                .max_by_key(|(_, balance)| *balance)
                                .map(|(pubkey, _)| *pubkey)
                                .expect("no signers found");
                        }
                        Err(err) => {
                            error!("fail to get balances for signers: {err:#}");
                        }
                    };

                    if txs.is_empty() {
                        ixs.push(jito::build_bribe_ix(&fee_payer, jito_tip));
                    }

                    txs.push(Transaction::new_with_payer(&ixs, Some(&fee_payer)));
                    accounts_in_this_batch += signers.len();
                    signers_for_txs.push(signers);
                }

                if txs.is_empty() {
                    break;
                }

                if total_rewards_in_this_batch < args.threshold() {
                    info!(
                        total.rewards.remaing = format_reward!(remaining),
                        this.batch.rewards = format_reward!(total_rewards_in_this_batch),
                        this.batch.accounts = accounts_in_this_batch,
                        "batch reward is less than threshold, will not claim"
                    );
                    break;
                }

                let (send_at_slot, blockhash) = match Self::get_latest_blockhash_and_slot(&client).await {
                    Ok(value) => value,
                    Err(err) => {
                        error!("fail to get latest blockhash: {err:#}");
                        continue;
                    }
                };

                let bundle = txs
                    .iter()
                    .zip(signers_for_txs.iter())
                    .map(|(tx, signers)| {
                        let mut tx = tx.clone();
                        tx.sign(signers.as_slice(), blockhash);
                        tx
                    })
                    .collect::<Vec<_>>();

                let mut sim_failed = false;

                for tx in &bundle {
                    let sim_result = client
                        .simulate_transaction_with_config(
                            tx,
                            RpcSimulateTransactionConfig {
                                sig_verify: false,
                                commitment: Some(CommitmentConfig::processed()),
                                encoding: None,
                                accounts: None,
                                min_context_slot: None,
                                replace_recent_blockhash: true,
                                inner_instructions: false,
                            },
                        )
                        .await;

                    debug!("simulation result: {sim_result:?}");
                    match sim_result {
                        Ok(r) => {
                            if let Some(err) = &r.value.err {
                                error!("simulation returns error: {err:#}");
                            } else {
                                continue;
                            }
                        }
                        Err(err) => {
                            error!("fail to simulate transaction: {err:#}");
                        }
                    }

                    sim_failed = true;
                    break;
                }

                if sim_failed {
                    txs.clear();
                    remaining -= total_rewards_in_this_batch;
                    accounts_in_this_batch = 0;
                    total_rewards_in_this_batch = 0;
                    signers_for_txs.clear();
                    continue;
                }

                let (tx, bundle_id) = match jito::send_bundle(bundle).await {
                    Ok(value) => value,
                    Err(err) => {
                        error!("fail to send bundle: {err:#}");
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        continue;
                    }
                };

                info!(
                    first_tx = %tx,
                    %bundle_id,
                    total.rewards.remaing = format_reward!(remaining),
                    this.batch.rewards = format_reward!(total_rewards_in_this_batch),
                    this.batch.accounts = accounts_in_this_batch,
                    slot = send_at_slot,
                    "bundle sent");

                let mut latest_slot = send_at_slot;
                let mut mined = false;

                while !mined && latest_slot < send_at_slot + constant::SLOT_EXPIRATION {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    debug!(
                        ?tx,
                        total.rewards.remaing = format_reward!(remaining),
                        this.batch.rewards = format_reward!(total_rewards_in_this_batch),
                        this.batch.accounts = accounts_in_this_batch,
                        slot = send_at_slot,
                        "checking bundle status"
                    );

                    let (statuses, slot) = match Self::get_signature_statuses(&client, &[tx]).await {
                        Ok(value) => value,
                        Err(err) => {
                            error!(send_at_slot, "fail to get bundle status: {err:#}");
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            continue;
                        }
                    };

                    mined = !utils::find_landed_txs(&[tx], statuses).is_empty();
                    latest_slot = slot;
                }

                if mined {
                    info!(
                        total.rewards.remaing = format_reward!(remaining),
                        this.batch.rewards = format_reward!(total_rewards_in_this_batch),
                        this.batch.accounts = accounts_in_this_batch,
                        remaining,
                        "claim successfully"
                    );

                    txs.clear();
                    remaining -= total_rewards_in_this_batch;
                    accounts_in_this_batch = 0;
                    signers_for_txs.clear();
                    total_rewards_in_this_batch = 0;
                } else {
                    error!(
                        total.rewards.remaing = format_reward!(remaining),
                        this.batch.rewards = format_reward!(total_rewards_in_this_batch),
                        this.batch.accounts = accounts_in_this_batch,
                        remaining,
                        slot = send_at_slot,
                        "bundle dropped, retrying"
                    );
                }
            }

            if !args.auto {
                break;
            }

            info!("will check reward again in {RECHECK_INTERVAL:?}");
            tokio::time::sleep(RECHECK_INTERVAL).await
        }
    }
}
