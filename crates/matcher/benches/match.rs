use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use matcher::{Book, NewOrder, Price, Side, Size};

fn book_with_resting_asks(price_scale: u64, levels: u64, size_per: u64, start_price: u64) -> Book {
    let mut book = Book::with_capacity(price_scale, levels as usize);
    for p in 0..levels {
        book.submit(
            NewOrder {
                side: Side::Ask,
                limit_price: Price(start_price + p),
                max_size: Size(size_per),
            },
            |_| {},
        )
        .unwrap();
    }
    book
}

fn book_with_levels_both_sides(price_scale: u64, levels: u64, size_per: u64) -> Book {
    let mut book = Book::with_capacity(price_scale, levels as usize * 2);
    for p in 0..levels {
        book.submit(
            NewOrder {
                side: Side::Bid,
                limit_price: Price(1_000 - p),
                max_size: Size(size_per),
            },
            |_| {},
        )
        .unwrap();
        book.submit(
            NewOrder {
                side: Side::Ask,
                limit_price: Price(2_000 + p),
                max_size: Size(size_per),
            },
            |_| {},
        )
        .unwrap();
    }
    book
}

/// Pure insertion: empty book, single resting bid, no cross.
fn bench_submit_rest_no_cross(c: &mut Criterion) {
    c.bench_function("submit_rest_no_cross", |b| {
        b.iter_batched_ref(
            || Book::with_capacity(1, 1024),
            |book| {
                book.submit(
                    black_box(NewOrder {
                        side: Side::Bid,
                        limit_price: Price(50),
                        max_size: Size(100),
                    }),
                    |_| {},
                )
                .unwrap();
            },
            BatchSize::SmallInput,
        );
    });
}

/// Simplest match: one resting ask, taker bid fully consumes it.
fn bench_submit_consume_one_maker(c: &mut Criterion) {
    c.bench_function("submit_consume_one_maker", |b| {
        b.iter_batched_ref(
            || book_with_resting_asks(1, 1, 100, 10),
            |book| {
                book.submit(
                    black_box(NewOrder {
                        side: Side::Bid,
                        limit_price: Price(10),
                        max_size: Size(100),
                    }),
                    |_| {},
                )
                .unwrap();
            },
            BatchSize::SmallInput,
        );
    });
}

/// Typical-microstructure baseline: sweep 10 price levels in one cross.
fn bench_submit_walk_10_levels_full_sweep(c: &mut Criterion) {
    c.bench_function("submit_walk_10_levels_full_sweep", |b| {
        b.iter_batched_ref(
            || book_with_resting_asks(1, 10, 10, 10),
            |book| {
                book.submit(
                    black_box(NewOrder {
                        side: Side::Bid,
                        limit_price: Price(20),
                        max_size: Size(100),
                    }),
                    |_| {},
                )
                .unwrap();
            },
            BatchSize::SmallInput,
        );
    });
}

/// Swap-to-arena trigger: if this bench shows the intrusive-list cache cost biting at
/// scale, that's the signal to migrate the slab/list off `Vec<OrderNode>` onto a
/// purpose-built arena. 100-level full sweep.
fn bench_submit_walk_100_levels_full_sweep(c: &mut Criterion) {
    c.bench_function("submit_walk_100_levels_full_sweep", |b| {
        b.iter_batched_ref(
            || book_with_resting_asks(1, 100, 10, 100),
            |book| {
                book.submit(
                    black_box(NewOrder {
                        side: Side::Bid,
                        limit_price: Price(200),
                        max_size: Size(1_000),
                    }),
                    |_| {},
                )
                .unwrap();
            },
            BatchSize::LargeInput,
        );
    });
}

/// Match-then-insert: consume one maker, rest the remainder.
fn bench_submit_partial_then_rest(c: &mut Criterion) {
    c.bench_function("submit_partial_then_rest", |b| {
        b.iter_batched_ref(
            || book_with_resting_asks(1, 1, 50, 10),
            |book| {
                book.submit(
                    black_box(NewOrder {
                        side: Side::Bid,
                        limit_price: Price(10),
                        max_size: Size(100),
                    }),
                    |_| {},
                )
                .unwrap();
            },
            BatchSize::SmallInput,
        );
    });
}

/// Isolated cancel cost: rebuild a single-order book per iter, measure only the cancel.
fn bench_cancel_resting_at_known_id(c: &mut Criterion) {
    c.bench_function("cancel_resting_at_known_id", |b| {
        b.iter_batched(
            || {
                let mut book = Book::with_capacity(1, 4);
                let id = book
                    .submit(
                        NewOrder {
                            side: Side::Bid,
                            limit_price: Price(50),
                            max_size: Size(100),
                        },
                        |_| {},
                    )
                    .unwrap();
                (book, id)
            },
            |(mut book, id)| {
                book.cancel(black_box(id));
            },
            BatchSize::SmallInput,
        );
    });
}

/// Top-of-book read with depth: 50 levels per side, take top 10.
fn bench_snapshot_top_10_with_50_levels(c: &mut Criterion) {
    c.bench_function("snapshot_top_10_with_50_levels", |b| {
        b.iter_batched_ref(
            || book_with_levels_both_sides(1, 50, 10),
            |book| {
                black_box(book.snapshot(black_box(10)));
            },
            BatchSize::LargeInput,
        );
    });
}

criterion_group!(
    benches,
    bench_submit_rest_no_cross,
    bench_submit_consume_one_maker,
    bench_submit_walk_10_levels_full_sweep,
    bench_submit_walk_100_levels_full_sweep,
    bench_submit_partial_then_rest,
    bench_cancel_resting_at_known_id,
    bench_snapshot_top_10_with_50_levels,
);
criterion_main!(benches);
