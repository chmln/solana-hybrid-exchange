//! Full-lifecycle smoke test for the solana-hybrid-exchange program.
//!
//! Walks one market through init -> two-user deposit -> matched settle -> withdraw
//! and asserts on-chain ledger state and external token balances reconcile exactly.

mod common;

use {
    anchor_lang::{
        prelude::Pubkey, solana_program::instruction::Instruction, AccountDeserialize,
        InstructionData, ToAccountMetas,
    },
    common::*,
    litesvm::LiteSVM,
    solana_hybrid_exchange::{
        accounts as program_accounts, instruction as program_ix,
        order::{canonical_serialize, OrderHash, Side, SignedOrderArgs, IX_SYSVAR_ID},
        state::UserAccount,
    },
    solana_keypair::Keypair,
    solana_signer::Signer,
};

fn fresh_svm() -> LiteSVM {
    let mut svm = LiteSVM::new();
    svm.add_program(
        solana_hybrid_exchange::id(),
        include_bytes!("../../../target/deploy/solana_hybrid_exchange.so"),
    )
    .unwrap();
    svm
}

fn build_init_market_ix(
    admin: &Pubkey,
    base_mint: &Pubkey,
    quote_mint: &Pubkey,
    market: &Pubkey,
    base_vault: &Pubkey,
    quote_vault: &Pubkey,
) -> Instruction {
    let accounts = program_accounts::InitMarket {
        admin: *admin,
        base_mint: *base_mint,
        quote_mint: *quote_mint,
        market: *market,
        base_vault: *base_vault,
        quote_vault: *quote_vault,
        token_program: spl_token_2022::id(),
        system_program: SYSTEM_PROGRAM_ID,
    };
    Instruction::new_with_bytes(
        solana_hybrid_exchange::id(),
        &program_ix::InitMarket {}.data(),
        accounts.to_account_metas(None),
    )
}

fn build_deposit_ix(
    user: &Pubkey,
    market: &Pubkey,
    user_account: &Pubkey,
    user_token_account: &Pubkey,
    vault: &Pubkey,
    mint: &Pubkey,
    amount: u64,
) -> Instruction {
    let accounts = program_accounts::Deposit {
        user: *user,
        market: *market,
        user_account: *user_account,
        user_token_account: *user_token_account,
        vault: *vault,
        mint: *mint,
        token_program: spl_token_2022::id(),
        system_program: SYSTEM_PROGRAM_ID,
    };
    Instruction::new_with_bytes(
        solana_hybrid_exchange::id(),
        &program_ix::Deposit { amount }.data(),
        accounts.to_account_metas(None),
    )
}

fn build_withdraw_ix(
    user: &Pubkey,
    market: &Pubkey,
    user_account: &Pubkey,
    user_token_account: &Pubkey,
    vault: &Pubkey,
    mint: &Pubkey,
    amount: u64,
) -> Instruction {
    let accounts = program_accounts::Withdraw {
        user: *user,
        market: *market,
        user_account: *user_account,
        user_token_account: *user_token_account,
        vault: *vault,
        mint: *mint,
        token_program: spl_token_2022::id(),
    };
    Instruction::new_with_bytes(
        solana_hybrid_exchange::id(),
        &program_ix::Withdraw { amount }.data(),
        accounts.to_account_metas(None),
    )
}

#[allow(clippy::too_many_arguments)]
fn build_settle_ix(
    operator: &Pubkey,
    market: &Pubkey,
    maker_user_account: &Pubkey,
    taker_user_account: &Pubkey,
    maker_order_marker: &Pubkey,
    taker_order_marker: &Pubkey,
    maker: SignedOrderArgs,
    taker: SignedOrderArgs,
    fill_price: u64,
    fill_size: u64,
    maker_order_hash: OrderHash,
    taker_order_hash: OrderHash,
) -> Instruction {
    let accounts = program_accounts::Settle {
        operator: *operator,
        market: *market,
        maker_user_account: *maker_user_account,
        taker_user_account: *taker_user_account,
        maker_order_marker: *maker_order_marker,
        taker_order_marker: *taker_order_marker,
        instructions_sysvar: IX_SYSVAR_ID,
        system_program: SYSTEM_PROGRAM_ID,
    };
    Instruction::new_with_bytes(
        solana_hybrid_exchange::id(),
        &program_ix::Settle {
            maker,
            taker,
            fill_price,
            fill_size,
            maker_order_hash,
            taker_order_hash,
        }
        .data(),
        accounts.to_account_metas(None),
    )
}

fn fetch_user_account(svm: &LiteSVM, pda: &Pubkey) -> UserAccount {
    let acc = svm.get_account(pda).expect("user account missing");
    UserAccount::try_deserialize(&mut acc.data.as_slice()).expect("deserialize UserAccount")
}

#[test]
fn e2e_smoke() {
    let mut svm = fresh_svm();

    // ----- Setup keys -----
    let admin = Keypair::new();
    let mint_authority = Keypair::new();
    let alice = Keypair::new();
    let bob = Keypair::new();

    for kp in [&admin, &mint_authority, &alice, &bob] {
        svm.airdrop(&kp.pubkey(), 100_000_000_000).unwrap();
    }

    // ----- Mints -----
    let base_decimals = 9u8;
    let quote_decimals = 6u8;
    let base_mint = create_mint(
        &mut svm,
        &admin,
        base_decimals,
        &mint_authority.pubkey(),
        &[],
    );
    let quote_mint = create_mint(
        &mut svm,
        &admin,
        quote_decimals,
        &mint_authority.pubkey(),
        &[],
    );

    // ----- User token accounts + initial funding (1_000 base + 1_000_000_000 quote each) -----
    let alice_base = create_token_account(&mut svm, &alice, &base_mint, &alice.pubkey());
    let alice_quote = create_token_account(&mut svm, &alice, &quote_mint, &alice.pubkey());
    let bob_base = create_token_account(&mut svm, &bob, &base_mint, &bob.pubkey());
    let bob_quote = create_token_account(&mut svm, &bob, &quote_mint, &bob.pubkey());

    let initial_base = 1_000u64;
    let initial_quote = 1_000_000_000u64;
    mint_to(
        &mut svm,
        &admin,
        &mint_authority,
        &base_mint,
        &alice_base,
        initial_base,
    );
    mint_to(
        &mut svm,
        &admin,
        &mint_authority,
        &quote_mint,
        &alice_quote,
        initial_quote,
    );
    mint_to(
        &mut svm,
        &admin,
        &mint_authority,
        &base_mint,
        &bob_base,
        initial_base,
    );
    mint_to(
        &mut svm,
        &admin,
        &mint_authority,
        &quote_mint,
        &bob_quote,
        initial_quote,
    );

    // ----- Init market -----
    let (market, _market_bump) = market_pda(&base_mint, &quote_mint);
    let base_vault_kp = Keypair::new();
    let quote_vault_kp = Keypair::new();
    let base_vault = base_vault_kp.pubkey();
    let quote_vault = quote_vault_kp.pubkey();

    let init_ix = build_init_market_ix(
        &admin.pubkey(),
        &base_mint,
        &quote_mint,
        &market,
        &base_vault,
        &quote_vault,
    );
    send_tx(
        &mut svm,
        &[init_ix],
        &[&admin, &base_vault_kp, &quote_vault_kp],
    )
    .expect("init_market failed");

    // ----- Deposits (each user 500 base + 500_000_000 quote) -----
    let deposit_base = 500u64;
    let deposit_quote = 500_000_000u64;

    let (alice_user_account, _) = user_account_pda(&market, &alice.pubkey());
    let (bob_user_account, _) = user_account_pda(&market, &bob.pubkey());

    // Alice deposits base
    let ix = build_deposit_ix(
        &alice.pubkey(),
        &market,
        &alice_user_account,
        &alice_base,
        &base_vault,
        &base_mint,
        deposit_base,
    );
    send_tx(&mut svm, &[ix], &[&alice]).expect("alice deposit base failed");

    // Alice deposits quote
    let ix = build_deposit_ix(
        &alice.pubkey(),
        &market,
        &alice_user_account,
        &alice_quote,
        &quote_vault,
        &quote_mint,
        deposit_quote,
    );
    send_tx(&mut svm, &[ix], &[&alice]).expect("alice deposit quote failed");

    // Bob deposits base
    let ix = build_deposit_ix(
        &bob.pubkey(),
        &market,
        &bob_user_account,
        &bob_base,
        &base_vault,
        &base_mint,
        deposit_base,
    );
    send_tx(&mut svm, &[ix], &[&bob]).expect("bob deposit base failed");

    // Bob deposits quote
    let ix = build_deposit_ix(
        &bob.pubkey(),
        &market,
        &bob_user_account,
        &bob_quote,
        &quote_vault,
        &quote_mint,
        deposit_quote,
    );
    send_tx(&mut svm, &[ix], &[&bob]).expect("bob deposit quote failed");

    // Post-deposit sanity: each user's ledger balances match deposit amounts.
    let alice_ua = fetch_user_account(&svm, &alice_user_account);
    assert_eq!(alice_ua.base_free, deposit_base);
    assert_eq!(alice_ua.quote_free, deposit_quote);
    let bob_ua = fetch_user_account(&svm, &bob_user_account);
    assert_eq!(bob_ua.base_free, deposit_base);
    assert_eq!(bob_ua.quote_free, deposit_quote);

    // External token balances dropped by the deposit amounts.
    assert_eq!(
        token_balance(&svm, &alice_base),
        initial_base - deposit_base
    );
    assert_eq!(
        token_balance(&svm, &alice_quote),
        initial_quote - deposit_quote
    );
    assert_eq!(token_balance(&svm, &bob_base), initial_base - deposit_base);
    assert_eq!(
        token_balance(&svm, &bob_quote),
        initial_quote - deposit_quote
    );

    // ----- Settle: Alice (maker, Bid) <-> Bob (taker, Ask) -----
    // price_scale = 10^6 (quote_decimals). fill_price = 1_000_000 yields
    // quote_amount = 500 * 1_000_000 / 1_000_000 = 500.
    let fill_price = 1_000_000u64;
    let fill_size = 500u64;
    let expiry: i64 = i64::MAX / 2; // far future

    let maker = SignedOrderArgs {
        user: alice.pubkey(),
        market,
        side: Side::Bid,
        limit_price: fill_price,
        max_size: fill_size,
        nonce: 1,
        expiry,
    };
    let taker = SignedOrderArgs {
        user: bob.pubkey(),
        market,
        side: Side::Ask,
        limit_price: fill_price,
        max_size: fill_size,
        nonce: 2,
        expiry,
    };

    let maker_bytes = canonical_serialize(&maker);
    let taker_bytes = canonical_serialize(&taker);
    let maker_hash = solana_sha256_hasher::hashv(&[&maker_bytes]).to_bytes();
    let taker_hash = solana_sha256_hasher::hashv(&[&taker_bytes]).to_bytes();

    let maker_sig: [u8; 64] = alice.sign_message(&maker_bytes).into();
    let taker_sig: [u8; 64] = bob.sign_message(&taker_bytes).into();

    let ed_maker = ed25519_verify_ix(&alice.pubkey().to_bytes(), &maker_sig, &maker_bytes);
    let ed_taker = ed25519_verify_ix(&bob.pubkey().to_bytes(), &taker_sig, &taker_bytes);

    let (maker_order_marker, _) = order_marker_pda(&maker_hash);
    let (taker_order_marker, _) = order_marker_pda(&taker_hash);

    let settle = build_settle_ix(
        &admin.pubkey(),
        &market,
        &alice_user_account,
        &bob_user_account,
        &maker_order_marker,
        &taker_order_marker,
        maker,
        taker,
        fill_price,
        fill_size,
        OrderHash(maker_hash),
        OrderHash(taker_hash),
    );

    send_tx(&mut svm, &[ed_maker, ed_taker, settle], &[&admin]).expect("settle failed");

    // Post-settle ledger state.
    let alice_ua = fetch_user_account(&svm, &alice_user_account);
    let bob_ua = fetch_user_account(&svm, &bob_user_account);
    // Alice (bid/buyer): +500 base, -500 quote (in raw quote units, post price-scale).
    assert_eq!(
        alice_ua.base_free,
        deposit_base + fill_size,
        "alice base after settle"
    );
    assert_eq!(
        alice_ua.quote_free,
        deposit_quote - 500,
        "alice quote after settle"
    );
    // Bob (ask/seller): -500 base, +500 quote.
    assert_eq!(
        bob_ua.base_free,
        deposit_base - fill_size,
        "bob base after settle"
    );
    assert_eq!(
        bob_ua.quote_free,
        deposit_quote + 500,
        "bob quote after settle"
    );

    // ----- Withdrawals -----
    // Alice: 1000 base, 499_999_500 quote.
    let alice_withdraw_base = alice_ua.base_free;
    let alice_withdraw_quote = alice_ua.quote_free;
    let bob_withdraw_quote = bob_ua.quote_free;

    let ix = build_withdraw_ix(
        &alice.pubkey(),
        &market,
        &alice_user_account,
        &alice_base,
        &base_vault,
        &base_mint,
        alice_withdraw_base,
    );
    send_tx(&mut svm, &[ix], &[&alice]).expect("alice withdraw base failed");

    let ix = build_withdraw_ix(
        &alice.pubkey(),
        &market,
        &alice_user_account,
        &alice_quote,
        &quote_vault,
        &quote_mint,
        alice_withdraw_quote,
    );
    send_tx(&mut svm, &[ix], &[&alice]).expect("alice withdraw quote failed");

    // Bob's base_free is zero; skip the base withdraw. Withdraw all quote.
    let ix = build_withdraw_ix(
        &bob.pubkey(),
        &market,
        &bob_user_account,
        &bob_quote,
        &quote_vault,
        &quote_mint,
        bob_withdraw_quote,
    );
    send_tx(&mut svm, &[ix], &[&bob]).expect("bob withdraw quote failed");

    // ----- Final assertions -----
    let alice_ua = fetch_user_account(&svm, &alice_user_account);
    let bob_ua = fetch_user_account(&svm, &bob_user_account);
    assert_eq!(alice_ua.base_free, 0, "alice ledger base zero");
    assert_eq!(alice_ua.quote_free, 0, "alice ledger quote zero");
    assert_eq!(bob_ua.base_free, 0, "bob ledger base zero");
    assert_eq!(bob_ua.quote_free, 0, "bob ledger quote zero");

    // External balances:
    //   Alice base: 1000 - 500 deposit + 1000 withdraw = 1500
    //   Alice quote: 1_000_000_000 - 500_000_000 deposit + 499_999_500 withdraw = 999_999_500
    //   Bob base: 1000 - 500 deposit + 0 withdraw = 500
    //   Bob quote: 1_000_000_000 - 500_000_000 deposit + 500_000_500 withdraw = 1_000_000_500
    assert_eq!(
        token_balance(&svm, &alice_base),
        1_500,
        "alice external base"
    );
    assert_eq!(
        token_balance(&svm, &alice_quote),
        999_999_500,
        "alice external quote"
    );
    assert_eq!(token_balance(&svm, &bob_base), 500, "bob external base");
    assert_eq!(
        token_balance(&svm, &bob_quote),
        1_000_000_500,
        "bob external quote"
    );

    // Vault reconciliation:
    //   base_vault: 1000 deposited - 1000 alice withdraw - 0 bob withdraw = 0
    //   But bob still has 500 base un-withdrawable (his ledger is 0 — he sold it). The
    //   vault keeps that 500 since Alice only withdraws her 1000. Wait — Alice's ledger
    //   credited her the 500 base from settle, then she withdraws 1000 total. So base_vault
    //   goes from 1000 to 0.
    //   quote_vault: 1_000_000_000 deposited - 499_999_500 alice - 500_000_500 bob = 0
    assert_eq!(token_balance(&svm, &base_vault), 0, "base vault drained");
    assert_eq!(token_balance(&svm, &quote_vault), 0, "quote vault drained");
}
