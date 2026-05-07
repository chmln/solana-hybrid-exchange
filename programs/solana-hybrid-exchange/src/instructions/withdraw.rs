use anchor_lang::prelude::*;
use anchor_spl::token_2022::Token2022;
use anchor_spl::token_interface::{transfer_checked, Mint, TokenAccount, TransferChecked};

use crate::constants::{MARKET_SEED, USER_ACCOUNT_SEED};
use crate::error::ExchangeError;
use crate::state::{Market, UserAccount};

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        seeds = [MARKET_SEED, market.base_mint.as_ref(), market.quote_mint.as_ref()],
        bump = market.bump,
    )]
    pub market: Account<'info, Market>,

    #[account(
        mut,
        seeds = [USER_ACCOUNT_SEED, market.key().as_ref(), user.key().as_ref()],
        bump = user_account.bump,
        has_one = market,
        constraint = user_account.owner == user.key() @ ExchangeError::InsufficientFreeBalance,
    )]
    pub user_account: Account<'info, UserAccount>,

    #[account(
        mut,
        token::mint = mint,
        token::authority = user,
    )]
    pub user_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub vault: InterfaceAccount<'info, TokenAccount>,

    pub mint: InterfaceAccount<'info, Mint>,

    pub token_program: Program<'info, Token2022>,
}

pub(crate) fn handler(ctx: Context<Withdraw>, amount: u64) -> Result<()> {
    let market = &ctx.accounts.market;
    let mint_key = ctx.accounts.mint.key();
    let vault_key = ctx.accounts.vault.key();

    let is_base = mint_key == market.base_mint;
    let is_quote = mint_key == market.quote_mint;
    require!(is_base || is_quote, ExchangeError::WrongMint);

    if is_base {
        require_keys_eq!(vault_key, market.base_vault, ExchangeError::WrongMint);
    } else {
        require_keys_eq!(vault_key, market.quote_vault, ExchangeError::WrongMint);
    }

    let user_account = &mut ctx.accounts.user_account;
    if is_base {
        user_account.base_free = user_account
            .base_free
            .checked_sub(amount)
            .ok_or(ExchangeError::InsufficientFreeBalance)?;
    } else {
        user_account.quote_free = user_account
            .quote_free
            .checked_sub(amount)
            .ok_or(ExchangeError::InsufficientFreeBalance)?;
    }

    let base_mint = market.base_mint;
    let quote_mint = market.quote_mint;
    let market_bump = market.bump;
    let signer_seeds: &[&[&[u8]]] = &[&[
        MARKET_SEED,
        base_mint.as_ref(),
        quote_mint.as_ref(),
        std::slice::from_ref(&market_bump),
    ]];

    let cpi_accounts = TransferChecked {
        from: ctx.accounts.vault.to_account_info(),
        mint: ctx.accounts.mint.to_account_info(),
        to: ctx.accounts.user_token_account.to_account_info(),
        authority: ctx.accounts.market.to_account_info(),
    };
    let cpi_ctx =
        CpiContext::new_with_signer(ctx.accounts.token_program.key(), cpi_accounts, signer_seeds);
    transfer_checked(cpi_ctx, amount, ctx.accounts.mint.decimals)?;

    Ok(())
}
