# solana-hybrid-exchange

Solana exchange with off-chain matching and on-chain settlement. The
operator runs the orderbook but can't touch user funds. Every fill
needs two user signatures verified on-chain through the Ed25519
precompile.

## How it works

Orders are signed messages, not transactions. Fills are the only
on-chain action: the operator submits Ed25519(maker), Ed25519(taker),
and settle in one tx. Vaults only move on deposit and withdraw. Settle
is a ledger edit, not a token transfer.

Token-2022 only. init_market rejects mints with TransferFeeConfig,
TransferHook, NonTransferable, PermanentDelegate, Pausable, or
MintCloseAuthority extensions.

## Build

    anchor build

## Test

    cargo test

LiteSVM runs in-process with the Ed25519 precompile feature on, so
tests don't need solana-test-validator.
