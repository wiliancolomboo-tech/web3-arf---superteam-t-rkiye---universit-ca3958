use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};
use crate::state::LiquidityPool;
use crate::events::CorridorSettlementEvent;
use crate::errors::ArfError;

#[derive(Accounts)]
pub struct SettleCorridor<'info> {
    #[account(mut)]
    pub sender_msb: Signer<'info>,

    #[account(
        seeds = [b"liquidity_pool", mint.key().as_ref()],
        bump = pool.bump,
        has_one = mint
    )]
    pub pool: Account<'info, LiquidityPool>,

    pub mint: Account<'info, Mint>,

    #[account(
        mut,
        constraint = sender_token.mint == mint.key(),
        constraint = sender_token.owner == sender_msb.key()
    )]
    pub sender_token: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = receiver_token.mint == mint.key()
    )]
    pub receiver_token: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"pool_vault", pool.key().as_ref()],
        bump = pool.vault_bump
    )]
    pub fee_vault: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(
    ctx: Context<SettleCorridor>,
    corridor_id: [u8; 16],
    amount: u64,
    fee_bps: u16,
) -> Result<()> {
    require!(amount > 0, ArfError::ZeroSettlementAmount);
    require!(fee_bps <= 500, ArfError::InvalidInterestRate); // Fee cap: max 5%

    // Calculate corridor routing fee
    let fee_amount = (amount as u128)
        .checked_mul(fee_bps as u128)
        .ok_or(ArfError::MathOverflow)?
        .checked_div(10_000)
        .ok_or(ArfError::MathOverflow)? as u64;

    let net_settlement = amount
        .checked_sub(fee_amount)
        .ok_or(ArfError::MathOverflow)?;

    // 1. Transfer net funds from Sender MSB directly to Destination MSB payout account
    let cpi_net = Transfer {
        from: ctx.accounts.sender_token.to_account_info(),
        to: ctx.accounts.receiver_token.to_account_info(),
        authority: ctx.accounts.sender_msb.to_account_info(),
    };
    let cpi_program = ctx.accounts.token_program.to_account_info();
    token::transfer(CpiContext::new(cpi_program.clone(), cpi_net), net_settlement)?;

    // 2. Transfer fee into the Liquidity Pool Vault
    if fee_amount > 0 {
        let cpi_fee = Transfer {
            from: ctx.accounts.sender_token.to_account_info(),
            to: ctx.accounts.fee_vault.to_account_info(),
            authority: ctx.accounts.sender_msb.to_account_info(),
        };
        token::transfer(CpiContext::new(cpi_program, cpi_fee), fee_amount)?;
    }

    emit!(CorridorSettlementEvent {
        corridor_id,
        sender_msb: ctx.accounts.sender_msb.key(),
        receiver_msb: ctx.accounts.receiver_token.owner,
        settlement_amount: amount,
        fee_collected: fee_amount,
        timestamp: Clock::get()?.unix_timestamp,
    });

    Ok(())
}