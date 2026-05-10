use anchor_lang::prelude::*;

#[account(zero_copy(unsafe))]
#[repr(C)]
#[derive(Default)]
pub struct OrderMarker {
    pub filled_size: u64,
    pub bump: u8,
}
