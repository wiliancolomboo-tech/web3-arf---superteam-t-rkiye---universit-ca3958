# 1. Build Solana Program
anchor build

# 2. Get Program Keypair Address and Update lib.rs / Anchor.toml
solana address -k target/deploy/arf_settlement-keypair.json

# 3. Run Local Anchor Verification Matrix
anchor test

# 4. Deploy to Solana Devnet
anchor deploy --provider.cluster devnet

# 5. Launch Enterprise Relayer & Telemetry Service
cd relayer
cp .env.example .env
npm install
npm run worker