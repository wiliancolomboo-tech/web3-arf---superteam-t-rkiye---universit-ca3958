# 1. Set target network
solana config set --url https://api.devnet.solana.com

# 2. Deploy Program
anchor deploy --provider.cluster devnet

# 3. Initialize Corridor (e.g. TR-EU Remittance Corridor)
npx ts-node relayer/cli.ts healthcheck