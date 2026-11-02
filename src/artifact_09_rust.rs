use anchor_lang::prelude::*;
use crate::state::{LiquidityPool, CreditLine};
use crate::events::CreditLineOpenedEvent;
use crate::errors::ArfError;

#[derive(Accounts)]
#[instruction(borrower: Pubkey)]
pub struct OpenCreditLine<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        seeds = [b"liquidity_pool", pool.mint.as_ref()],
        bump = pool.bump,
        has_one = admin @ ArfError::Unauthorized
    )]
    pub pool: Account<'info, LiquidityPool>,

    #[account(
        init,
        payer = admin,
        space = CreditLine::LEN,
        seeds = [b"credit_line", pool.key().as_ref(), borrower.as_ref()],
        bump
    )]
    pub credit_line: Account<'info, CreditLine>,

    pub system_program: Program<'info, System>,
}

pub fn handler(
    ctx: Context<OpenCreditLine>,
    borrower: Pubkey,
    credit_limit: u64,
) -> Result<()> {
    require!(credit_limit > 0, ArfError::ZeroSettlementAmount);

    let credit_line = &mut ctx.accounts.credit_line;
    let clock = Clock::get()?;

    credit_line.borrower = borrower;
    credit_line.credit_limit = credit_limit;
    credit_line.drawn_principal = 0;
    credit_line.accrued_interest = 0;
    credit_line.last_interest_timestamp = clock.unix_timestamp;
    credit_line.is_active = true;
    credit_line.bump = ctx.bumps.credit_line;

    emit!(CreditLineOpenedEvent {
        borrower,
        credit_limit,
        approved_by: ctx.accounts.admin.key(),
        timestamp: clock.unix_timestamp,
    });

    Ok(())
}