import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { ArfSettlement } from "../target/types/arf_settlement";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { Keypair, PublicKey } from "@solana/web3.js";
import { assert, expect } from "chai";

describe("arf_settlement enterprise test matrix", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.ArfSettlement as Program<ArfSettlement>;

  let usdcMint: PublicKey;
  let admin = (provider.wallet as anchor.Wallet).payer;
  let lpProvider = Keypair.generate();
  let msbBorrower = Keypair.generate();
  let msbReceiver = Keypair.generate();

  let poolPda: PublicKey;
  let poolBump: number;
  let vaultPda: PublicKey;
  let vaultBump: number;
  let creditLinePda: PublicKey;

  let lpTokenAccount: PublicKey;
  let msbBorrowerTokenAccount: PublicKey;
  let msbReceiverTokenAccount: PublicKey;

  const BASE_APR_BPS = 850; // 8.50% APR
  const MAX_CAPACITY = new anchor.BN(10_000_000 * 1_000_000); // 10,000,000 USDC
  const CREDIT_LIMIT = new anchor.BN(500_000 * 1_000_000); // 500,000 USDC

  before(async () => {
    // 1. Airdrop SOL to test actors
    const airdrops = [lpProvider, msbBorrower, msbReceiver].map(async (acc) => {
      const sig = await provider.connection.requestAirdrop(acc.publicKey, 2 * anchor.web3.LAMPORTS_PER_SOL);
      await provider.connection.confirmTransaction(sig);
    });
    await Promise.all(airdrops);

    // 2. Create USDC Mock Token Mint (6 decimals)
    usdcMint = await createMint(
      provider.connection,
      admin,
      admin.publicKey,
      null,
      6
    );

    // 3. Derive Program PDAs
    [poolPda, poolBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("liquidity_pool"), usdcMint.toBuffer()],
      program.programId
    );

    [vaultPda, vaultBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("pool_vault"), poolPda.toBuffer()],
      program.programId
    );

    [creditLinePda] = PublicKey.findProgramAddressSync(
      [Buffer.from("credit_line"), poolPda.toBuffer(), msbBorrower.publicKey.toBuffer()],
      program.programId
    );

    // 4. Create Token Accounts
    lpTokenAccount = await createAccount(provider.connection, lpProvider, usdcMint, lpProvider.publicKey);
    msbBorrowerTokenAccount = await createAccount(provider.connection, msbBorrower, usdcMint, msbBorrower.publicKey);
    msbReceiverTokenAccount = await createAccount(provider.connection, msbReceiver, usdcMint, msbReceiver.publicKey);

    // 5. Mint Initial Liquidity to LP (1,000,000 USDC)
    await mintTo(
      provider.connection,
      admin,
      usdcMint,
      lpTokenAccount,
      admin,
      1_000_000 * 1_000_000
    );
  });

  it("1. Deterministically initializes the Institutional Liquidity Pool", async () => {
    await program.methods
      .initializePool(BASE_APR_BPS, MAX_CAPACITY)
      .accounts({
        admin: admin.publicKey,
        mint: usdcMint,
        pool: poolPda,
        vault: vaultPda,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    const poolState = await program.account.liquidityPool.fetch(poolPda);
    assert.strictEqual(poolState.admin.toBase58(), admin.publicKey.toBase58());
    assert.strictEqual(poolState.baseAprBps, BASE_APR_BPS);
    assert.isFalse(poolState.isPaused);
  });

  it("2. Institutional LP deposits 500,000 USDC into the Vault", async () => {
    const depositAmount = new anchor.BN(500_000 * 1_000_000);

    await program.methods
      .depositLiquidity(depositAmount)
      .accounts({
        provider: lpProvider.publicKey,
        pool: poolPda,
        mint: usdcMint,
        vault: vaultPda,
        providerToken: lpTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([lpProvider])
      .rpc();

    const vaultInfo = await getAccount(provider.connection, vaultPda);
    assert.strictEqual(vaultInfo.amount.toString(), depositAmount.toString());

    const poolState = await program.account.liquidityPool.fetch(poolPda);
    assert.strictEqual(poolState.totalDeposited.toString(), depositAmount.toString());
  });

  it("3. Risk Manager approves a $500,000 USDC Credit Line for an MSB", async () => {
    await program.methods
      .openCreditLine(msbBorrower.publicKey, CREDIT_LIMIT)
      .accounts({
        admin: admin.publicKey,
        pool: poolPda,
        creditLine: creditLinePda,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .rpc();

    const creditState = await program.account.creditLine.fetch(creditLinePda);
    assert.isTrue(creditState.isActive);
    assert.strictEqual(creditState.creditLimit.toString(), CREDIT_LIMIT.toString());
    assert.strictEqual(creditState.drawnPrincipal.toString(), "0");
  });

  it("4. MSB draws $100,000 USDC on-demand to meet corridor liquidity", async () => {
    const drawAmount = new anchor.BN(100_000 * 1_000_000);

    await program.methods
      .drawCredit(drawAmount)
      .accounts({
        borrower: msbBorrower.publicKey,
        pool: poolPda,
        mint: usdcMint,
        vault: vaultPda,
        creditLine: creditLinePda,
        borrowerToken: msbBorrowerTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([msbBorrower])
      .rpc();

    const borrowerBalance = await getAccount(provider.connection, msbBorrowerTokenAccount);
    assert.strictEqual(borrowerBalance.amount.toString(), drawAmount.toString());

    const creditState = await program.account.creditLine.fetch(creditLinePda);
    assert.strictEqual(creditState.drawnPrincipal.toString(), drawAmount.toString());
  });

  it("5. Rejects credit draw exceeding authorized credit ceiling", async () => {
    const excessiveDraw = new anchor.BN(450_000 * 1_000_000); // 100k + 450k = 550k > 500k limit

    try {
      await program.methods
        .drawCredit(excessiveDraw)
        .accounts({
          borrower: msbBorrower.publicKey,
          pool: poolPda,
          mint: usdcMint,
          vault: vaultPda,
          creditLine: creditLinePda,
          borrowerToken: msbBorrowerTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([msbBorrower])
        .rpc();
      assert.fail("Should have failed with CreditLimitExceeded");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("CreditLimitExceeded");
    }
  });

  it("6. Executes atomic B2B cross-border corridor settlement with fee routing", async () => {
    const corridorId = Array.from(Buffer.from("TRY-EUR-00000001")); // 16 bytes
    const settlementAmount = new anchor.BN(20_000 * 1_000_000); // 20,000 USDC
    const feeBps = 10; // 10 bps = 0.1% = 20 USDC fee

    await program.methods
      .settleCorridor(corridorId, settlementAmount, feeBps)
      .accounts({
        senderMsb: msbBorrower.publicKey,
        pool: poolPda,
        mint: usdcMint,
        senderToken: msbBorrowerTokenAccount,
        receiverToken: msbReceiverTokenAccount,
        feeVault: vaultPda,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([msbBorrower])
      .rpc();

    const receiverBalance = await getAccount(provider.connection, msbReceiverTokenAccount);
    assert.strictEqual(receiverBalance.amount.toString(), (19_980 * 1_000_000).toString());
  });

  it("7. Borrower repays principal balance cleanly", async () => {
    const repayAmount = new anchor.BN(50_000 * 1_000_000);

    await program.methods
      .repayCredit(repayAmount)
      .accounts({
        borrower: msbBorrower.publicKey,
        pool: poolPda,
        mint: usdcMint,
        vault: vaultPda,
        creditLine: creditLinePda,
        borrowerToken: msbBorrowerTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([msbBorrower])
      .rpc();

    const creditState = await program.account.creditLine.fetch(creditLinePda);
    assert.strictEqual(creditState.drawnPrincipal.toString(), (50_000 * 1_000_000).toString());
  });
});