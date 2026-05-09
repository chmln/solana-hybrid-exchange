//! Smoke test: load the program `.so` into LiteSVM and airdrop a payer.

use {litesvm::LiteSVM, solana_keypair::Keypair, solana_signer::Signer};

#[test]
fn harness_smoke() {
    let mut svm = LiteSVM::new();
    let bytes = include_bytes!("../../../target/deploy/solana_hybrid_exchange.so");
    svm.add_program(solana_hybrid_exchange::id(), bytes)
        .unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();
}
