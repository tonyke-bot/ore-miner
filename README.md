# $ORE Miner

ORE Miner built on top of Jito bundle service by [@tonyke_bot](https://x.com/tonyke_bot) and [@shoucccc](https://twitter.com/shoucccc).

Shipped with both CPU and GPU hashing support.

Each miner is able to carry 400 wallets on a single RTX 4090 card. Should expect 10~20% improvement if the code is optimized. 

## Preparations

1. Get a reliable, fastest Solana RPC
2. Clone the repo and build
    ```shell
    git clone https://github.com/tonyke-bot/ore-miner.git
    cd ore-miner
    cargo build --release
    ```

3. (Optional) Install CUDA development environment
4. (Optional) Build CUDA miner
    ```shell
    ./build-cuda-miner.sh
    ```
   
5. Generate wallets and fund them with SOL

### Feature
* Evenly consumed SOL: Choose richest wallet to tip bundle and richest wallet in a transaction to pay the transaction fee.
* Adaptive tip: Automatically adjust tip based on the Jito tip stream.
* Bulk operation support: mine, register, claim, batch transfer

## Usage

#### Mine with GPU
```
export CUDA_VISIBLE_DEVICES=<GPU_INDEX>

cargo run --release -- \
    --rpc-url <RPC_URL> \
    --priority-fee 500000 \                     # Tip used for Jito bundle. If max adaptive tip is set, this will be the initial tip.
    bundle-mine-gpu \
    --key-folder <FOLDER_CONTAINS_YOUR_KEYS> \  # Folder contains your Solana keys
    --max-adaptive-tip 400000 \                 # Max tip used, if this is set, use tip min(tips.p50, max)****

```

#### Multi Claim
```
cargo run --release -- \
    --rpc-url <RPC_URL> \
    --priority-fee 500000 \                     # Tip used for Jito bundle. 
    claim \
    --key-folder <FOLDER_CONTAINS_YOUR_KEYS> \  # Folder contains your Solana keys
    --beneficiary <YOUR_PUBKEY_TO_RECEIVE_ORE>
```

#### Register
```
cargo run --release -- \
    --rpc-url <RPC_URL> \
    --priority-fee 500000 \                     # Tip used for Jito bundle. 
    register \
    --key-folder <FOLDER_CONTAINS_YOUR_KEYS> \  # Folder contains your Solana keys
```

### Buy me ☕️

* SOL: `tonyi4UznxNzae5RBinHTU8Gxr91RRGBcdx7mmimN8F`
* EVM: `0x45Fce32abB76fd0722882326FBf2d1182e6b982B`

Appreciate your support!
