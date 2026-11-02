use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};
use crate::state::{CreditFacility, LiquidityPool};
use crate::errors::ClearStreamError;
use crate::events::FacilityRepaymentEvent;

#[derive(Accounts)]
pub struct RepayCredit<'info> {
    #[account(
        mut,
        seeds = [b"credit_facility", pool.key().as_ref(), borrower.key().as_ref()],
        bump = facility.bump,
        has_one = pool,
        has_one = borrower,
    )]
    pub facility: Account<'info, CreditFacility>,

    #[account(
        mut,
        seeds = [b"liquidity_pool", pool.name.as_bytes()],
        bump = pool.bump,
        has_one = vault,
    )]
    pub pool: Account<'info, LiquidityPool>,

    #[account(mut)]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = payer_token_account.mint == pool.mint
    )]
    pub payer_token_account: Account<'info, TokenAccount>,

    pub borrower: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<RepayCredit>, amount: u64) -> Result<()> {
    require!(amount > 0, ClearStreamError::ZeroRepaymentAmount);
    let now = Clock::get()?.unix_timestamp;
    let facility = &mut ctx.accounts.facility;

    facility.update_interest(now)?;

    let mut remaining_payment = amount;

    // Waterfall: Pay accrued interest first, then principal
    if facility.accrued_interest > 0 {
        if remaining_payment >= facility.accrued_interest {
            remaining_payment = remaining_payment
                .checked_sub(facility.accrued_interest)
                .ok_or(ClearStreamError::MathOverflow)?;
            facility.accrued_interest = 0;
        } else {
            facility.accrued_interest = facility
                .accrued_interest
                .checked_sub(remaining_payment)
                .ok_or(ClearStreamError::MathOverflow)?;
            remaining_payment = 0;
        }
    }

    if remaining_payment > 0 {
        if remaining_payment >= facility.outstanding_principal {
            let principal_paid = facility.outstanding_principal;
            facility.outstanding_principal = 0;
            remaining_payment = remaining_payment
                .checked_sub(principal_paid)
                .ok_or(ClearStreamError::MathOverflow)?;
            
            let pool = &mut ctx.accounts.pool;
            pool.total_utilized = pool
                .total_utilized
                .saturating_sub(principal_paid);
        } else {
            facility.outstanding_principal = facility
                .outstanding_principal
                .checked_sub(remaining_payment)
                .ok_or(ClearStreamError::MathOverflow)?;
            
            let pool = &mut ctx.accounts.pool;
            pool.total_utilized = pool
                .total_utilized
                .saturating_sub(remaining_payment);
            remaining_payment = 0;
        }
    }

    // Execute transfer from payer to vault
    let cpi_accounts = Transfer {
        from: ctx.accounts.payer_token_account.to_account_info(),
        to: ctx.accounts.vault.to_account_info(),
        authority: ctx.accounts.borrower.to_account_info(),
    };
    let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
    token::transfer(cpi_ctx, amount)?;

    emit!(FacilityRepaymentEvent {
        facility: facility.key(),
        borrower: facility.borrower,
        amount_paid: amount,
        remaining_principal: facility.outstanding_principal,
        remaining_interest: facility.accrued_interest,
        timestamp: now,
    });

    Ok(())
}