use clap::Parser;
use serde_json::json;
use solana_sdk::signature::{Keypair, Signer};

use crate::Miner;

#[derive(Debug, Parser, Clone, Copy)]
pub struct GenerateWalletArgs {
    #[arg()]
    pub count: usize,
}

impl Miner {
    pub fn generate_wallet(&self, args: &GenerateWalletArgs) {
        for _ in 0..args.count {
            let keypair = Keypair::new();
            let valued = keypair.to_bytes().iter().map(|b| json!(*b)).collect::<Vec<_>>();

            let key_array = serde_json::to_string(&json!(valued)).unwrap();

            println!("{key_array} | {}", keypair.pubkey());
        }
    }
}
