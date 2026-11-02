import axios from "axios";
import { logger } from "../utils/logger";

export class SlackService {
  private webhookUrl: string;

  constructor(webhookUrl: string) {
    this.webhookUrl = webhookUrl;
  }

  async sendRiskAlert(
    facilityPubkey: string,
    borrowerPubkey: string,
    amountUSDC: number,
    utilizationPercent: number
  ): Promise<boolean> {
    try {
      const payload = {
        blocks: [
          {
            type: "header",
            text: {
              type: "plain_text",
              text: "🚨 Arf ClearStream: Credit Drawdown Alert",
              emoji: true,
            },
          },
          {
            type: "section",
            fields: [
              {
                type: "mrkdwn",
                text: `*Facility:*\n\`${facilityPubkey.slice(0, 12)}...\``,
              },
              {
                type: "mrkdwn",
                text: `*Amount Drawn:*\n*$${amountUSDC.toLocaleString()} USDC*`,
              },
              {
                type: "mrkdwn",
                text: `*Borrower:*\n\`${borrowerPubkey.slice(0, 12)}...\``,
              },
              {
                type: "mrkdwn",
                text: `*Current Utilization:*\n*${utilizationPercent.toFixed(2)}%*`,
              },
            ],
          },
        ],
      };

      await axios.post(this.webhookUrl, payload, { timeout: 5000 });
      logger.info(`Slack alert sent for facility ${facilityPubkey}`);
      return true;
    } catch (err) {
      logger.error(`Failed to dispatch Slack alert: ${err}`);
      return false;
    }
  }
}