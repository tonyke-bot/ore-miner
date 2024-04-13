use std::{
    io::{Read, Write},
    sync::{atomic::AtomicBool, Arc},
};

use sha3::{
    digest::{FixedOutputReset, Update},
    Keccak256,
};

fn main() {
    let mut threads_and_diff = [0u8; 33];
    let mut preimage = [0u8; 32 + 32];

    let mut stdin = std::io::stdin().lock();
    stdin.read_exact(&mut threads_and_diff).unwrap();

    let threads = threads_and_diff[0] as usize;
    let difficulty: [u8; 32] = threads_and_diff[1..].try_into().unwrap();

    while stdin.read_exact(&mut preimage[..64]).is_ok() {
        let found = Arc::new(AtomicBool::new(false));
        let thread_handles: Vec<_> = (0..threads)
            .map(|i| {
                let preimage = preimage;
                let found = found.clone();

                let mut hasher = Keccak256::default();
                let mut hash_result = Default::default();

                std::thread::spawn(move || {
                    let mut nonce: u64 = u64::MAX.saturating_div(threads as u64).saturating_mul(i as u64);

                    loop {
                        hasher.update(&preimage);
                        hasher.update(&nonce.to_le_bytes());
                        hasher.finalize_into_reset(&mut hash_result);

                        if nonce % 10000 == 0 && found.load(std::sync::atomic::Ordering::Relaxed) {
                            return;
                        }

                        if hash_result.as_slice().le(&difficulty) {
                            if found.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                return;
                            }

                            let mut stdout = std::io::stdout().lock();

                            stdout.write_all(&hash_result).unwrap();
                            stdout.write_all(&nonce.to_le_bytes()).unwrap();
                        }

                        nonce += 1;
                    }
                })
            })
            .collect();

        for thread_handle in thread_handles {
            thread_handle.join().unwrap();
        }
    }

    std::io::stdout().flush().unwrap();
}
