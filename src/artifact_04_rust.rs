use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("Arf1111111111111111111111111111111111111111");

pub const SECONDS_PER_YEAR: u64 = 31_536_000;
pub const BPS_DIVISOR: u128 = 10_000;
pub const PRECISION: u128 = 1_000_000_000_000; // 1e12 fixed point precision

#[program]
pub mod arf_settlement_protocol {
    use super::*;

    /// Initialize the global treasury vault and protocol parameters
    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        pool_id: String,
        base_rate_bps: u16,
    ) -> Result<()> {
        require!(pool_id.len() <= 32, ArfError::StringTooLong);
        let pool = &mut ctx.accounts.pool;
        pool.authority = ctx.accounts.authority.key();
        pool.mint = ctx.accounts.mint.key();
        pool.vault = ctx.accounts.vault.key();
        pool.pool_id = pool_id;
        pool.base_rate_bps = base_rate_bps;
        pool.total_deposited = 0;
        pool.total_borrowed = 0;
        pool.is_paused = false;
        pool.bump = ctx.bumps.pool;
        pool.vault_bump = ctx.bumps.vault;

        emit!(PoolInitializedEvent {
            pool: pool.key(),
            authority: pool.authority,
            mint: pool.mint,
            base_rate_bps,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    /// Institutional LP deposits stable liquidity into the pool
    pub fn deposit_liquidity(ctx: Context<DepositLiquidity>, amount: u64) -> Result<()> {
        require!(amount > 0, ArfError::InvalidAmount);
        let pool = &mut ctx.accounts.pool;
        require!(!pool.is_paused, ArfError::PoolIsPaused);

        // Transfer funds from LP to Pool Vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.depositor_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.depositor.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        pool.total_deposited = pool
            .total_deposited
            .checked_add(amount)
            .ok_or(ArfError::MathOverflow)?;

        emit!(LiquidityDepositedEvent {
            pool: pool.key(),
            depositor: ctx.accounts.depositor.key(),
            amount,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    /// Initialize an Underwritten Credit Facility for a licensed MSB partner
    pub fn open_credit_facility(
        ctx: Context<OpenCreditFacility>,
        msb_client_id: String,
        credit_limit: u64,
        margin_rate_bps: u16,
    ) -> Result<()> {
        require!(msb_client_id.len() <= 32, ArfError::StringTooLong);
        require!(credit_limit > 0, ArfError::InvalidAmount);

        let facility = &mut ctx.accounts.credit_facility;
        let clock = Clock::get()?;

        facility.pool = ctx.accounts.pool.key();
        facility.msb_borrower = ctx.accounts.msb_borrower.key();
        facility.compliance_cosigner = ctx.accounts.compliance_cosigner.key();
        facility.msb_client_id = msb_client_id;
        facility.credit_limit = credit_limit;
        facility.drawn_principal = 0;
        facility.accrued_interest = 0;
        facility.margin_rate_bps = margin_rate_bps;
        facility.last_interest_update = clock.unix_timestamp;
        facility.is_active = true;
        facility.bump = ctx.bumps.credit_facility;

        emit!(CreditFacilityOpenedEvent {
            facility: facility.key(),
            msb_borrower: facility.msb_borrower,
            credit_limit,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    /// Autonomous Drawdown & Corridor Settlement Execution
    pub fn execute_corridor_settlement(
        ctx: Context<ExecuteCorridorSettlement>,
        corridor_id: String,
        drawdown_amount: u64,
        kyb_hash: [u8; 32],
    ) -> Result<()> {
        require!(drawdown_amount > 0, ArfError::InvalidAmount);
        require!(corridor_id.len() <= 16, ArfError::StringTooLong);

        let clock = Clock::get()?;
        let pool = &mut ctx.accounts.pool;
        let facility = &mut ctx.accounts.credit_facility;

        require!(!pool.is_paused, ArfError::PoolIsPaused);
        require!(facility.is_active, ArfError::FacilityInactive);

        // Accrue pending interest up to this second
        accrue_interest_internal(facility, pool.base_rate_bps, clock.unix_timestamp)?;

        // Check credit limit solvency
        let total_obligation = facility
            .drawn_principal
            .checked_add(drawdown_amount)
            .ok_or(ArfError::MathOverflow)?;
        require!(
            total_obligation <= facility.credit_limit,
            ArfError::CreditLimitExceeded
        );

        // Check pool liquidity
        let available_liquidity = ctx.accounts.vault.amount;
        require!(
            available_liquidity >= drawdown_amount,
            ArfError::InsufficientVaultLiquidity
        );

        // Update facility state
        facility.drawn_principal = total_obligation;
        pool.total_borrowed = pool
            .total_borrowed
            .checked_add(drawdown_amount)
            .ok_or(ArfError::MathOverflow)?;

        // Transfer funds via PDA signing
        let pool_id_bytes = pool.pool_id.as_bytes();
        let pool_bump = pool.bump;
        let signer_seeds: &[&[&[u8]]] = &[&[
            b"arf_pool",
            pool_id_bytes,
            &[pool_bump],
        ]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault.to_account_info(),
            to: ctx.accounts.settlement_destination.to_account_info(),
            authority: pool.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer_seeds);
        token::transfer(cpi_ctx, drawdown_amount)?;

        emit!(SettlementExecutedEvent {
            corridor_id,
            facility: facility.key(),
            msb_borrower: facility.msb_borrower,
            settlement_destination: ctx.accounts.settlement_destination.key(),
            amount: drawdown_amount,
            kyb_hash,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    /// Repay Credit Facility Drawdown + Accrued Interest
    pub fn repay_facility(ctx: Context<RepayFacility>, principal_payment: u64) -> Result<()> {
        let clock = Clock::get()?;
        let pool = &mut ctx.accounts.pool;
        let facility = &mut ctx.accounts.credit_facility;

        accrue_interest_internal(facility, pool.base_rate_bps, clock.unix_timestamp)?;

        let total_due = facility
            .drawn_principal
            .checked_add(facility.accrued_interest)
            .ok_or(ArfError::MathOverflow)?;

        let actual_payment = std::cmp::min(principal_payment, total_due);
        require!(actual_payment > 0, ArfError::InvalidAmount);

        // Repay interest first, remainder to principal
        let interest_paid = std::cmp::min(actual_payment, facility.accrued_interest);
        let principal_paid = actual_payment
            .checked_sub(interest_paid)
            .ok_or(ArfError::MathOverflow)?;

        facility.accrued_interest = facility
            .accrued_interest
            .checked_sub(interest_paid)
            .ok_or(ArfError::MathOverflow)?;
        facility.drawn_principal = facility
            .drawn_principal
            .checked_sub(principal_paid)
            .ok_or(ArfError::MathOverflow)?;
        pool.total_borrowed = pool
            .total_borrowed
            .checked_sub(principal_paid)
            .ok_or(ArfError::MathOverflow)?;

        // Transfer tokens from MSB repayment account to pool vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.payer_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.payer.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, actual_payment)?;

        emit!(FacilityRepaidEvent {
            facility: facility.key(),
            principal_paid,
            interest_paid,
            remaining_principal: facility.drawn_principal,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    /// Emergency Circuit Breaker: Freeze pool in case of external volatility
    pub fn toggle_pool_freeze(ctx: Context<TogglePoolFreeze>, pause_state: bool) -> Result<()> {
        let pool = &mut ctx.accounts.pool;
        pool.is_paused = pause_state;
        emit!(PoolFreezeToggledEvent {
            pool: pool.key(),
            is_paused: pause_state,
            timestamp: Clock::get()?.unix_timestamp,
        });
        Ok(())
    }
}

/// Computes linear accrued interest down to second-level precision:
/// Interest = Principal * (Base Rate + Margin Rate) * elapsed_seconds / (SECONDS_PER_YEAR * 10,000)
fn accrue_interest_internal(
    facility: &mut Account<CreditFacility>,
    base_rate_bps: u16,
    now: i64,
) -> Result<()> {
    if now <= facility.last_interest_update || facility.drawn_principal == 0 {
        facility.last_interest_update = now;
        return Ok(());
    }

    let elapsed = (now - facility.last_interest_update) as u128;
    let total_apr_bps = (base_rate_bps as u128)
        .checked_add(facility.margin_rate_bps as u128)
        .ok_or(ArfError::MathOverflow)?;

    let principal = facility.drawn_principal as u128;

    let interest_numerator = principal
        .checked_mul(total_apr_bps)
        .ok_or(ArfError::MathOverflow)?
        .checked_mul(elapsed)
        .ok_or(ArfError::MathOverflow)?;

    let interest_denominator = (SECONDS_PER_YEAR as u128)
        .checked_mul(BPS_DIVISOR)
        .ok_or(ArfError::MathOverflow)?;

    let incremental_interest = (interest_numerator / interest_denominator) as u64;

    facility.accrued_interest = facility
        .accrued_interest
        .checked_add(incremental_interest)
        .ok_or(ArfError::MathOverflow)?;
    facility.last_interest_update = now;

    Ok(())
}

// -----------------------------------------------------------------------------
// ACCOUNT VALIDATION STRUCTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(pool_id: String)]
pub struct InitializePool<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + Pool::INIT_SPACE,
        seeds = [b"arf_pool", pool_id.as_bytes()],
        bump
    )]
    pub pool: Account<'info, Pool>,

    pub mint: Account<'info, Mint>,

    #[account(
        init,
        payer = authority,
        seeds = [b"arf_vault", pool.key().as_ref()],
        bump,
        token::mint = mint,
        token::authority = pool,
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub authority: Signer<'info>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct DepositLiquidity<'info> {
    #[account(
        mut,
        seeds = [b"arf_pool", pool.pool_id.as_bytes()],
        bump = pool.bump
    )]
    pub pool: Account<'info, Pool>,

    #[account(
        mut,
        seeds = [b"arf_vault", pool.key().as_ref()],
        bump = pool.vault_bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub depositor_token_account: Account<'info, TokenAccount>,

    pub depositor: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
#[instruction(msb_client_id: String)]
pub struct OpenCreditFacility<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + CreditFacility::INIT_SPACE,
        seeds = [b"credit_facility", pool.key().as_ref(), msb_borrower.key().as_ref()],
        bump
    )]
    pub credit_facility: Account<'info, CreditFacility>,

    pub pool: Account<'info, Pool>,

    /// CHECK: The external MSB company public key
    pub msb_borrower: AccountInfo<'info>,

    /// Autonomous compliance oracle cosigner
    pub compliance_cosigner: Signer<'info>,

    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ExecuteCorridorSettlement<'info> {
    #[account(
        mut,
        seeds = [b"arf_pool", pool.pool_id.as_bytes()],
        bump = pool.bump
    )]
    pub pool: Account<'info, Pool>,

    #[account(
        mut,
        seeds = [b"arf_vault", pool.key().as_ref()],
        bump = pool.vault_bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"credit_facility", pool.key().as_ref(), msb_borrower.key().as_ref()],
        bump = credit_facility.bump,
        has_one = msb_borrower,
        has_one = compliance_cosigner
    )]
    pub credit_facility: Account<'info, CreditFacility>,

    #[account(mut)]
    pub settlement_destination: Account<'info, TokenAccount>,

    pub msb_borrower: Signer<'info>,
    pub compliance_cosigner: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct RepayFacility<'info> {
    #[account(
        mut,
        seeds = [b"arf_pool", pool.pool_id.as_bytes()],
        bump = pool.bump
    )]
    pub pool: Account<'info, Pool>,

    #[account(
        mut,
        seeds = [b"arf_vault", pool.key().as_ref()],
        bump = pool.vault_bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"credit_facility", pool.key().as_ref(), credit_facility.msb_borrower.as_ref()],
        bump = credit_facility.bump
    )]
    pub credit_facility: Account<'info, CreditFacility>,

    #[account(mut)]
    pub payer_token_account: Account<'info, TokenAccount>,

    pub payer: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct TogglePoolFreeze<'info> {
    #[account(
        mut,
        has_one = authority
    )]
    pub pool: Account<'info, Pool>,
    pub authority: Signer<'info>,
}

// -----------------------------------------------------------------------------
// STATE STRUCTS
// -----------------------------------------------------------------------------

#[account]
pub struct Pool {
    pub authority: Pubkey,        // 32
    pub mint: Pubkey,             // 32
    pub vault: Pubkey,            // 32
    pub pool_id: String,          // 4 + 32
    pub base_rate_bps: u16,       // 2
    pub total_deposited: u64,     // 8
    pub total_borrowed: u64,      // 8
    pub is_paused: bool,          // 1
    pub bump: u8,                 // 1
    pub vault_bump: u8,           // 1
}

impl Pool {
    pub const INIT_SPACE: usize = 32 + 32 + 32 + (4 + 32) + 2 + 8 + 8 + 1 + 1 + 1;
}

#[account]
pub struct CreditFacility {
    pub pool: Pubkey,                 // 32
    pub msb_borrower: Pubkey,         // 32
    pub compliance_cosigner: Pubkey,  // 32
    pub msb_client_id: String,        // 4 + 32
    pub credit_limit: u64,            // 8
    pub drawn_principal: u64,         // 8
    pub accrued_interest: u64,        // 8
    pub margin_rate_bps: u16,         // 2
    pub last_interest_update: i64,    // 8
    pub is_active: bool,              // 1
    pub bump: u8,                     // 1
}

impl CreditFacility {
    pub const INIT_SPACE: usize = 32 + 32 + 32 + (4 + 32) + 8 + 8 + 8 + 2 + 8 + 1 + 1;
}

// -----------------------------------------------------------------------------
// EVENTS & ERRORS
// -----------------------------------------------------------------------------

#[event]
pub struct PoolInitializedEvent {
    pub pool: Pubkey,
    pub authority: Pubkey,
    pub mint: Pubkey,
    pub base_rate_bps: u16,
    pub timestamp: i64,
}

#[event]
pub struct LiquidityDepositedEvent {
    pub pool: Pubkey,
    pub depositor: Pubkey,
    pub amount: u64,
    pub timestamp: i64,
}

#[event]
pub struct CreditFacilityOpenedEvent {
    pub facility: Pubkey,
    pub msb_borrower: Pubkey,
    pub credit_limit: u64,
    pub timestamp: i64,
}

#[event]
pub struct SettlementExecutedEvent {
    pub corridor_id: String,
    pub facility: Pubkey,
    pub msb_borrower: Pubkey,
    pub settlement_destination: Pubkey,
    pub amount: u64,
    pub kyb_hash: [u8; 32],
    pub timestamp: i64,
}

#[event]
pub struct FacilityRepaidEvent {
    pub facility: Pubkey,
    pub principal_paid: u64,
    pub interest_paid: u64,
    pub remaining_principal: u64,
    pub timestamp: i64,
}

#[event]
pub struct PoolFreezeToggledEvent {
    pub pool: Pubkey,
    pub is_paused: bool,
    pub timestamp: i64,
}

#[error_code]
pub enum ArfError {
    #[msg("String exceeds allocated character capacity")]
    StringTooLong,
    #[msg("Amount must be greater than zero")]
    InvalidAmount,
    #[msg("Global protocol pool is currently paused by risk management")]
    PoolIsPaused,
    #[msg("Credit facility is inactive or revoked")]
    FacilityInactive,
    #[msg("Requested drawdown exceeds active credit facility limit")]
    CreditLimitExceeded,
    #[msg("Vault has insufficient liquid reserve tokens")]
    InsufficientVaultLiquidity,
    #[msg("Arithmetic overflow occurred during computation")]
    MathOverflow,
}