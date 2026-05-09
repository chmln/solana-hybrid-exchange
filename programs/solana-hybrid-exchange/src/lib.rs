use anchor_lang::prelude::*;

pub mod constants;
pub mod error;
pub mod instructions;
pub mod order;
pub mod state;

pub use crate::instructions::*;
use crate::order::SignedOrderArgs;

declare_id!("EjJwgDWLSeFmSf6T1MA8qVDCXSnH2qT4NVMugnb21Vzc");

#[program]
pub mod solana_hybrid_exchange {
    use super::*;

    pub fn init_market(ctx: Context<InitMarket>) -> Result<()> {
        instructions::init_market::handler(ctx)
    }

    pub fn deposit(ctx: Context<Deposit>, amount: u64) -> Result<()> {
        instructions::deposit::handler(ctx, amount)
    }

    pub fn withdraw(ctx: Context<Withdraw>, amount: u64) -> Result<()> {
        instructions::withdraw::handler(ctx, amount)
    }

    pub fn settle(
        ctx: Context<Settle>,
        maker: SignedOrderArgs,
        taker: SignedOrderArgs,
        fill_price: u64,
        fill_size: u64,
        maker_order_hash: [u8; 32],
        taker_order_hash: [u8; 32],
    ) -> Result<()> {
        instructions::settle::handler(
            ctx,
            maker,
            taker,
            fill_price,
            fill_size,
            maker_order_hash,
            taker_order_hash,
        )
    }
}
