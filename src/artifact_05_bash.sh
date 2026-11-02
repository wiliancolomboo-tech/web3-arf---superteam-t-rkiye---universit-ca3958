# 1. Install prerequisites
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
sh -c "$(curl -sSfL https://release.solana.com/v1.18.4/install)"
npm install -g @coral-xyz/anchor-cli

# 2. Build the Anchor Program
anchor build

# 3. Synchronize Program ID
solana address -k target/deploy/arf_flow-keypair.json
# Replace 'ArfF1ow111111111111111111111111111111111111' in lib.rs & Anchor.toml

# 4. Deploy to Solana Devnet
solana config set --url https://api.devnet.solana.com
solana airdrop 2
anchor deploy --provider.cluster devnet

# 5. Run Integration Test Suite
anchor test