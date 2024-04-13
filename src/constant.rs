use rand::Rng;
use solana_sdk::{pubkey, pubkey::Pubkey};

pub const FEE_PER_SIGNER: u64 = 5000;

pub const SLOT_EXPIRATION: u64 = 151 + 5;

pub const FETCH_ACCOUNT_LIMIT: usize = 100;
pub const TRANSFER_BATCH_SIZE: usize = 21;

pub const JITO_RECIPIENTS: [Pubkey; 8] = [
    pubkey!("96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5"),
    pubkey!("HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe"),
    pubkey!("Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY"),
    pubkey!("ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49"),
    pubkey!("DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh"),
    pubkey!("ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt"),
    pubkey!("DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL"),
    pubkey!("3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT"),
];

pub fn pick_jito_recipient() -> &'static Pubkey {
    &JITO_RECIPIENTS[rand::thread_rng().gen_range(0..JITO_RECIPIENTS.len())]
}
