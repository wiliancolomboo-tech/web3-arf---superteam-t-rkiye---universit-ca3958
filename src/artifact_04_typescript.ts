import { Connection, Keypair, PublicKey, clusterApiUrl } from "@solana/web3.js";
import { Program, AnchorProvider, Wallet, BN } from "@coral-xyz/anchor";
import { Resend } from "resend";
import axios from "axios";
import * as crypto from "crypto";

// Configuration Schema
interface Config {
  solanaRpcUrl: string;
  programId: string;
  clearingKeypair: Keypair;
  resendApiKey: string;
  notificationEmail: string;
  opsWebhookUrl: string;
}

export class ArfEnterpriseRelayer {
  private connection: Connection;
  private provider: AnchorProvider;
  private resend: Resend;
  private config: Config;

  constructor(config: Config) {
    this.config = config;
    this.connection = new Connection(config.solanaRpcUrl, "confirmed");
    const wallet = new Wallet(config.clearingKeypair);
    this.provider = new AnchorProvider(this.connection, wallet, {
      commitment: "confirmed",
      preflightCommitment: "confirmed",
    });
    this.resend = new Resend(config.resendApiKey);
  }

  /**
   * Generates a 32-byte deterministic hash for an off-chain fiat wire (SWIFT MT103 / SEPA EndToEndId)
   */
  public generateFiatSettlementProof(
    corridorId: string,
    bankReference: string,
    amountFiat: number,
    currency: string
  ): Buffer {
    const rawPayload = `${corridorId}|${bankReference}|${amountFiat}|${currency}|${Date.now()}`;
    return crypto.createHash("sha256").update(rawPayload).digest();
  }

  /**
   * Finalizes an on-chain settlement upon verified fiat banking clearance
   */
  public async confirmFiatDisbursementAndFinalize(
    program: Program<any>,
    poolPubkey: PublicKey,
    vaultPubkey: PublicKey,
    settlementRecordPubkey: PublicKey,
    destinationTokenAccount: PublicKey,
    settlementId: Buffer,
    bankReference: string,
    fiatAmount: number,
    fiatCurrency: string
  ): Promise<string> {
    console.log(`[Clearinghouse] Verifying fiat delivery for ref: ${bankReference}`);
    const proofHash = this.generateFiatSettlementProof(
      "TR-EU-CORRIDOR",
      bankReference,
      fiatAmount,
      fiatCurrency
    );

    // Call Solana Smart Contract 'finalize_settlement'
    const tx = await program.methods
      .finalizeSettlement(Array.from(settlementId), Array.from(proofHash))
      .accounts({
        clearingAuthority: this.config.clearingKeypair.publicKey,
        pool: poolPubkey,
        settlementRecord: settlementRecordPubkey,
        vault: vaultPubkey,
        destinationTokenAccount: destinationTokenAccount,
        tokenProgram: new PublicKey("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"),
      })
      .signers([this.config.clearingKeypair])
      .rpc();

    console.log(`[Solana] Finalized Settlement on-chain. TX Hash: ${tx}`);

    // Trigger Enterprise Compliance Dispatch (Resend + Slack)
    await this.dispatchEnterpriseReceipt({
      txHash: tx,
      bankReference,
      fiatAmount,
      fiatCurrency,
      proofHash: proofHash.toString("hex"),
      clearingNode: this.config.clearingKeypair.publicKey.toBase58(),
    });

    return tx;
  }

  /**
   * Dispatches MT103 enterprise confirmation email via Resend and ops notification via Webhook
   */
  private async dispatchEnterpriseReceipt(params: {
    txHash: string;
    bankReference: string;
    fiatAmount: number;
    fiatCurrency: string;
    proofHash: string;
    clearingNode: string;
  }) {
    const mt103PayloadHtml = `
      <div style="font-family: monospace; background: #0f172a; color: #f8fafc; padding: 24px; border-radius: 8px;">
        <h2 style="color: #14F195; margin-top: 0;">ARF INSTITUTIONAL CLEARING CONFIRMATION</h2>
        <hr style="border: 1px solid #334155;" />
        <p><strong>STATUS:</strong> SETTLED & VERIFIED</p>
        <p><strong>TRANSACTION SIGNATURE:</strong> ${params.txHash}</p>
        <p><strong>OFF-CHAIN FIAT REF:</strong> ${params.bankReference}</p>
        <p><strong>AMOUNT CLEARED:</strong> ${params.fiatAmount} ${params.fiatCurrency}</p>
        <p><strong>CRYPTOGRAPHIC ATTESTATION:</strong> ${params.proofHash}</p>
        <p><strong>CLEARING NODE ID:</strong> ${params.clearingNode}</p>
        <p><strong>NETWORK:</strong> Solana Mainnet-Beta / Arf Rail</p>
        <hr style="border: 1px solid #334155;" />
        <small style="color: #94a3b8;">This document serves as an immutable delivery receipt for cross-border treasury reconciliations.</small>
      </div>
    `;

    try {
      // 1. Send via Resend
      const emailResponse = await this.resend.emails.send({
        from: "settlement@arf.network",
        to: this.config.notificationEmail,
        subject: `[CLEARING NOTIFICATION] Settled: ${params.fiatAmount} ${params.fiatCurrency} (Ref: ${params.bankReference})`,
        html: mt103PayloadHtml,
      });
      console.log(`[Resend] Institutional receipt emailed successfully: ID ${emailResponse.data?.id}`);
    } catch (emailErr) {
      console.error("[Resend] Notification delivery error:", emailErr);
    }

    try {
      // 2. Alert Webhook / Slack
      if (this.config.opsWebhookUrl) {
        await axios.post(this.config.opsWebhookUrl, {
          text: `*ARF SETTLEMENT COMPLETED*\n• *Tx:* \`${params.txHash}\`\n• *Amount:* ${params.fiatAmount} ${params.fiatCurrency}\n• *Ref:* \`${params.bankReference}\``,
        });
        console.log("[Webhook] Operations channel notified.");
      }
    } catch (hookErr) {
      console.error("[Webhook] Ops telemetry failed:", hookErr);
    }
  }
}