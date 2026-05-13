use matcher::{Book, NewOrder, Price, PriceLevel, Side, Size};
use std::num::NonZeroU64;

#[test]
fn empty_book_submit_rests() {
    let mut book = Book::new(NonZeroU64::MIN);
    let mut fills = Vec::new();
    let id = book
        .submit(
            NewOrder {
                side: Side::Bid,
                limit_price: Price(10),
                max_size: Size(100),
            },
            |f| fills.push(f),
        )
        .unwrap();
    assert!(fills.is_empty());
    let view = book.get(id).expect("rests");
    assert_eq!(view.remaining, Size(100));
    assert_eq!(view.max_size, Size(100));
    assert_eq!(book.best_bid(), Some(Price(10)));
    assert_eq!(book.best_ask(), None);
}

#[test]
fn exact_size_match() {
    let mut book = Book::new(NonZeroU64::MIN);
    let ask_id = book
        .submit(
            NewOrder {
                side: Side::Ask,
                limit_price: Price(10),
                max_size: Size(100),
            },
            |_| {},
        )
        .unwrap();
    let mut fills = Vec::new();
    let bid_id = book
        .submit(
            NewOrder {
                side: Side::Bid,
                limit_price: Price(10),
                max_size: Size(100),
            },
            |f| fills.push(f),
        )
        .unwrap();
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].fill_size, Size(100));
    assert_eq!(fills[0].fill_price, Price(10));
    assert_eq!(fills[0].maker_id, ask_id);
    assert_eq!(fills[0].taker_id, bid_id);
    assert!(book.get(ask_id).is_none());
    assert!(book.get(bid_id).is_none());
    assert_eq!(book.best_bid(), None);
    assert_eq!(book.best_ask(), None);
}

#[test]
fn partial_taker() {
    let mut book = Book::new(NonZeroU64::MIN);
    book.submit(
        NewOrder {
            side: Side::Ask,
            limit_price: Price(10),
            max_size: Size(50),
        },
        |_| {},
    )
    .unwrap();
    let mut fills = Vec::new();
    let bid_id = book
        .submit(
            NewOrder {
                side: Side::Bid,
                limit_price: Price(10),
                max_size: Size(100),
            },
            |f| fills.push(f),
        )
        .unwrap();
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].fill_size, Size(50));
    let view = book.get(bid_id).expect("bid rests");
    assert_eq!(view.remaining, Size(50));
    assert_eq!(book.best_bid(), Some(Price(10)));
    assert_eq!(book.best_ask(), None);
}

#[test]
fn partial_maker() {
    let mut book = Book::new(NonZeroU64::MIN);
    let ask_id = book
        .submit(
            NewOrder {
                side: Side::Ask,
                limit_price: Price(10),
                max_size: Size(100),
            },
            |_| {},
        )
        .unwrap();
    let mut fills = Vec::new();
    book.submit(
        NewOrder {
            side: Side::Bid,
            limit_price: Price(10),
            max_size: Size(50),
        },
        |f| fills.push(f),
    )
    .unwrap();
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].fill_size, Size(50));
    let view = book.get(ask_id).expect("ask rests");
    assert_eq!(view.remaining, Size(50));
    assert_eq!(book.best_ask(), Some(Price(10)));
}

#[test]
fn multi_level_walk() {
    let mut book = Book::new(NonZeroU64::MIN);
    for price in [10u64, 11, 12] {
        book.submit(
            NewOrder {
                side: Side::Ask,
                limit_price: Price(price),
                max_size: Size(10),
            },
            |_| {},
        )
        .unwrap();
    }
    let mut fills = Vec::new();
    let bid_id = book
        .submit(
            NewOrder {
                side: Side::Bid,
                limit_price: Price(12),
                max_size: Size(25),
            },
            |f| fills.push(f),
        )
        .unwrap();
    assert_eq!(fills.len(), 3);
    assert_eq!(fills[0].fill_price, Price(10));
    assert_eq!(fills[0].fill_size, Size(10));
    assert_eq!(fills[1].fill_price, Price(11));
    assert_eq!(fills[1].fill_size, Size(10));
    assert_eq!(fills[2].fill_price, Price(12));
    assert_eq!(fills[2].fill_size, Size(5));
    assert!(book.get(bid_id).is_none());
    assert_eq!(book.best_ask(), Some(Price(12)));
    let snap = book.snapshot(usize::MAX);
    assert_eq!(
        snap.asks,
        vec![PriceLevel {
            price: Price(12),
            size: Size(5)
        }]
    );
}

#[test]
fn no_cross_rests_separately() {
    let mut book = Book::new(NonZeroU64::MIN);
    let mut fills = Vec::new();
    book.submit(
        NewOrder {
            side: Side::Bid,
            limit_price: Price(9),
            max_size: Size(100),
        },
        |f| fills.push(f),
    )
    .unwrap();
    book.submit(
        NewOrder {
            side: Side::Ask,
            limit_price: Price(10),
            max_size: Size(100),
        },
        |f| fills.push(f),
    )
    .unwrap();
    assert!(fills.is_empty());
    assert_eq!(book.best_bid(), Some(Price(9)));
    assert_eq!(book.best_ask(), Some(Price(10)));
}

#[test]
fn cancel_resting() {
    let mut book = Book::new(NonZeroU64::MIN);
    let id = book
        .submit(
            NewOrder {
                side: Side::Bid,
                limit_price: Price(10),
                max_size: Size(100),
            },
            |_| {},
        )
        .unwrap();
    let info = book.cancel(id).expect("cancel resting");
    assert_eq!(info.side, Side::Bid);
    assert_eq!(info.limit_price, Price(10));
    assert_eq!(info.remaining, Size(100));
    assert!(book.get(id).is_none());
    assert_eq!(book.best_bid(), None);
}

#[test]
fn cancel_stale_id() {
    let mut book = Book::new(NonZeroU64::MIN);
    let ask_id = book
        .submit(
            NewOrder {
                side: Side::Ask,
                limit_price: Price(10),
                max_size: Size(100),
            },
            |_| {},
        )
        .unwrap();
    book.submit(
        NewOrder {
            side: Side::Bid,
            limit_price: Price(10),
            max_size: Size(100),
        },
        |_| {},
    )
    .unwrap();
    assert!(book.get(ask_id).is_none());
    assert!(book.cancel(ask_id).is_none());
}

#[test]
fn fifo_within_level() {
    let mut book = Book::new(NonZeroU64::MIN);
    let first = book
        .submit(
            NewOrder {
                side: Side::Ask,
                limit_price: Price(10),
                max_size: Size(50),
            },
            |_| {},
        )
        .unwrap();
    let _second = book
        .submit(
            NewOrder {
                side: Side::Ask,
                limit_price: Price(10),
                max_size: Size(50),
            },
            |_| {},
        )
        .unwrap();
    let mut fills = Vec::new();
    book.submit(
        NewOrder {
            side: Side::Bid,
            limit_price: Price(10),
            max_size: Size(50),
        },
        |f| fills.push(f),
    )
    .unwrap();
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].maker_id, first);
}

#[test]
fn self_trade_allowed() {
    let mut book = Book::new(NonZeroU64::MIN);
    book.submit(
        NewOrder {
            side: Side::Ask,
            limit_price: Price(10),
            max_size: Size(100),
        },
        |_| {},
    )
    .unwrap();
    let mut fills = Vec::new();
    book.submit(
        NewOrder {
            side: Side::Bid,
            limit_price: Price(10),
            max_size: Size(100),
        },
        |f| fills.push(f),
    )
    .unwrap();
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].fill_size, Size(100));
}
