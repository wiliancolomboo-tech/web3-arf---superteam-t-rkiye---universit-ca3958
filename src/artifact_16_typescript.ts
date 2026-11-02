import axios from 'axios';
import { Resend } from 'resend';

export class EnterpriseTelemetry {
  private resend: Resend | null = null;
  private slackWebhookUrl: string | null = null;
  private treasuryAlertEmail: string;

  constructor() {
    if (process.env.RESEND_API_KEY) {
      this.resend = new Resend(process.env.RESEND_API_KEY);
    }
    this.slackWebhookUrl = process.env.SLACK_WEBHOOK_URL || null;
    this.treasuryAlertEmail = process.env.TREASURY_OPS_EMAIL || 'treasury-ops@arf.one';
  }

  async notifyCreditDraw(params: {
    borrower: string;
    amountUsdc: number;
    signature: string;
    totalDrawn: number;
  }) {
    const title = `🚨 [ARF TREASURY] Credit Line Draw: ${params.amountUsdc.toLocaleString()} USDC`;
    const message = `Borrower: \`${params.borrower}\`\nAmount: \`${params.amountUsdc.toLocaleString()} USDC\`\nTotal Drawn: \`${params.totalDrawn.toLocaleString()} USDC\`\nTx Sig: <https://explorer.solana.com/tx/${params.signature}?cluster=devnet|${params.signature.slice(0, 12)}...>`;

    // 1. Emit to Slack Channel
    await this.sendSlack(title, message, '#FF9900');

    // 2. Emit to Resend Institutional Email
    await this.sendEmail(
      `Arf Treasury Alert: Credit Draw Executed (${params.amountUsdc} USDC)`,
      `<h2>Arf Settlement Engine - Credit Draw Notification</h2>
       <p><strong>Borrower Public Key:</strong> ${params.borrower}</p>
       <p><strong>Principal Drawn:</strong> $${params.amountUsdc.toLocaleString()} USDC</p>
       <p><strong>Cumulative Outstanding Debt:</strong> $${params.totalDrawn.toLocaleString()} USDC</p>
       <p><strong>Solana Transaction Signature:</strong> <a href="https://explorer.solana.com/tx/${params.signature}?cluster=devnet">${params.signature}</a></p>
       <hr/>
       <p><em>Security Notice: This transaction was deterministically signed and verified on Solana Mainnet/Devnet via Arf Smart Contracts.</em></p>`
    );
  }

  async notifySettlementBatch(params: {
    corridorId: string;
    sender: string;
    receiver: string;
    amountUsdc: number;
    feeUsdc: number;
    signature: string;
  }) {
    const title = `✅ [ARF CORRIDOR] B2B Settlement Completed: ${params.corridorId}`;
    const message = `Corridor: \`${params.corridorId}\`\nVolume: \`${params.amountUsdc.toLocaleString()} USDC\`\nFee Retained: \`${params.feeUsdc.toLocaleString()} USDC\`\nTx: <https://explorer.solana.com/tx/${params.signature}?cluster=devnet|${params.signature.slice(0, 8)}...>`;

    await this.sendSlack(title, message, '#36A64F');
  }

  private async sendSlack(title: string, text: string, color: string) {
    if (!this.slackWebhookUrl) {
      console.log(`[SLACK DISABLED] ${title}\n${text}`);
      return;
    }
    try {
      await axios.post(this.slackWebhookUrl, {
        attachments: [
          {
            color,
            title,
            text,
            ts: Math.floor(Date.now() / 1000),
          },
        ],
      });
    } catch (err: any) {
      console.error(`Slack webhook error:`, err.message);
    }
  }

  private async sendEmail(subject: string, html: string) {
    if (!this.resend) {
      console.log(`[RESEND DISABLED] Subject: ${subject}`);
      return;
    }
    try {
      await this.resend.emails.send({
        from: 'Arf Automation Engine <alerts@arf.solutions>',
        to: [this.treasuryAlertEmail],
        subject,
        html,
      });
    } catch (err: any) {
      console.error(`Resend email dispatch error:`, err.message);
    }
  }
}