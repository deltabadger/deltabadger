//! A hostile bare number in a venue body is rejected without allocating in proportion to it. This binary counts the bytes
//! the current thread allocates, so it holds nothing else.
use deltabadger::venue::http::{decode_json, DecodeError};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;
thread_local! { static BYTES: Cell<usize> = const { Cell::new(0) }; }
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { BYTES.with(|b| b.set(b.get() + l.size())); unsafe { System.alloc(l) } }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { unsafe { System.dealloc(p, l) } }
}
#[global_allocator]
static A: Counting = Counting;

fn allocated_by(f: impl FnOnce()) -> usize { let before = BYTES.with(Cell::get); f(); BYTES.with(Cell::get) - before }

#[test]
fn a_multi_megabyte_bare_number_is_rejected_without_copying_it() {
    let four_mb = 4 << 20;
    for body in [format!(r#"{{"filled_qty":1.{}}}"#, "1".repeat(four_mb)), format!(r#"{{"filled_qty":{}}}"#, "9".repeat(four_mb)),
                 format!(r#"{{"filled_qty":1e{}}}"#, "9".repeat(four_mb)), format!(r#"[0.{}1]"#, "0".repeat(four_mb))] {
        let mut out = None;
        let bytes = allocated_by(|| out = Some(decode_json(&body)));
        let Some(Err(DecodeError::OutOfRange(m))) = out else { panic!("must be rejected: {:?}", out.map(|r| r.map(|_| ()))) };
        assert!(m.len() < 200, "a short diagnostic: {} bytes", m.len());
        assert!(bytes < 16 << 10, "allocated {bytes} bytes for a {}-byte body", body.len());
    }
}
