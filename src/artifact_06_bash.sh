# 1. Clone repository & install dependencies
git clone https://github.com/superteam-turkey/arfflow-settlement.git
cd arfflow-settlement
yarn install

# 2. Build Anchor smart contract & generate TypeScript IDL types
anchor build

# 3. Synchronize program keys
anchor keys sync

# 4. Deploy to Solana Devnet / Testnet
solana config set --url https://api.devnet.solana.com
anchor deploy --provider.cluster devnet

# 5. Run the End-to-End Enterprise Test Suite
anchor test