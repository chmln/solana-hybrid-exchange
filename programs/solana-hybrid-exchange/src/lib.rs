use anchor_lang::prelude::*;

pub mod constants;
pub mod error;
pub mod instructions;
pub mod order;
pub mod state;

pub use crate::instructions::*;

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
}
