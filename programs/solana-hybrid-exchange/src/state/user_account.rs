use anchor_lang::prelude::*;

#[account]
#[derive(InitSpace)]
pub struct UserAccount {
    pub owner: Pubkey,
    pub market: Pubkey,
    pub base_free: u64,
    pub quote_free: u64,
    pub bump: u8,
}
