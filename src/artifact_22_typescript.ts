import { Resend } from "resend";
import { logger } from "../utils/logger";

export class EmailService {
  private resend: Resend;
  private defaultSender: string;

  constructor(apiKey: string, defaultSender: string) {
    this.resend = new Resend(apiKey);
    this.defaultSender = defaultSender;
  }

  async sendDrawdownStatement(
    recipient: string,
    facilityPubkey: string,
    borrowerPubkey: string,
    amountUSDC: number,
    settlementHash: string,
    totalOutstanding: number
  ): Promise<boolean> {
    try {
      const emailHtml = `
        <div style="font-family: Arial, sans-serif; max-width: 600px; margin: 0 auto; border: 1px solid #e2e8f0; border-radius: 8px; padding: 24px;">
          <h2 style="color: #0f172a;">Arf ClearStream Settlement Receipt</h2>
          <p style="color: #475569;">A programmatic working capital drawdown has been verified on Solana.</p>
          <hr style="border: 0; border-top: 1px solid #e2e8f0; margin: 16px 0;" />
          <table style="width: 100%; border-collapse: collapse; font-size: 14px;">
            <tr><td style="padding: 8px 0; color: #64748b;">Facility ID:</td><td style="font-family: monospace; font-weight: bold;">${facilityPubkey}</td></tr>
            <tr><td style="padding: 8px 0; color: #64748b;">Borrower:</td><td style="font-family: monospace;">${borrowerPubkey}</td></tr>
            <tr><td style="padding: 8px 0; color: #64748b;">Drawdown Amount:</td><td style="font-weight: bold; color: #16a34a;">${amountUSDC.toLocaleString()} USDC</td></tr>
            <tr><td style="padding: 8px 0; color: #64748b;">Settlement Hash:</td><td style="font-family: monospace; font-size: 12px;">${settlementHash}</td></tr>
            <tr><td style="padding: 8px 0; color: #64748b;">Updated Outstanding Balance:</td><td style="font-weight: bold;">${totalOutstanding.toLocaleString()} USDC</td></tr>
          </table>
          <div style="margin-top: 24px; padding: 12px; background-color: #f8fafc; border-radius: 6px; font-size: 12px; color: #64748b;">
            This transaction is anchored deterministically on Solana and satisfies Arf FinTech bilateral clearing standards.
          </div>
        </div>
      `;

      const response = await this.resend.emails.send({
        from: this.defaultSender,
        to: recipient,
        subject: `[CLEARING NOTIFICATION] ${amountUSDC.toLocaleString()} USDC Disbursed - ${facilityPubkey.slice(0, 8)}`,
        html: emailHtml,
      });

      logger.info(`Settlement email successfully dispatched to ${recipient}. Resend ID: ${response.data?.id}`);
      return true;
    } catch (error) {
      logger.error(`Failed to send settlement email via Resend: ${error}`);
      return false;
    }
  }
}