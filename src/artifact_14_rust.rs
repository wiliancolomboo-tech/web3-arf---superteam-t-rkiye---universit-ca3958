use anchor_lang::prelude::*;

pub mod errors;
pub mod events;
pub mod instructions;
pub mod state;

use instructions::*;

declare_id!("ARFXsUpErTeAm111111111111111111111111111111");

#[program]
pub mod arf_settlement {
    use super::*;

    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        base_apr_bps: u16,
        max_pool_capacity: u64,
    ) -> Result<()> {
        instructions::initialize_pool::handler(ctx, base_apr_bps, max_pool_capacity)
    }

    pub fn deposit_liquidity(ctx: Context<DepositLiquidity>, amount: u64) -> Result<()> {
        instructions::deposit_liquidity::handler(ctx, amount)
    }

    pub fn open_credit_line(
        ctx: Context<OpenCreditLine>,
        borrower: Pubkey,
        credit_limit: u64,
    ) -> Result<()> {
        instructions::open_credit_line::handler(ctx, borrower, credit_limit)
    }

    pub fn draw_credit(ctx: Context<DrawCredit>, amount: u64) -> Result<()> {
        instructions::draw_credit::handler(ctx, amount)
    }

    pub fn repay_credit(ctx: Context<RepayCredit>, amount: u64) -> Result<()> {
        instructions::repay_credit::handler(ctx, amount)
    }

    pub fn settle_corridor(
        ctx: Context<SettleCorridor>,
        corridor_id: [u8; 16],
        amount: u64,
        fee_bps: u16,
    ) -> Result<()> {
        instructions::settle_corridor::handler(ctx, corridor_id, amount, fee_bps)
    }
}