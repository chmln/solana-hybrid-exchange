use matcher::{Book, Fill, NewOrder, OrderId, Price, Side, Size, SubmitError};
use proptest::prelude::*;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroU64;

#[derive(Clone, Debug)]
enum Action {
    Submit { side: Side, price: u64, size: u64 },
    Cancel { id_index: usize },
}

fn action_strategy() -> impl Strategy<Value = Action> {
    prop_oneof![
        8 => (any::<bool>(), 1u64..=20, 1u64..=1000).prop_map(|(is_bid, price, size)| {
            Action::Submit {
                side: if is_bid { Side::Bid } else { Side::Ask },
                price,
                size,
            }
        }),
        2 => (0usize..256).prop_map(|id_index| Action::Cancel { id_index }),
    ]
}

fn check_fill_against_orders(
    fill: &Fill,
    submitted: &HashMap<OrderId, NewOrder>,
) -> Result<(), TestCaseError> {
    let maker = submitted.get(&fill.maker_id).expect("maker submitted");
    let taker = submitted.get(&fill.taker_id).expect("taker submitted");
    prop_assert!(fill.fill_size.0 > 0, "fill_size must be > 0");
    prop_assert_eq!(maker.side, taker.side.opposite(), "opposing sides");
    prop_assert_eq!(
        fill.fill_price,
        maker.limit_price,
        "fill price equals maker limit"
    );
    let (bid, ask) = match maker.side {
        Side::Bid => (maker, taker),
        Side::Ask => (taker, maker),
    };
    prop_assert!(bid.limit_price >= ask.limit_price, "bid >= ask crosses");
    prop_assert!(
        fill.fill_price >= ask.limit_price && fill.fill_price <= bid.limit_price,
        "fill price within crossing band"
    );
    Ok(())
}

proptest! {
    #[test]
    fn book_invariants_hold(actions in proptest::collection::vec(action_strategy(), 1..200)) {
        // price_scale = 1 means every (price >= 1, size >= 1) order passes the precondition.
        let mut book = Book::new(NonZeroU64::MIN);
        let mut submitted: HashMap<OrderId, NewOrder> = HashMap::new();
        let mut order_of_submission: Vec<OrderId> = Vec::new();
        let mut fills_by_id: HashMap<OrderId, u64> = HashMap::new();
        let mut cancelled: HashSet<OrderId> = HashSet::new();

        for action in actions {
            match action {
                Action::Submit { side, price, size } => {
                    let order = NewOrder {
                        side,
                        limit_price: Price(price),
                        max_size: Size(size),
                    };
                    let mut new_fills: Vec<Fill> = Vec::new();
                    let id = book
                        .submit(order, |f| new_fills.push(f))
                        .expect("price>=1 size>=1 with scale=1 passes precondition");
                    submitted.insert(id, order);
                    order_of_submission.push(id);

                    for fill in &new_fills {
                        check_fill_against_orders(fill, &submitted)?;
                        *fills_by_id.entry(fill.maker_id).or_insert(0) += fill.fill_size.0;
                        *fills_by_id.entry(fill.taker_id).or_insert(0) += fill.fill_size.0;
                    }
                }
                Action::Cancel { id_index } => {
                    if order_of_submission.is_empty() {
                        continue;
                    }
                    let id = order_of_submission[id_index % order_of_submission.len()];
                    if let Some(info) = book.cancel(id) {
                        let order = submitted.get(&id).expect("cancelled id was submitted");
                        prop_assert_eq!(info.side, order.side, "cancel side matches");
                        prop_assert_eq!(
                            info.limit_price, order.limit_price,
                            "cancel price matches"
                        );
                        let consumed = fills_by_id.get(&id).copied().unwrap_or(0);
                        prop_assert_eq!(
                            info.remaining.0 + consumed,
                            order.max_size.0,
                            "cancel remaining + consumed == max_size"
                        );
                        cancelled.insert(id);
                    }
                }
            }

            let snap = book.snapshot(usize::MAX);
            for w in snap.bids.windows(2) {
                prop_assert!(w[0].price > w[1].price, "bids strictly descending");
            }
            for w in snap.asks.windows(2) {
                prop_assert!(w[0].price < w[1].price, "asks strictly ascending");
            }
            for level in snap.bids.iter().chain(snap.asks.iter()) {
                prop_assert!(level.size.0 > 0, "no dead levels");
            }

            prop_assert_eq!(
                book.best_bid(),
                snap.bids.first().map(|l| l.price),
                "best_bid matches snapshot top"
            );
            prop_assert_eq!(
                book.best_ask(),
                snap.asks.first().map(|l| l.price),
                "best_ask matches snapshot top"
            );

            for (&id, order) in &submitted {
                let consumed = fills_by_id.get(&id).copied().unwrap_or(0);
                prop_assert!(consumed <= order.max_size.0, "no overfill");

                if cancelled.contains(&id) {
                    prop_assert!(book.get(id).is_none(), "cancelled id must be gone");
                    continue;
                }
                match book.get(id) {
                    Some(view) => {
                        prop_assert_eq!(view.side, order.side);
                        prop_assert_eq!(view.limit_price, order.limit_price);
                        prop_assert_eq!(view.max_size, order.max_size);
                        prop_assert_eq!(
                            view.remaining.0 + consumed,
                            order.max_size.0,
                            "remaining + consumed == max_size"
                        );
                    }
                    None => {
                        prop_assert_eq!(
                            consumed, order.max_size.0,
                            "missing id must be fully consumed"
                        );
                    }
                }
            }
        }
    }
}

// Targeted test for the no-zero-quote precondition. With price_scale=100, any submit
// with limit_price<100 must be rejected; with limit_price>=100, no fill can produce
// quote_amount=0 regardless of subsequent partial-fill geometry. Replays the original
// crossed-book scenario from the code-artisan review.
#[test]
fn precondition_rejects_dust_and_prevents_crossed_book() {
    let mut book = Book::new(NonZeroU64::new(100).unwrap());

    // Below-scale ask: rejected.
    let err = book
        .submit(
            NewOrder {
                side: Side::Ask,
                limit_price: Price(5),
                max_size: Size(1),
            },
            |_| {},
        )
        .unwrap_err();
    assert_eq!(err, SubmitError::PriceBelowScale);

    // Zero size: rejected even at valid price.
    let err = book
        .submit(
            NewOrder {
                side: Side::Bid,
                limit_price: Price(100),
                max_size: Size(0),
            },
            |_| {},
        )
        .unwrap_err();
    assert_eq!(err, SubmitError::ZeroSize);

    // Valid bid + valid ask: matches cleanly, no crossed book.
    book.submit(
        NewOrder {
            side: Side::Ask,
            limit_price: Price(100),
            max_size: Size(1),
        },
        |_| {},
    )
    .unwrap();
    let mut fills = Vec::new();
    book.submit(
        NewOrder {
            side: Side::Bid,
            limit_price: Price(100),
            max_size: Size(1),
        },
        |f| fills.push(f),
    )
    .unwrap();
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].fill_size, Size(1));
    assert_eq!(book.best_bid(), None);
    assert_eq!(book.best_ask(), None);
}
