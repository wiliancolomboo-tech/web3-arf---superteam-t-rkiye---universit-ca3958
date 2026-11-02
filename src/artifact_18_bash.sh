# 1. Clone repository
git clone https://github.com/arf-financial/arf-nexus-solana.git
cd arf-nexus-solana

# 2. Build on-chain program
anchor build

# 3. Deploy to Solana Devnet (or Mainnet-beta)
solana config set --url https://api.devnet.solana.com
anchor deploy --program-name arf_nexus

# 4. Configure environment credentials
cp .env.example .env
# Edit .env with your RESEND_API_KEY and SLACK_WEBHOOK_URL

# 5. Launch the Enterprise Orchestrator Daemon via Docker
docker-compose up -d --build

# 6. Verify healthcheck and streaming logs
docker-compose logs -f arf-nexus-daemon