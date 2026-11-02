import { Connection, PublicKey } from "@solana/web3.js";
import { BorshCoder, EventParser } from "@coral-xyz/anchor";
import { CONFIG } from "./config";
import { sendDrawdownEmailNotification } from "./notifications/resendService";
import { sendSlackRiskAlert } from "./notifications/slackService";
import * as http from "http";

// Minimal IDL schema for parser decoding
const IDL = {
  version: "0.1.0",
  name: "arf_core_nexus",
  events: [
    {
      name: "DrawdownExecutedEvent",
      fields: [
        { name: "escrow", type: "publicKey", index: false },
        { name: "corridor", type: "publicKey", index: false },
        { name: "beneficiary", type: "publicKey", index: false },
        { name: "principalAmount", type: "u64", index: false },
        { name: "feeAmount", type: "u64", index: false },
        { name: "dueTimestamp", type: "i64", index: false },
      ],
    },
    {
      name: "LiquidationTriggeredEvent",
      fields: [
        { name: "escrow", type: "publicKey", index: false },
        { name: "corridor", type: "publicKey", index: false },
        { name: "liquidatedCollateral", type: "u64", index: false },
        { name: "badDebtLiquidated", type: "u64", index: false },
        { name: "timestamp", type: "i64", index: false },
      ],
    },
  ],
};

async function main() {
  console.log("====================================================");
  console.log("   ARFCORE NEXUS - SOLANA LIQUIDITY DAEMON ONLINE   ");
  console.log("====================================================");
  console.log(`Connecting to RPC: ${CONFIG.SOLANA_RPC_URL}`);
  console.log(`Listening for Program ID: ${CONFIG.PROGRAM_ID}`);

  const connection = new Connection(CONFIG.SOLANA_RPC_URL, {
    wsEndpoint: CONFIG.SOLANA_WSS_URL,
    commitment: "confirmed",
  });

  const programId = new PublicKey(CONFIG.PROGRAM_ID);
  const coder = new BorshCoder(IDL as any);
  const eventParser = new EventParser(programId, coder);

  connection.onLogs(
    programId,
    async ({ logs, err, signature }) => {
      if (err) return;

      const events = eventParser.parseLogs(logs);
      for (const event of events) {
        console.log(`\n[Anchor Event Detected] [Tx: ${signature.slice(0, 10)}...] -> ${event.name}`);

        if (event.name === "DrawdownExecutedEvent") {
          const data: any = event.data;
          const principal = Number(data.principalAmount) / 1_000_000;
          console.log(`-> Principal: $${principal} USDC`);
          console.log(`-> Beneficiary: ${data.beneficiary.toBase58()}`);

          await sendDrawdownEmailNotification(
            data.escrow.toBase58(),
            data.corridor.toBase58(),
            data.beneficiary.toBase58(),
            principal,
            Number(data.dueTimestamp)
          );
        }

        if (event.name === "LiquidationTriggeredEvent") {
          const data: any = event.data;
          const badDebt = Number(data.badDebtLiquidated) / 1_000_000;
          const collateral = Number(data.liquidatedCollateral) / 1_000_000;

          await sendSlackRiskAlert(
            "CRITICAL: Corridor Loan Default & Liquidation Triggered",
            data.corridor.toBase58(),
            data.escrow.toBase58(),
            badDebt,
            collateral
          );
        }
      }
    },
    "confirmed"
  );

  // Expose HTTP Health Endpoint for Docker Container Orchestration
  const server = http.createServer((req, res) => {
    if (req.url === "/healthz") {
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ status: "healthy", timestamp: Date.now(), uptime: process.uptime() }));
    } else {
      res.writeHead(404);
      res.end();
    }
  });

  server.listen(CONFIG.PORT, () => {
    console.log(`[Daemon Telemetry] Health check listening on :${CONFIG.PORT}/healthz`);
  });
}

main().catch((err) => {
  console.error("Fatal Daemon Loop Crash:", err);
  process.exit(1);
});