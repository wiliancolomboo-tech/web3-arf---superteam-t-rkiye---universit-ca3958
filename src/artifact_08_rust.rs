use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};
use crate::state::LiquidityPool;
use crate::events::LiquidityDepositedEvent;
use crate::errors::ArfError;

#[derive(Accounts)]
pub struct DepositLiquidity<'info> {
    #[account(mut)]
    pub provider: Signer<'info>,

    #[account(
        mut,
        seeds = [b"liquidity_pool", mint.key().as_ref()],
        bump = pool.bump,
        has_one = vault,
        has_one = mint,
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
        constraint = provider_token.mint == mint.key(),
        constraint = provider_token.owner == provider.key()
    )]
    pub provider_token: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<DepositLiquidity>, amount: u64) -> Result<()> {
    require!(!ctx.accounts.pool.is_paused, ArfError::PoolPaused);
    require!(amount > 0, ArfError::ZeroSettlementAmount);

    let pool = &mut ctx.accounts.pool;

    let new_total = pool
        .total_deposited
        .checked_add(amount)
        .ok_or(ArfError::MathOverflow)?;

    require!(new_total <= pool.max_pool_capacity, ArfError::CreditLimitExceeded);

    // Transfer USDC tokens from provider to pool vault
    let cpi_accounts = Transfer {
        from: ctx.accounts.provider_token.to_account_info(),
        to: ctx.accounts.vault.to_account_info(),
        authority: ctx.accounts.provider.to_account_info(),
    };
    let cpi_program = ctx.accounts.token_program.to_account_info();
    let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
    token::transfer(cpi_ctx, amount)?;

    pool.total_deposited = new_total;

    emit!(LiquidityDepositedEvent {
        provider: ctx.accounts.provider.key(),
        amount,
        total_pool_liquidity: pool.total_deposited,
        timestamp: Clock::get()?.unix_timestamp,
    });

    Ok(())
}