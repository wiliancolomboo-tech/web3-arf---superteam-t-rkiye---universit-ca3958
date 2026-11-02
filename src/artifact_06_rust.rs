use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("ArfNxus1111111111111111111111111111111111111");

pub const SECONDS_PER_YEAR: u64 = 31_536_000;
pub const BPS_DENOMINATOR: u128 = 10_000;
pub const POOL_SEED_PREFIX: &[u8] = b"arf_pool";
pub const VAULT_SEED_PREFIX: &[u8] = b"arf_vault";
pub const CREDIT_LINE_SEED_PREFIX: &[u8] = b"arf_credit_line";

#[program]
pub mod arf_nexus {
    use super::*;

    /// Initializes a new institutional settlement pool for a designated stablecoin.
    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        pool_name: String,
        protocol_fee_bps: u16,
    ) -> Result<()> {
        require!(pool_name.len() <= 32, ArfError::NameTooLong);
        require!(protocol_fee_bps <= 2000, ArfError::FeeTooHigh);

        let pool = &mut ctx.accounts.pool;
        pool.authority = ctx.accounts.authority.key();
        pool.underlying_mint = ctx.accounts.underlying_mint.key();
        pool.vault = ctx.accounts.vault.key();
        pool.total_liquidity = 0;
        pool.total_borrowed = 0;
        pool.protocol_fee_bps = protocol_fee_bps;
        pool.bump = ctx.bumps.pool;
        pool.vault_bump = ctx.bumps.vault;

        let mut name_bytes = [0u8; 32];
        name_bytes[..pool_name.len()].copy_from_slice(pool_name.as_bytes());
        pool.pool_name = name_bytes;

        emit!(PoolInitializedEvent {
            pool: pool.key(),
            authority: pool.authority,
            mint: pool.underlying_mint,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    /// Institutional LP deposits liquidity into the pool vault.
    pub fn deposit_liquidity(ctx: Context<DepositLiquidity>, amount: u64) -> Result<()> {
        require!(amount > 0, ArfError::ZeroAmount);

        let pool = &mut ctx.accounts.pool;

        // Perform token transfer from LP to vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.depositor_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.depositor.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        pool.total_liquidity = pool
            .total_liquidity
            .checked_add(amount)
            .ok_or(ArfError::MathOverflow)?;

        emit!(LiquidityDepositedEvent {
            pool: pool.key(),
            depositor: ctx.accounts.depositor.key(),
            amount,
            new_total_liquidity: pool.total_liquidity,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    /// Underwrites and establishes a revolving credit line for a compliant MSB borrower.
    pub fn open_credit_line(
        ctx: Context<OpenCreditLine>,
        credit_limit: u64,
        interest_rate_bps: u16,
        tenor_seconds: i64,
    ) -> Result<()> {
        require!(credit_limit > 0, ArfError::ZeroAmount);
        require!(interest_rate_bps > 0 && interest_rate_bps <= 5000, ArfError::InvalidInterestRate);
        require!(tenor_seconds >= 3600, ArfError::TenorTooShort); // Min 1 hour

        let credit_line = &mut ctx.accounts.credit_line;
        credit_line.pool = ctx.accounts.pool.key();
        credit_line.borrower = ctx.accounts.borrower.key();
        credit_line.credit_limit = credit_limit;
        credit_line.current_principal = 0;
        credit_line.last_accrual_timestamp = Clock::get()?.unix_timestamp;
        credit_line.accrued_interest = 0;
        credit_line.interest_rate_bps = interest_rate_bps;
        credit_line.tenor_seconds = tenor_seconds;
        credit_line.last_draw_timestamp = 0;
        credit_line.status = CreditStatus::Active;
        credit_line.bump = ctx.bumps.credit_line;

        emit!(CreditLineOpenedEvent {
            credit_line: credit_line.key(),
            pool: ctx.accounts.pool.key(),
            borrower: credit_line.borrower,
            credit_limit,
            interest_rate_bps,
            tenor_seconds,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    /// MSB executes an on-demand drawdown directly to payout/escrow address.
    pub fn draw_credit(ctx: Context<DrawCredit>, amount: u64) -> Result<()> {
        require!(amount > 0, ArfError::ZeroAmount);

        let pool = &mut ctx.accounts.pool;
        let credit_line = &mut ctx.accounts.credit_line;
        let clock = Clock::get()?;

        require!(credit_line.status == CreditStatus::Active, ArfError::CreditLineNotActive);

        // Check Tenor status if active loan exists
        if credit_line.current_principal > 0 {
            let elapsed = clock.unix_timestamp.saturating_sub(credit_line.last_draw_timestamp);
            require!(elapsed <= credit_line.tenor_seconds, ArfError::CreditLineDelinquent);
        }

        // Accrue pending interest up to now
        accrue_interest_internal(credit_line, clock.unix_timestamp)?;

        let new_principal = credit_line
            .current_principal
            .checked_add(amount)
            .ok_or(ArfError::MathOverflow)?;

        let total_exposure = new_principal
            .checked_add(credit_line.accrued_interest)
            .ok_or(ArfError::MathOverflow)?;

        require!(total_exposure <= credit_line.credit_limit, ArfError::ExceedsCreditLimit);

        // Verify Vault Liquidity
        require!(ctx.accounts.vault.amount >= amount, ArfError::InsufficientVaultLiquidity);

        // Update balances
        credit_line.current_principal = new_principal;
        credit_line.last_draw_timestamp = clock.unix_timestamp;
        credit_line.last_accrual_timestamp = clock.unix_timestamp;

        pool.total_borrowed = pool
            .total_borrowed
            .checked_add(amount)
            .ok_or(ArfError::MathOverflow)?;

        // Transfer funds from vault PDA to destination payout account
        let pool_key = pool.key();
        let seeds = &[
            VAULT_SEED_PREFIX,
            pool_key.as_ref(),
            &[pool.vault_bump],
        ];
        let signer = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault.to_account_info(),
            to: ctx.accounts.destination_account.to_account_info(),
            authority: ctx.accounts.vault.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer);
        token::transfer(cpi_ctx, amount)?;

        emit!(CreditDrawnEvent {
            credit_line: credit_line.key(),
            borrower: credit_line.borrower,
            amount,
            destination: ctx.accounts.destination_account.key(),
            current_principal: credit_line.current_principal,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    /// MSB repays loan principal and accrued interest.
    pub fn repay_credit(ctx: Context<RepayCredit>, amount: u64) -> Result<()> {
        require!(amount > 0, ArfError::ZeroAmount);

        let pool = &mut ctx.accounts.pool;
        let credit_line = &mut ctx.accounts.credit_line;
        let clock = Clock::get()?;

        // Accrue interest up to current timestamp
        accrue_interest_internal(credit_line, clock.unix_timestamp)?;

        let total_owed = credit_line
            .current_principal
            .checked_add(credit_line.accrued_interest)
            .ok_or(ArfError::MathOverflow)?;

        require!(total_owed > 0, ArfError::NoOutstandingDebt);

        let payment_amount = std::cmp::min(amount, total_owed);

        // Distribute repayment between interest and principal
        let mut remaining = payment_amount;
        let mut interest_paid = 0u64;
        let mut principal_paid = 0u64;

        if credit_line.accrued_interest > 0 {
            if remaining >= credit_line.accrued_interest {
                interest_paid = credit_line.accrued_interest;
                remaining = remaining
                    .checked_sub(credit_line.accrued_interest)
                    .ok_or(ArfError::MathOverflow)?;
                credit_line.accrued_interest = 0;
            } else {
                interest_paid = remaining;
                credit_line.accrued_interest = credit_line
                    .accrued_interest
                    .checked_sub(remaining)
                    .ok_or(ArfError::MathOverflow)?;
                remaining = 0;
            }
        }

        if remaining > 0 {
            principal_paid = remaining;
            credit_line.current_principal = credit_line
                .current_principal
                .checked_sub(remaining)
                .ok_or(ArfError::MathOverflow)?;
            pool.total_borrowed = pool
                .total_borrowed
                .checked_sub(remaining)
                .ok_or(ArfError::MathOverflow)?;
        }

        // Transfer funds from payer to vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.payer_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.payer.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, payment_amount)?;

        // Pool balance increases by interest paid (yield accrued to pool)
        pool.total_liquidity = pool
            .total_liquidity
            .checked_add(interest_paid)
            .ok_or(ArfError::MathOverflow)?;

        // If debt fully paid off, reset delinquent status to Active
        if credit_line.current_principal == 0 && credit_line.accrued_interest == 0 {
            credit_line.status = CreditStatus::Active;
            credit_line.last_draw_timestamp = 0;
        }

        emit!(CreditRepaidEvent {
            credit_line: credit_line.key(),
            borrower: credit_line.borrower,
            interest_paid,
            principal_paid,
            remaining_principal: credit_line.current_principal,
            remaining_interest: credit_line.accrued_interest,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    /// Evaluates account status against tenor and flags delinquent facilities.
    pub fn assess_delinquency(ctx: Context<AssessDelinquency>) -> Result<()> {
        let credit_line = &mut ctx.accounts.credit_line;
        let clock = Clock::get()?;

        if credit_line.current_principal > 0 {
            let elapsed = clock.unix_timestamp.saturating_sub(credit_line.last_draw_timestamp);
            if elapsed > credit_line.tenor_seconds {
                credit_line.status = CreditStatus::Delinquent;
                emit!(DelinquencyFlaggedEvent {
                    credit_line: credit_line.key(),
                    borrower: credit_line.borrower,
                    principal_overdue: credit_line.current_principal,
                    timestamp: clock.unix_timestamp,
                });
            }
        }

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// INTERNAL HELPERS
// -----------------------------------------------------------------------------

fn accrue_interest_internal(credit_line: &mut Account<BorrowerCreditLine>, now: i64) -> Result<()> {
    if credit_line.current_principal == 0 {
        credit_line.last_accrual_timestamp = now;
        return Ok(());
    }

    let time_delta = now.saturating_sub(credit_line.last_accrual_timestamp);
    if time_delta <= 0 {
        return Ok(());
    }

    let principal = credit_line.current_principal as u128;
    let rate_bps = credit_line.interest_rate_bps as u128;
    let delta = time_delta as u128;

    // formula: (principal * rate_bps * delta) / (10_000 * SECONDS_PER_YEAR)
    let numerator = principal
        .checked_mul(rate_bps)
        .ok_or(ArfError::MathOverflow)?
        .checked_mul(delta)
        .ok_or(ArfError::MathOverflow)?;

    let denominator = BPS_DENOMINATOR
        .checked_mul(SECONDS_PER_YEAR as u128)
        .ok_or(ArfError::MathOverflow)?;

    let interest = (numerator / denominator) as u64;

    if interest > 0 {
        credit_line.accrued_interest = credit_line
            .accrued_interest
            .checked_add(interest)
            .ok_or(ArfError::MathOverflow)?;
    }

    credit_line.last_accrual_timestamp = now;
    Ok(())
}

// -----------------------------------------------------------------------------
// INSTRUCTION ACCOUNTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(pool_name: String)]
pub struct InitializePool<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    pub underlying_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = authority,
        space = 8 + InstitutionalPool::LEN,
        seeds = [POOL_SEED_PREFIX, underlying_mint.key().as_ref(), authority.key().as_ref()],
        bump
    )]
    pub pool: Account<'info, InstitutionalPool>,

    #[account(
        init,
        payer = authority,
        seeds = [VAULT_SEED_PREFIX, pool.key().as_ref()],
        bump,
        token::mint = underlying_mint,
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
    pub depositor: Signer<'info>,

    #[account(
        mut,
        has_one = vault @ ArfError::InvalidVaultAccount,
    )]
    pub pool: Account<'info, InstitutionalPool>,

    #[account(mut)]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = depositor_token_account.owner == depositor.key() @ ArfError::Unauthorized,
        constraint = depositor_token_account.mint == pool.underlying_mint @ ArfError::InvalidTokenMint
    )]
    pub depositor_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct OpenCreditLine<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(
        has_one = authority @ ArfError::Unauthorized
    )]
    pub pool: Account<'info, InstitutionalPool>,

    /// CHECK: Validated as borrower identity
    pub borrower: UncheckedAccount<'info>,

    #[account(
        init,
        payer = authority,
        space = 8 + BorrowerCreditLine::LEN,
        seeds = [CREDIT_LINE_SEED_PREFIX, pool.key().as_ref(), borrower.key().as_ref()],
        bump
    )]
    pub credit_line: Account<'info, BorrowerCreditLine>,

    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct DrawCredit<'info> {
    pub borrower: Signer<'info>,

    #[account(
        mut,
        has_one = vault @ ArfError::InvalidVaultAccount
    )]
    pub pool: Account<'info, InstitutionalPool>,

    #[account(
        mut,
        seeds = [CREDIT_LINE_SEED_PREFIX, pool.key().as_ref(), borrower.key().as_ref()],
        bump = credit_line.bump,
        has_one = pool @ ArfError::PoolMismatch,
        has_one = borrower @ ArfError::Unauthorized
    )]
    pub credit_line: Account<'info, BorrowerCreditLine>,

    #[account(
        mut,
        seeds = [VAULT_SEED_PREFIX, pool.key().as_ref()],
        bump = pool.vault_bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = destination_account.mint == pool.underlying_mint @ ArfError::InvalidTokenMint
    )]
    pub destination_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct RepayCredit<'info> {
    pub payer: Signer<'info>,

    #[account(
        mut,
        has_one = vault @ ArfError::InvalidVaultAccount
    )]
    pub pool: Account<'info, InstitutionalPool>,

    #[account(
        mut,
        has_one = pool @ ArfError::PoolMismatch
    )]
    pub credit_line: Account<'info, BorrowerCreditLine>,

    #[account(
        mut,
        seeds = [VAULT_SEED_PREFIX, pool.key().as_ref()],
        bump = pool.vault_bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = payer_token_account.owner == payer.key() @ ArfError::Unauthorized,
        constraint = payer_token_account.mint == pool.underlying_mint @ ArfError::InvalidTokenMint
    )]
    pub payer_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct AssessDelinquency<'info> {
    #[account(mut)]
    pub credit_line: Account<'info, BorrowerCreditLine>,
}

// -----------------------------------------------------------------------------
// STATE SCHEMAS
// -----------------------------------------------------------------------------

#[account]
pub struct InstitutionalPool {
    pub authority: Pubkey,
    pub underlying_mint: Pubkey,
    pub vault: Pubkey,
    pub total_liquidity: u64,
    pub total_borrowed: u64,
    pub protocol_fee_bps: u16,
    pub bump: u8,
    pub vault_bump: u8,
    pub pool_name: [u8; 32],
}

impl InstitutionalPool {
    pub const LEN: usize = 32 + 32 + 32 + 8 + 8 + 2 + 1 + 1 + 32;
}

#[account]
pub struct BorrowerCreditLine {
    pub pool: Pubkey,
    pub borrower: Pubkey,
    pub credit_limit: u64,
    pub current_principal: u64,
    pub last_accrual_timestamp: i64,
    pub accrued_interest: u64,
    pub interest_rate_bps: u16,
    pub tenor_seconds: i64,
    pub last_draw_timestamp: i64,
    pub status: CreditStatus,
    pub bump: u8,
}

impl BorrowerCreditLine {
    pub const LEN: usize = 32 + 32 + 8 + 8 + 8 + 8 + 2 + 8 + 8 + 1 + 1;
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum CreditStatus {
    Active,
    Frozen,
    Delinquent,
    Closed,
}

// -----------------------------------------------------------------------------
// EVENTS
// -----------------------------------------------------------------------------

#[event]
pub struct PoolInitializedEvent {
    pub pool: Pubkey,
    pub authority: Pubkey,
    pub mint: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct LiquidityDepositedEvent {
    pub pool: Pubkey,
    pub depositor: Pubkey,
    pub amount: u64,
    pub new_total_liquidity: u64,
    pub timestamp: i64,
}

#[event]
pub struct CreditLineOpenedEvent {
    pub credit_line: Pubkey,
    pub pool: Pubkey,
    pub borrower: Pubkey,
    pub credit_limit: u64,
    pub interest_rate_bps: u16,
    pub tenor_seconds: i64,
    pub timestamp: i64,
}

#[event]
pub struct CreditDrawnEvent {
    pub credit_line: Pubkey,
    pub borrower: Pubkey,
    pub amount: u64,
    pub destination: Pubkey,
    pub current_principal: u64,
    pub timestamp: i64,
}

#[event]
pub struct CreditRepaidEvent {
    pub credit_line: Pubkey,
    pub borrower: Pubkey,
    pub interest_paid: u64,
    pub principal_paid: u64,
    pub remaining_principal: u64,
    pub remaining_interest: u64,
    pub timestamp: i64,
}

#[event]
pub struct DelinquencyFlaggedEvent {
    pub credit_line: Pubkey,
    pub borrower: Pubkey,
    pub principal_overdue: u64,
    pub timestamp: i64,
}

// -----------------------------------------------------------------------------
// ERROR CODES
// -----------------------------------------------------------------------------

#[error_code]
pub enum ArfError {
    #[msg("String length exceeds the 32-byte allocated maximum.")]
    NameTooLong,
    #[msg("Protocol fee exceeds hard boundary of 2000 bps (20%).")]
    FeeTooHigh,
    #[msg("Amount must be greater than zero.")]
    ZeroAmount,
    #[msg("Provided token account does not match expected pool vault.")]
    InvalidVaultAccount,
    #[msg("Caller is unauthorized to perform this institutional operation.")]
    Unauthorized,
    #[msg("Token account mint does not match the underlying pool mint.")]
    InvalidTokenMint,
    #[msg("Interest rate cannot be zero and must not exceed 5000 bps (50%).")]
    InvalidInterestRate,
    #[msg("Maturity tenor must be at least 3,600 seconds (1 hour).")]
    TenorTooShort,
    #[msg("Mathematical operation resulted in an arithmetic overflow.")]
    MathOverflow,
    #[msg("Credit line is not in Active status.")]
    CreditLineNotActive,
    #[msg("Credit line is past due tenor maturity and marked delinquent.")]
    CreditLineDelinquent,
    #[msg("Drawdown request exceeds approved revolving credit limit.")]
    ExceedsCreditLimit,
    #[msg("Institutional vault does not have sufficient idle liquidity.")]
    InsufficientVaultLiquidity,
    #[msg("No outstanding principal or interest to repay.")]
    NoOutstandingDebt,
    #[msg("Pool does not match the credit line constraint.")]
    PoolMismatch,
}