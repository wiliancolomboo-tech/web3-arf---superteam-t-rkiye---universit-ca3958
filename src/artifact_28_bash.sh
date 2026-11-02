# 1. Clone repository and install dependencies
git clone https://github.com/arf-clearstream/protocol.git
cd protocol

# 2. Build Anchor smart contract
anchor build

# 3. Deploy to Solana Devnet / Mainnet
solana config set --url https://api.devnet.solana.com
anchor deploy --provider.cluster devnet

# 4. Run automated test suite verifying 100% assertions
anchor test

# 5. Launch Enterprise Telemetry Daemon via Docker
cp worker/.env.example .env
# Edit .env with your RESEND_API_KEY and SLACK_WEBHOOK_URL
docker compose up -d --build

# 6. Verify healthcheck status
curl http://localhost:8080/health