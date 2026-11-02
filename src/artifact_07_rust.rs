use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};
use crate::state::LiquidityPool;
use crate::events::PoolInitializedEvent;
use crate::errors::ArfError;

#[derive(Accounts)]
pub struct InitializePool<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    pub mint: Account<'info, Mint>,

    #[account(
        init,
        payer = admin,
        space = LiquidityPool::LEN,
        seeds = [b"liquidity_pool", mint.key().as_ref()],
        bump
    )]
    pub pool: Account<'info, LiquidityPool>,

    #[account(
        init,
        payer = admin,
        seeds = [b"pool_vault", pool.key().as_ref()],
        bump,
        token::mint = mint,
        token::authority = pool
    )]
    pub vault: Account<'info, TokenAccount>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

pub fn handler(
    ctx: Context<InitializePool>,
    base_apr_bps: u16,
    max_pool_capacity: u64,
) -> Result<()> {
    require!(base_apr_bps <= 5000, ArfError::InvalidInterestRate); // Max 50.00% APR guardrail

    let pool = &mut ctx.accounts.pool;
    pool.admin = ctx.accounts.admin.key();
    pool.mint = ctx.accounts.mint.key();
    pool.vault = ctx.accounts.vault.key();
    pool.base_apr_bps = base_apr_bps;
    pool.max_pool_capacity = max_pool_capacity;
    pool.total_deposited = 0;
    pool.total_borrowed = 0;
    pool.is_paused = false;
    pool.bump = ctx.bumps.pool;
    pool.vault_bump = ctx.bumps.vault;

    emit!(PoolInitializedEvent {
        admin: pool.admin,
        pool: pool.key(),
        base_apr_bps,
        max_pool_capacity,
        timestamp: Clock::get()?.unix_timestamp,
    });

    Ok(())
}