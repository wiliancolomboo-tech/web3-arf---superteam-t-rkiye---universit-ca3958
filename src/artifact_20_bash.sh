# Set Solana CLI to devnet
solana config set --url https://api.devnet.solana.com

# Build Anchor Program
anchor build

# Sync Program ID into lib.rs and Anchor.toml
anchor keys sync

# Deploy to Devnet
anchor deploy --program-name arf_settlement --provider.cluster devnet