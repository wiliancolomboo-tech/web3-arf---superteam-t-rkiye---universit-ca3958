import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { ArfFlow } from "../target/types/arf_flow";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { assert, expect } from "chai";
import * as crypto from "crypto";

describe("Arf-Flow Settlement Clearinghouse Tests", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.ArfFlow as Program<ArfFlow>;

  let mint: anchor.web3.PublicKey;
  let adminTokenAccount: anchor.web3.PublicKey;
  let borrowerTokenAccount: anchor.web3.PublicKey;
  let poolPda: anchor.web3.PublicKey;
  let vaultPda: anchor.web3.PublicKey;

  const admin = (provider.wallet as anchor.Wallet).payer;
  const clearingAuthority = anchor.web3.Keypair.generate();
  const borrower = anchor.web3.Keypair.generate();
  const corridorId = "EUR-TRY-CORRIDOR-01";
  const creditLimit = new anchor.BN(1_000_000 * 10 ** 6); // 1,000,000 USDC
  const initialPoolLiquidity = new anchor.BN(500_000 * 10 ** 6); // 500,000 USDC

  before(async () => {
    // AirDrop SOL to test accounts
    const sig1 = await provider.connection.requestAirdrop(clearingAuthority.publicKey, 2 * anchor.web3.LAMPORTS_PER_SOL);
    const sig2 = await provider.connection.requestAirdrop(borrower.publicKey, 2 * anchor.web3.LAMPORTS_PER_SOL);
    await provider.connection.confirmTransaction(sig1);
    await provider.connection.confirmTransaction(sig2);

    // Create Token Mint (Institutional Mock USDC)
    mint = await createMint(
      provider.connection,
      admin,
      admin.publicKey,
      null,
      6
    );

    // Derive PDA addresses
    [poolPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("pool"), Buffer.from(corridorId), mint.toBuffer()],
      program.programId
    );

    [vaultPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), poolPda.toBuffer()],
      program.programId
    );

    // Create Token Accounts
    adminTokenAccount = await createAccount(provider.connection, admin, mint, admin.publicKey);
    borrowerTokenAccount = await createAccount(provider.connection, admin, mint, borrower.publicKey);

    // Mint liquidity to admin
    await mintTo(provider.connection, admin, mint, adminTokenAccount, admin, 10_000_000 * 10 ** 6);
  });

  it("1. Successfully initializes a new corridor pool and PDA vault", async () => {
    await program.methods
      .initializePool(corridorId, creditLimit, new anchor.BN(3600))
      .accounts({
        admin: admin.publicKey,
        clearingAuthority: clearingAuthority.publicKey,
        mint: mint,
        pool: poolPda,
        vault: vaultPda,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    const poolAccount = await program.account.pool.fetch(poolPda);
    expect(poolAccount.corridorId).to.equal(corridorId);
    expect(poolAccount.creditLimit.eq(creditLimit)).to.be.true;
    expect(poolAccount.currentBorrowed.toNumber()).to.equal(0);
    expect(poolAccount.clearingAuthority.toBase58()).to.equal(clearingAuthority.publicKey.toBase58());
  });

  it("2. Deposits liquidity into the corridor vault PDA", async () => {
    await program.methods
      .depositLiquidity(initialPoolLiquidity)
      .accounts({
        funder: admin.publicKey,
        pool: poolPda,
        mint: mint,
        funderTokenAccount: adminTokenAccount,
        vault: vaultPda,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .rpc();

    const vaultAccount = await getAccount(provider.connection, vaultPda);
    expect(Number(vaultAccount.amount)).to.equal(initialPoolLiquidity.toNumber());
  });

  it("3. Initiates an atomic settlement drawdown", async () => {
    const settlementId = crypto.randomBytes(32);
    const drawdownAmount = new anchor.BN(150_000 * 10 ** 6);

    const [settlementPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("settlement"), poolPda.toBuffer(), settlementId],
      program.programId
    );

    await program.methods
      .initiateSettlement(Array.from(settlementId), drawdownAmount)
      .accounts({
        borrower: borrower.publicKey,
        pool: poolPda,
        vault: vaultPda,
        settlementRecord: settlementPda,
        destinationTokenAccount: borrowerTokenAccount,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([borrower])
      .rpc();

    const settlementData = await program.account.settlementRecord.fetch(settlementPda);
    expect(settlementData.amount.eq(drawdownAmount)).to.be.true;
    expect(settlementData.corridorBorrower.toBase58()).to.equal(borrower.publicKey.toBase58());

    const poolData = await program.account.pool.fetch(poolPda);
    expect(poolData.currentBorrowed.eq(drawdownAmount)).to.be.true;
  });

  it("4. Rejects settlement finalization by unauthorized party", async () => {
    const maliciousActor = anchor.web3.Keypair.generate();
    const settlementId = crypto.randomBytes(32);
    const mockHash = crypto.randomBytes(32);

    const [settlementPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("settlement"), poolPda.toBuffer(), settlementId],
      program.programId
    );

    try {
      await program.methods
        .finalizeSettlement(Array.from(settlementId), Array.from(mockHash))
        .accounts({
          clearingAuthority: maliciousActor.publicKey,
          pool: poolPda,
          settlementRecord: settlementPda,
          vault: vaultPda,
          destinationTokenAccount: borrowerTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([maliciousActor])
        .rpc();
      assert.fail("Should have thrown UnauthorizedClearing error");
    } catch (err: any) {
      expect(err.message).to.include("UnauthorizedClearing");
    }
  });

  it("5. Finalizes settlement through authorized clearing authority", async () => {
    const settlementId = crypto.randomBytes(32);
    const drawdownAmount = new anchor.BN(50_000 * 10 ** 6);

    const [settlementPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("settlement"), poolPda.toBuffer(), settlementId],
      program.programId
    );

    // Initiate
    await program.methods
      .initiateSettlement(Array.from(settlementId), drawdownAmount)
      .accounts({
        borrower: borrower.publicKey,
        pool: poolPda,
        vault: vaultPda,
        settlementRecord: settlementPda,
        destinationTokenAccount: borrowerTokenAccount,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([borrower])
      .rpc();

    // Balance check before
    const borrowerBalanceBefore = (await getAccount(provider.connection, borrowerTokenAccount)).amount;

    // Clearing Authority Attests Off-Chain Fiat Delivery
    const offchainRefHash = crypto.createHash("sha256").update("SEPA-TX-109283-CONFIRMED").digest();

    await program.methods
      .finalizeSettlement(Array.from(settlementId), Array.from(offchainRefHash))
      .accounts({
        clearingAuthority: clearingAuthority.publicKey,
        pool: poolPda,
        settlementRecord: settlementPda,
        vault: vaultPda,
        destinationTokenAccount: borrowerTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([clearingAuthority])
      .rpc();

    const borrowerBalanceAfter = (await getAccount(provider.connection, borrowerTokenAccount)).amount;
    expect(Number(borrowerBalanceAfter) - Number(borrowerBalanceBefore)).to.equal(drawdownAmount.toNumber());

    const settlementRecord = await program.account.settlementRecord.fetch(settlementPda);
    expect(settlementRecord.status).to.deep.equal({ settled: {} });
  });

  it("6. Repays credit back into pool and recovers borrowing ceiling", async () => {
    const poolBefore = await program.account.pool.fetch(poolPda);
    const repayAmount = new anchor.BN(50_000 * 10 ** 6);

    await program.methods
      .repayCredit(repayAmount)
      .accounts({
        payer: admin.publicKey,
        pool: poolPda,
        mint: mint,
        payerTokenAccount: adminTokenAccount,
        vault: vaultPda,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .rpc();

    const poolAfter = await program.account.pool.fetch(poolPda);
    expect(poolAfter.currentBorrowed.toNumber()).to.equal(
      poolBefore.currentBorrowed.sub(repayAmount).toNumber()
    );
  });
});