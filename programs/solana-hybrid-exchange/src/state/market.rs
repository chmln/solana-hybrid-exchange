use anchor_lang::prelude::*;

#[account(zero_copy(unsafe))]
#[repr(C)]
#[derive(Default)]
pub struct Market {
    pub admin: Pubkey,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub base_vault: Pubkey,
    pub quote_vault: Pubkey,
    pub price_scale: u64,
    pub bump: u8,
}
