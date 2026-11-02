import * as anchor from "@coral-xyz/anchor";
import { Program, Idl } from "@coral-xyz/anchor";
import { Keypair, PublicKey, Connection, clusterApiUrl } from "@solana/web3.js";
import { getOrCreateAssociatedTokenAccount } from "@solana/spl-token";
import { Resend } from "resend";
import axios from "axios";
import * as dotenv from "dotenv";

dotenv.config();

// Configuration
const RESEND_API_KEY = process.env.RESEND_API_KEY || "re_mock_test_key";
const SLACK_WEBHOOK_URL = process.env.SLACK_WEBHOOK_URL || "";
const SOLANA_RPC_URL = process.env.SOLANA_RPC_URL || clusterApiUrl("devnet");
const TREASURY_AUDIT_EMAIL = process.env.TREASURY_AUDIT_EMAIL || "treasury-audit@arf.one";

const resend = new Resend(RESEND_API_KEY);

interface CorridorDrawdownNotification {
  facility: string;
  borrower: string;
  amount: number;
  corridor: string;
  txHash: string;
  timestamp: string;
}

export class ArfEnterpriseRelay {
  private connection: Connection;
  private provider: anchor.AnchorProvider;

  constructor(walletKeypair: Keypair) {
    this.connection = new Connection(SOLANA_RPC_URL, "confirmed");
    const wallet = new anchor.Wallet(walletKeypair);
    this.provider = new anchor.AnchorProvider(this.connection, wallet, {
      commitment: "confirmed",
    });
  }

  /**
   * Broadcasts real-time risk alert to corporate Slack channel
   */
  public async alertSlack(notification: CorridorDrawdownNotification): Promise<void> {
    if (!SLACK_WEBHOOK_URL) {
      console.log("[TELEMETRY] No SLACK_WEBHOOK_URL configured. Skipping Slack dispatch.");
      return;
    }

    const payload = {
      text: `🚨 *Arf-Flow Treasury Drawdown Notice*`,
      attachments: [
        {
          color: "#0052FF",
          fields: [
            { title: "Corridor", value: notification.corridor, short: true },
            { title: "Amount (USDC)", value: `$${notification.amount.toLocaleString()}`, short: true },
            { title: "Borrower MSB", value: `\`${notification.borrower}\``, short: false },
            { title: "Transaction Hash", value: `https://solscan.io/tx/${notification.txHash}?cluster=devnet`, short: false },
            { title: "Timestamp", value: notification.timestamp, short: true },
          ],
        },
      ],
    };

    try {
      await axios.post(SLACK_WEBHOOK_URL, payload);
      console.log("✅ [SLACK] Institutional alert dispatched successfully.");
    } catch (error) {
      console.error("❌ [SLACK] Failed to send webhook dispatch:", error);
    }
  }

  /**
   * Dispatches cryptographic transaction receipt via Resend Email API
   */
  public async sendAuditEmail(notification: CorridorDrawdownNotification): Promise<void> {
    try {
      const emailHtml = `
        <div style="font-family: Arial, sans-serif; max-width: 600px; margin: auto; padding: 24px; border: 1px solid #e0e0e0; border-radius: 8px;">
          <h2 style="color: #0052ff;">Arf × Superteam Türkiye — Proof-of-Settlement Receipt</h2>
          <p>This automated statement confirms that an intraday working capital drawdown has been disbursed on-chain.</p>
          <table style="width: 100%; border-collapse: collapse; margin-top: 16px;">
            <tr style="background: #f8f9fa;"><td style="padding: 8px;"><b>Borrower MSB:</b></td><td style="padding: 8px;">${notification.borrower}</td></tr>
            <tr><td style="padding: 8px;"><b>Corridor:</b></td><td style="padding: 8px;">${notification.corridor}</td></tr>
            <tr style="background: #f8f9fa;"><td style="padding: 8px;"><b>Disbursed Amount:</b></td><td style="padding: 8px;"><b>${notification.amount} USDC</b></td></tr>
            <tr><td style="padding: 8px;"><b>Solana Transaction:</b></td><td style="padding: 8px;"><a href="https://solscan.io/tx/${notification.txHash}?cluster=devnet">${notification.txHash.slice(0, 16)}...</a></td></tr>
            <tr style="background: #f8f9fa;"><td style="padding: 8px;"><b>Settlement Time:</b></td><td style="padding: 8px;">${notification.timestamp}</td></tr>
          </table>
          <p style="margin-top: 24px; font-size: 12px; color: #6c757d;">Arf-Flow Protocol &bull; Zero Pre-funding Cross-Border Liquidity Engine &bull; Istanbul, TR</p>
        </div>
      `;

      const response = await resend.emails.send({
        from: "Arf Liquidity Protocol <settlements@arf.one>",
        to: [TREASURY_AUDIT_EMAIL],
        subject: `[CONFIRMATION] Liquidity Drawdown: ${notification.amount} USDC (${notification.corridor})`,
        html: emailHtml,
      });

      console.log(`✉️ [RESEND] Compliance statement emailed. Dispatch ID: ${response.data?.id}`);
    } catch (error) {
      console.error("❌ [RESEND] Failed to send compliance invoice:", error);
    }
  }

  /**
   * Monitor Solana on-chain event stream and dispatch enterprise events
   */
  public async listenToEvents(program: Program): Promise<void> {
    console.log("🚀 [DAEMON] Listening for Arf-Flow Drawdown and Repayment Events...");

    program.addEventListener("DrawdownExecuted", async (event: any, slot: number) => {
      console.log(`\n🔔 Drawdown Detected at slot ${slot}`);
      const payload: CorridorDrawdownNotification = {
        facility: event.facility.toBase58(),
        borrower: event.borrower.toBase58(),
        amount: event.amount.toNumber() / 1_000_000,
        corridor: "TRY-USD",
        txHash: "simulated_slot_" + slot,
        timestamp: new Date().toISOString(),
      };

      await Promise.all([
        this.alertSlack(payload),
        this.sendAuditEmail(payload),
      ]);
    });
  }
}

// Quick CLI execution wrapper
async function main() {
  const dummyKeypair = Keypair.generate();
  const orchestrator = new ArfEnterpriseRelay(dummyKeypair);
  console.log("Arf-Flow Enterprise Integration Service initialized.");
  console.log(`Monitoring Node: ${dummyKeypair.publicKey.toBase58()}`);
}

if (require.main === module) {
  main().catch(console.error);
}