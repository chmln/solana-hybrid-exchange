use anchor_lang::prelude::*;

#[account]
#[derive(InitSpace)]
pub struct OrderMarker {
    pub filled_size: u64,
    pub bump: u8,
}
