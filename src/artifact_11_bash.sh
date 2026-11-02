# 1. Clone repository
git clone https://github.com/superteam-turkey/arf-nexus.git
cd arf-nexus

# 2. Build on-chain program
anchor build

# 3. Synchronize Program ID
anchor keys sync

# 4. Deploy to Solana Devnet
solana config set --url https://api.devnet.solana.com
solana airdrop 2
anchor deploy --provider.cluster devnet

# 5. Initialize Institutional Environment
yarn install
anchor test --skip-local-validator