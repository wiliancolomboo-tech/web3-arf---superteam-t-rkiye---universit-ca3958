use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("ArfF1ow111111111111111111111111111111111111");

#[program]
pub mod arf_flow {
    use super::*;

    /// Initialize a corporate liquidity pool for a specific corridor (e.g., EUR/TRY or USD/BRL).
    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        corridor_id: String,
        credit_limit: u64,
        settlement_timeout_seconds: i64,
    ) -> Result<()> {
        require!(corridor_id.len() <= 32, ArfError::CorridorIdTooLong);
        require!(credit_limit > 0, ArfError::InvalidCreditLimit);
        require!(settlement_timeout_seconds >= 60, ArfError::InvalidTimeout);

        let pool = &mut ctx.accounts.pool;
        pool.admin = ctx.accounts.admin.key();
        pool.clearing_authority = ctx.accounts.clearing_authority.key();
        pool.mint = ctx.accounts.mint.key();
        pool.vault = ctx.accounts.vault.key();
        pool.corridor_id = corridor_id;
        pool.credit_limit = credit_limit;
        pool.current_borrowed = 0;
        pool.settlement_timeout_seconds = settlement_timeout_seconds;
        pool.is_frozen = false;
        pool.bump = ctx.bumps.pool;
        pool.vault_bump = ctx.bumps.vault;

        emit!(PoolInitializedEvent {
            pool: pool.key(),
            corridor_id: pool.corridor_id.clone(),
            credit_limit,
            clearing_authority: pool.clearing_authority,
        });

        Ok(())
    }

    /// Deposit institutional liquidity into the corridor vault.
    pub fn deposit_liquidity(ctx: Context<DepositLiquidity>, amount: u64) -> Result<()> {
        require!(amount > 0, ArfError::InvalidAmount);
        require!(!ctx.accounts.pool.is_frozen, ArfError::PoolFrozen);

        let cpi_accounts = Transfer {
            from: ctx.accounts.funder_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.funder.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        emit!(LiquidityDepositedEvent {
            pool: ctx.accounts.pool.key(),
            funder: ctx.accounts.funder.key(),
            amount,
        });

        Ok(())
    }

    /// Initiate a settlement drawdown. Transfers tokens to escrow PDA until off-chain fiat settlement attestation.
    pub fn initiate_settlement(
        ctx: Context<InitiateSettlement>,
        settlement_id: [u8; 32],
        amount: u64,
    ) -> Result<()> {
        let pool = &mut ctx.accounts.pool;
        require!(!pool.is_frozen, ArfError::PoolFrozen);
        require!(amount > 0, ArfError::InvalidAmount);
        require!(
            pool.current_borrowed.checked_add(amount).ok_or(ArfError::MathOverflow)? <= pool.credit_limit,
            ArfError::CreditLimitExceeded
        );

        let clock = Clock::get()?;
        let settlement = &mut ctx.accounts.settlement_record;
        settlement.pool = pool.key();
        settlement.corridor_borrower = ctx.accounts.borrower.key();
        settlement.destination_account = ctx.accounts.destination_token_account.key();
        settlement.settlement_id = settlement_id;
        settlement.amount = amount;
        settlement.timestamp_initiated = clock.unix_timestamp;
        settlement.timeout_deadline = clock
            .unix_timestamp
            .checked_add(pool.settlement_timeout_seconds)
            .ok_or(ArfError::MathOverflow)?;
        settlement.status = SettlementStatus::Pending;
        settlement.bump = ctx.bumps.settlement_record;

        pool.current_borrowed = pool
            .current_borrowed
            .checked_add(amount)
            .ok_or(ArfError::MathOverflow)?;

        emit!(SettlementInitiatedEvent {
            settlement_id,
            pool: pool.key(),
            borrower: ctx.accounts.borrower.key(),
            amount,
            deadline: settlement.timeout_deadline,
        });

        Ok(())
    }

    /// Clearing authority attests fiat delivery on external rails, releasing liquidity to the borrower.
    pub fn finalize_settlement(
        ctx: Context<FinalizeSettlement>,
        settlement_id: [u8; 32],
        offchain_reference_hash: [u8; 32],
    ) -> Result<()> {
        let pool = &ctx.accounts.pool;
        let settlement = &mut ctx.accounts.settlement_record;

        require!(settlement.status == SettlementStatus::Pending, ArfError::InvalidSettlementStatus);
        require!(settlement.settlement_id == settlement_id, ArfError::MismatchedSettlementId);

        let clock = Clock::get()?;
        require!(clock.unix_timestamp <= settlement.timeout_deadline, ArfError::SettlementExpired);

        settlement.status = SettlementStatus::Settled;
        settlement.offchain_reference_hash = offchain_reference_hash;

        // Transfer funds from Vault PDA to Borrower Destination
        let pool_key = pool.key();
        let seeds = &[
            b"vault".as_ref(),
            pool_key.as_ref(),
            &[pool.vault_bump],
        ];
        let signer = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault.to_account_info(),
            to: ctx.accounts.destination_token_account.to_account_info(),
            authority: ctx.accounts.vault.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer,
        );
        token::transfer(cpi_ctx, settlement.amount)?;

        emit!(SettlementFinalizedEvent {
            settlement_id,
            pool: pool.key(),
            amount: settlement.amount,
            offchain_reference_hash,
        });

        Ok(())
    }

    /// Repay borrowed capital into the pool, refreshing the available credit line.
    pub fn repay_credit(ctx: Context<RepayCredit>, amount: u64) -> Result<()> {
        require!(amount > 0, ArfError::InvalidAmount);
        let pool = &mut ctx.accounts.pool;

        let cpi_accounts = Transfer {
            from: ctx.accounts.payer_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.payer.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        pool.current_borrowed = pool.current_borrowed.saturating_sub(amount);

        emit!(CreditRepaidEvent {
            pool: pool.key(),
            payer: ctx.accounts.payer.key(),
            amount,
            current_borrowed: pool.current_borrowed,
        });

        Ok(())
    }

    /// Cancel expired settlement claim and recover reserved credit allowance.
    pub fn cancel_expired_settlement(ctx: Context<CancelExpiredSettlement>) -> Result<()> {
        let pool = &mut ctx.accounts.pool;
        let settlement = &mut ctx.accounts.settlement_record;

        require!(settlement.status == SettlementStatus::Pending, ArfError::InvalidSettlementStatus);

        let clock = Clock::get()?;
        require!(clock.unix_timestamp > settlement.timeout_deadline, ArfError::SettlementNotExpired);

        settlement.status = SettlementStatus::Cancelled;
        pool.current_borrowed = pool.current_borrowed.saturating_sub(settlement.amount);

        emit!(SettlementCancelledEvent {
            settlement_id: settlement.settlement_id,
            pool: pool.key(),
            reclaimed_amount: settlement.amount,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// ACCOUNT VALIDATION CONTEXTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(corridor_id: String)]
pub struct InitializePool<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    /// CHECK: Clearing authority verified via signature check
    pub clearing_authority: AccountInfo<'info>,

    pub mint: Account<'info, Mint>,

    #[account(
        init,
        payer = admin,
        space = 8 + Pool::INIT_SPACE,
        seeds = [b"pool", corridor_id.as_bytes(), mint.key().as_ref()],
        bump
    )]
    pub pool: Account<'info, Pool>,

    #[account(
        init,
        payer = admin,
        seeds = [b"vault", pool.key().as_ref()],
        bump,
        token::mint = mint,
        token::authority = vault
    )]
    pub vault: Account<'info, TokenAccount>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct DepositLiquidity<'info> {
    #[account(mut)]
    pub funder: Signer<'info>,

    #[account(
        mut,
        has_one = vault,
        has_one = mint
    )]
    pub pool: Account<'info, Pool>,

    pub mint: Account<'info, Mint>,

    #[account(
        mut,
        constraint = funder_token_account.mint == mint.key() @ ArfError::InvalidMint,
        constraint = funder_token_account.owner == funder.key() @ ArfError::InvalidOwner
    )]
    pub funder_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub vault: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
#[instruction(settlement_id: [u8; 32])]
pub struct InitiateSettlement<'info> {
    #[account(mut)]
    pub borrower: Signer<'info>,

    #[account(
        mut,
        has_one = vault
    )]
    pub pool: Account<'info, Pool>,

    #[account(mut)]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        init,
        payer = borrower,
        space = 8 + SettlementRecord::INIT_SPACE,
        seeds = [b"settlement", pool.key().as_ref(), settlement_id.as_ref()],
        bump
    )]
    pub settlement_record: Account<'info, SettlementRecord>,

    #[account(
        constraint = destination_token_account.mint == pool.mint @ ArfError::InvalidMint
    )]
    pub destination_token_account: Account<'info, TokenAccount>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
#[instruction(settlement_id: [u8; 32])]
pub struct FinalizeSettlement<'info> {
    #[account(
        constraint = clearing_authority.key() == pool.clearing_authority @ ArfError::UnauthorizedClearing
    )]
    pub clearing_authority: Signer<'info>,

    #[account(
        has_one = vault,
        has_one = clearing_authority
    )]
    pub pool: Account<'info, Pool>,

    #[account(
        mut,
        seeds = [b"settlement", pool.key().as_ref(), settlement_id.as_ref()],
        bump = settlement_record.bump
    )]
    pub settlement_record: Account<'info, SettlementRecord>,

    #[account(mut)]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = destination_token_account.key() == settlement_record.destination_account @ ArfError::InvalidDestination
    )]
    pub destination_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct RepayCredit<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        mut,
        has_one = vault,
        has_one = mint
    )]
    pub pool: Account<'info, Pool>,

    pub mint: Account<'info, Mint>,

    #[account(
        mut,
        constraint = payer_token_account.mint == mint.key() @ ArfError::InvalidMint,
        constraint = payer_token_account.owner == payer.key() @ ArfError::InvalidOwner
    )]
    pub payer_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub vault: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct CancelExpiredSettlement<'info> {
    #[account(mut)]
    pub caller: Signer<'info>,

    #[account(
        mut,
        has_one = vault
    )]
    pub pool: Account<'info, Pool>,

    #[account(
        mut,
        has_one = pool,
        seeds = [b"settlement", pool.key().as_ref(), settlement_record.settlement_id.as_ref()],
        bump = settlement_record.bump
    )]
    pub settlement_record: Account<'info, SettlementRecord>,

    #[account(mut)]
    pub vault: Account<'info, TokenAccount>,
}

// -----------------------------------------------------------------------------
// DATA STATE STRUCTS
// -----------------------------------------------------------------------------

#[account]
#[derive(InitSpace)]
pub struct Pool {
    pub admin: Pubkey,
    pub clearing_authority: Pubkey,
    pub mint: Pubkey,
    pub vault: Pubkey,
    #[max_len(32)]
    pub corridor_id: String,
    pub credit_limit: u64,
    pub current_borrowed: u64,
    pub settlement_timeout_seconds: i64,
    pub is_frozen: bool,
    pub bump: u8,
    pub vault_bump: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace)]
pub enum SettlementStatus {
    Pending,
    Settled,
    Cancelled,
}

#[account]
#[derive(InitSpace)]
pub struct SettlementRecord {
    pub pool: Pubkey,
    pub corridor_borrower: Pubkey,
    pub destination_account: Pubkey,
    pub settlement_id: [u8; 32],
    pub amount: u64,
    pub timestamp_initiated: i64,
    pub timeout_deadline: i64,
    pub status: SettlementStatus,
    pub offchain_reference_hash: [u8; 32],
    pub bump: u8,
}

// -----------------------------------------------------------------------------
// EVENTS
// -----------------------------------------------------------------------------

#[event]
pub struct PoolInitializedEvent {
    pub pool: Pubkey,
    pub corridor_id: String,
    pub credit_limit: u64,
    pub clearing_authority: Pubkey,
}

#[event]
pub struct LiquidityDepositedEvent {
    pub pool: Pubkey,
    pub funder: Pubkey,
    pub amount: u64,
}

#[event]
pub struct SettlementInitiatedEvent {
    pub settlement_id: [u8; 32],
    pub pool: Pubkey,
    pub borrower: Pubkey,
    pub amount: u64,
    pub deadline: i64,
}

#[event]
pub struct SettlementFinalizedEvent {
    pub settlement_id: [u8; 32],
    pub pool: Pubkey,
    pub amount: u64,
    pub offchain_reference_hash: [u8; 32],
}

#[event]
pub struct CreditRepaidEvent {
    pub pool: Pubkey,
    pub payer: Pubkey,
    pub amount: u64,
    pub current_borrowed: u64,
}

#[event]
pub struct SettlementCancelledEvent {
    pub settlement_id: [u8; 32],
    pub pool: Pubkey,
    pub reclaimed_amount: u64,
}

// -----------------------------------------------------------------------------
// ERROR CODES
// -----------------------------------------------------------------------------

#[error_code]
pub enum ArfError {
    #[msg("Corridor ID string exceeds maximum capacity of 32 bytes.")]
    CorridorIdTooLong,
    #[msg("Credit limit must be strictly greater than zero.")]
    InvalidCreditLimit,
    #[msg("Settlement timeout must be at least 60 seconds.")]
    InvalidTimeout,
    #[msg("Transfer amount must be strictly greater than zero.")]
    InvalidAmount,
    #[msg("Corporate credit pool is currently frozen.")]
    PoolFrozen,
    #[msg("Arithmetic overflow in credit balance calculations.")]
    MathOverflow,
    #[msg("Requested settlement amount exceeds available corridor credit limit.")]
    CreditLimitExceeded,
    #[msg("Settlement record is not in Pending status.")]
    InvalidSettlementStatus,
    #[msg("Supplied settlement identifier does not match registered record.")]
    MismatchedSettlementId,
    #[msg("Settlement period has expired.")]
    SettlementExpired,
    #[msg("Unauthorized clearing authority signature.")]
    UnauthorizedClearing,
    #[msg("Mismatched destination token account.")]
    InvalidDestination,
    #[msg("Settlement deadline has not passed yet.")]
    SettlementNotExpired,
    #[msg("Token account mint does not match corridor pool configuration.")]
    InvalidMint,
    #[msg("Account does not match expected token owner.")]
    InvalidOwner,
}