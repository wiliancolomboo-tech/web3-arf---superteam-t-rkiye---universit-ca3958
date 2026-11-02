import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { ArfNexus } from "../target/types/arf_nexus";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { Keypair, PublicKey, SystemProgram } from "@solana/web3.js";
import { expect } from "chai";

describe("Arf Nexus Protocol Comprehensive QA Matrix", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.ArfNexus as Program<ArfNexus>;

  let usdcMint: PublicKey;
  let poolAuthority: Keypair;
  let lpProvider: Keypair;
  let msbBorrower: Keypair;
  let payoutRecipient: Keypair;

  let poolPda: PublicKey;
  let vaultPda: PublicKey;
  let creditLinePda: PublicKey;

  let lpUsdcAccount: PublicKey;
  let borrowerUsdcAccount: PublicKey;
  let recipientUsdcAccount: PublicKey;

  const POOL_SEED_PREFIX = Buffer.from("arf_pool");
  const VAULT_SEED_PREFIX = Buffer.from("arf_vault");
  const CREDIT_LINE_SEED_PREFIX = Buffer.from("arf_credit_line");

  before(async () => {
    poolAuthority = Keypair.generate();
    lpProvider = Keypair.generate();
    msbBorrower = Keypair.generate();
    payoutRecipient = Keypair.generate();

    // Airdrop SOL for gas
    for (const kp of [poolAuthority, lpProvider, msbBorrower, payoutRecipient]) {
      const sig = await provider.connection.requestAirdrop(kp.publicKey, 10 * anchor.web3.LAMPORTS_PER_SOL);
      await provider.connection.confirmTransaction(sig, "confirmed");
    }

    // Deploy mock USDC mint (6 decimals)
    usdcMint = await createMint(
      provider.connection,
      poolAuthority,
      poolAuthority.publicKey,
      null,
      6
    );

    // Derive PDAs
    [poolPda] = PublicKey.findProgramAddressSync(
      [POOL_SEED_PREFIX, usdcMint.toBuffer(), poolAuthority.publicKey.toBuffer()],
      program.programId
    );

    [vaultPda] = PublicKey.findProgramAddressSync(
      [VAULT_SEED_PREFIX, poolPda.toBuffer()],
      program.programId
    );

    [creditLinePda] = PublicKey.findProgramAddressSync(
      [CREDIT_LINE_SEED_PREFIX, poolPda.toBuffer(), msbBorrower.publicKey.toBuffer()],
      program.programId
    );

    // Setup Token Accounts
    lpUsdcAccount = await createAccount(provider.connection, lpProvider, usdcMint, lpProvider.publicKey);
    borrowerUsdcAccount = await createAccount(provider.connection, msbBorrower, usdcMint, msbBorrower.publicKey);
    recipientUsdcAccount = await createAccount(provider.connection, payoutRecipient, usdcMint, payoutRecipient.publicKey);

    // Mint 1,000,000 USDC to LP, and 50,000 USDC to MSB Borrower
    await mintTo(provider.connection, poolAuthority, usdcMint, lpUsdcAccount, poolAuthority, 1_000_000 * 1e6);
    await mintTo(provider.connection, poolAuthority, usdcMint, borrowerUsdcAccount, poolAuthority, 50_000 * 1e6);
  });

  it("1. Successfully initializes institutional pool with fee parameters", async () => {
    const poolName = "Arf-EURC-USDC-Settler";
    const feeBps = 150; // 1.5%

    await program.methods
      .initializePool(poolName, feeBps)
      .accounts({
        authority: poolAuthority.publicKey,
        underlyingMint: usdcMint,
        pool: poolPda,
        vault: vaultPda,
        systemProgram: SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([poolAuthority])
      .rpc();

    const poolAccount = await program.account.institutionalPool.fetch(poolPda);
    expect(poolAccount.authority.toBase58()).to.equal(poolAuthority.publicKey.toBase58());
    expect(poolAccount.underlyingMint.toBase58()).to.equal(usdcMint.toBase58());
    expect(poolAccount.totalLiquidity.toNumber()).to.equal(0);
    expect(poolAccount.totalBorrowed.toNumber()).to.equal(0);
  });

  it("2. Institutional LP deposits 500,000 USDC into pool vault", async () => {
    const depositAmount = new anchor.BN(500_000 * 1e6);

    await program.methods
      .depositLiquidity(depositAmount)
      .accounts({
        depositor: lpProvider.publicKey,
        pool: poolPda,
        vault: vaultPda,
        depositorTokenAccount: lpUsdcAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([lpProvider])
      .rpc();

    const poolAccount = await program.account.institutionalPool.fetch(poolPda);
    expect(poolAccount.totalLiquidity.toString()).to.equal(depositAmount.toString());

    const vaultInfo = await getAccount(provider.connection, vaultPda);
    expect(vaultInfo.amount.toString()).to.equal(depositAmount.toString());
  });

  it("3. Protocol underwrites and opens $100,000 credit line with 800 bps interest", async () => {
    const limit = new anchor.BN(100_000 * 1e6);
    const rateBps = 800; // 8.00% APR
    const tenorSeconds = new anchor.BN(86400 * 7); // 7-day facility

    await program.methods
      .openCreditLine(limit, rateBps, tenorSeconds)
      .accounts({
        authority: poolAuthority.publicKey,
        pool: poolPda,
        borrower: msbBorrower.publicKey,
        creditLine: creditLinePda,
        systemProgram: SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([poolAuthority])
      .rpc();

    const creditAccount = await program.account.borrowerCreditLine.fetch(creditLinePda);
    expect(creditAccount.creditLimit.toString()).to.equal(limit.toString());
    expect(creditAccount.interestRateBps).to.equal(rateBps);
    expect(creditAccount.currentPrincipal.toNumber()).to.equal(0);
  });

  it("4. MSB draws $40,000 directly to foreign exchange payout recipient", async () => {
    const drawAmount = new anchor.BN(40_000 * 1e6);

    await program.methods
      .drawCredit(drawAmount)
      .accounts({
        borrower: msbBorrower.publicKey,
        pool: poolPda,
        creditLine: creditLinePda,
        vault: vaultPda,
        destinationAccount: recipientUsdcAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([msbBorrower])
      .rpc();

    const creditAccount = await program.account.borrowerCreditLine.fetch(creditLinePda);
    expect(creditAccount.currentPrincipal.toString()).to.equal(drawAmount.toString());

    const recipientBalance = await getAccount(provider.connection, recipientUsdcAccount);
    expect(recipientBalance.amount.toString()).to.equal(drawAmount.toString());
  });

  it("5. Rejects unauthorized draw request exceeding credit limits", async () => {
    // Attempting to draw $70,000 more when limit is $100,000 and $40,000 already drawn
    const excessiveAmount = new anchor.BN(70_000 * 1e6);

    try {
      await program.methods
        .drawCredit(excessiveAmount)
        .accounts({
          borrower: msbBorrower.publicKey,
          pool: poolPda,
          creditLine: creditLinePda,
          vault: vaultPda,
          destinationAccount: recipientUsdcAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([msbBorrower])
        .rpc();
      expect.fail("Transaction should have failed with ExceedsCreditLimit");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("ExceedsCreditLimit");
    }
  });

  it("6. Rejects drawdown attempted by unauthorized third party", async () => {
    const attacker = Keypair.generate();
    const attackerSig = await provider.connection.requestAirdrop(attacker.publicKey, anchor.web3.LAMPORTS_PER_SOL);
    await provider.connection.confirmTransaction(attackerSig);

    try {
      await program.methods
        .drawCredit(new anchor.BN(5_000 * 1e6))
        .accounts({
          borrower: attacker.publicKey,
          pool: poolPda,
          creditLine: creditLinePda,
          vault: vaultPda,
          destinationAccount: recipientUsdcAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([attacker])
        .rpc();
      expect.fail("Should have failed with unauthorized error");
    } catch (err: any) {
      expect(err).to.exist;
    }
  });

  it("7. Borrower repays partial principal + interest, restoring revolving credit line", async () => {
    const repayAmount = new anchor.BN(20_000 * 1e6);

    await program.methods
      .repayCredit(repayAmount)
      .accounts({
        payer: msbBorrower.publicKey,
        pool: poolPda,
        creditLine: creditLinePda,
        vault: vaultPda,
        payerTokenAccount: borrowerUsdcAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([msbBorrower])
      .rpc();

    const creditAccount = await program.account.borrowerCreditLine.fetch(creditLinePda);
    expect(creditAccount.currentPrincipal.toNumber()).to.be.lessThan(40_000 * 1e6);
  });
});