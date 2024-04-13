# $ORE Miner

ORE Miner built on top of Jito bundle service. Shipped with both CPU and GPU hashing support.

Each miner is able to carry 400 wallets on a single RTX 4090 card. Should expect 10~20% improvement if the code is optimized. 

## Preparation

1. Get a reliable, fastest Solana RPC

2. Clone the the repo and build
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

