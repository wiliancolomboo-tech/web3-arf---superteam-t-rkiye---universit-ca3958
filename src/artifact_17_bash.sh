# 1. Build optimized program binary using Anchor
anchor build

# 2. Extract deterministic Program Keypair & Address
solana address -k target/deploy/arf_settler-keypair.json

# 3. Synchronize program ID in Anchor.toml and src/lib.rs
# Replace declare_id!("...") with the extracted key

# 4. Deploy to Solana Devnet
solana program deploy \
    --url https://api.devnet.solana.com \
    --keypair ~/.config/solana/id.json \
    target/deploy/arf_settler.so

# 5. Run full automated QA test suite
anchor test