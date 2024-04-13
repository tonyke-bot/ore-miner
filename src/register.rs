use std::{collections::HashSet, time::Duration};

use clap::Parser;
use rand::Rng;
use solana_sdk::{signer::Signer, transaction::Transaction};
use tracing::{error, info};

use crate::{constant, jito, utils, Miner};

#[derive(Parser, Debug, Clone)]
pub struct RegisterArgs {
    #[arg(long, help = "The folder that contains all the keys used to claim $ORE")]
    pub key_folder: String,
}

impl Miner {
    pub async fn register(&self, args: &RegisterArgs) {
        let client = Miner::get_client_confirmed(&self.rpc);
        let accounts = Self::read_keys(&args.key_folder);
        let jito_tip = self.priority_fee.expect("jito tip is required");

        let owner_proof_pdas = accounts
            .iter()
            .map(|signer| utils::get_proof_pda(signer.pubkey()))
            .collect::<Vec<_>>();

        if owner_proof_pdas.is_empty() {
            info!("No claimable accounts found");
            return;
        }

        let mut registered = HashSet::new();

        for batch in owner_proof_pdas.chunks(constant::FETCH_ACCOUNT_LIMIT) {
            client
                .get_multiple_accounts(batch)
                .await
                .expect("Failed to get Proof accounts")
                .into_iter()
                .zip(accounts.iter())
                .for_each(|(account, signer)| {
                    if account.is_some() {
                        registered.insert(signer.pubkey());
                    }
                });
        }

        let accounts = accounts
            .into_iter()
            .filter(|signer| !registered.contains(&signer.pubkey()))
            .collect::<Vec<_>>();

        info!("registering {} accounts", accounts.len());

        let mut batch_iter = accounts.chunks(5);
        let mut remaining = accounts.len();

        let mut txs = vec![];
        let mut accounts_in_this_batch = 0;
        let mut signers_for_txs = vec![];

        loop {
            while txs.len() < 5 {
                let batch = match batch_iter.next() {
                    Some(batch) => batch,
                    None => break,
                };

                let mut ixs = vec![];
                let mut signers = vec![];

                for signer in batch {
                    ixs.push(ore::instruction::register(signer.pubkey()));
                    signers.push(signer);
                }

                let fee_payer = signers[rand::thread_rng().gen_range(0..signers.len())].pubkey();

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

            let mut failed_batch = false;

            for tx in &bundle {
                let sim_result = match client.simulate_transaction(tx).await {
                    Ok(r) => r.value,
                    Err(err) => {
                        error!("fail to simulate transaction: {err:#}");
                        failed_batch = true;
                        break;
                    }
                };

                if let Some(err) = sim_result.err {
                    error!("fail to simulate transaction: {err:#}");
                    failed_batch = true;
                    break;
                }
            }

            if failed_batch {
                txs.clear();
                remaining -= accounts_in_this_batch;
                signers_for_txs.clear();
                accounts_in_this_batch = 0;
                continue;
            }

            let (tx, bundle_id) = jito::send_bundle(bundle).await.unwrap();

            info!(first_tx = ?tx, %bundle_id, accounts = accounts_in_this_batch, remaining, slot = send_at_slot, "bundle sent");

            let mut latest_slot = send_at_slot;
            let mut mined = false;

            while !mined && latest_slot < send_at_slot + constant::SLOT_EXPIRATION {
                tokio::time::sleep(Duration::from_secs(2)).await;

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
                txs.clear();
                remaining -= accounts_in_this_batch;
                signers_for_txs.clear();
                accounts_in_this_batch = 0;
                info!(
                    accounts = accounts_in_this_batch,
                    remaining, "bundle sent at slot {send_at_slot}, remaining accounts: {remaining}"
                );
            } else {
                error!(accounts = accounts_in_this_batch, remaining, "bundle dropped, retrying");
            }
        }
    }
}
