import express from "express";
import dotenv from "dotenv";
import { Connection, PublicKey } from "@solana/web3.js";
import { Program, AnchorProvider, Wallet } from "@coral-xyz/anchor";
import { Keypair } from "@solana/web3.js";
import { EmailService } from "./services/emailService";
import { SlackService } from "./services/slackService";
import { logger } from "./utils/logger";

dotenv.config();

const PORT = parseInt(process.env.PORT || "8080", 10);
const SOLANA_RPC = process.env.SOLANA_RPC || "http://127.0.0.1:8899";
const PROGRAM_ID = new PublicKey(process.env.PROGRAM_ID || "ArfCLeaR11111111111111111111111111111111111");

// Minimal ABI IDL snippet for event parsing
const IDL: any = {
  version: "0.1.0",
  name: "arf_clearstream",
  events: [
    {
      name: "FacilityDrawdownEvent",
      fields: [
        { name: "facility", type: "publicKey", index: false },
        { name: "borrower", type: "publicKey", index: false },
        { name: "drawdownAmount", type: "u64", index: false },
        { name: "outstandingPrincipal", type: "u64", index: false },
        { name: "totalAccruedInterest", type: "u64", index: false },
        { name: "settlementHash", type: { array: ["u8", 32] }, index: false },
        { name: "timestamp", type: "i64", index: false },
      ],
    },
    {
      name: "FacilityRepaymentEvent",
      fields: [
        { name: "facility", type: "publicKey", index: false },
        { name: "borrower", type: "publicKey", index: false },
        { name: "amountPaid", type: "u64", index: false },
        { name: "remainingPrincipal", type: "u64", index: false },
        { name: "remainingInterest", type: "u64", index: false },
        { name: "timestamp", type: "i64", index: false },
      ],
    },
  ],
};

async function main() {
  const emailService = new EmailService(
    process.env.RESEND_API_KEY || "re_mock_test_key",
    process.env.EMAIL_SENDER || "settlements@arf.one"
  );

  const slackService = new SlackService(
    process.env.SLACK_WEBHOOK_URL || "https://httpbin.org/post"
  );

  const connection = new Connection(SOLANA_RPC, "confirmed");
  const dummyWallet = new Wallet(Keypair.generate());
  const provider = new AnchorProvider(connection, dummyWallet, { commitment: "confirmed" });
  const program = new Program(IDL, PROGRAM_ID, provider);

  logger.info(`Starting Arf ClearStream Orchestrator daemon on RPC: ${SOLANA_RPC}`);
  logger.info(`Listening for on-chain events from Program: ${PROGRAM_ID.toBase58()}`);

  // Register Event Listeners
  program.addEventListener("FacilityDrawdownEvent", async (event: any) => {
    const facilityPubkey = event.facility.toBase58();
    const borrowerPubkey = event.borrower.toBase58();
    const amountUSDC = Number(event.drawdownAmount) / 1_000_000;
    const outstandingUSDC = Number(event.outstandingPrincipal) / 1_000_000;
    const hashHex = Buffer.from(event.settlementHash).toString("hex");

    logger.info(`[EVENT] FacilityDrawdown: ${amountUSDC} USDC to ${borrowerPubkey}`);

    // Off-chain audit dispatch
    await Promise.all([
      emailService.sendDrawdownStatement(
        process.env.TREASURY_RECIPIENT_EMAIL || "treasury-ops@partner.com",
        facilityPubkey,
        borrowerPubkey,
        amountUSDC,
        hashHex,
        outstandingUSDC
      ),
      slackService.sendRiskAlert(
        facilityPubkey,
        borrowerPubkey,
        amountUSDC,
        (outstandingUSDC / (outstandingUSDC + 100_000)) * 100 // Sample risk metric
      ),
    ]);
  });

  program.addEventListener("FacilityRepaymentEvent", (event: any) => {
    const amountPaid = Number(event.amountPaid) / 1_000_000;
    const remPrincipal = Number(event.remainingPrincipal) / 1_000_000;
    const remInterest = Number(event.remainingInterest) / 1_000_000;
    logger.info(
      `[EVENT] FacilityRepayment: Paid $${amountPaid} USDC. Remaining Principal: $${remPrincipal}, Remaining Interest: $${remInterest}`
    );
  });

  // Turnkey Health Check API
  const app = express();
  app.use(express.json());

  app.get("/health", async (_req, res) => {
    try {
      const slot = await connection.getSlot();
      res.status(200).json({
        status: "HEALTHY",
        chain: "solana",
        slot,
        programId: PROGRAM_ID.toBase58(),
        timestamp: new Date().toISOString(),
      });
    } catch (err: any) {
      res.status(503).json({ status: "DEGRADED", error: err.message });
    }
  });

  app.listen(PORT, "0.0.0.0", () => {
    logger.info(`ClearStream Telemetry and Health Service running on port ${PORT}`);
  });
}

main().catch((err) => {
  logger.error(`Critical Worker failure: ${err}`);
  process.exit(1);
});