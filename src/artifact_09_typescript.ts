import { Connection, PublicKey, clusterApiUrl } from "@solana/web3.js";
import { BorshCoder, EventParser } from "@coral-xyz/anchor";
import { Resend } from "resend";
import axios from "axios";
import * as dotenv from "dotenv";

dotenv.config();

// Configuration parameters
const SOLANA_RPC_URL = process.env.SOLANA_RPC_URL || clusterApiUrl("devnet");
const SOLANA_WS_URL = process.env.SOLANA_WS_URL || "wss://api.devnet.solana.com";
const PROGRAM_ID = new PublicKey(process.env.PROGRAM_ID || "ArfNxus1111111111111111111111111111111111111");
const RESEND_API_KEY = process.env.RESEND_API_KEY || "";
const SLACK_WEBHOOK_URL = process.env.SLACK_WEBHOOK_URL || "";
const ALERT_EMAIL_TO = process.env.ALERT_EMAIL_TO || "treasury-ops@arf.one";

const resendClient = RESEND_API_KEY ? new Resend(RESEND_API_KEY) : null;

// Minimal IDL for Event Parsing
const IDL: any = {
  version: "0.1.0",
  name: "arf_nexus",
  events: [
    {
      name: "PoolInitializedEvent",
      fields: [
        { name: "pool", type: "publicKey", index: false },
        { name: "authority", type: "publicKey", index: false },
        { name: "mint", type: "publicKey", index: false },
        { name: "timestamp", type: "i64", index: false },
      ],
    },
    {
      name: "LiquidityDepositedEvent",
      fields: [
        { name: "pool", type: "publicKey", index: false },
        { name: "depositor", type: "publicKey", index: false },
        { name: "amount", type: "u64", index: false },
        { name: "newTotalLiquidity", type: "u64", index: false },
        { name: "timestamp", type: "i64", index: false },
      ],
    },
    {
      name: "CreditLineOpenedEvent",
      fields: [
        { name: "creditLine", type: "publicKey", index: false },
        { name: "pool", type: "publicKey", index: false },
        { name: "borrower", type: "publicKey", index: false },
        { name: "creditLimit", type: "u64", index: false },
        { name: "interestRateBps", type: "u16", index: false },
        { name: "tenorSeconds", type: "i64", index: false },
        { name: "timestamp", type: "i64", index: false },
      ],
    },
    {
      name: "CreditDrawnEvent",
      fields: [
        { name: "creditLine", type: "publicKey", index: false },
        { name: "borrower", type: "publicKey", index: false },
        { name: "amount", type: "u64", index: false },
        { name: "destination", type: "publicKey", index: false },
        { name: "currentPrincipal", type: "u64", index: false },
        { name: "timestamp", type: "i64", index: false },
      ],
    },
    {
      name: "CreditRepaidEvent",
      fields: [
        { name: "creditLine", type: "publicKey", index: false },
        { name: "borrower", type: "publicKey", index: false },
        { name: "interestPaid", type: "u64", index: false },
        { name: "principalPaid", type: "u64", index: false },
        { name: "remainingPrincipal", type: "u64", index: false },
        { name: "remainingInterest", type: "u64", index: false },
        { name: "timestamp", type: "i64", index: false },
      ],
    },
    {
      name: "DelinquencyFlaggedEvent",
      fields: [
        { name: "creditLine", type: "publicKey", index: false },
        { name: "borrower", type: "publicKey", index: false },
        { name: "principalOverdue", type: "u64", index: false },
        { name: "timestamp", type: "i64", index: false },
      ],
    },
  ],
};

class ArfTelemetryService {
  private connection: Connection;
  private eventParser: EventParser;

  constructor() {
    this.connection = new Connection(SOLANA_RPC_URL, {
      wsEndpoint: SOLANA_WS_URL,
      commitment: "confirmed",
    });
    const coder = new BorshCoder(IDL);
    this.eventParser = new EventParser(PROGRAM_ID, coder);
  }

  public async start(): Promise<void> {
    console.log("=================================================");
    console.log("🚀 ARF NEXUS ENTERPRISE TELEMETRY DAEMON");
    console.log(`Program ID: ${PROGRAM_ID.toBase58()}`);
    console.log(`RPC Endpoint: ${SOLANA_RPC_URL}`);
    console.log("=================================================");

    this.connection.onLogs(
      PROGRAM_ID,
      async ({ logs, err, signature }) => {
        if (err) {
          console.error(`❌ Transaction failed in signature: ${signature}`);
          return;
        }

        const events = this.eventParser.parseLogs(logs);
        for (const event of events) {
          await this.handleProgramEvent(event, signature);
        }
      },
      "confirmed"
    );

    console.log("🟢 Listening to Arf Nexus protocol on-chain events...");
  }

  private async handleProgramEvent(event: any, signature: string): Promise<void> {
    console.log(`\n🔔 Event Captured: [${event.name}] in Tx: ${signature}`);

    switch (event.name) {
      case "CreditDrawnEvent":
        await this.onCreditDrawn(event.data, signature);
        break;
      case "CreditRepaidEvent":
        await this.onCreditRepaid(event.data, signature);
        break;
      case "DelinquencyFlaggedEvent":
        await this.onDelinquencyFlagged(event.data, signature);
        break;
      default:
        console.log(`Event details: ${JSON.stringify(event.data, null, 2)}`);
    }
  }

  private async onCreditDrawn(data: any, signature: string): Promise<void> {
    const formattedAmount = (Number(data.amount) / 1e6).toFixed(2);
    const message = `💳 *Arf Nexus Credit Drawdown Completed*\n` +
      `• *Borrower:* \`${data.borrower.toString()}\`\n` +
      `• *Amount Drawn:* \`$${formattedAmount} USDC\`\n` +
      `• *Destination:* \`${data.destination.toString()}\`\n` +
      `• *Tx Signature:* https://solscan.io/tx/${signature}?cluster=devnet`;

    await this.postSlackWebhook(message);
    await this.sendEmailNotification(
      `[Drawdown Confirmed] $${formattedAmount} USDC - Arf Nexus`,
      `<h3>Working Capital Drawdown Executed</h3>
       <p>Borrower <strong>${data.borrower.toString()}</strong> drew <strong>$${formattedAmount} USDC</strong>.</p>
       <p>Destination: <code>${data.destination.toString()}</code></p>
       <p>Transaction: <a href="https://solscan.io/tx/${signature}?cluster=devnet">${signature}</a></p>`
    );
  }

  private async onCreditRepaid(data: any, signature: string): Promise<void> {
    const principal = (Number(data.principalPaid) / 1e6).toFixed(2);
    const interest = (Number(data.interestPaid) / 1e6).toFixed(4);
    const message = `✅ *Arf Nexus Repayment Settled*\n` +
      `• *Borrower:* \`${data.borrower.toString()}\`\n` +
      `• *Principal Cleared:* \`$${principal} USDC\`\n` +
      `• *Interest Accrued & Settled:* \`$${interest} USDC\`\n` +
      `• *Tx Signature:* https://solscan.io/tx/${signature}?cluster=devnet`;

    await this.postSlackWebhook(message);
  }

  private async onDelinquencyFlagged(data: any, signature: string): Promise<void> {
    const overdue = (Number(data.principalOverdue) / 1e6).toFixed(2);
    const alertMessage = `🚨 *URGENT: Arf Nexus Delinquency Flagged*\n` +
      `• *Borrower:* \`${data.borrower.toString()}\`\n` +
      `• *Credit Line:* \`${data.creditLine.toString()}\`\n` +
      `• *Overdue Principal:* \`$${overdue} USDC\`\n` +
      `• *Action Required:* Initiate guarantor collateral remediation immediately.`;

    await this.postSlackWebhook(alertMessage);
    await this.sendEmailNotification(
      `🚨 [RISK ALERT] Delinquency Detected for MSB: ${data.borrower.toString().slice(0, 8)}...`,
      `<h2 style="color:red;">Delinquent Facility Notice</h2>
       <p>Borrower <strong>${data.borrower.toString()}</strong> failed to clear tenor maturity.</p>
       <p>Overdue Balance: <strong>$${overdue} USDC</strong></p>
       <p>Investigation Tx: <a href="https://solscan.io/tx/${signature}?cluster=devnet">${signature}</a></p>`
    );
  }

  private async postSlackWebhook(markdownText: string): Promise<void> {
    if (!SLACK_WEBHOOK_URL) return;
    try {
      await axios.post(SLACK_WEBHOOK_URL, { text: markdownText });
    } catch (err: any) {
      console.error(`Slack webhook error: ${err.message}`);
    }
  }

  private async sendEmailNotification(subject: string, htmlContent: string): Promise<void> {
    if (!resendClient) return;
    try {
      await resendClient.emails.send({
        from: "Arf Nexus Notifications <onboarding@resend.dev>",
        to: ALERT_EMAIL_TO,
        subject,
        html: htmlContent,
      });
    } catch (err: any) {
      console.error(`Resend email delivery error: ${err.message}`);
    }
  }
}

// Entrypoint
const service = new ArfTelemetryService();
service.start().catch((err) => {
  console.error("Fatal Daemon Startup Exception:", err);
  process.exit(1);
});