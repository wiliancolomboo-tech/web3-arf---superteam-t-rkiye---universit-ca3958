import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { ArfSettlementEngine } from "../target/types/arf_settlement_engine";
import {
  createMint,
  getOrCreateAssociatedTokenAccount,
  mintTo,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { assert } from "chai";

describe("arf_settlement_engine: Enterprise Verification Suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.ArfSettlementEngine as Program<ArfSettlementEngine>;
  const admin = provider.wallet as anchor.Wallet;

  const borrowerKeypair = anchor.web3.Keypair.generate();
  let stablecoinMint: anchor.web3.PublicKey;
  let poolPda: anchor.web3.PublicKey;
  let poolVaultPda: anchor.web3.PublicKey;
  let borrowerFacilityPda: anchor.web3.PublicKey;
  let borrowerAta: anchor.web3.PublicKey;
  let adminAta: anchor.web3.PublicKey;

  const POOL_SEED = Buffer.from("arf-liquidity-pool");
  const VAULT_SEED = Buffer.from("arf-token-vault");
  const BORROWER_SEED = Buffer.from("arf-borrower-facility");

  before(async () => {
    // 1. Airdrop SOL to borrower for gas
    const sig = await provider.connection.requestAirdrop(
      borrowerKeypair.publicKey,
      2 * anchor.web3.LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(sig);

    // 2. Initialize settlement token mint (e.g. USDC)
    stablecoinMint = await createMint(
      provider.connection,
      admin.payer,
      admin.publicKey,
      null,
      6
    );

    // 3. Derive PDA addresses
    [poolPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [POOL_SEED, stablecoinMint.toBuffer()],
      program.programId
    );

    [poolVaultPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [VAULT_SEED, poolPda.toBuffer()],
      program.programId
    );

    [borrowerFacilityPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [BORROWER_SEED, poolPda.toBuffer(), borrowerKeypair.publicKey.toBuffer()],
      program.programId
    );

    // 4. Token accounts
    const adminAtaInfo = await getOrCreateAssociatedTokenAccount(
      provider.connection,
      admin.payer,
      stablecoinMint,
      admin.publicKey
    );
    adminAta = adminAtaInfo.address;

    const borrowerAtaInfo = await getOrCreateAssociatedTokenAccount(
      provider.connection,
      admin.payer,
      stablecoinMint,
      borrowerKeypair.publicKey
    );
    borrowerAta = borrowerAtaInfo.address;

    // Mint 1,000,000 USDC to admin for liquidity seeding
    await mintTo(
      provider.connection,
      admin.payer,
      stablecoinMint,
      adminAta,
      admin.publicKey,
      1_000_000_000_000 // 1M tokens with 6 decimals
    );
  });

  it("1. Successfully initializes liquidity pool with risk configuration", async () => {
    const baseRateBps = 400; // 4.00%
    const slopeRateBps = 800; // 8.00%
    const reserveFactorBps = 1000; // 10.00%

    await program.methods
      .initializePool(baseRateBps, slopeRateBps, reserveFactorBps)
      .accounts({
        admin: admin.publicKey,
        tokenMint: stablecoinMint,
        pool: poolPda,
        poolVault: poolVaultPda,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    const poolAccount = await program.account.liquidityPool.fetch(poolPda);
    assert.equal(poolAccount.admin.toBase58(), admin.publicKey.toBase58());
    assert.equal(poolAccount.baseRateBps, 400);
    assert.equal(poolAccount.isActive, true);
    assert.equal(poolAccount.totalDeposited.toNumber(), 0);
  });

  it("2. Liquid provider deposits 500,000 USDC into pool vault", async () => {
    const depositAmount = new anchor.BN(500_000_000_000);

    await program.methods
      .depositLiquidity(depositAmount)
      .accounts({
        provider: admin.publicKey,
        pool: poolPda,
        tokenMint: stablecoinMint,
        poolVault: poolVaultPda,
        providerTokenAccount: adminAta,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .rpc();

    const poolAccount = await program.account.liquidityPool.fetch(poolPda);
    assert.equal(poolAccount.totalDeposited.toString(), depositAmount.toString());
  });

  it("3. Admin approves and registers licensed FinTech credit facility", async () => {
    const creditLimit = new anchor.BN(100_000_000_000); // 100,000 USDC credit limit
    const collateralRatioBps = 11000; // 110%
    const durationSeconds = new anchor.BN(86400 * 30); // 30-day tenor

    await program.methods
      .registerBorrower(creditLimit, collateralRatioBps, durationSeconds)
      .accounts({
        admin: admin.publicKey,
        pool: poolPda,
        borrowerIdentity: borrowerKeypair.publicKey,
        borrowerFacility: borrowerFacilityPda,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .rpc();

    const facility = await program.account.borrowerFacility.fetch(borrowerFacilityPda);
    assert.equal(facility.borrowerPubkey.toBase58(), borrowerKeypair.publicKey.toBase58());
    assert.equal(facility.authorizedCreditLimit.toString(), creditLimit.toString());
    assert.equal(facility.principalBorrowed.toNumber(), 0);
  });

  it("4. FinTech draws down 50,000 USDC for instant corridor payout", async () => {
    const drawdownAmount = new anchor.BN(50_000_000_000);

    await program.methods
      .drawdownCredit(drawdownAmount)
      .accounts({
        borrower: borrowerKeypair.publicKey,
        pool: poolPda,
        tokenMint: stablecoinMint,
        poolVault: poolVaultPda,
        borrowerFacility: borrowerFacilityPda,
        borrowerTokenAccount: borrowerAta,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([borrowerKeypair])
      .rpc();

    const facility = await program.account.borrowerFacility.fetch(borrowerFacilityPda);
    const pool = await program.account.liquidityPool.fetch(poolPda);

    assert.equal(facility.principalBorrowed.toString(), drawdownAmount.toString());
    assert.equal(pool.totalBorrowed.toString(), drawdownAmount.toString());
  });

  it("5. Rejects drawdown exceeding authorized credit line limit", async () => {
    const excessDrawdown = new anchor.BN(60_000_000_000); // Total would be 110k > 100k limit

    try {
      await program.methods
        .drawdownCredit(excessDrawdown)
        .accounts({
          borrower: borrowerKeypair.publicKey,
          pool: poolPda,
          tokenMint: stablecoinMint,
          poolVault: poolVaultPda,
          borrowerFacility: borrowerFacilityPda,
          borrowerTokenAccount: borrowerAta,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([borrowerKeypair])
        .rpc();
      assert.fail("Should have thrown CreditLimitExceeded error");
    } catch (_err: any) {
      assert.include(_err.toString(), "CreditLimitExceeded");
    }
  });

  it("6. FinTech repays outstanding loan in full", async () => {
    const facilityBefore = await program.account.borrowerFacility.fetch(borrowerFacilityPda);
    const repayAmount = facilityBefore.principalBorrowed;

    await program.methods
      .repayCredit(repayAmount)
      .accounts({
        borrower: borrowerKeypair.publicKey,
        pool: poolPda,
        tokenMint: stablecoinMint,
        poolVault: poolVaultPda,
        borrowerFacility: borrowerFacilityPda,
        borrowerTokenAccount: borrowerAta,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([borrowerKeypair])
      .rpc();

    const facilityAfter = await program.account.borrowerFacility.fetch(borrowerFacilityPda);
    assert.equal(facilityAfter.principalBorrowed.toNumber(), 0);
  });
});