use clap::Parser;
use crate::Miner;
use solana_sdk::{
    signature::{Keypair, Signer},
    signer::EncodableKey,
    transaction::Transaction,
};

use tracing::{info, error};


#[derive(Parser, Debug, Clone)]
pub struct InitClaimArgs {
    #[arg(long, help = "The keypair to initalize the $ORE token account with.")]
    pub keypair: String
}

impl Miner {
    pub async fn init_claim(&self, args: &InitClaimArgs) { 
        let client = Miner::get_client_confirmed(&self.rpc);
        // initalize the token account
        let keypair = Keypair::read_from_file(&args.keypair).unwrap();

        // build instructions.
        let token_account_pubkey = spl_associated_token_account::get_associated_token_address(
        &keypair.pubkey(),
        &ore::MINT_ADDRESS,
        );

        // Check if ata already exists
        if let Ok(Some(_ata)) = client.get_token_account(&token_account_pubkey).await {
            info!("Token account already exists: {:?}", token_account_pubkey);
        }

        // Sign and send transaction.
        let instruction = spl_associated_token_account::instruction::create_associated_token_account(
            &keypair.pubkey(),
            &keypair.pubkey(),
            &ore::MINT_ADDRESS,
            &spl_token::id(),
        );

        let recent_blockhash = client
                .get_latest_blockhash()
                .await
                .expect("Failed to get recent blockhash");

        let transaction = Transaction::new_signed_with_payer(&[instruction],
             Some(&keypair.pubkey()), 
             &[&keypair], recent_blockhash);

        println!("Creating token account {} for {}...", token_account_pubkey, keypair.pubkey());
        match client.send_and_confirm_transaction(&transaction).await {
            Ok(_sig) => info!("Created token account {:?} for {}", token_account_pubkey, keypair.pubkey()),
            Err(e) => error!("Transaction failed: {:?}", e),
        }
    }
}