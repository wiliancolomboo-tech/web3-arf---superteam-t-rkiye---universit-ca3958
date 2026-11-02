import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import { Keypair, PublicKey, LAMPORTS_PER_SOL } from "@solana/web3.js";
import { assert } from "chai";
import crypto from "crypto";

describe("arf-clearstream smart contract tests", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.ArfClearstream as Program<any>;

  const poolAuthority = Keypair.generate();
  const borrower = Keypair.generate();
  const liquidityProvider = Keypair.generate();

  let mint: PublicKey;
  let poolVaultPda: PublicKey;
  let poolPda: PublicKey;
  let facilityPda: PublicKey;

  let providerTokenAccount: PublicKey;
  let borrowerTokenAccount: PublicKey;

  const POOL_NAME = "Turk-Euro-Liquidity";
  const CREDIT_LIMIT = new anchor.BN(500_000 * 1_000_000); // 500,000 USDC (6 decimals)
  const INTEREST_BPS = 1000; // 10.00% APR
  const TENOR_SECONDS = new anchor.BN(86400 * 30); // 30-day tenor

  before(async () => {
    // Fund test wallets
    for (const kp of [poolAuthority, borrower, liquidityProvider]) {
      const tx = await provider.connection.requestAirdrop(kp.publicKey, 2 * LAMPORTS_PER_SOL);
      await provider.connection.confirmTransaction(tx, "confirmed");
    }

    // Deploy mock USDC mint (6 decimals)
    mint = await createMint(
      provider.connection,
      poolAuthority,
      poolAuthority.publicKey,
      null,
      6
    );

    // Derive Pool PDA
    [poolPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("liquidity_pool"), Buffer.from(POOL_NAME)],
      program.programId
    );

    // Derive Pool Vault PDA
    [poolVaultPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("pool_vault"), poolPda.toBuffer()],
      program.programId
    );

    // Derive Facility PDA
    [facilityPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("credit_facility"), poolPda.toBuffer(), borrower.publicKey.toBuffer()],
      program.programId
    );

    // Associated accounts
    providerTokenAccount = await createAccount(
      provider.connection,
      liquidityProvider,
      mint,
      liquidityProvider.publicKey
    );

    borrowerTokenAccount = await createAccount(
      provider.connection,
      borrower,
      mint,
      borrower.publicKey
    );

    // Mint 1,000,000 USDC to Liquidity Provider
    await mintTo(
      provider.connection,
      poolAuthority,
      mint,
      providerTokenAccount,
      poolAuthority,
      1_000_000 * 1_000_000
    );
  });

  it("1. Successfully initializes master liquidity pool", async () => {
    await program.methods
      .initializePool(POOL_NAME)
      .accounts({
        pool: poolPda,
        mint: mint,
        vault: poolVaultPda,
        authority: poolAuthority.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([poolAuthority])
      .rpc();

    const poolAccount = await program.account.liquidityPool.fetch(poolPda);
    assert.strictEqual(poolAccount.name, POOL_NAME);
    assert.strictEqual(poolAccount.authority.toBase58(), poolAuthority.publicKey.toBase58());
    assert.strictEqual(poolAccount.totalDeposited.toNumber(), 0);
  });

  it("2. LP successfully deposits 500,000 USDC into pool vault", async () => {
    const depositAmount = new anchor.BN(500_000 * 1_000_000);

    await program.methods
      .depositLiquidity(depositAmount)
      .accounts({
        pool: poolPda,
        vault: poolVaultPda,
        userTokenAccount: providerTokenAccount,
        provider: liquidityProvider.publicKey,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
      })
      .signers([liquidityProvider])
      .rpc();

    const vaultAccount = await getAccount(provider.connection, poolVaultPda);
    assert.strictEqual(vaultAccount.amount.toString(), depositAmount.toString());

    const pool = await program.account.liquidityPool.fetch(poolPda);
    assert.strictEqual(pool.totalDeposited.toString(), depositAmount.toString());
  });

  it("3. Authority establishes revolving credit facility for MTO", async () => {
    await program.methods
      .initializeFacility(CREDIT_LIMIT, INTEREST_BPS, TENOR_SECONDS)
      .accounts({
        facility: facilityPda,
        pool: poolPda,
        borrower: borrower.publicKey,
        authority: poolAuthority.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([poolAuthority])
      .rpc();

    const facility = await program.account.creditFacility.fetch(facilityPda);
    assert.strictEqual(facility.borrower.toBase58(), borrower.publicKey.toBase58());
    assert.strictEqual(facility.creditLimit.toString(), CREDIT_LIMIT.toString());
    assert.strictEqual(facility.interestRateBps, INTEREST_BPS);
    assert.isTrue(facility.isActive);
  });

  it("4. Borrower executes programmatic drawdown with settlement invoice hash", async () => {
    const drawdownAmount = new anchor.BN(150_000 * 1_000_000); // 150k USDC
    const rawInvoice = "INVOICE-TURKEY-TRY-CORRIDOR-2025-001";
    const invoiceHash = crypto.createHash("sha256").update(rawInvoice).digest();

    const [attestationPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("attestation"), facilityPda.toBuffer(), invoiceHash],
      program.programId
    );

    await program.methods
      .drawdownCredit(drawdownAmount, Array.from(invoiceHash))
      .accounts({
        facility: facilityPda,
        pool: poolPda,
        vault: poolVaultPda,
        borrowerTokenAccount: borrowerTokenAccount,
        attestation: attestationPda,
        borrower: borrower.publicKey,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([borrower])
      .rpc();

    const borrowerBalance = await getAccount(provider.connection, borrowerTokenAccount);
    assert.strictEqual(borrowerBalance.amount.toString(), drawdownAmount.toString());

    const facility = await program.account.creditFacility.fetch(facilityPda);
    assert.strictEqual(facility.outstandingPrincipal.toString(), drawdownAmount.toString());

    const attestation = await program.account.settlementAttestation.fetch(attestationPda);
    assert.deepEqual(Buffer.from(attestation.settlementHash), invoiceHash);
  });

  it("5. Rejects drawdown exceeding remaining credit limit", async () => {
    const excessAmount = new anchor.BN(400_000 * 1_000_000); // Exceeds (500k - 150k = 350k remaining)
    const dummyHash = crypto.randomBytes(32);

    const [attestationPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("attestation"), facilityPda.toBuffer(), dummyHash],
      program.programId
    );

    try {
      await program.methods
        .drawdownCredit(excessAmount, Array.from(dummyHash))
        .accounts({
          facility: facilityPda,
          pool: poolPda,
          vault: poolVaultPda,
          borrowerTokenAccount: borrowerTokenAccount,
          attestation: attestationPda,
          borrower: borrower.publicKey,
          tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
          systemProgram: anchor.web3.SystemProgram.programId,
        })
        .signers([borrower])
        .rpc();
      assert.fail("Should have thrown CreditLimitExceeded");
    } catch (err: any) {
      assert.include(err.toString(), "CreditLimitExceeded");
    }
  });

  it("6. Borrower repays interest and partial principal", async () => {
    const repayAmount = new anchor.BN(50_000 * 1_000_000); // 50,000 USDC

    await program.methods
      .repayCredit(repayAmount)
      .accounts({
        facility: facilityPda,
        pool: poolPda,
        vault: poolVaultPda,
        payerTokenAccount: borrowerTokenAccount,
        borrower: borrower.publicKey,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
      })
      .signers([borrower])
      .rpc();

    const facility = await program.account.creditFacility.fetch(facilityPda);
    assert.strictEqual(facility.outstandingPrincipal.toNumber(), 100_000 * 1_000_000);
  });
});