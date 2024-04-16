
use clap::Parser;
use solana_sdk::{
    pubkey::Pubkey, signature::{Keypair, Signer},
    signer::EncodableKey, system_instruction, transaction::Transaction,
    message::Message
};
use tracing::{error, info};
use crate::Miner;

#[derive(Parser, Debug, Clone)]
pub struct CollectArgs {
    #[arg(long, help = "The folder that contains all the keys used to collect the remaining balance.")]
    pub key_folder: String,

    #[arg(long, help = "The beneficiary account that will receive the remaining balance.")]
    pub beneficiary: Pubkey,

    #[arg(long, default_value = "", help = "The keypair file to use as fee payer. If not provided, the first key in the key_folder will be used.")]
    pub fee_payer: String,
}

impl Miner {
    pub async fn collect(&self, args: &CollectArgs) {
        let client = Miner::get_client_confirmed(&self.rpc);
        let accounts = Self::read_keys(&args.key_folder);
       
        let fee_payer_account: Keypair = if (&args.fee_payer).is_empty() {
            accounts[0].insecure_clone() // sorry for this
        } else {
            Keypair::read_from_file(&args.fee_payer).unwrap()
        };

        info!("use account {} as fee payer", fee_payer_account.pubkey());
        
        let mut instructions = Vec::new();
        let mut signers = Vec::new();
       
        let balance_fee_payer = client
            .get_balance(&fee_payer_account.pubkey())
            .await
            .expect("Failed to get balance");

        info!("Fee payer balance: {}", balance_fee_payer);

        for keypair in accounts.iter() {
            let pubkey = keypair.pubkey();
            let balance = client
                .get_balance(&pubkey)
                .await
                .expect("Failed to get balance");
            let rent_exemption = client
                .get_minimum_balance_for_rent_exemption(0)
                .await
                .expect("Failed to get minimum balance for rent exemption");

            if balance - rent_exemption > 0 {
                let instruction = system_instruction::transfer(
                    &pubkey,
                    &args.beneficiary,
                    balance - rent_exemption,
                );
                instructions.push(instruction);
                signers.push(keypair);
                info!("Bundling transfer of {} from {} to {}", balance, pubkey, args.beneficiary)
            }

            if instructions.len() >= 8 {
                signers.push(&fee_payer_account);
                
                let recent_blockhash = client
                .get_latest_blockhash()
                .await
                .expect("Failed to get recent blockhash");

                let transaction = Transaction::new_signed_with_payer(
                    &instructions,
                    Some(&fee_payer_account.pubkey()),
                    &signers,
                    recent_blockhash,
                );
                
                let message = Message::new(&instructions, Some(&fee_payer_account.pubkey()));
                let estimate_transfer_fee = client.get_fee_for_message(&message).await.expect("Failed to get fee for message");

                if estimate_transfer_fee > balance_fee_payer {
                    error!("Insufficient funds to pay for transaction fee");
                    return;
                }

                info!("Estimate transfer fee: {}", estimate_transfer_fee);

                match client.send_and_confirm_transaction(&transaction).await {
                    Ok(signature) => {
                        info!("Bundled transfer succeeded. Signature: {}", signature);
                    }
                    Err(err) => {
                        error!("Bundled transfer failed: err {}", err);
                    }
                }

                instructions.clear();
                signers.clear();
            }
        }
        
     
    }

}