//! Slab-backed order storage with niche-packed typed handles.
//!
//! `SlotIdx` is a `NonZeroU32` newtype that stores `raw + 1` internally, so
//! `Option<SlotIdx>` fits in 4 bytes (the `0` encoding is reserved for
//! `None`). This replaces a hand-rolled `u32 + NIL` sentinel at no size cost —
//! `OrderNode` stays one cache line.
//!
//! `SlotIdx` is only constructible from inside this module (via `allocate` or
//! the bounds-checked `lookup_raw`), so by the time one reaches a `Slab`
//! index expression it has already been validated. The slab's `Vec` is
//! append-only — `free` recycles slots via a free-list but never shrinks —
//! so any once-valid `SlotIdx` stays in bounds for the slab's lifetime.

use std::num::NonZeroU32;
use std::ops::{Index, IndexMut};

use crate::types::{Price, Side, Size};

/// Typed slab slot handle. See module docs for the construction contract.
#[repr(transparent)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct SlotIdx(NonZeroU32);

const _: () = assert!(std::mem::size_of::<Option<SlotIdx>>() == std::mem::size_of::<u32>());

impl SlotIdx {
    fn from_raw(raw: u32) -> Option<Self> {
        // raw=0 → NonZeroU32(1); raw=u32::MAX-1 → NonZeroU32(u32::MAX);
        // raw=u32::MAX → 0 → invalid (reserved as the None niche).
        NonZeroU32::new(raw.wrapping_add(1)).map(Self)
    }

    pub(crate) fn raw(self) -> u32 {
        self.0.get() - 1
    }

    fn as_usize(self) -> usize {
        self.raw() as usize
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct OrderNode {
    pub(crate) limit_price: Price,
    pub(crate) max_size: Size,
    pub(crate) remaining: Size,
    pub(crate) prev: Option<SlotIdx>,
    pub(crate) next: Option<SlotIdx>,
    pub(crate) generation: u32,
    pub(crate) side: Side,
    pub(crate) occupied: bool,
}

impl OrderNode {
    fn fresh(generation: u32, occupied: bool) -> Self {
        Self {
            limit_price: Price(0),
            max_size: Size(0),
            remaining: Size(0),
            prev: None,
            next: None,
            generation,
            side: Side::Bid,
            occupied,
        }
    }
}

pub(crate) struct Slab {
    nodes: Vec<OrderNode>,
    free_head: Option<SlotIdx>,
}

impl Slab {
    pub(crate) fn with_capacity(cap: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(cap),
            free_head: None,
        }
    }

    /// Bounds-checked lookup for externally-supplied raw indices (e.g. from
    /// `OrderId`, which is `pub` and could carry any `u64` from a buggy gateway).
    pub(crate) fn lookup_raw(&self, raw_idx: u32) -> Option<SlotIdx> {
        if (raw_idx as usize) < self.nodes.len() {
            SlotIdx::from_raw(raw_idx)
        } else {
            None
        }
    }

    /// Allocates a slot, marking it occupied. Returns `Err` if the slab is at
    /// capacity (`u32::MAX - 1` entries — `u32::MAX` is reserved as the
    /// `Option<SlotIdx>` niche).
    pub(crate) fn allocate(&mut self) -> Result<SlotIdx, SlabFull> {
        if let Some(idx) = self.free_head {
            let reused = &self[idx];
            let next_free = reused.next;
            let generation = reused.generation;
            self.free_head = next_free;
            self[idx] = OrderNode::fresh(generation, true);
            return Ok(idx);
        }
        let raw_idx: u32 = self.nodes.len().try_into().map_err(|_| SlabFull)?;
        let slot = SlotIdx::from_raw(raw_idx).ok_or(SlabFull)?;
        self.nodes.push(OrderNode::fresh(0, true));
        Ok(slot)
    }

    /// Releases a slot to the free list. Bumps the slot's generation so any
    /// outstanding `OrderId` referring to this slot is now stale.
    pub(crate) fn free(&mut self, idx: SlotIdx) {
        let next_free = self.free_head;
        let node = &mut self[idx];
        let next_gen = node.generation.wrapping_add(1);
        *node = OrderNode::fresh(next_gen, false);
        node.next = next_free;
        self.free_head = Some(idx);
    }
}

impl Index<SlotIdx> for Slab {
    type Output = OrderNode;
    #[inline]
    fn index(&self, idx: SlotIdx) -> &OrderNode {
        &self.nodes[idx.as_usize()]
    }
}

impl IndexMut<SlotIdx> for Slab {
    #[inline]
    fn index_mut(&mut self, idx: SlotIdx) -> &mut OrderNode {
        &mut self.nodes[idx.as_usize()]
    }
}

pub(crate) struct SlabFull;
