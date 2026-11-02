use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};
use crate::state::{LiquidityPool, CreditLine};
use crate::events::CreditRepaidEvent;
use crate::errors::ArfError;

#[derive(Accounts)]
pub struct RepayCredit<'info> {
    #[account(mut)]
    pub borrower: Signer<'info>,

    #[account(
        mut,
        seeds = [b"liquidity_pool", mint.key().as_ref()],
        bump = pool.bump,
        has_one = vault,
        has_one = mint
    )]
    pub pool: Account<'info, LiquidityPool>,

    pub mint: Account<'info, Mint>,

    #[account(
        mut,
        seeds = [b"pool_vault", pool.key().as_ref()],
        bump = pool.vault_bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"credit_line", pool.key().as_ref(), borrower.key().as_ref()],
        bump = credit_line.bump,
        has_one = borrower @ ArfError::Unauthorized
    )]
    pub credit_line: Account<'info, CreditLine>,

    #[account(
        mut,
        constraint = borrower_token.mint == mint.key(),
        constraint = borrower_token.owner == borrower.key()
    )]
    pub borrower_token: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<RepayCredit>, amount: u64) -> Result<()> {
    require!(amount > 0, ArfError::ZeroSettlementAmount);

    let pool = &mut ctx.accounts.pool;
    let credit_line = &mut ctx.accounts.credit_line;
    let clock = Clock::get()?;

    credit_line.accrue_interest(pool.base_apr_bps, clock.unix_timestamp)?;

    let total_debt = credit_line
        .drawn_principal
        .checked_add(credit_line.accrued_interest)
        .ok_or(ArfError::MathOverflow)?;

    require!(amount <= total_debt, ArfError::RepaymentExceedsDebt);

    // Repay interest first, remainder goes to principal
    let interest_portion = std::cmp::min(amount, credit_line.accrued_interest);
    let principal_portion = amount
        .checked_sub(interest_portion)
        .ok_or(ArfError::MathOverflow)?;

    // Transfer repayment tokens into pool vault
    let cpi_accounts = Transfer {
        from: ctx.accounts.borrower_token.to_account_info(),
        to: ctx.accounts.vault.to_account_info(),
        authority: ctx.accounts.borrower.to_account_info(),
    };
    let cpi_program = ctx.accounts.token_program.to_account_info();
    let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
    token::transfer(cpi_ctx, amount)?;

    credit_line.accrued_interest = credit_line
        .accrued_interest
        .checked_sub(interest_portion)
        .ok_or(ArfError::MathOverflow)?;

    credit_line.drawn_principal = credit_line
        .drawn_principal
        .checked_sub(principal_portion)
        .ok_or(ArfError::MathOverflow)?;

    pool.total_borrowed = pool
        .total_borrowed
        .checked_sub(principal_portion)
        .ok_or(ArfError::MathOverflow)?;

    let remaining_debt = credit_line
        .drawn_principal
        .checked_add(credit_line.accrued_interest)
        .ok_or(ArfError::MathOverflow)?;

    emit!(CreditRepaidEvent {
        borrower: ctx.accounts.borrower.key(),
        principal_repaid: principal_portion,
        interest_repaid: interest_portion,
        remaining_debt,
        timestamp: clock.unix_timestamp,
    });

    Ok(())
}