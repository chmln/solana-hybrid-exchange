use anchor_lang::prelude::*;
use anchor_spl::token_2022::Token2022;
use anchor_spl::token_interface::spl_token_2022::extension::{
    BaseStateWithExtensions, ExtensionType, StateWithExtensions,
};
use anchor_spl::token_interface::spl_token_2022::state::Mint as SplMint;
use anchor_spl::token_interface::{Mint, TokenAccount};

use crate::constants::MARKET_SEED;
use crate::error::ExchangeError;
use crate::state::Market;

const DISALLOWED_EXTENSIONS: &[ExtensionType] = &[
    ExtensionType::TransferFeeConfig,
    ExtensionType::TransferHook,
    ExtensionType::NonTransferable,
    ExtensionType::PermanentDelegate,
    ExtensionType::Pausable,
    ExtensionType::MintCloseAuthority,
];

#[derive(Accounts)]
pub struct InitMarket<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    pub base_mint: InterfaceAccount<'info, Mint>,
    pub quote_mint: InterfaceAccount<'info, Mint>,

    #[account(
        init,
        payer = admin,
        space = 8 + Market::INIT_SPACE,
        seeds = [MARKET_SEED, base_mint.key().as_ref(), quote_mint.key().as_ref()],
        bump,
    )]
    pub market: Account<'info, Market>,

    #[account(
        init,
        payer = admin,
        token::mint = base_mint,
        token::authority = market,
        token::token_program = token_program,
    )]
    pub base_vault: InterfaceAccount<'info, TokenAccount>,

    #[account(
        init,
        payer = admin,
        token::mint = quote_mint,
        token::authority = market,
        token::token_program = token_program,
    )]
    pub quote_vault: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Program<'info, Token2022>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

pub(crate) fn handler(ctx: Context<InitMarket>) -> Result<()> {
    let token_2022_id = anchor_spl::token_2022::ID;

    require_keys_eq!(
        *ctx.accounts.base_mint.to_account_info().owner,
        token_2022_id,
        ExchangeError::WrongTokenProgram
    );
    require_keys_eq!(
        *ctx.accounts.quote_mint.to_account_info().owner,
        token_2022_id,
        ExchangeError::WrongTokenProgram
    );

    check_mint_extensions(&ctx.accounts.base_mint.to_account_info())?;
    check_mint_extensions(&ctx.accounts.quote_mint.to_account_info())?;

    let quote_decimals = ctx.accounts.quote_mint.decimals;
    let price_scale = 10u64
        .checked_pow(quote_decimals as u32)
        .ok_or(ExchangeError::Overflow)?;

    let market = &mut ctx.accounts.market;
    market.admin = ctx.accounts.admin.key();
    market.base_mint = ctx.accounts.base_mint.key();
    market.quote_mint = ctx.accounts.quote_mint.key();
    market.base_vault = ctx.accounts.base_vault.key();
    market.quote_vault = ctx.accounts.quote_vault.key();
    market.price_scale = price_scale;
    market.bump = ctx.bumps.market;

    Ok(())
}

fn check_mint_extensions(mint_ai: &AccountInfo) -> Result<()> {
    let data = mint_ai.data.borrow();
    let parsed = StateWithExtensions::<SplMint>::unpack(&data)
        .map_err(|_| error!(ExchangeError::MintHasDisallowedExtension))?;
    let extensions = parsed
        .get_extension_types()
        .map_err(|_| error!(ExchangeError::MintHasDisallowedExtension))?;
    for ext in extensions {
        if DISALLOWED_EXTENSIONS.contains(&ext) {
            return err!(ExchangeError::MintHasDisallowedExtension);
        }
    }
    Ok(())
}
