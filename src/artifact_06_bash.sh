# 1. Clone repository and install dependencies
git clone https://github.com/superteam-turkiye/arf-settlement-velocity-engine.git
cd arf-settlement-velocity-engine
yarn install

# 2. Build the Anchor Solana Program
anchor build

# 3. Synchronize program ID across source and Anchor.toml
anchor keys sync

# 4. Deploy to Solana Devnet (ensure deployer has devnet SOL)
solana airdrop 2 -u devnet
anchor deploy --provider.cluster devnet

# 5. Execute 100% Passing Integration Test Suite
anchor test