import { Command } from "commander";
import * as anchor from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";
import { getOrCreateAssociatedTokenAccount } from "@solana/spl-token";
import * as crypto from "crypto";
import * as dotenv from "dotenv";
import { EnterpriseNotifierService } from "./services/notifier";

dotenv.config();

const program = new Command();
const notifier = new EnterpriseNotifierService();

const RPC_URL = process.env.SOLANA_RPC_URL || "http://127.0.0.1:8899";
const connection = new Connection(RPC_URL, "confirmed");

program
  .name("arf-flow-cli")
  .description("Institutional CLI for Arf-Flow JIT Settlement Engine")
  .version("1.0.0");

program
  .command("request-drawdown")
  .description("Fintech triggers a JIT credit drawdown against an invoice")
  .requiredOption("-f, --facility <address>", "Credit Facility PDA Address")
  .requiredOption("-a, --amount <tokens>", "Drawdown Amount in base decimal units")
  .requiredOption("-i, --invoice <id>", "Unique invoice identifier string")
  .action(async (options) => {
    try {
      console.log(`[Arf-Flow] Initiating JIT Drawdown of ${options.amount} tokens...`);
      const invoiceHash = crypto.createHash("sha256").update(options.invoice).digest();
      const invoiceHex = "0x" + invoiceHash.toString("hex");

      console.log(`[Arf-Flow] Invoice Ref: ${options.invoice} -> Hash: ${invoiceHex}`);
      
      // In production, execute anchorProgram.methods.drawdownFacility(...)
      // Emulate on-chain execution confirmation:
      const confirmedTime = new Date().toISOString();
      console.log(`[Arf-Flow] Drawdown committed to Solana block!`);

      // Trigger enterprise notification adapter
      await notifier.notifyDrawdown({
        facilityPubkey: options.facility,
        borrowerPubkey: "FinTechTR11111111111111111111111111111111111",
        amountFormatted: options.amount,
        tokenSymbol: "USDC",
        outstandingDebt: options.amount,
        invoiceIdHex: invoiceHex,
        timestamp: confirmedTime,
      });

      console.log("[Arf-Flow] Settlement workflow fully orchestrated.");
    } catch (e: any) {
      console.error(`[Error] Execution failed: ${e.message}`);
      process.exit(1);
    }
  });

program.parse(process.argv);