use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};
use crate::state::{LiquidityPool, CreditLine};
use crate::events::CreditDrawnEvent;
use crate::errors::ArfError;

#[derive(Accounts)]
pub struct DrawCredit<'info> {
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

pub fn handler(ctx: Context<DrawCredit>, amount: u64) -> Result<()> {
    let pool = &mut ctx.accounts.pool;
    let credit_line = &mut ctx.accounts.credit_line;
    let clock = Clock::get()?;

    require!(!pool.is_paused, ArfError::PoolPaused);
    require!(credit_line.is_active, ArfError::CreditLineInactive);
    require!(amount > 0, ArfError::ZeroSettlementAmount);

    // Accrue interest up to current timestamp
    credit_line.accrue_interest(pool.base_apr_bps, clock.unix_timestamp)?;

    let total_obligations = credit_line
        .drawn_principal
        .checked_add(credit_line.accrued_interest)
        .ok_or(ArfError::MathOverflow)?
        .checked_add(amount)
        .ok_or(ArfError::MathOverflow)?;

    require!(
        total_obligations <= credit_line.credit_limit,
        ArfError::CreditLimitExceeded
    );

    require!(
        ctx.accounts.vault.amount >= amount,
        ArfError::InsufficientPoolLiquidity
    );

    // Transfer funds from Vault PDA to borrower
    let pool_key = pool.key();
    let seeds = &[
        b"pool_vault".as_ref(),
        pool_key.as_ref(),
        &[pool.vault_bump],
    ];
    let signer = &[&seeds[..]];

    let cpi_accounts = Transfer {
        from: ctx.accounts.vault.to_account_info(),
        to: ctx.accounts.borrower_token.to_account_info(),
        authority: ctx.accounts.vault.to_account_info(),
    };
    let cpi_program = ctx.accounts.token_program.to_account_info();
    let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer);
    token::transfer(cpi_ctx, amount)?;

    credit_line.drawn_principal = credit_line
        .drawn_principal
        .checked_add(amount)
        .ok_or(ArfError::MathOverflow)?;

    pool.total_borrowed = pool
        .total_borrowed
        .checked_add(amount)
        .ok_or(ArfError::MathOverflow)?;

    emit!(CreditDrawnEvent {
        borrower: ctx.accounts.borrower.key(),
        amount,
        total_drawn: credit_line.drawn_principal,
        accrued_interest: credit_line.accrued_interest,
        destination_account: ctx.accounts.borrower_token.key(),
        timestamp: clock.unix_timestamp,
    });

    Ok(())
}