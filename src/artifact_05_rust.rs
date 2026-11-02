use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("ArfStr1111111111111111111111111111111111111");

pub const BPS_DIVISOR: u128 = 10_000;
pub const SECONDS_PER_YEAR: u128 = 31_536_000;
pub const ESTIMATED_SECONDS_PER_SLOT: u128 = 1; // Optimized for high-throughput slot time estimation

#[program]
pub mod arf_stream {
    use super::*;

    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        pool_name: String,
        base_annual_rate_bps: u16,
    ) -> Result<()> {
        require!(pool_name.len() <= 32, ArfError::PoolNameTooLong);
        require!(base_annual_rate_bps > 0 && base_annual_rate_bps <= 5000, ArfError::InvalidRateBps);

        let pool = &mut ctx.accounts.pool;
        pool.admin = ctx.accounts.admin.key();
        pool.mint = ctx.accounts.mint.key();
        pool.vault = ctx.accounts.vault.key();
        pool.pool_name = pool_name;
        pool.base_annual_rate_bps = base_annual_rate_bps;
        pool.total_liquidity = 0;
        pool.total_borrowed = 0;
        pool.is_frozen = false;
        pool.bump = ctx.bumps.pool;
        pool.vault_bump = ctx.bumps.vault;

        emit!(PoolInitializedEvent {
            pool: pool.key(),
            admin: pool.admin,
            mint: pool.mint,
            annual_rate_bps: base_annual_rate_bps,
        });

        Ok(())
    }

    pub fn open_credit_line(
        ctx: Context<OpenCreditLine>,
        credit_limit: u64,
    ) -> Result<()> {
        require!(credit_limit > 0, ArfError::InvalidCreditLimit);

        let credit_line = &mut ctx.accounts.credit_line;
        credit_line.pool = ctx.accounts.pool.key();
        credit_line.borrower = ctx.accounts.borrower.key();
        credit_line.credit_limit = credit_limit;
        credit_line.principal_debt = 0;
        credit_line.unpaid_interest = 0;
        credit_line.last_accrual_slot = Clock::get()?.slot;
        credit_line.is_active = true;
        credit_line.bump = ctx.bumps.credit_line;

        emit!(CreditLineOpenedEvent {
            credit_line: credit_line.key(),
            pool: ctx.accounts.pool.key(),
            borrower: ctx.accounts.borrower.key(),
            credit_limit,
        });

        Ok(())
    }

    pub fn deposit_liquidity(ctx: Context<DepositLiquidity>, amount: u64) -> Result<()> {
        require!(amount > 0, ArfError::ZeroTransferAmount);
        let pool = &mut ctx.accounts.pool;
        require!(!pool.is_frozen, ArfError::PoolIsFrozen);

        // SPL CPI Transfer from Provider to Vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.provider_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.provider.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        pool.total_liquidity = pool.total_liquidity.checked_add(amount)
            .ok_or(ArfError::MathOverflow)?;

        emit!(LiquidityDepositedEvent {
            pool: pool.key(),
            provider: ctx.accounts.provider.key(),
            amount,
            total_liquidity: pool.total_liquidity,
        });

        Ok(())
    }

    pub fn drawdown(ctx: Context<Drawdown>, amount: u64) -> Result<()> {
        require!(amount > 0, ArfError::ZeroTransferAmount);
        let pool = &mut ctx.accounts.pool;
        let credit_line = &mut ctx.accounts.credit_line;
        let current_slot = Clock::get()?.slot;

        require!(!pool.is_frozen, ArfError::PoolIsFrozen);
        require!(credit_line.is_active, ArfError::CreditLineInactive);

        // Accrue pending interest up to this exact slot
        let accrued_interest = compute_accrued_interest(
            credit_line.principal_debt,
            pool.base_annual_rate_bps,
            credit_line.last_accrual_slot,
            current_slot,
        )?;

        credit_line.unpaid_interest = credit_line.unpaid_interest
            .checked_add(accrued_interest)
            .ok_or(ArfError::MathOverflow)?;
        credit_line.last_accrual_slot = current_slot;

        // Verify credit boundary limits
        let total_obligation = credit_line.principal_debt
            .checked_add(credit_line.unpaid_interest)
            .ok_or(ArfError::MathOverflow)?
            .checked_add(amount)
            .ok_or(ArfError::MathOverflow)?;

        require!(total_obligation <= credit_line.credit_limit, ArfError::CreditLimitExceeded);
        require!(ctx.accounts.vault.amount >= amount, ArfError::InsufficientPoolLiquidity);

        credit_line.principal_debt = credit_line.principal_debt
            .checked_add(amount)
            .ok_or(ArfError::MathOverflow)?;

        pool.total_borrowed = pool.total_borrowed
            .checked_add(amount)
            .ok_or(ArfError::MathOverflow)?;
        pool.total_liquidity = pool.total_liquidity
            .checked_sub(amount)
            .ok_or(ArfError::MathOverflow)?;

        // Program PDA signer seeds for Vault execution
        let pool_name_bytes = pool.pool_name.as_bytes();
        let pool_key = pool.key();
        let seeds = &[
            b"arf_vault",
            pool_key.as_ref(),
            &[pool.vault_bump],
        ];
        let signer = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault.to_account_info(),
            to: ctx.accounts.borrower_token_account.to_account_info(),
            authority: ctx.accounts.vault.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer);
        token::transfer(cpi_ctx, amount)?;

        emit!(DrawdownExecutedEvent {
            pool: pool.key(),
            credit_line: credit_line.key(),
            borrower: credit_line.borrower,
            amount_drawn: amount,
            new_principal_debt: credit_line.principal_debt,
            current_slot,
        });

        Ok(())
    }

    pub fn repay(ctx: Context<Repay>, amount: u64) -> Result<()> {
        require!(amount > 0, ArfError::ZeroTransferAmount);
        let pool = &mut ctx.accounts.pool;
        let credit_line = &mut ctx.accounts.credit_line;
        let current_slot = Clock::get()?.slot;

        // Accrue interest up to repayment slot
        let accrued_interest = compute_accrued_interest(
            credit_line.principal_debt,
            pool.base_annual_rate_bps,
            credit_line.last_accrual_slot,
            current_slot,
        )?;

        credit_line.unpaid_interest = credit_line.unpaid_interest
            .checked_add(accrued_interest)
            .ok_or(ArfError::MathOverflow)?;
        credit_line.last_accrual_slot = current_slot;

        let mut remaining_repayment = amount;

        // 1. Pay accrued interest first
        let interest_settlement = remaining_repayment.min(credit_line.unpaid_interest);
        credit_line.unpaid_interest = credit_line.unpaid_interest
            .checked_sub(interest_settlement)
            .ok_or(ArfError::MathOverflow)?;
        remaining_repayment = remaining_repayment
            .checked_sub(interest_settlement)
            .ok_or(ArfError::MathOverflow)?;

        // 2. Pay principal debt balance
        let principal_settlement = remaining_repayment.min(credit_line.principal_debt);
        credit_line.principal_debt = credit_line.principal_debt
            .checked_sub(principal_settlement)
            .ok_or(ArfError::MathOverflow)?;

        // CPI transfer from borrower to pool vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.borrower_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.borrower.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        pool.total_liquidity = pool.total_liquidity
            .checked_add(amount)
            .ok_or(ArfError::MathOverflow)?;
        pool.total_borrowed = pool.total_borrowed
            .saturating_sub(principal_settlement);

        emit!(RepayExecutedEvent {
            pool: pool.key(),
            credit_line: credit_line.key(),
            borrower: credit_line.borrower,
            amount_repaid: amount,
            interest_paid: interest_settlement,
            principal_paid: principal_settlement,
            remaining_principal: credit_line.principal_debt,
            remaining_interest: credit_line.unpaid_interest,
            current_slot,
        });

        Ok(())
    }

    pub fn set_freeze_state(ctx: Context<SetFreezeState>, frozen: bool) -> Result<()> {
        let pool = &mut ctx.accounts.pool;
        pool.is_frozen = frozen;
        emit!(PoolFreezeStateUpdatedEvent {
            pool: pool.key(),
            is_frozen: frozen,
        });
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Pure Deterministic Math Helpers
// -----------------------------------------------------------------------------
pub fn compute_accrued_interest(
    principal: u64,
    rate_bps: u16,
    last_slot: u64,
    current_slot: u64,
) -> Result<u64> {
    if principal == 0 || current_slot <= last_slot {
        return Ok(0);
    }

    let elapsed_slots = (current_slot - last_slot) as u128;
    let elapsed_seconds = elapsed_slots.checked_mul(ESTIMATED_SECONDS_PER_SLOT)
        .ok_or(ArfError::MathOverflow)?;

    let principal_u128 = principal as u128;
    let rate_u128 = rate_bps as u128;

    // Formula: (Principal * RateBps * ElapsedSeconds) / (BPS_DIVISOR * SECONDS_PER_YEAR)
    let numerator = principal_u128
        .checked_mul(rate_u128).ok_or(ArfError::MathOverflow)?
        .checked_mul(elapsed_seconds).ok_or(ArfError::MathOverflow)?;

    let denominator = BPS_DIVISOR
        .checked_mul(SECONDS_PER_YEAR).ok_or(ArfError::MathOverflow)?;

    let accrued = numerator.checked_div(denominator).ok_or(ArfError::MathOverflow)?;

    u64::try_from(accrued).map_err(|_| ArfError::MathOverflow.into())
}

// -----------------------------------------------------------------------------
// Account Context Structs
// -----------------------------------------------------------------------------
#[derive(Accounts)]
#[instruction(pool_name: String)]
pub struct InitializePool<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + Pool::INIT_SPACE,
        seeds = [b"arf_pool", pool_name.as_bytes()],
        bump
    )]
    pub pool: Account<'info, Pool>,

    pub mint: Account<'info, Mint>,

    #[account(
        init,
        payer = admin,
        token::mint = mint,
        token::authority = vault,
        seeds = [b"arf_vault", pool.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct OpenCreditLine<'info> {
    #[account(
        mut,
        has_one = admin @ ArfError::Unauthorized
    )]
    pub pool: Account<'info, Pool>,

    #[account(
        init,
        payer = admin,
        space = 8 + CreditLine::INIT_SPACE,
        seeds = [b"arf_credit", pool.key().as_ref(), borrower.key().as_ref()],
        bump
    )]
    pub credit_line: Account<'info, CreditLine>,

    /// CHECK: Validated institutional borrower public key
    pub borrower: AccountInfo<'info>,

    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct DepositLiquidity<'info> {
    #[account(mut)]
    pub pool: Account<'info, Pool>,

    #[account(
        mut,
        address = pool.vault @ ArfError::InvalidVaultAddress
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = provider_token_account.mint == pool.mint @ ArfError::InvalidMint
    )]
    pub provider_token_account: Account<'info, TokenAccount>,

    pub provider: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct Drawdown<'info> {
    #[account(mut)]
    pub pool: Account<'info, Pool>,

    #[account(
        mut,
        seeds = [b"arf_credit", pool.key().as_ref(), borrower.key().as_ref()],
        bump = credit_line.bump,
        has_one = pool @ ArfError::PoolMismatch,
        has_one = borrower @ ArfError::Unauthorized
    )]
    pub credit_line: Account<'info, CreditLine>,

    #[account(
        mut,
        address = pool.vault @ ArfError::InvalidVaultAddress
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = borrower_token_account.mint == pool.mint @ ArfError::InvalidMint
    )]
    pub borrower_token_account: Account<'info, TokenAccount>,

    pub borrower: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct Repay<'info> {
    #[account(mut)]
    pub pool: Account<'info, Pool>,

    #[account(
        mut,
        seeds = [b"arf_credit", pool.key().as_ref(), borrower.key().as_ref()],
        bump = credit_line.bump,
        has_one = pool @ ArfError::PoolMismatch,
        has_one = borrower @ ArfError::Unauthorized
    )]
    pub credit_line: Account<'info, CreditLine>,

    #[account(
        mut,
        address = pool.vault @ ArfError::InvalidVaultAddress
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = borrower_token_account.mint == pool.mint @ ArfError::InvalidMint
    )]
    pub borrower_token_account: Account<'info, TokenAccount>,

    pub borrower: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct SetFreezeState<'info> {
    #[account(
        mut,
        has_one = admin @ ArfError::Unauthorized
    )]
    pub pool: Account<'info, Pool>,
    pub admin: Signer<'info>,
}

// -----------------------------------------------------------------------------
// State Schema
// -----------------------------------------------------------------------------
#[account]
#[derive(InitSpace)]
pub struct Pool {
    pub admin: Pubkey,
    pub mint: Pubkey,
    pub vault: Pubkey,
    #[max_len(32)]
    pub pool_name: String,
    pub base_annual_rate_bps: u16,
    pub total_liquidity: u64,
    pub total_borrowed: u64,
    pub is_frozen: bool,
    pub bump: u8,
    pub vault_bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct CreditLine {
    pub pool: Pubkey,
    pub borrower: Pubkey,
    pub credit_limit: u64,
    pub principal_debt: u64,
    pub unpaid_interest: u64,
    pub last_accrual_slot: u64,
    pub is_active: bool,
    pub bump: u8,
}

// -----------------------------------------------------------------------------
// Events
// -----------------------------------------------------------------------------
#[event]
pub struct PoolInitializedEvent {
    pub pool: Pubkey,
    pub admin: Pubkey,
    pub mint: Pubkey,
    pub annual_rate_bps: u16,
}

#[event]
pub struct CreditLineOpenedEvent {
    pub credit_line: Pubkey,
    pub pool: Pubkey,
    pub borrower: Pubkey,
    pub credit_limit: u64,
}

#[event]
pub struct LiquidityDepositedEvent {
    pub pool: Pubkey,
    pub provider: Pubkey,
    pub amount: u64,
    pub total_liquidity: u64,
}

#[event]
pub struct DrawdownExecutedEvent {
    pub pool: Pubkey,
    pub credit_line: Pubkey,
    pub borrower: Pubkey,
    pub amount_drawn: u64,
    pub new_principal_debt: u64,
    pub current_slot: u64,
}

#[event]
pub struct RepayExecutedEvent {
    pub pool: Pubkey,
    pub credit_line: Pubkey,
    pub borrower: Pubkey,
    pub amount_repaid: u64,
    pub interest_paid: u64,
    pub principal_paid: u64,
    pub remaining_principal: u64,
    pub remaining_interest: u64,
    pub current_slot: u64,
}

#[event]
pub struct PoolFreezeStateUpdatedEvent {
    pub pool: Pubkey,
    pub is_frozen: bool,
}

// -----------------------------------------------------------------------------
// Custom Errors
// -----------------------------------------------------------------------------
#[error_code]
pub enum ArfError {
    #[msg("Pool name cannot exceed 32 characters.")]
    PoolNameTooLong,
    #[msg("Invalid base annual rate BPS.")]
    InvalidRateBps,
    #[msg("Credit limit must be strictly greater than 0.")]
    InvalidCreditLimit,
    #[msg("Transfer amount cannot be 0.")]
    ZeroTransferAmount,
    #[msg("Pool is currently frozen by risk administrator.")]
    PoolIsFrozen,
    #[msg("Credit line account is inactive.")]
    CreditLineInactive,
    #[msg("Requested drawdown exceeds credit limit + accrued interest.")]
    CreditLimitExceeded,
    #[msg("Insufficient unallocated liquidity in the pool vault.")]
    InsufficientPoolLiquidity,
    #[msg("Mathematical overflow occurred.")]
    MathOverflow,
    #[msg("Unauthorized signer for this instruction.")]
    Unauthorized,
    #[msg("Invalid token mint supplied.")]
    InvalidMint,
    #[msg("Invalid vault PDA address supplied.")]
    InvalidVaultAddress,
    #[msg("Credit line pool mismatch.")]
    PoolMismatch,
}