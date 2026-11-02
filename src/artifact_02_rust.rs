use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("ArfF1ow111111111111111111111111111111111111");

#[program]
pub mod arf_flow {
    use super::*;

    /// Initialize the global institutional liquidity pool
    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        pool_name: String,
        base_interest_bps: u16,
    ) -> Result<()> {
        require!(pool_name.len() <= 32, ArfError::PoolNameTooLong);
        require!(base_interest_bps <= 5000, ArfError::ExcessiveInterestRate);

        let pool = &mut ctx.accounts.pool;
        pool.admin = ctx.accounts.admin.key();
        pool.underwriter = ctx.accounts.underwriter.key();
        pool.mint = ctx.accounts.mint.key();
        pool.vault = ctx.accounts.vault.key();
        pool.total_liquidity = 0;
        pool.total_borrowed = 0;
        pool.base_interest_bps = base_interest_bps;
        pool.is_paused = false;
        pool.bump = ctx.bumps.pool;

        emit!(PoolInitialized {
            pool: pool.key(),
            admin: pool.admin,
            mint: pool.mint,
            base_interest_bps,
        });

        Ok(())
    }

    /// Institutional LP deposits stablecoins into the vault
    pub fn deposit_liquidity(ctx: Context<DepositLiquidity>, amount: u64) -> Result<()> {
        require!(amount > 0, ArfError::ZeroDepositAmount);
        require!(!ctx.accounts.pool.is_paused, ArfError::PoolPaused);

        // Transfer funds from LP to the Vault Account
        let cpi_accounts = Transfer {
            from: ctx.accounts.lp_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.lp_authority.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        // Update LP Position
        let position = &mut ctx.accounts.lp_position;
        if position.owner == Pubkey::default() {
            position.owner = ctx.accounts.lp_authority.key();
            position.pool = ctx.accounts.pool.key();
            position.deposited_amount = 0;
            position.last_deposit_timestamp = Clock::get()?.unix_timestamp;
            position.bump = ctx.bumps.lp_position;
        }

        position.deposited_amount = position
            .deposited_amount
            .checked_add(amount)
            .ok_or(ArfError::MathOverflow)?;
        position.last_deposit_timestamp = Clock::get()?.unix_timestamp;

        // Update Pool Aggregates
        let pool = &mut ctx.accounts.pool;
        pool.total_liquidity = pool
            .total_liquidity
            .checked_add(amount)
            .ok_or(ArfError::MathOverflow)?;

        emit!(LiquiditySupplied {
            pool: pool.key(),
            lp: ctx.accounts.lp_authority.key(),
            amount,
            total_pool_liquidity: pool.total_liquidity,
        });

        Ok(())
    }

    /// Underwriter configures or updates an MSB's revolving credit facility
    pub fn configure_facility(
        ctx: Context<ConfigureFacility>,
        credit_limit: u64,
        custom_interest_bps: u16,
        corridor_id: [u8; 8], // ISO corridor encoding, e.g. "TRY-USD "
    ) -> Result<()> {
        require!(
            ctx.accounts.underwriter.key() == ctx.accounts.pool.underwriter,
            ArfError::UnauthorizedUnderwriter
        );
        require!(custom_interest_bps <= 5000, ArfError::ExcessiveInterestRate);

        let facility = &mut ctx.accounts.facility;
        facility.borrower = ctx.accounts.borrower.key();
        facility.pool = ctx.accounts.pool.key();
        facility.credit_limit = credit_limit;
        facility.custom_interest_bps = custom_interest_bps;
        facility.current_drawn = 0;
        facility.corridor_id = corridor_id;
        facility.last_drawdown_timestamp = 0;
        facility.is_active = true;
        facility.bump = ctx.bumps.facility;

        emit!(FacilityConfigured {
            facility: facility.key(),
            borrower: facility.borrower,
            credit_limit,
            custom_interest_bps,
            corridor_id,
        });

        Ok(())
    }

    /// MSB executes a drawdown from their revolving credit line
    pub fn drawdown(ctx: Context<Drawdown>, amount: u64) -> Result<()> {
        require!(amount > 0, ArfError::ZeroDrawdownAmount);
        let pool = &ctx.accounts.pool;
        require!(!pool.is_paused, ArfError::PoolPaused);

        let facility = &mut ctx.accounts.facility;
        require!(facility.is_active, ArfError::FacilityDisabled);

        // Check Available Facility Limit
        let new_drawn = facility
            .current_drawn
            .checked_add(amount)
            .ok_or(ArfError::MathOverflow)?;
        require!(new_drawn <= facility.credit_limit, ArfError::LimitExceeded);

        // Check Pool Liquidity
        let available_cash = ctx.accounts.vault.amount;
        require!(available_cash >= amount, ArfError::InsufficientVaultLiquidity);

        // Accrue interest if there was previous principal
        let clock = Clock::get()?;
        let current_time = clock.unix_timestamp;

        facility.current_drawn = new_drawn;
        facility.last_drawdown_timestamp = current_time;

        let pool_key = pool.key();
        let seeds = &[
            b"pool".as_ref(),
            pool.mint.as_ref(),
            &[pool.bump],
        ];
        let signer_seeds = &[&seeds[..]];

        // Transfer funds from Vault PDA to MSB destination account
        let cpi_accounts = Transfer {
            from: ctx.accounts.vault.to_account_info(),
            to: ctx.accounts.borrower_token_account.to_account_info(),
            authority: ctx.accounts.pool.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer_seeds);
        token::transfer(cpi_ctx, amount)?;

        // Update pool states
        let pool_mut = &mut ctx.accounts.pool;
        pool_mut.total_borrowed = pool_mut
            .total_borrowed
            .checked_add(amount)
            .ok_or(ArfError::MathOverflow)?;

        emit!(DrawdownExecuted {
            facility: facility.key(),
            borrower: facility.borrower,
            amount,
            current_drawn: facility.current_drawn,
            timestamp: current_time,
        });

        Ok(())
    }

    /// MSB repays drawn capital plus accrued interest fees
    pub fn repay(ctx: Context<Repay>, principal_amount: u64, fee_amount: u64) -> Result<()> {
        require!(principal_amount > 0, ArfError::ZeroRepayAmount);
        let facility = &mut ctx.accounts.facility;

        require!(
            principal_amount <= facility.current_drawn,
            ArfError::RepayExceedsDebt
        );

        let total_repay = principal_amount
            .checked_add(fee_amount)
            .ok_or(ArfError::MathOverflow)?;

        // Transfer repayment (Principal + Fee) from Borrower to Vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.borrower_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.borrower.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, total_repay)?;

        // Update facility balances
        facility.current_drawn = facility
            .current_drawn
            .checked_sub(principal_amount)
            .ok_or(ArfError::MathOverflow)?;

        // Update global pool balances
        let pool = &mut ctx.accounts.pool;
        pool.total_borrowed = pool
            .total_borrowed
            .checked_sub(principal_amount)
            .ok_or(ArfError::MathOverflow)?;

        // Interest fee increases total LP equity
        pool.total_liquidity = pool
            .total_liquidity
            .checked_add(fee_amount)
            .ok_or(ArfError::MathOverflow)?;

        emit!(RepaymentSettled {
            facility: facility.key(),
            borrower: facility.borrower,
            principal_repaid: principal_amount,
            fee_paid: fee_amount,
            remaining_debt: facility.current_drawn,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    /// Emergency pause for vault operations
    pub fn toggle_pause(ctx: Context<AdminOnly>) -> Result<()> {
        let pool = &mut ctx.accounts.pool;
        pool.is_paused = !pool.is_paused;
        emit!(PoolPauseToggled { is_paused: pool.is_paused });
        Ok(())
    }
}

// ----------------------------------------------------------------------------
// ACCOUNTS CONTEXTS
// ----------------------------------------------------------------------------

#[derive(Accounts)]
pub struct InitializePool<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + CreditPool::LEN,
        seeds = [b"pool", mint.key().as_ref()],
        bump
    )]
    pub pool: Account<'info, CreditPool>,

    pub mint: Account<'info, Mint>,

    #[account(
        init,
        payer = admin,
        seeds = [b"vault", pool.key().as_ref()],
        bump,
        token::mint = mint,
        token::authority = pool,
    )]
    pub vault: Account<'info, TokenAccount>,

    /// CHECK: Safe underwriter authority verification
    pub underwriter: UncheckedAccount<'info>,

    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct DepositLiquidity<'info> {
    #[account(
        mut,
        seeds = [b"pool", pool.mint.as_ref()],
        bump = pool.bump
    )]
    pub pool: Account<'info, CreditPool>,

    #[account(
        mut,
        seeds = [b"vault", pool.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        init_if_needed,
        payer = lp_authority,
        space = 8 + LpPosition::LEN,
        seeds = [b"lp_position", pool.key().as_ref(), lp_authority.key().as_ref()],
        bump
    )]
    pub lp_position: Account<'info, LpPosition>,

    #[account(
        mut,
        constraint = lp_token_account.mint == pool.mint,
        constraint = lp_token_account.owner == lp_authority.key()
    )]
    pub lp_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub lp_authority: Signer<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ConfigureFacility<'info> {
    #[account(
        mut,
        seeds = [b"pool", pool.mint.as_ref()],
        bump = pool.bump,
        has_one = underwriter
    )]
    pub pool: Account<'info, CreditPool>,

    #[account(
        init_if_needed,
        payer = underwriter,
        space = 8 + CreditFacility::LEN,
        seeds = [b"facility", pool.key().as_ref(), borrower.key().as_ref()],
        bump
    )]
    pub facility: Account<'info, CreditFacility>,

    /// CHECK: Authorized borrower / MSB identity
    pub borrower: UncheckedAccount<'info>,

    #[account(mut)]
    pub underwriter: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Drawdown<'info> {
    #[account(
        mut,
        seeds = [b"pool", pool.mint.as_ref()],
        bump = pool.bump
    )]
    pub pool: Account<'info, CreditPool>,

    #[account(
        mut,
        seeds = [b"vault", pool.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"facility", pool.key().as_ref(), borrower.key().as_ref()],
        bump = facility.bump,
        has_one = borrower
    )]
    pub facility: Account<'info, CreditFacility>,

    #[account(
        mut,
        constraint = borrower_token_account.mint == pool.mint,
        constraint = borrower_token_account.owner == borrower.key()
    )]
    pub borrower_token_account: Account<'info, TokenAccount>,

    pub borrower: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct Repay<'info> {
    #[account(
        mut,
        seeds = [b"pool", pool.mint.as_ref()],
        bump = pool.bump
    )]
    pub pool: Account<'info, CreditPool>,

    #[account(
        mut,
        seeds = [b"vault", pool.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"facility", pool.key().as_ref(), borrower.key().as_ref()],
        bump = facility.bump,
        has_one = borrower
    )]
    pub facility: Account<'info, CreditFacility>,

    #[account(
        mut,
        constraint = borrower_token_account.mint == pool.mint,
        constraint = borrower_token_account.owner == borrower.key()
    )]
    pub borrower_token_account: Account<'info, TokenAccount>,

    pub borrower: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct AdminOnly<'info> {
    #[account(
        mut,
        seeds = [b"pool", pool.mint.as_ref()],
        bump = pool.bump,
        has_one = admin
    )]
    pub pool: Account<'info, CreditPool>,
    pub admin: Signer<'info>,
}

// ----------------------------------------------------------------------------
// DATA STRUCTURES
// ----------------------------------------------------------------------------

#[account]
pub struct CreditPool {
    pub admin: Pubkey,
    pub underwriter: Pubkey,
    pub mint: Pubkey,
    pub vault: Pubkey,
    pub total_liquidity: u64,
    pub total_borrowed: u64,
    pub base_interest_bps: u16,
    pub is_paused: bool,
    pub bump: u8,
}

impl CreditPool {
    pub const LEN: usize = 32 + 32 + 32 + 32 + 8 + 8 + 2 + 1 + 1;
}

#[account]
pub struct LpPosition {
    pub owner: Pubkey,
    pub pool: Pubkey,
    pub deposited_amount: u64,
    pub last_deposit_timestamp: i64,
    pub bump: u8,
}

impl LpPosition {
    pub const LEN: usize = 32 + 32 + 8 + 8 + 1;
}

#[account]
pub struct CreditFacility {
    pub borrower: Pubkey,
    pub pool: Pubkey,
    pub credit_limit: u64,
    pub current_drawn: u64,
    pub custom_interest_bps: u16,
    pub corridor_id: [u8; 8],
    pub last_drawdown_timestamp: i64,
    pub is_active: bool,
    pub bump: u8,
}

impl CreditFacility {
    pub const LEN: usize = 32 + 32 + 8 + 8 + 2 + 8 + 8 + 1 + 1;
}

// ----------------------------------------------------------------------------
// EVENTS & ERRORS
// ----------------------------------------------------------------------------

#[event]
pub struct PoolInitialized {
    pub pool: Pubkey,
    pub admin: Pubkey,
    pub mint: Pubkey,
    pub base_interest_bps: u16,
}

#[event]
pub struct LiquiditySupplied {
    pub pool: Pubkey,
    pub lp: Pubkey,
    pub amount: u64,
    pub total_pool_liquidity: u64,
}

#[event]
pub struct FacilityConfigured {
    pub facility: Pubkey,
    pub borrower: Pubkey,
    pub credit_limit: u64,
    pub custom_interest_bps: u16,
    pub corridor_id: [u8; 8],
}

#[event]
pub struct DrawdownExecuted {
    pub facility: Pubkey,
    pub borrower: Pubkey,
    pub amount: u64,
    pub current_drawn: u64,
    pub timestamp: i64,
}

#[event]
pub struct RepaymentSettled {
    pub facility: Pubkey,
    pub borrower: Pubkey,
    pub principal_repaid: u64,
    pub fee_paid: u64,
    pub remaining_debt: u64,
    pub timestamp: i64,
}

#[event]
pub struct PoolPauseToggled {
    pub is_paused: bool,
}

#[error_code]
pub enum ArfError {
    #[msg("Pool name cannot exceed 32 bytes")]
    PoolNameTooLong,
    #[msg("Interest rate exceeds 5000 bps ceiling")]
    ExcessiveInterestRate,
    #[msg("Deposit amount must be strictly greater than zero")]
    ZeroDepositAmount,
    #[msg("Drawdown amount must be strictly greater than zero")]
    ZeroDrawdownAmount,
    #[msg("Repayment amount must be strictly greater than zero")]
    ZeroRepayAmount,
    #[msg("Facility is inactive or disabled by underwriter")]
    FacilityDisabled,
    #[msg("Drawdown request exceeds approved credit limit")]
    LimitExceeded,
    #[msg("Pool lacks sufficient idle liquid funds")]
    InsufficientVaultLiquidity,
    #[msg("Repayment exceeds outstanding debt")]
    RepayExceedsDebt,
    #[msg("Unauthorized underwriter signing this transaction")]
    UnauthorizedUnderwriter,
    #[msg("Operation halted: Pool is currently paused")]
    PoolPaused,
    #[msg("Deterministic mathematical overflow encountered")]
    MathOverflow,
}