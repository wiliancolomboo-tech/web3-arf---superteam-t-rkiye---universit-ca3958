use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("ArfSVE1111111111111111111111111111111111111");

pub const SECONDS_PER_YEAR: u64 = 31_536_000;
pub const BPS_DIVISOR: u128 = 10_000;
pub const MAX_BPS: u16 = 2_500; // Max 25% APR

#[program]
pub mod arf_sve {
    use super::*;

    pub fn initialize_corridor_protocol(
        ctx: Context<InitializeCorridorProtocol>,
        admin_fee_bps: u16,
    ) -> Result<()> {
        require!(admin_fee_bps <= 500, ArfError::FeeExceedsCap); // Max 5% protocol fee
        let config = &mut ctx.accounts.config;
        config.admin = ctx.accounts.admin.key();
        config.treasury_mint = ctx.accounts.treasury_mint.key();
        config.treasury_vault = ctx.accounts.treasury_vault.key();
        config.admin_fee_bps = admin_fee_bps;
        config.total_credit_allocated = 0;
        config.total_credit_drawn = 0;
        config.is_paused = false;
        config.bump = ctx.bumps.config;
        config.vault_bump = ctx.bumps.treasury_vault;

        emit!(ProtocolInitializedEvent {
            admin: config.admin,
            treasury_mint: config.treasury_mint,
            admin_fee_bps,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    pub fn open_credit_facility(
        ctx: Context<OpenCreditFacility>,
        credit_limit: u64,
        apr_bps: u16,
        corridor_id: [u8; 16],
    ) -> Result<()> {
        require!(credit_limit > 0, ArfError::ZeroCreditLimit);
        require!(apr_bps <= MAX_BPS, ArfError::InterestRateExceedsCap);

        let facility = &mut ctx.accounts.facility;
        let clock = Clock::get()?;

        facility.borrower = ctx.accounts.borrower.key();
        facility.corridor_id = corridor_id;
        facility.credit_limit = credit_limit;
        facility.current_principal = 0;
        facility.accrued_interest = 0;
        facility.last_accrual_timestamp = clock.unix_timestamp;
        facility.apr_bps = apr_bps;
        facility.is_active = true;
        facility.bump = ctx.bumps.facility;

        let config = &mut ctx.accounts.config;
        config.total_credit_allocated = config
            .total_credit_allocated
            .checked_add(credit_limit)
            .ok_or(ArfError::MathOverflow)?;

        emit!(CreditFacilityOpenedEvent {
            borrower: facility.borrower,
            corridor_id,
            credit_limit,
            apr_bps,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    pub fn draw_down_and_settle(
        ctx: Context<DrawDownAndSettle>,
        amount: u64,
        external_sequence_id: [u8; 32],
    ) -> Result<()> {
        require!(!ctx.accounts.config.is_paused, ArfError::ProtocolPaused);
        require!(amount > 0, ArfError::InvalidAmount);

        let clock = Clock::get()?;
        let facility = &mut ctx.accounts.facility;
        require!(facility.is_active, ArfError::FacilityInactive);

        // 1. Calculate continuous linear interest accrual
        let elapsed_seconds = (clock.unix_timestamp)
            .checked_sub(facility.last_accrual_timestamp)
            .ok_or(ArfError::InvalidTimestamp)?;

        if elapsed_seconds > 0 && facility.current_principal > 0 {
            let incremental_interest = (facility.current_principal as u128)
                .checked_mul(facility.apr_bps as u128)
                .ok_or(ArfError::MathOverflow)?
                .checked_mul(elapsed_seconds as u128)
                .ok_or(ArfError::MathOverflow)?
                .checked_div(BPS_DIVISOR)
                .ok_or(ArfError::MathOverflow)?
                .checked_div(SECONDS_PER_YEAR as u128)
                .ok_or(ArfError::MathOverflow)? as u64;

            facility.accrued_interest = facility
                .accrued_interest
                .checked_add(incremental_interest)
                .ok_or(ArfError::MathOverflow)?;
        }
        facility.last_accrual_timestamp = clock.unix_timestamp;

        // 2. Validate Available Line of Credit Capacity
        let total_owed = facility
            .current_principal
            .checked_add(facility.accrued_interest)
            .ok_or(ArfError::MathOverflow)?;

        let prospective_obligation = total_owed
            .checked_add(amount)
            .ok_or(ArfError::MathOverflow)?;

        require!(
            prospective_obligation <= facility.credit_limit,
            ArfError::CreditLimitExceeded
        );

        facility.current_principal = facility
            .current_principal
            .checked_add(amount)
            .ok_or(ArfError::MathOverflow)?;

        let config = &mut ctx.accounts.config;
        config.total_credit_drawn = config
            .total_credit_drawn
            .checked_add(amount)
            .ok_or(ArfError::MathOverflow)?;

        // 3. Execute CPI Transfer from Protocol Treasury Vault to Destination Recipient
        let seeds = &[
            b"treasury_vault".as_ref(),
            config.to_account_info().key.as_ref(),
            &[config.vault_bump],
        ];
        let signer = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.treasury_vault.to_account_info(),
            to: ctx.accounts.recipient_token_account.to_account_info(),
            authority: ctx.accounts.treasury_vault.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer,
        );
        token::transfer(cpi_ctx, amount)?;

        // 4. Populate Settlement Audit Record PDA
        let record = &mut ctx.accounts.settlement_record;
        record.facility = facility.key();
        record.amount = amount;
        record.recipient = ctx.accounts.recipient_token_account.key();
        record.sequence_id = external_sequence_id;
        record.timestamp = clock.unix_timestamp;
        record.bump = ctx.bumps.settlement_record;

        emit!(SettlementExecutedEvent {
            borrower: facility.borrower,
            facility: facility.key(),
            recipient: record.recipient,
            amount,
            sequence_id: external_sequence_id,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    pub fn repay_credit_facility(ctx: Context<RepayCreditFacility>, amount: u64) -> Result<()> {
        require!(amount > 0, ArfError::InvalidAmount);
        let clock = Clock::get()?;
        let facility = &mut ctx.accounts.facility;

        // 1. Accrue remaining interest to this exact second
        let elapsed_seconds = (clock.unix_timestamp)
            .checked_sub(facility.last_accrual_timestamp)
            .ok_or(ArfError::InvalidTimestamp)?;

        if elapsed_seconds > 0 && facility.current_principal > 0 {
            let incremental_interest = (facility.current_principal as u128)
                .checked_mul(facility.apr_bps as u128)
                .ok_or(ArfError::MathOverflow)?
                .checked_mul(elapsed_seconds as u128)
                .ok_or(ArfError::MathOverflow)?
                .checked_div(BPS_DIVISOR)
                .ok_or(ArfError::MathOverflow)?
                .checked_div(SECONDS_PER_YEAR as u128)
                .ok_or(ArfError::MathOverflow)? as u64;

            facility.accrued_interest = facility
                .accrued_interest
                .checked_add(incremental_interest)
                .ok_or(ArfError::MathOverflow)?;
        }
        facility.last_accrual_timestamp = clock.unix_timestamp;

        // 2. Cascade repayment: First against accrued interest, second against principal
        let mut remainder = amount;
        let mut interest_repaid = 0u64;
        let mut principal_repaid = 0u64;

        if facility.accrued_interest > 0 {
            if remainder >= facility.accrued_interest {
                interest_repaid = facility.accrued_interest;
                remainder = remainder
                    .checked_sub(facility.accrued_interest)
                    .ok_or(ArfError::MathOverflow)?;
                facility.accrued_interest = 0;
            } else {
                interest_repaid = remainder;
                facility.accrued_interest = facility
                    .accrued_interest
                    .checked_sub(remainder)
                    .ok_or(ArfError::MathOverflow)?;
                remainder = 0;
            }
        }

        if remainder > 0 {
            if remainder >= facility.current_principal {
                principal_repaid = facility.current_principal;
                facility.current_principal = 0;
            } else {
                principal_repaid = remainder;
                facility.current_principal = facility
                    .current_principal
                    .checked_sub(remainder)
                    .ok_or(ArfError::MathOverflow)?;
            }
        }

        let actual_total_repayment = interest_repaid
            .checked_add(principal_repaid)
            .ok_or(ArfError::MathOverflow)?;

        require!(actual_total_repayment > 0, ArfError::NoDebtToRepay);

        // Update protocol metrics
        let config = &mut ctx.accounts.config;
        config.total_credit_drawn = config
            .total_credit_drawn
            .saturating_sub(principal_repaid);

        // 3. Transfer repayment funds from payer to protocol treasury vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.payer_token_account.to_account_info(),
            to: ctx.accounts.treasury_vault.to_account_info(),
            authority: ctx.accounts.payer.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
        );
        token::transfer(cpi_ctx, actual_total_repayment)?;

        emit!(FacilityRepaidEvent {
            facility: facility.key(),
            borrower: facility.borrower,
            interest_paid: interest_repaid,
            principal_paid: principal_repaid,
            remaining_principal: facility.current_principal,
            remaining_interest: facility.accrued_interest,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    pub fn set_protocol_pause(ctx: Context<AdminOnly>, pause: bool) -> Result<()> {
        let config = &mut ctx.accounts.config;
        config.is_paused = pause;
        emit!(ProtocolPauseUpdatedEvent {
            is_paused: pause,
            admin: ctx.accounts.admin.key(),
            timestamp: Clock::get()?.unix_timestamp,
        });
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// ACCOUNT VALIDATION CONTEXTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
pub struct InitializeCorridorProtocol<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + ProtocolConfig::LEN,
        seeds = [b"protocol_config"],
        bump
    )]
    pub config: Account<'info, ProtocolConfig>,

    pub treasury_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = admin,
        seeds = [b"treasury_vault", config.key().as_ref()],
        bump,
        token::mint = treasury_mint,
        token::authority = treasury_vault
    )]
    pub treasury_vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub admin: Signer<'info>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
#[instruction(credit_limit: u64, apr_bps: u16, corridor_id: [u8; 16])]
pub struct OpenCreditFacility<'info> {
    #[account(
        mut,
        seeds = [b"protocol_config"],
        bump = config.bump,
        has_one = admin @ ArfError::UnauthorizedAdmin
    )]
    pub config: Account<'info, ProtocolConfig>,

    /// CHECK: The institutional recipient/borrower authorized by admin
    pub borrower: AccountInfo<'info>,

    #[account(
        init,
        payer = admin,
        space = 8 + CreditFacility::LEN,
        seeds = [b"facility", config.key().as_ref(), borrower.key().as_ref(), &corridor_id],
        bump
    )]
    pub facility: Account<'info, CreditFacility>,

    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(amount: u64, external_sequence_id: [u8; 32])]
pub struct DrawDownAndSettle<'info> {
    #[account(
        mut,
        seeds = [b"protocol_config"],
        bump = config.bump
    )]
    pub config: Account<'info, ProtocolConfig>,

    #[account(
        mut,
        seeds = [b"facility", config.key().as_ref(), borrower.key().as_ref(), &facility.corridor_id],
        bump = facility.bump,
        has_one = borrower @ ArfError::UnauthorizedBorrower
    )]
    pub facility: Account<'info, CreditFacility>,

    #[account(
        mut,
        seeds = [b"treasury_vault", config.key().as_ref()],
        bump = config.vault_bump
    )]
    pub treasury_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = recipient_token_account.mint == config.treasury_mint @ ArfError::InvalidMint
    )]
    pub recipient_token_account: Account<'info, TokenAccount>,

    #[account(
        init,
        payer = borrower,
        space = 8 + SettlementRecord::LEN,
        seeds = [b"settlement_record", facility.key().as_ref(), &external_sequence_id],
        bump
    )]
    pub settlement_record: Account<'info, SettlementRecord>,

    #[account(mut)]
    pub borrower: Signer<'info>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct RepayCreditFacility<'info> {
    #[account(
        mut,
        seeds = [b"protocol_config"],
        bump = config.bump
    )]
    pub config: Account<'info, ProtocolConfig>,

    #[account(
        mut,
        seeds = [b"facility", config.key().as_ref(), facility.borrower.key().as_ref(), &facility.corridor_id],
        bump = facility.bump
    )]
    pub facility: Account<'info, CreditFacility>,

    #[account(
        mut,
        seeds = [b"treasury_vault", config.key().as_ref()],
        bump = config.vault_bump
    )]
    pub treasury_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = payer_token_account.mint == config.treasury_mint @ ArfError::InvalidMint
    )]
    pub payer_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub payer: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct AdminOnly<'info> {
    #[account(
        mut,
        seeds = [b"protocol_config"],
        bump = config.bump,
        has_one = admin @ ArfError::UnauthorizedAdmin
    )]
    pub config: Account<'info, ProtocolConfig>,
    pub admin: Signer<'info>,
}

// -----------------------------------------------------------------------------
// STATE SCHEMAS
// -----------------------------------------------------------------------------

#[account]
pub struct ProtocolConfig {
    pub admin: Pubkey,                // 32
    pub treasury_mint: Pubkey,        // 32
    pub treasury_vault: Pubkey,       // 32
    pub total_credit_allocated: u64,  // 8
    pub total_credit_drawn: u64,      // 8
    pub admin_fee_bps: u16,           // 2
    pub is_paused: bool,              // 1
    pub bump: u8,                     // 1
    pub vault_bump: u8,               // 1
}

impl ProtocolConfig {
    pub const LEN: usize = 32 + 32 + 32 + 8 + 8 + 2 + 1 + 1 + 1;
}

#[account]
pub struct CreditFacility {
    pub borrower: Pubkey,             // 32
    pub corridor_id: [u8; 16],        // 16
    pub credit_limit: u64,            // 8
    pub current_principal: u64,       // 8
    pub accrued_interest: u64,        // 8
    pub last_accrual_timestamp: i64,  // 8
    pub apr_bps: u16,                 // 2
    pub is_active: bool,              // 1
    pub bump: u8,                     // 1
}

impl CreditFacility {
    pub const LEN: usize = 32 + 16 + 8 + 8 + 8 + 8 + 2 + 1 + 1;
}

#[account]
pub struct SettlementRecord {
    pub facility: Pubkey,             // 32
    pub recipient: Pubkey,            // 32
    pub amount: u64,                  // 8
    pub sequence_id: [u8; 32],        // 32
    pub timestamp: i64,               // 8
    pub bump: u8,                     // 1
}

impl SettlementRecord {
    pub const LEN: usize = 32 + 32 + 8 + 32 + 8 + 1;
}

// -----------------------------------------------------------------------------
// EVENTS & ERRORS
// -----------------------------------------------------------------------------

#[event]
pub struct ProtocolInitializedEvent {
    pub admin: Pubkey,
    pub treasury_mint: Pubkey,
    pub admin_fee_bps: u16,
    pub timestamp: i64,
}

#[event]
pub struct CreditFacilityOpenedEvent {
    pub borrower: Pubkey,
    pub corridor_id: [u8; 16],
    pub credit_limit: u64,
    pub apr_bps: u16,
    pub timestamp: i64,
}

#[event]
pub struct SettlementExecutedEvent {
    pub borrower: Pubkey,
    pub facility: Pubkey,
    pub recipient: Pubkey,
    pub amount: u64,
    pub sequence_id: [u8; 32],
    pub timestamp: i64,
}

#[event]
pub struct FacilityRepaidEvent {
    pub facility: Pubkey,
    pub borrower: Pubkey,
    pub interest_paid: u64,
    pub principal_paid: u64,
    pub remaining_principal: u64,
    pub remaining_interest: u64,
    pub timestamp: i64,
}

#[event]
pub struct ProtocolPauseUpdatedEvent {
    pub is_paused: bool,
    pub admin: Pubkey,
    pub timestamp: i64,
}

#[error_code]
pub enum ArfError {
    #[msg("Administrative fee exceeds maximum protocol cap.")]
    FeeExceedsCap,
    #[msg("Credit limit must be strictly greater than zero.")]
    ZeroCreditLimit,
    #[msg("Configured interest rate APR exceeds maximum ceiling.")]
    InterestRateExceedsCap,
    #[msg("Protocol is currently paused by risk admins.")]
    ProtocolPaused,
    #[msg("Requested settlement amount is invalid.")]
    InvalidAmount,
    #[msg("Credit facility is inactive or suspended.")]
    FacilityInactive,
    #[msg("Arithmetic overflow occurred during settlement computation.")]
    MathOverflow,
    #[msg("Settlement request exceeds authorized dynamic credit ceiling.")]
    CreditLimitExceeded,
    #[msg("Supplied token mint does not match treasury corridor specification.")]
    InvalidMint,
    #[msg("Signer is not authorized as protocol admin.")]
    UnauthorizedAdmin,
    #[msg("Signer is not authorized borrower for this facility.")]
    UnauthorizedBorrower,
    #[msg("Invalid timestamp delta calculation.")]
    InvalidTimestamp,
    #[msg("No principal or interest outstanding to repay.")]
    NoDebtToRepay,
}