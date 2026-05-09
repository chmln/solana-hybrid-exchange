//! Tests for the `settle` instruction.

mod common;

use {
    anchor_lang::{
        solana_program::instruction::Instruction, AccountDeserialize, InstructionData,
        ToAccountMetas,
    },
    common::*,
    litesvm::{types::FailedTransactionMetadata, LiteSVM},
    solana_hybrid_exchange::{
        accounts as program_accounts, instruction as program_ix,
        order::{canonical_serialize, Side, SignedOrderArgs, IX_SYSVAR_ID},
        state::{OrderMarker, UserAccount},
    },
    solana_keypair::Keypair,
    solana_sha256_hasher::hashv,
    solana_signer::Signer,
};

// Anchor user error codes start at 6000. Indices match `ExchangeError` declaration order.
const ERR_INSUFFICIENT_FREE_BALANCE: u32 = 6003;
const ERR_ED25519_DATA_MISMATCH: u32 = 6005;
const ERR_SAME_SIDE: u32 = 6007;
const ERR_PRICE_DOES_NOT_CROSS: u32 = 6008;
const ERR_FILL_PRICE_OUT_OF_RANGE: u32 = 6009;
const ERR_ORDER_EXPIRED: u32 = 6010;
const ERR_ORDER_OVERFILLED: u32 = 6011;
const ERR_ZERO_FILL_SIZE: u32 = 6012;

const RENT_SYSVAR_ID: anchor_lang::prelude::Pubkey =
    anchor_lang::pubkey!("SysvarRent111111111111111111111111111111111");

fn has_custom_error(meta: &FailedTransactionMetadata, code: u32) -> bool {
    let dbg = format!("{:?}", meta.err);
    if dbg.contains(&format!("Custom({code})")) {
        return true;
    }
    let needle = format!("Error Number: {code}");
    meta.meta.logs.iter().any(|l| l.contains(&needle))
}

fn fresh_svm() -> LiteSVM {
    let mut svm = LiteSVM::new();
    svm.add_program(
        solana_hybrid_exchange::id(),
        include_bytes!("../../../target/deploy/solana_hybrid_exchange.so"),
    )
    .unwrap();
    svm
}

struct Setup {
    svm: LiteSVM,
    operator: Keypair,
    market: anchor_lang::prelude::Pubkey,
    maker: Keypair,
    taker: Keypair,
}

/// Spin up a fresh SVM with a market, two funded users, and an operator.
/// Each user has deposited base + quote into their UserAccount.
fn setup_market_with_users(base_deposit: u64, quote_deposit: u64) -> Setup {
    let mut svm = fresh_svm();

    let admin = Keypair::new();
    let mint_authority = Keypair::new();
    let operator = Keypair::new();
    svm.airdrop(&admin.pubkey(), 100_000_000_000).unwrap();
    svm.airdrop(&mint_authority.pubkey(), 100_000_000_000)
        .unwrap();
    svm.airdrop(&operator.pubkey(), 100_000_000_000).unwrap();

    let base_mint = create_mint(&mut svm, &admin, 9, &mint_authority.pubkey(), &[]);
    let quote_mint = create_mint(&mut svm, &admin, 6, &mint_authority.pubkey(), &[]);

    let (market, _) = market_pda(&base_mint, &quote_mint);
    let base_vault_kp = Keypair::new();
    let quote_vault_kp = Keypair::new();

    let init_ix = Instruction::new_with_bytes(
        solana_hybrid_exchange::id(),
        &program_ix::InitMarket {}.data(),
        program_accounts::InitMarket {
            admin: admin.pubkey(),
            base_mint,
            quote_mint,
            market,
            base_vault: base_vault_kp.pubkey(),
            quote_vault: quote_vault_kp.pubkey(),
            token_program: spl_token_2022::id(),
            system_program: SYSTEM_PROGRAM_ID,
            rent: RENT_SYSVAR_ID,
        }
        .to_account_metas(None),
    );
    send_tx(
        &mut svm,
        &[init_ix],
        &[&admin, &base_vault_kp, &quote_vault_kp],
    )
    .expect("init_market failed");

    let maker = Keypair::new();
    let taker = Keypair::new();
    svm.airdrop(&maker.pubkey(), 100_000_000_000).unwrap();
    svm.airdrop(&taker.pubkey(), 100_000_000_000).unwrap();

    fund_user(
        &mut svm,
        &maker,
        &mint_authority,
        &market,
        &base_mint,
        &quote_mint,
        &base_vault_kp.pubkey(),
        &quote_vault_kp.pubkey(),
        base_deposit,
        quote_deposit,
    );
    fund_user(
        &mut svm,
        &taker,
        &mint_authority,
        &market,
        &base_mint,
        &quote_mint,
        &base_vault_kp.pubkey(),
        &quote_vault_kp.pubkey(),
        base_deposit,
        quote_deposit,
    );

    Setup {
        svm,
        operator,
        market,
        maker,
        taker,
    }
}

#[allow(clippy::too_many_arguments)]
fn fund_user(
    svm: &mut LiteSVM,
    user: &Keypair,
    mint_authority: &Keypair,
    market: &anchor_lang::prelude::Pubkey,
    base_mint: &anchor_lang::prelude::Pubkey,
    quote_mint: &anchor_lang::prelude::Pubkey,
    base_vault: &anchor_lang::prelude::Pubkey,
    quote_vault: &anchor_lang::prelude::Pubkey,
    base_amount: u64,
    quote_amount: u64,
) {
    let user_base_ata = create_token_account(svm, user, base_mint, &user.pubkey());
    let user_quote_ata = create_token_account(svm, user, quote_mint, &user.pubkey());
    mint_to(
        svm,
        user,
        mint_authority,
        base_mint,
        &user_base_ata,
        base_amount,
    );
    mint_to(
        svm,
        user,
        mint_authority,
        quote_mint,
        &user_quote_ata,
        quote_amount,
    );

    deposit(
        svm,
        user,
        market,
        base_mint,
        base_vault,
        &user_base_ata,
        base_amount,
    );
    deposit(
        svm,
        user,
        market,
        quote_mint,
        quote_vault,
        &user_quote_ata,
        quote_amount,
    );
}

#[allow(clippy::too_many_arguments)]
fn deposit(
    svm: &mut LiteSVM,
    user: &Keypair,
    market: &anchor_lang::prelude::Pubkey,
    mint: &anchor_lang::prelude::Pubkey,
    vault: &anchor_lang::prelude::Pubkey,
    user_token_account: &anchor_lang::prelude::Pubkey,
    amount: u64,
) {
    let (user_account, _) = user_account_pda(market, &user.pubkey());
    let ix = Instruction::new_with_bytes(
        solana_hybrid_exchange::id(),
        &program_ix::Deposit { amount }.data(),
        program_accounts::Deposit {
            user: user.pubkey(),
            market: *market,
            user_account,
            user_token_account: *user_token_account,
            vault: *vault,
            mint: *mint,
            token_program: spl_token_2022::id(),
            system_program: SYSTEM_PROGRAM_ID,
        }
        .to_account_metas(None),
    );
    send_tx(svm, &[ix], &[user]).expect("deposit failed");
}

/// Sign canonical bytes of `order` with `signer` and return everything needed to attach
/// the Ed25519 precompile ix to a settle tx.
struct SignedOrder {
    bytes: Vec<u8>,
    hash: [u8; 32],
    sig: [u8; 64],
    pk: [u8; 32],
}

fn sign_order(signer: &Keypair, order: &SignedOrderArgs) -> SignedOrder {
    let bytes = canonical_serialize(order).to_vec();
    let hash: [u8; 32] = hashv(&[&bytes]).to_bytes();
    let sig: [u8; 64] = signer.sign_message(&bytes).into();
    let pk: [u8; 32] = signer.pubkey().to_bytes();
    SignedOrder {
        bytes,
        hash,
        sig,
        pk,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_settle_ixs(
    market: &anchor_lang::prelude::Pubkey,
    operator: &anchor_lang::prelude::Pubkey,
    maker_order_args: SignedOrderArgs,
    taker_order_args: SignedOrderArgs,
    maker_signed: &SignedOrder,
    taker_signed: &SignedOrder,
    fill_price: u64,
    fill_size: u64,
) -> Vec<Instruction> {
    let ed_maker = ed25519_verify_ix(&maker_signed.pk, &maker_signed.sig, &maker_signed.bytes);
    let ed_taker = ed25519_verify_ix(&taker_signed.pk, &taker_signed.sig, &taker_signed.bytes);

    let settle_ix = Instruction::new_with_bytes(
        solana_hybrid_exchange::id(),
        &program_ix::Settle {
            maker: maker_order_args,
            taker: taker_order_args,
            fill_price,
            fill_size,
            maker_order_hash: maker_signed.hash,
            taker_order_hash: taker_signed.hash,
        }
        .data(),
        program_accounts::Settle {
            operator: *operator,
            market: *market,
            maker_user_account: user_account_pda(market, &maker_order_args.user).0,
            taker_user_account: user_account_pda(market, &taker_order_args.user).0,
            maker_order_marker: order_marker_pda(&maker_signed.hash).0,
            taker_order_marker: order_marker_pda(&taker_signed.hash).0,
            instructions_sysvar: IX_SYSVAR_ID,
            system_program: SYSTEM_PROGRAM_ID,
        }
        .to_account_metas(None),
    );

    vec![ed_maker, ed_taker, settle_ix]
}

fn read_user(svm: &LiteSVM, pda: &anchor_lang::prelude::Pubkey) -> UserAccount {
    let acc = svm.get_account(pda).expect("user account missing");
    UserAccount::try_deserialize(&mut acc.data.as_slice()).expect("deserialize UserAccount")
}

fn read_marker(svm: &LiteSVM, pda: &anchor_lang::prelude::Pubkey) -> OrderMarker {
    let acc = svm.get_account(pda).expect("marker account missing");
    OrderMarker::try_deserialize(&mut acc.data.as_slice()).expect("deserialize OrderMarker")
}

// price_scale = 10^6 for quote_decimals = 6
const PRICE_SCALE: u64 = 1_000_000;
const BASE_DEPOSIT: u64 = 1_000_000;
const QUOTE_DEPOSIT: u64 = 1_000_000_000_000;

fn quote_for(fill_price: u64, fill_size: u64) -> u64 {
    ((fill_price as u128) * (fill_size as u128) / (PRICE_SCALE as u128)) as u64
}

#[test]
fn settle_happy_path() {
    let mut s = setup_market_with_users(BASE_DEPOSIT, QUOTE_DEPOSIT);

    let maker_order = SignedOrderArgs {
        user: s.maker.pubkey(),
        market: s.market,
        side: Side::Bid,
        limit_price: 100,
        max_size: 1000,
        nonce: 1,
        expiry: i64::MAX,
    };
    let taker_order = SignedOrderArgs {
        user: s.taker.pubkey(),
        market: s.market,
        side: Side::Ask,
        limit_price: 100,
        max_size: 1000,
        nonce: 2,
        expiry: i64::MAX,
    };
    let maker_signed = sign_order(&s.maker, &maker_order);
    let taker_signed = sign_order(&s.taker, &taker_order);

    let ixs = build_settle_ixs(
        &s.market,
        &s.operator.pubkey(),
        maker_order,
        taker_order,
        &maker_signed,
        &taker_signed,
        100,
        1000,
    );
    send_tx(&mut s.svm, &ixs, &[&s.operator]).expect("settle happy-path failed");

    let q = quote_for(100, 1000);
    let maker_pda = user_account_pda(&s.market, &s.maker.pubkey()).0;
    let taker_pda = user_account_pda(&s.market, &s.taker.pubkey()).0;
    let maker_state = read_user(&s.svm, &maker_pda);
    let taker_state = read_user(&s.svm, &taker_pda);

    // Maker is bidder: gains base, loses quote.
    assert_eq!(maker_state.base_free, BASE_DEPOSIT + 1000);
    assert_eq!(maker_state.quote_free, QUOTE_DEPOSIT - q);
    // Taker is asker: loses base, gains quote.
    assert_eq!(taker_state.base_free, BASE_DEPOSIT - 1000);
    assert_eq!(taker_state.quote_free, QUOTE_DEPOSIT + q);

    let maker_marker = read_marker(&s.svm, &order_marker_pda(&maker_signed.hash).0);
    let taker_marker = read_marker(&s.svm, &order_marker_pda(&taker_signed.hash).0);
    assert_eq!(maker_marker.filled_size, 1000);
    assert_eq!(taker_marker.filled_size, 1000);
}

#[test]
fn settle_two_partial_fills() {
    let mut s = setup_market_with_users(BASE_DEPOSIT, QUOTE_DEPOSIT);

    let maker_order = SignedOrderArgs {
        user: s.maker.pubkey(),
        market: s.market,
        side: Side::Bid,
        limit_price: 100,
        max_size: 1000,
        nonce: 1,
        expiry: i64::MAX,
    };
    let taker_order = SignedOrderArgs {
        user: s.taker.pubkey(),
        market: s.market,
        side: Side::Ask,
        limit_price: 100,
        max_size: 1000,
        nonce: 2,
        expiry: i64::MAX,
    };
    let maker_signed = sign_order(&s.maker, &maker_order);
    let taker_signed = sign_order(&s.taker, &taker_order);

    for _ in 0..2 {
        let ixs = build_settle_ixs(
            &s.market,
            &s.operator.pubkey(),
            maker_order,
            taker_order,
            &maker_signed,
            &taker_signed,
            100,
            500,
        );
        send_tx(&mut s.svm, &ixs, &[&s.operator]).expect("partial fill failed");
        s.svm.expire_blockhash();
    }

    let q = quote_for(100, 1000);
    let maker_state = read_user(&s.svm, &user_account_pda(&s.market, &s.maker.pubkey()).0);
    let taker_state = read_user(&s.svm, &user_account_pda(&s.market, &s.taker.pubkey()).0);
    assert_eq!(maker_state.base_free, BASE_DEPOSIT + 1000);
    assert_eq!(maker_state.quote_free, QUOTE_DEPOSIT - q);
    assert_eq!(taker_state.base_free, BASE_DEPOSIT - 1000);
    assert_eq!(taker_state.quote_free, QUOTE_DEPOSIT + q);

    let maker_marker = read_marker(&s.svm, &order_marker_pda(&maker_signed.hash).0);
    let taker_marker = read_marker(&s.svm, &order_marker_pda(&taker_signed.hash).0);
    assert_eq!(maker_marker.filled_size, 1000);
    assert_eq!(taker_marker.filled_size, 1000);
}

#[test]
fn settle_overfill_rejected() {
    let mut s = setup_market_with_users(BASE_DEPOSIT, QUOTE_DEPOSIT);

    let maker_order = SignedOrderArgs {
        user: s.maker.pubkey(),
        market: s.market,
        side: Side::Bid,
        limit_price: 100,
        max_size: 1000,
        nonce: 1,
        expiry: i64::MAX,
    };
    let taker_order = SignedOrderArgs {
        user: s.taker.pubkey(),
        market: s.market,
        side: Side::Ask,
        limit_price: 100,
        max_size: 1000,
        nonce: 2,
        expiry: i64::MAX,
    };
    let maker_signed = sign_order(&s.maker, &maker_order);
    let taker_signed = sign_order(&s.taker, &taker_order);

    let ixs = build_settle_ixs(
        &s.market,
        &s.operator.pubkey(),
        maker_order,
        taker_order,
        &maker_signed,
        &taker_signed,
        100,
        1000,
    );
    send_tx(&mut s.svm, &ixs, &[&s.operator]).expect("first settle failed");
    s.svm.expire_blockhash();

    let ixs = build_settle_ixs(
        &s.market,
        &s.operator.pubkey(),
        maker_order,
        taker_order,
        &maker_signed,
        &taker_signed,
        100,
        1,
    );
    let err = send_tx(&mut s.svm, &ixs, &[&s.operator])
        .expect_err("second settle should reject overfill");
    assert!(
        has_custom_error(&err, ERR_ORDER_OVERFILLED),
        "expected OrderOverfilled, got err={:?} logs={:?}",
        err.err,
        err.meta.logs,
    );
}

#[test]
fn settle_full_replay_rejected() {
    let mut s = setup_market_with_users(BASE_DEPOSIT, QUOTE_DEPOSIT);

    let maker_order = SignedOrderArgs {
        user: s.maker.pubkey(),
        market: s.market,
        side: Side::Bid,
        limit_price: 100,
        max_size: 1000,
        nonce: 1,
        expiry: i64::MAX,
    };
    let taker_order = SignedOrderArgs {
        user: s.taker.pubkey(),
        market: s.market,
        side: Side::Ask,
        limit_price: 100,
        max_size: 1000,
        nonce: 2,
        expiry: i64::MAX,
    };
    let maker_signed = sign_order(&s.maker, &maker_order);
    let taker_signed = sign_order(&s.taker, &taker_order);

    let ixs = build_settle_ixs(
        &s.market,
        &s.operator.pubkey(),
        maker_order,
        taker_order,
        &maker_signed,
        &taker_signed,
        100,
        1000,
    );
    send_tx(&mut s.svm, &ixs, &[&s.operator]).expect("first settle failed");
    s.svm.expire_blockhash();

    let ixs = build_settle_ixs(
        &s.market,
        &s.operator.pubkey(),
        maker_order,
        taker_order,
        &maker_signed,
        &taker_signed,
        100,
        1000,
    );
    let err = send_tx(&mut s.svm, &ixs, &[&s.operator]).expect_err("replay should reject overfill");
    assert!(
        has_custom_error(&err, ERR_ORDER_OVERFILLED),
        "expected OrderOverfilled, got err={:?} logs={:?}",
        err.err,
        err.meta.logs,
    );
}

#[test]
fn settle_zero_fill_size_rejected() {
    let mut s = setup_market_with_users(BASE_DEPOSIT, QUOTE_DEPOSIT);

    let maker_order = SignedOrderArgs {
        user: s.maker.pubkey(),
        market: s.market,
        side: Side::Bid,
        limit_price: 100,
        max_size: 1000,
        nonce: 1,
        expiry: i64::MAX,
    };
    let taker_order = SignedOrderArgs {
        user: s.taker.pubkey(),
        market: s.market,
        side: Side::Ask,
        limit_price: 100,
        max_size: 1000,
        nonce: 2,
        expiry: i64::MAX,
    };
    let maker_signed = sign_order(&s.maker, &maker_order);
    let taker_signed = sign_order(&s.taker, &taker_order);

    let ixs = build_settle_ixs(
        &s.market,
        &s.operator.pubkey(),
        maker_order,
        taker_order,
        &maker_signed,
        &taker_signed,
        100,
        0,
    );
    let err = send_tx(&mut s.svm, &ixs, &[&s.operator]).expect_err("zero fill should be rejected");
    assert!(
        has_custom_error(&err, ERR_ZERO_FILL_SIZE),
        "expected ZeroFillSize, got err={:?} logs={:?}",
        err.err,
        err.meta.logs,
    );
}

#[test]
fn settle_tampered_args_rejected() {
    let mut s = setup_market_with_users(BASE_DEPOSIT, QUOTE_DEPOSIT);

    let signed_order = SignedOrderArgs {
        user: s.maker.pubkey(),
        market: s.market,
        side: Side::Bid,
        limit_price: 100,
        max_size: 1000,
        nonce: 1,
        expiry: i64::MAX,
    };
    let taker_order = SignedOrderArgs {
        user: s.taker.pubkey(),
        market: s.market,
        side: Side::Ask,
        limit_price: 100,
        max_size: 1000,
        nonce: 2,
        expiry: i64::MAX,
    };
    let maker_signed = sign_order(&s.maker, &signed_order);
    let taker_signed = sign_order(&s.taker, &taker_order);

    // Tamper: pass settle args with a different limit_price than what was signed.
    // The precompile still verifies the original bytes (it sees them inline), but the
    // handler compares hash(canonical_serialize(new_args)) and rejects.
    let mut tampered_args = signed_order;
    tampered_args.limit_price = 110;

    // The handler checks maker_order_hash matches canonical_serialize(maker) (the args),
    // so if we pass the ORIGINAL hash, that's the first check it fails. Pass the tampered
    // hash to bypass that and force the precompile-mismatch path. But the marker PDA is
    // derived from the hash too — we need a coherent setup so the precompile path is hit.
    let tampered_bytes = canonical_serialize(&tampered_args).to_vec();
    let tampered_hash: [u8; 32] = hashv(&[&tampered_bytes]).to_bytes();

    let ed_maker = ed25519_verify_ix(&maker_signed.pk, &maker_signed.sig, &maker_signed.bytes);
    let ed_taker = ed25519_verify_ix(&taker_signed.pk, &taker_signed.sig, &taker_signed.bytes);

    let settle_ix = Instruction::new_with_bytes(
        solana_hybrid_exchange::id(),
        &program_ix::Settle {
            maker: tampered_args,
            taker: taker_order,
            fill_price: 100,
            fill_size: 500,
            maker_order_hash: tampered_hash,
            taker_order_hash: taker_signed.hash,
        }
        .data(),
        program_accounts::Settle {
            operator: s.operator.pubkey(),
            market: s.market,
            maker_user_account: user_account_pda(&s.market, &s.maker.pubkey()).0,
            taker_user_account: user_account_pda(&s.market, &s.taker.pubkey()).0,
            maker_order_marker: order_marker_pda(&tampered_hash).0,
            taker_order_marker: order_marker_pda(&taker_signed.hash).0,
            instructions_sysvar: IX_SYSVAR_ID,
            system_program: SYSTEM_PROGRAM_ID,
        }
        .to_account_metas(None),
    );

    let err = send_tx(&mut s.svm, &[ed_maker, ed_taker, settle_ix], &[&s.operator])
        .expect_err("tampered args should be rejected");
    assert!(
        has_custom_error(&err, ERR_ED25519_DATA_MISMATCH),
        "expected Ed25519DataMismatch, got err={:?} logs={:?}",
        err.err,
        err.meta.logs,
    );
}

#[test]
fn settle_bad_signature_rejected() {
    let mut s = setup_market_with_users(BASE_DEPOSIT, QUOTE_DEPOSIT);

    let maker_order = SignedOrderArgs {
        user: s.maker.pubkey(),
        market: s.market,
        side: Side::Bid,
        limit_price: 100,
        max_size: 1000,
        nonce: 1,
        expiry: i64::MAX,
    };
    let taker_order = SignedOrderArgs {
        user: s.taker.pubkey(),
        market: s.market,
        side: Side::Ask,
        limit_price: 100,
        max_size: 1000,
        nonce: 2,
        expiry: i64::MAX,
    };
    let mut maker_signed = sign_order(&s.maker, &maker_order);
    let taker_signed = sign_order(&s.taker, &taker_order);

    // Flip a byte in the maker signature. The Ed25519 precompile must reject.
    maker_signed.sig[0] ^= 0xAA;

    let ixs = build_settle_ixs(
        &s.market,
        &s.operator.pubkey(),
        maker_order,
        taker_order,
        &maker_signed,
        &taker_signed,
        100,
        500,
    );
    let err =
        send_tx(&mut s.svm, &ixs, &[&s.operator]).expect_err("bad signature should be rejected");
    // The precompile aborts the tx with an InstructionError on the first ix (the maker
    // Ed25519 verify). It's NOT one of our 6000+ Anchor codes — confirm the program
    // never ran by checking the error is on ix 0 and no settle logs were emitted.
    let dbg = format!("{:?}", err.err);
    assert!(
        dbg.contains("InstructionError(0,"),
        "expected precompile error on ix 0, got err={dbg}",
    );
    assert!(
        !err.meta
            .logs
            .iter()
            .any(|l| l.contains("Instruction: Settle")),
        "settle handler should not run when precompile fails, logs={:?}",
        err.meta.logs,
    );
}

#[test]
fn settle_expired_rejected() {
    let mut s = setup_market_with_users(BASE_DEPOSIT, QUOTE_DEPOSIT);

    // LiteSVM's clock defaults to unix_timestamp=0, so an expiry of 1 wouldn't be in the
    // past — advance the clock past it.
    let mut clock = s
        .svm
        .get_sysvar::<anchor_lang::solana_program::clock::Clock>();
    clock.unix_timestamp = 1_000_000;
    s.svm
        .set_sysvar::<anchor_lang::solana_program::clock::Clock>(&clock);

    let maker_order = SignedOrderArgs {
        user: s.maker.pubkey(),
        market: s.market,
        side: Side::Bid,
        limit_price: 100,
        max_size: 1000,
        nonce: 1,
        expiry: 1, // long past
    };
    let taker_order = SignedOrderArgs {
        user: s.taker.pubkey(),
        market: s.market,
        side: Side::Ask,
        limit_price: 100,
        max_size: 1000,
        nonce: 2,
        expiry: i64::MAX,
    };
    let maker_signed = sign_order(&s.maker, &maker_order);
    let taker_signed = sign_order(&s.taker, &taker_order);

    let ixs = build_settle_ixs(
        &s.market,
        &s.operator.pubkey(),
        maker_order,
        taker_order,
        &maker_signed,
        &taker_signed,
        100,
        500,
    );
    let err =
        send_tx(&mut s.svm, &ixs, &[&s.operator]).expect_err("expired order should be rejected");
    assert!(
        has_custom_error(&err, ERR_ORDER_EXPIRED),
        "expected OrderExpired, got err={:?} logs={:?}",
        err.err,
        err.meta.logs,
    );
}

#[test]
fn settle_same_side_rejected() {
    let mut s = setup_market_with_users(BASE_DEPOSIT, QUOTE_DEPOSIT);

    let maker_order = SignedOrderArgs {
        user: s.maker.pubkey(),
        market: s.market,
        side: Side::Bid,
        limit_price: 100,
        max_size: 1000,
        nonce: 1,
        expiry: i64::MAX,
    };
    let taker_order = SignedOrderArgs {
        user: s.taker.pubkey(),
        market: s.market,
        side: Side::Bid, // same side
        limit_price: 100,
        max_size: 1000,
        nonce: 2,
        expiry: i64::MAX,
    };
    let maker_signed = sign_order(&s.maker, &maker_order);
    let taker_signed = sign_order(&s.taker, &taker_order);

    let ixs = build_settle_ixs(
        &s.market,
        &s.operator.pubkey(),
        maker_order,
        taker_order,
        &maker_signed,
        &taker_signed,
        100,
        500,
    );
    let err = send_tx(&mut s.svm, &ixs, &[&s.operator]).expect_err("same-side should be rejected");
    assert!(
        has_custom_error(&err, ERR_SAME_SIDE),
        "expected SameSide, got err={:?} logs={:?}",
        err.err,
        err.meta.logs,
    );
}

#[test]
fn settle_price_does_not_cross_rejected() {
    let mut s = setup_market_with_users(BASE_DEPOSIT, QUOTE_DEPOSIT);

    let maker_order = SignedOrderArgs {
        user: s.maker.pubkey(),
        market: s.market,
        side: Side::Bid,
        limit_price: 50, // bid below ask
        max_size: 1000,
        nonce: 1,
        expiry: i64::MAX,
    };
    let taker_order = SignedOrderArgs {
        user: s.taker.pubkey(),
        market: s.market,
        side: Side::Ask,
        limit_price: 100,
        max_size: 1000,
        nonce: 2,
        expiry: i64::MAX,
    };
    let maker_signed = sign_order(&s.maker, &maker_order);
    let taker_signed = sign_order(&s.taker, &taker_order);

    let ixs = build_settle_ixs(
        &s.market,
        &s.operator.pubkey(),
        maker_order,
        taker_order,
        &maker_signed,
        &taker_signed,
        75,
        500,
    );
    let err = send_tx(&mut s.svm, &ixs, &[&s.operator])
        .expect_err("non-crossing prices should be rejected");
    assert!(
        has_custom_error(&err, ERR_PRICE_DOES_NOT_CROSS),
        "expected PriceDoesNotCross, got err={:?} logs={:?}",
        err.err,
        err.meta.logs,
    );
}

#[test]
fn settle_fill_price_out_of_range() {
    let mut s = setup_market_with_users(BASE_DEPOSIT, QUOTE_DEPOSIT);

    // bid @ 100, ask @ 50 → crosses. Range is [50, 100]. fill_price=200 is out of range.
    let maker_order = SignedOrderArgs {
        user: s.maker.pubkey(),
        market: s.market,
        side: Side::Bid,
        limit_price: 100,
        max_size: 1000,
        nonce: 1,
        expiry: i64::MAX,
    };
    let taker_order = SignedOrderArgs {
        user: s.taker.pubkey(),
        market: s.market,
        side: Side::Ask,
        limit_price: 50,
        max_size: 1000,
        nonce: 2,
        expiry: i64::MAX,
    };
    let maker_signed = sign_order(&s.maker, &maker_order);
    let taker_signed = sign_order(&s.taker, &taker_order);

    let ixs = build_settle_ixs(
        &s.market,
        &s.operator.pubkey(),
        maker_order,
        taker_order,
        &maker_signed,
        &taker_signed,
        200,
        500,
    );
    let err = send_tx(&mut s.svm, &ixs, &[&s.operator])
        .expect_err("out-of-range fill price should be rejected");
    assert!(
        has_custom_error(&err, ERR_FILL_PRICE_OUT_OF_RANGE),
        "expected FillPriceOutOfRange, got err={:?} logs={:?}",
        err.err,
        err.meta.logs,
    );
}

#[test]
fn settle_buyer_underflow_rejected() {
    // Buyer deposits only 1 quote unit (and some base for the seller side).
    // quote_for(100, 1_000_000) = 100_000_000 / 1_000_000 = 100 quote units → buyer can't pay.
    let mut s = setup_market_with_users(10_000_000, 1);

    let maker_order = SignedOrderArgs {
        user: s.maker.pubkey(),
        market: s.market,
        side: Side::Bid,
        limit_price: 100,
        max_size: 10_000_000,
        nonce: 1,
        expiry: i64::MAX,
    };
    let taker_order = SignedOrderArgs {
        user: s.taker.pubkey(),
        market: s.market,
        side: Side::Ask,
        limit_price: 100,
        max_size: 10_000_000,
        nonce: 2,
        expiry: i64::MAX,
    };
    let maker_signed = sign_order(&s.maker, &maker_order);
    let taker_signed = sign_order(&s.taker, &taker_order);

    let ixs = build_settle_ixs(
        &s.market,
        &s.operator.pubkey(),
        maker_order,
        taker_order,
        &maker_signed,
        &taker_signed,
        100,
        1_000_000,
    );
    let err =
        send_tx(&mut s.svm, &ixs, &[&s.operator]).expect_err("buyer underflow should be rejected");
    assert!(
        has_custom_error(&err, ERR_INSUFFICIENT_FREE_BALANCE),
        "expected InsufficientFreeBalance, got err={:?} logs={:?}",
        err.err,
        err.meta.logs,
    );
}
