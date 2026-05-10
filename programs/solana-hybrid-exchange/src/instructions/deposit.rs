use anchor_lang::prelude::*;
use anchor_spl::token_2022::Token2022;
use anchor_spl::token_interface::{transfer_checked, Mint, TokenAccount, TransferChecked};

use crate::constants::{MARKET_SEED, USER_ACCOUNT_SEED};
use crate::error::ExchangeError;
use crate::state::{Market, UserAccount};

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        seeds = [MARKET_SEED, market.load()?.base_mint.as_ref(), market.load()?.quote_mint.as_ref()],
        bump = market.load()?.bump,
    )]
    pub market: AccountLoader<'info, Market>,

    #[account(
        init_if_needed,
        payer = user,
        space = 8 + std::mem::size_of::<UserAccount>(),
        seeds = [USER_ACCOUNT_SEED, market.key().as_ref(), user.key().as_ref()],
        bump,
    )]
    pub user_account: AccountLoader<'info, UserAccount>,

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
    pub system_program: Program<'info, System>,
}

pub(crate) fn handler(ctx: Context<Deposit>, amount: u64) -> Result<()> {
    let mint_key = ctx.accounts.mint.key();
    let vault_key = ctx.accounts.vault.key();
    let market_key = ctx.accounts.market.key();

    let (base_mint, quote_mint, base_vault, quote_vault) = {
        let market = ctx.accounts.market.load()?;
        (
            market.base_mint,
            market.quote_mint,
            market.base_vault,
            market.quote_vault,
        )
    };

    let is_base = mint_key == base_mint;
    let is_quote = mint_key == quote_mint;
    require!(is_base || is_quote, ExchangeError::WrongMint);

    if is_base {
        require_keys_eq!(vault_key, base_vault, ExchangeError::WrongMint);
    } else {
        require_keys_eq!(vault_key, quote_vault, ExchangeError::WrongMint);
    }

    let mut user_account = ctx
        .accounts
        .user_account
        .load_init()
        .or_else(|_| ctx.accounts.user_account.load_mut())?;
    if user_account.owner == Pubkey::default() {
        user_account.owner = ctx.accounts.user.key();
        user_account.market = market_key;
        user_account.base_free = 0;
        user_account.quote_free = 0;
        user_account.bump = ctx.bumps.user_account;
    }

    let pre = ctx.accounts.vault.amount;

    let cpi_accounts = TransferChecked {
        from: ctx.accounts.user_token_account.to_account_info(),
        mint: ctx.accounts.mint.to_account_info(),
        to: ctx.accounts.vault.to_account_info(),
        authority: ctx.accounts.user.to_account_info(),
    };
    let cpi_ctx = CpiContext::new(ctx.accounts.token_program.key(), cpi_accounts);
    transfer_checked(cpi_ctx, amount, ctx.accounts.mint.decimals)?;

    ctx.accounts.vault.reload()?;
    let post = ctx.accounts.vault.amount;
    let delta = post.checked_sub(pre).ok_or(ExchangeError::Overflow)?;

    if is_base {
        user_account.base_free = user_account
            .base_free
            .checked_add(delta)
            .ok_or(ExchangeError::Overflow)?;
    } else {
        user_account.quote_free = user_account
            .quote_free
            .checked_add(delta)
            .ok_or(ExchangeError::Overflow)?;
    }

    Ok(())
}
