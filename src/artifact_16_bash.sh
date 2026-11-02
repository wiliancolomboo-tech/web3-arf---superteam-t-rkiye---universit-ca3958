# 1. Clone repository and setup workspace
git clone https://github.com/superteam-turkiye/arf-instant-settlement-engine.git
cd arf-instant-settlement-engine

# 2. Build on-chain Anchor Rust program
anchor build

# 3. Execute comprehensive integration test suite
anchor test

# 4. Deploy to Solana Devnet
solana config set --url https://api.devnet.solana.com
solana airdrop 2
anchor deploy --provider.cluster devnet