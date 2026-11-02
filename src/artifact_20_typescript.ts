import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { ArfCoreNexus } from "../target/types/arf_core_nexus";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { assert } from "chai";

describe("ArfCore Nexus Institutional Clearance Test Matrix", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.ArfCoreNexus as Program<ArfCoreNexus>;
  const admin = (provider.wallet as anchor.Wallet).payer;

  let usdcMint: anchor.web3.PublicKey;
  let collateralMint: anchor.web3.PublicKey;
  let facilityPda: anchor.web3.PublicKey;
  let facilityVaultPda: anchor.web3.PublicKey;
  let corridorPda: anchor.web3.PublicKey;
  let collateralVaultPda: anchor.web3.PublicKey;

  let corridorOwner: anchor.web3.Keypair;
  let corridorCollateralAccount: anchor.web3.PublicKey;
  let beneficiaryAccount: anchor.web3.PublicKey;
  let corridorRepayerUsdcAccount: anchor.web3.PublicKey;

  const corridorId = Array.from(Buffer.from("TR-DE-PAYMENT-01")); // 16 bytes
  const BASE_RATE_BPS = 800; // 8.00% APR
  const COLLATERAL_RATIO_BPS = 12000; // 120% Collateralization
  const CREDIT_LIMIT = new anchor.BN(100_000_000_000); // 100,000 USDC
  const INITIAL_COLLATERAL = new anchor.BN(150_000_000_000); // 150,000 Collateral units

  before(async () => {
    corridorOwner = anchor.web3.Keypair.generate();

    // Airdrop SOL to Corridor Owner for transactions
    const airdropSig = await provider.connection.requestAirdrop(
      corridorOwner.publicKey,
      10 * anchor.web3.LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(airdropSig);

    // Create Mock USDC Mint (6 Decimals)
    usdcMint = await createMint(
      provider.connection,
      admin,
      admin.publicKey,
      null,
      6
    );

    // Create Mock Collateral Mint (6 Decimals)
    collateralMint = await createMint(
      provider.connection,
      admin,
      admin.publicKey,
      null,
      6
    );

    // Derive Facility PDAs
    [facilityPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("facility"), admin.publicKey.toBuffer(), usdcMint.toBuffer()],
      program.programId
    );

    [facilityVaultPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("facility_vault"), facilityPda.toBuffer()],
      program.programId
    );

    // Derive Corridor PDAs
    [corridorPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("corridor"), facilityPda.toBuffer(), Buffer.from(corridorId)],
      program.programId
    );

    [collateralVaultPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("collateral_vault"), corridorPda.toBuffer()],
      program.programId
    );

    // Setup Token Accounts for Corridor Owner
    corridorCollateralAccount = await createAccount(
      provider.connection,
      corridorOwner,
      collateralMint,
      corridorOwner.publicKey
    );

    beneficiaryAccount = await createAccount(
      provider.connection,
      corridorOwner,
      usdcMint,
      corridorOwner.publicKey
    );

    corridorRepayerUsdcAccount = await createAccount(
      provider.connection,
      corridorOwner,
      usdcMint,
      corridorOwner.publicKey
    );

    // Mint Collateral to Corridor Owner
    await mintTo(
      provider.connection,
      admin,
      collateralMint,
      corridorCollateralAccount,
      admin,
      200_000_000_000
    );
  });

  it("1. Initializes the Arf Credit Facility and Seeds Liquidity Vault", async () => {
    await program.methods
      .initializeFacility(BASE_RATE_BPS)
      .accounts({
        authority: admin.publicKey,
        liquidityMint: usdcMint,
        facility: facilityPda,
        liquidityVault: facilityVaultPda,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    // Deposit 500,000 USDC into the Facility Vault for lending
    await mintTo(
      provider.connection,
      admin,
      usdcMint,
      facilityVaultPda,
      admin,
      500_000_000_000
    );

    const facilityAccount = await program.account.creditFacility.fetch(facilityPda);
    assert.equal(facilityAccount.baseRateBps, BASE_RATE_BPS);
    assert.isTrue(facilityAccount.isActive);

    const vaultInfo = await getAccount(provider.connection, facilityVaultPda);
    assert.equal(Number(vaultInfo.amount), 500_000_000_000);
  });

  it("2. Registers a Corridor with Over-Collateralization", async () => {
    await program.methods
      .registerCorridor(
        corridorId,
        CREDIT_LIMIT,
        COLLATERAL_RATIO_BPS,
        INITIAL_COLLATERAL
      )
      .accounts({
        owner: corridorOwner.publicKey,
        facility: facilityPda,
        authority: admin.publicKey,
        collateralMint: collateralMint,
        corridor: corridorPda,
        collateralVault: collateralVaultPda,
        ownerCollateralSource: corridorCollateralAccount,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([corridorOwner])
      .rpc();

    const corridorState = await program.account.corridor.fetch(corridorPda);
    assert.equal(corridorState.creditLimit.toString(), CREDIT_LIMIT.toString());
    assert.equal(corridorState.collateralAmount.toString(), INITIAL_COLLATERAL.toString());
    assert.isTrue(corridorState.isActive);
  });

  it("3. Executes Instant Drawdown and Direct-to-Beneficiary Routing", async () => {
    const nonce = new anchor.BN(101);
    const drawdownAmount = new anchor.BN(25_000_000_000); // 25,000 USDC
    const tenorSeconds = new anchor.BN(30 * 86400); // 30 Days

    const [escrowPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("escrow"), corridorPda.toBuffer(), nonce.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    const beneficiaryPreBalance = await getAccount(provider.connection, beneficiaryAccount);

    await program.methods
      .executeDrawdown(nonce, drawdownAmount, tenorSeconds)
      .accounts({
        owner: corridorOwner.publicKey,
        facility: facilityPda,
        liquidityVault: facilityVaultPda,
        corridor: corridorPda,
        escrow: escrowPda,
        beneficiaryAccount: beneficiaryAccount,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([corridorOwner])
      .rpc();

    const beneficiaryPostBalance = await getAccount(provider.connection, beneficiaryAccount);
    const diff = Number(beneficiaryPostBalance.amount) - Number(beneficiaryPreBalance.amount);
    assert.equal(diff, 25_000_000_000);

    const escrowState = await program.account.drawdownEscrow.fetch(escrowPda);
    assert.equal(escrowState.principalAmount.toString(), drawdownAmount.toString());
    assert.isFalse(escrowState.isSettled);
  });

  it("4. Prevents Drawdowns Exceeding Collateralization Limits", async () => {
    const nonce = new anchor.BN(102);
    // Request an amount that would breach the 120% collateral ratio
    const excessiveDrawdown = new anchor.BN(140_000_000_000); // Exceeds credit limit and collateral bounds

    const [escrowPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("escrow"), corridorPda.toBuffer(), nonce.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    try {
      await program.methods
        .executeDrawdown(nonce, excessiveDrawdown, new anchor.BN(86400))
        .accounts({
          owner: corridorOwner.publicKey,
          facility: facilityPda,
          liquidityVault: facilityVaultPda,
          corridor: corridorPda,
          escrow: escrowPda,
          beneficiaryAccount: beneficiaryAccount,
          systemProgram: anchor.web3.SystemProgram.programId,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([corridorOwner])
        .rpc();
      assert.fail("Should have thrown CreditLimitExceeded or Undercollateralized");
    } catch (err: any) {
      assert.include(err.toString(), "CreditLimitExceeded");
    }
  });

  it("5. Settles Drawdown Repayment With Interest Swept to Facility", async () => {
    const nonce = new anchor.BN(101);
    const [escrowPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("escrow"), corridorPda.toBuffer(), nonce.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    // Fund corridor owner with USDC to repay loan
    await mintTo(
      provider.connection,
      admin,
      usdcMint,
      corridorRepayerUsdcAccount,
      admin,
      30_000_000_000
    );

    await program.methods
      .settleRepayment()
      .accounts({
        repayer: corridorOwner.publicKey,
        facility: facilityPda,
        liquidityVault: facilityVaultPda,
        corridor: corridorPda,
        escrow: escrowPda,
        repayerTokenAccount: corridorRepayerUsdcAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([corridorOwner])
      .rpc();

    const escrowState = await program.account.drawdownEscrow.fetch(escrowPda);
    assert.isTrue(escrowState.isSettled);

    const corridorState = await program.account.corridor.fetch(corridorPda);
    assert.equal(corridorState.drawnPrincipal.toNumber(), 0);
  });
});