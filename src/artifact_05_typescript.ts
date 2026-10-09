import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { ArfSve } from "../target/types/arf_sve";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { Keypair, PublicKey, SystemProgram } from "@solana/web3.js";
import { assert, expect } from "chai";
import * as crypto from "crypto";

describe("Arf-SVE: Institutional Settlement Suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.ArfSve as Program<ArfSve>;

  const admin = (provider.wallet as anchor.Wallet).payer;
  const borrower = Keypair.generate();
  const recipient = Keypair.generate();
  const unauthorizedActor = Keypair.generate();

  let treasuryMint: PublicKey;
  let configPda: PublicKey;
  let treasuryVaultPda: PublicKey;
  let facilityPda: PublicKey;

  const corridorId = Buffer.alloc(16);
  Buffer.from("TRY-USDC-CORR-01").copy(corridorId, 0, 0, 16);

  const CREDIT_LIMIT = new anchor.BN(1_000_000 * 1_000_000); // 1,000,000 USDC
  const APR_BPS = 850; // 8.50% APR

  before(async () => {
    // 1. Fund accounts with SOL
    const airdropSig1 = await provider.connection.requestAirdrop(borrower.publicKey, 20 * anchor.web3.LAMPORTS_PER_SOL);
    await provider.connection.confirmTransaction(airdropSig1);

    const airdropSig2 = await provider.connection.requestAirdrop(unauthorizedActor.publicKey, 5 * anchor.web3.LAMPORTS_PER_SOL);
    await provider.connection.confirmTransaction(airdropSig2);

    // 2. Create Treasury Settlement Token Mint (USDC equivalent)
    treasuryMint = await createMint(
      provider.connection,
      admin,
      admin.publicKey,
      null,
      6
    );

    // 3. Derive PDAs
    [configPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("protocol_config")],
      program.programId
    );

    [treasuryVaultPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("treasury_vault"), configPda.toBuffer()],
      program.programId
    );

    [facilityPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("facility"), configPda.toBuffer(), borrower.publicKey.toBuffer(), corridorId],
      program.programId
    );
  });

  it("1. Successfully initializes protocol config & treasury vault", async () => {
    const adminFeeBps = 15; // 0.15% fee

    await program.methods
      .initializeCorridorProtocol(adminFeeBps)
      .accounts({
        config: configPda,
        treasuryMint: treasuryMint,
        treasuryVault: treasuryVaultPda,
        admin: admin.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    const configAccount = await program.account.protocolConfig.fetch(configPda);
    assert.equal(configAccount.admin.toBase58(), admin.publicKey.toBase58());
    assert.equal(configAccount.adminFeeBps, adminFeeBps);
    assert.equal(configAccount.isPaused, false);

    // Fund the treasury vault with 5,000,000 USDC liquidity
    await mintTo(
      provider.connection,
      admin,
      treasuryMint,
      treasuryVaultPda,
      admin,
      5_000_000 * 1_000_000
    );

    const vaultAccount = await getAccount(provider.connection, treasuryVaultPda);
    assert.equal(vaultAccount.amount.toString(), (5_000_000 * 1_000_000).toString());
  });

  it("2. Prevents non-admin from opening a credit facility", async () => {
    try {
      await program.methods
        .openCreditFacility(CREDIT_LIMIT, APR_BPS, Array.from(corridorId))
        .accounts({
          config: configPda,
          borrower: borrower.publicKey,
          facility: facilityPda,
          admin: unauthorizedActor.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .signers([unauthorizedActor])
        .rpc();
      assert.fail("Should have thrown UnauthorizedAdmin");
    } catch (err: any) {
      expect(err.message).to.include("UnauthorizedAdmin");
    }
  });

  it("3. Admin authorizes a dynamic credit facility for licensed borrower", async () => {
    await program.methods
      .openCreditFacility(CREDIT_LIMIT, APR_BPS, Array.from(corridorId))
      .accounts({
        config: configPda,
        borrower: borrower.publicKey,
        facility: facilityPda,
        admin: admin.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .rpc();

    const facility = await program.account.creditFacility.fetch(facilityPda);
    assert.equal(facility.borrower.toBase58(), borrower.publicKey.toBase58());
    assert.equal(facility.creditLimit.toNumber(), CREDIT_LIMIT.toNumber());
    assert.equal(facility.currentPrincipal.toNumber(), 0);
    assert.equal(facility.isActive, true);
  });

  it("4. Executes atomic drawdown and settlement to destination token account", async () => {
    const drawAmount = new anchor.BN(250_000 * 1_000_000); // 250,000 USDC
    const sequenceId = crypto.randomBytes(32);

    const recipientAta = await createAccount(
      provider.connection,
      borrower,
      treasuryMint,
      recipient.publicKey
    );

    const [settlementRecordPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("settlement_record"), facilityPda.toBuffer(), sequenceId],
      program.programId
    );

    await program.methods
      .drawDownAndSettle(drawAmount, Array.from(sequenceId))
      .accounts({
        config: configPda,
        facility: facilityPda,
        treasuryVault: treasuryVaultPda,
        recipientTokenAccount: recipientAta,
        settlementRecord: settlementRecordPda,
        borrower: borrower.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
      })
      .signers([borrower])
      .rpc();

    // Verify recipient received exact settled funds
    const recipientBalance = await getAccount(provider.connection, recipientAta);
    assert.equal(recipientBalance.amount.toString(), drawAmount.toString());

    // Verify state update on facility
    const facility = await program.account.creditFacility.fetch(facilityPda);
    assert.equal(facility.currentPrincipal.toString(), drawAmount.toString());

    // Verify settlement audit record
    const record = await program.account.settlementRecord.fetch(settlementRecordPda);
    assert.equal(record.amount.toString(), drawAmount.toString());
    assert.equal(record.recipient.toBase58(), recipientAta.toBase58());
  });

  it("5. Rejects drawdown exceeding credit ceiling", async () => {
    const excessiveAmount = new anchor.BN(800_000 * 1_000_000); // 250k + 800k = 1.05M > 1M
    const sequenceId = crypto.randomBytes(32);

    const recipientAta = await createAccount(
      provider.connection,
      borrower,
      treasuryMint,
      Keypair.generate().publicKey
    );

    const [settlementRecordPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("settlement_record"), facilityPda.toBuffer(), sequenceId],
      program.programId
    );

    try {
      await program.methods
        .drawDownAndSettle(excessiveAmount, Array.from(sequenceId))
        .accounts({
          config: configPda,
          facility: facilityPda,
          treasuryVault: treasuryVaultPda,
          recipientTokenAccount: recipientAta,
          settlementRecord: settlementRecordPda,
          borrower: borrower.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
        })
        .signers([borrower])
        .rpc();
      assert.fail("Should have failed with CreditLimitExceeded");
    } catch (err: any) {
      expect(err.message).to.include("CreditLimitExceeded");
    }
  });

  it("6. Handles repayment cascading against interest and principal", async () => {
    // Fund borrower ATA for repayment
    const borrowerAta = await createAccount(
      provider.connection,
      borrower,
      treasuryMint,
      borrower.publicKey
    );

    const repayAmount = new anchor.BN(100_000 * 1_000_000); // 100,000 USDC

    await mintTo(
      provider.connection,
      admin,
      treasuryMint,
      borrowerAta,
      admin,
      repayAmount.toNumber()
    );

    await program.methods
      .repayCreditFacility(repayAmount)
      .accounts({
        config: configPda,
        facility: facilityPda,
        treasuryVault: treasuryVaultPda,
        payerTokenAccount: borrowerAta,
        payer: borrower.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([borrower])
      .rpc();

    const facility = await program.account.creditFacility.fetch(facilityPda);
    assert.equal(facility.currentPrincipal.toNumber(), 150_000 * 1_000_000);
  });

  it("7. Emergency Pause blocks new settlements", async () => {
    // Activate Pause
    await program.methods
      .setProtocolPause(true)
      .accounts({
        config: configPda,
        admin: admin.publicKey,
      })
      .rpc();

    const sequenceId = crypto.randomBytes(32);
    const recipientAta = await createAccount(
      provider.connection,
      borrower,
      treasuryMint,
      Keypair.generate().publicKey
    );

    const [settlementRecordPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("settlement_record"), facilityPda.toBuffer(), sequenceId],
      program.programId
    );

    try {
      await program.methods
        .drawDownAndSettle(new anchor.BN(10_000 * 1_000_000), Array.from(sequenceId))
        .accounts({
          config: configPda,
          facility: facilityPda,
          treasuryVault: treasuryVaultPda,
          recipientTokenAccount: recipientAta,
          settlementRecord: settlementRecordPda,
          borrower: borrower.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
        })
        .signers([borrower])
        .rpc();
      assert.fail("Should have halted under ProtocolPaused");
    } catch (err: any) {
      expect(err.message).to.include("ProtocolPaused");
    }

    // Unpause protocol
    await program.methods
      .setProtocolPause(false)
      .accounts({
        config: configPda,
        admin: admin.publicKey,
      })
      .rpc();
  });
});