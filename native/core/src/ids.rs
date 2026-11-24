//! Block / lane / rule ids (Python: `graph.new_id()` = 8 hex digits of a uuid4).
//!
//! Tests and the comparison with the Python app switch to a sequence (`id000001`, `id000002`, ...) so the ids
//! made while validating a graph can be compared one by one.

use std::cell::Cell;
use std::hash::{BuildHasher, Hasher};

thread_local! {
    static SEQUENCE: Cell<Option<u32>> = const { Cell::new(None) };
}

/// From now on (this thread), ids are `id000001`, `id000002`, ...
pub fn use_sequence() {
    SEQUENCE.with(|s| s.set(Some(0)));
}

pub fn new_id() -> String {
    if let Some(n) = SEQUENCE.with(|s| s.get().map(|n| n + 1)) {
        SEQUENCE.with(|s| s.set(Some(n)));
        return format!("id{n:06}");
    }
    // random: the std hasher is seeded randomly per process; mix in the time and a counter
    thread_local!(static COUNT: Cell<u64> = const { Cell::new(0) });
    let c = COUNT.with(|c| {
        c.set(c.get() + 1);
        c.get()
    });
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(c);
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    format!("{:08x}", h.finish() as u32)
}
