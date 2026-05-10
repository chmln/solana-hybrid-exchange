use anchor_lang::prelude::*;
use solana_instructions_sysvar::load_instruction_at_checked;

use crate::constants::{MARKET_SEED, ORDER_MARKER_SEED, USER_ACCOUNT_SEED};
use crate::error::ExchangeError;
use crate::order::{
    canonical_serialize, parse_ed25519_precompile_ix, Side, SignedOrderArgs, ED25519_PROGRAM_ID,
    IX_SYSVAR_ID,
};
use crate::state::{Market, OrderMarker, UserAccount};

#[derive(Accounts)]
#[instruction(
    maker: SignedOrderArgs,
    taker: SignedOrderArgs,
    fill_price: u64,
    fill_size: u64,
    maker_order_hash: [u8; 32],
    taker_order_hash: [u8; 32],
)]
pub struct Settle<'info> {
    #[account(mut)]
    pub operator: Signer<'info>,

    #[account(
        seeds = [MARKET_SEED, market.load()?.base_mint.as_ref(), market.load()?.quote_mint.as_ref()],
        bump = market.load()?.bump,
    )]
    pub market: AccountLoader<'info, Market>,

    #[account(
        mut,
        seeds = [USER_ACCOUNT_SEED, market.key().as_ref(), maker_user_account.load()?.owner.as_ref()],
        bump = maker_user_account.load()?.bump,
        constraint = maker_user_account.load()?.market == market.key(),
    )]
    pub maker_user_account: AccountLoader<'info, UserAccount>,

    #[account(
        mut,
        seeds = [USER_ACCOUNT_SEED, market.key().as_ref(), taker_user_account.load()?.owner.as_ref()],
        bump = taker_user_account.load()?.bump,
        constraint = taker_user_account.load()?.market == market.key(),
    )]
    pub taker_user_account: AccountLoader<'info, UserAccount>,

    #[account(
        init_if_needed,
        payer = operator,
        space = 8 + std::mem::size_of::<OrderMarker>(),
        seeds = [ORDER_MARKER_SEED, maker_order_hash.as_ref()],
        bump,
    )]
    pub maker_order_marker: AccountLoader<'info, OrderMarker>,

    #[account(
        init_if_needed,
        payer = operator,
        space = 8 + std::mem::size_of::<OrderMarker>(),
        seeds = [ORDER_MARKER_SEED, taker_order_hash.as_ref()],
        bump,
    )]
    pub taker_order_marker: AccountLoader<'info, OrderMarker>,

    /// CHECK: address-pinned to the Instructions sysvar; used for Ed25519 precompile verification.
    #[account(address = IX_SYSVAR_ID)]
    pub instructions_sysvar: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

pub(crate) fn handler(
    ctx: Context<Settle>,
    maker: SignedOrderArgs,
    taker: SignedOrderArgs,
    fill_price: u64,
    fill_size: u64,
    maker_order_hash: [u8; 32],
    taker_order_hash: [u8; 32],
) -> Result<()> {
    let maker_bytes = canonical_serialize(&maker);
    let taker_bytes = canonical_serialize(&taker);
    let expected_maker_hash = solana_sha256_hasher::hashv(&[&maker_bytes]);
    let expected_taker_hash = solana_sha256_hasher::hashv(&[&taker_bytes]);
    require!(
        expected_maker_hash.to_bytes() == maker_order_hash,
        ExchangeError::Ed25519DataMismatch
    );
    require!(
        expected_taker_hash.to_bytes() == taker_order_hash,
        ExchangeError::Ed25519DataMismatch
    );

    let sysvar_ai = ctx.accounts.instructions_sysvar.to_account_info();
    verify_precompile(&sysvar_ai, 0, &maker, &maker_bytes)?;
    verify_precompile(&sysvar_ai, 1, &taker, &taker_bytes)?;

    let market_key = ctx.accounts.market.key();
    require_keys_eq!(maker.market, market_key, ExchangeError::WrongMarket);
    require_keys_eq!(taker.market, market_key, ExchangeError::WrongMarket);

    require!(maker.side != taker.side, ExchangeError::SameSide);
    require!(fill_size > 0, ExchangeError::ZeroFillSize);

    require_keys_eq!(
        ctx.accounts.maker_user_account.load()?.owner,
        maker.user,
        ExchangeError::Ed25519DataMismatch
    );
    require_keys_eq!(
        ctx.accounts.taker_user_account.load()?.owner,
        taker.user,
        ExchangeError::Ed25519DataMismatch
    );

    let (bid, ask, bid_is_maker) = if maker.side == Side::Bid {
        (&maker, &taker, true)
    } else {
        (&taker, &maker, false)
    };

    require!(
        bid.limit_price >= ask.limit_price,
        ExchangeError::PriceDoesNotCross
    );
    require!(
        fill_price >= ask.limit_price && fill_price <= bid.limit_price,
        ExchangeError::FillPriceOutOfRange
    );

    let now = Clock::get()?.unix_timestamp;
    let earliest_expiry = maker.expiry.min(taker.expiry);
    require!(now < earliest_expiry, ExchangeError::OrderExpired);

    {
        let mut maker_marker = ctx
            .accounts
            .maker_order_marker
            .load_init()
            .or_else(|_| ctx.accounts.maker_order_marker.load_mut())?;
        update_marker(
            &mut maker_marker,
            ctx.bumps.maker_order_marker,
            fill_size,
            maker.max_size,
        )?;
    }
    {
        let mut taker_marker = ctx
            .accounts
            .taker_order_marker
            .load_init()
            .or_else(|_| ctx.accounts.taker_order_marker.load_mut())?;
        update_marker(
            &mut taker_marker,
            ctx.bumps.taker_order_marker,
            fill_size,
            taker.max_size,
        )?;
    }

    let price_scale = ctx.accounts.market.load()?.price_scale;
    let quote_amount_u128 = (fill_price as u128)
        .checked_mul(fill_size as u128)
        .ok_or(ExchangeError::Overflow)?
        / (price_scale as u128);
    let quote_amount: u64 = quote_amount_u128
        .try_into()
        .map_err(|_| error!(ExchangeError::Overflow))?;

    let mut maker_ua = ctx.accounts.maker_user_account.load_mut()?;
    let mut taker_ua = ctx.accounts.taker_user_account.load_mut()?;

    let (buyer, seller) = if bid_is_maker {
        (&mut *maker_ua, &mut *taker_ua)
    } else {
        (&mut *taker_ua, &mut *maker_ua)
    };

    buyer.quote_free = buyer
        .quote_free
        .checked_sub(quote_amount)
        .ok_or(ExchangeError::InsufficientFreeBalance)?;
    buyer.base_free = buyer
        .base_free
        .checked_add(fill_size)
        .ok_or(ExchangeError::Overflow)?;

    seller.base_free = seller
        .base_free
        .checked_sub(fill_size)
        .ok_or(ExchangeError::InsufficientFreeBalance)?;
    seller.quote_free = seller
        .quote_free
        .checked_add(quote_amount)
        .ok_or(ExchangeError::Overflow)?;

    Ok(())
}

fn verify_precompile(
    sysvar_ai: &AccountInfo,
    index: usize,
    order: &SignedOrderArgs,
    expected_message: &[u8],
) -> Result<()> {
    let ix = load_instruction_at_checked(index, sysvar_ai)
        .map_err(|_| error!(ExchangeError::MissingEd25519Verify))?;
    require_keys_eq!(
        ix.program_id,
        ED25519_PROGRAM_ID,
        ExchangeError::MissingEd25519Verify
    );
    let (pubkey, message) = parse_ed25519_precompile_ix(&ix.data)?;
    require_keys_eq!(pubkey, order.user, ExchangeError::Ed25519DataMismatch);
    require!(
        message == expected_message,
        ExchangeError::Ed25519DataMismatch
    );
    Ok(())
}

fn update_marker(marker: &mut OrderMarker, bump: u8, fill_size: u64, max_size: u64) -> Result<()> {
    if marker.bump == 0 {
        marker.bump = bump;
        marker.filled_size = 0;
    }
    let new_filled = marker
        .filled_size
        .checked_add(fill_size)
        .ok_or(ExchangeError::Overflow)?;
    require!(new_filled <= max_size, ExchangeError::OrderOverfilled);
    marker.filled_size = new_filled;
    Ok(())
}
