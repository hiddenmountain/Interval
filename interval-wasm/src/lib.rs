//! interval-wasm — raw `cdylib` WASM wrapper exposing interval-core's resolver to JS.
//!
//! No wasm-bindgen: strings cross the boundary via a JSON-over-linear-memory bridge.
//! JS allocates input buffers with `interval_alloc`, writes UTF-8 bytes, calls a
//! resolver export, then reads a JSON result whose `(ptr, len)` are packed into the
//! `u64` return as `(ptr << 32) | len`, and frees both with `interval_dealloc`.
//! interval-core is WASM-safe; this wrapper adds only the linear-memory marshalling.

use std::alloc::{alloc as rust_alloc, dealloc as rust_dealloc, Layout};

/// Allocate `len` bytes in linear memory and return the pointer. JS writes input
/// here (and the resolver returns result buffers allocated the same way). align = 1
/// (UTF-8 bytes). Returns null for `len == 0`, for `len` too large for a valid
/// `Layout`, and on allocation failure — callers must null-check before writing.
#[no_mangle]
pub extern "C" fn interval_alloc(len: usize) -> *mut u8 {
    let Ok(layout) = Layout::from_size_align(len, 1) else {
        return std::ptr::null_mut();
    };
    if layout.size() == 0 {
        return std::ptr::null_mut();
    }
    // SAFETY: layout is valid and non-zero-sized.
    unsafe { rust_alloc(layout) }
}

/// Free a buffer previously returned by `interval_alloc` or a resolver result
/// buffer.
///
/// # Safety
///
/// `ptr` must be a pointer returned by `interval_alloc`/a resolver result that
/// has not already been freed, and `len` must match the original allocation
/// (JS tracks it). Passing anything else corrupts the allocator.
#[no_mangle]
pub unsafe extern "C" fn interval_dealloc(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    let Ok(layout) = Layout::from_size_align(len, 1) else {
        return;
    };
    if layout.size() == 0 {
        return;
    }
    // SAFETY: ptr/len pair came from interval_alloc / pack_json (align 1).
    unsafe { rust_dealloc(ptr, layout) }
}

/// Resolve a chord symbol (Roman numeral OR letter chord) against a key into a JSON
/// `{ rendered, relative, root, intervals }` (or JSON `null` if it can't be parsed).
/// `symbol` / `mode` are UTF-8 byte ranges in linear memory; `key_root` is a pitch
/// class 0..11. Returns the result buffer's `(ptr << 32) | len` (0 on allocation
/// failure); JS reads it then calls `interval_dealloc(ptr, len)`.
///
/// # Safety
///
/// `[symbol_ptr, symbol_ptr + symbol_len)` and `[mode_ptr, mode_ptr + mode_len)`
/// must be initialized byte ranges in linear memory (null/0 is allowed and reads
/// as the empty string).
#[no_mangle]
pub unsafe extern "C" fn interval_describe_chord(
    symbol_ptr: *const u8,
    symbol_len: usize,
    key_root: u8,
    mode_ptr: *const u8,
    mode_len: usize,
) -> u64 {
    let symbol = read_str(symbol_ptr, symbol_len);
    let mode = read_str(mode_ptr, mode_len);
    let json = match interval_core::introspect::describe_chord_in_key(&symbol, key_root, &mode) {
        Some(desc) => serde_json::to_string(&desc).unwrap_or_else(|_| "null".to_string()),
        None => "null".to_string(),
    };
    pack_json(json)
}

/// Read a UTF-8 string from a linear-memory byte range (lossy; empty for null/0).
fn read_str(ptr: *const u8, len: usize) -> String {
    if ptr.is_null() || len == 0 {
        return String::new();
    }
    // SAFETY: JS guarantees [ptr, ptr+len) is an initialized buffer it wrote.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    String::from_utf8_lossy(bytes).into_owned()
}

/// Copy a string into a fresh linear-memory buffer and pack `(ptr << 32) | len`.
/// The buffer is leaked to JS, which frees it via `interval_dealloc(ptr, len)`.
/// Returns 0 for an empty string or on allocation failure.
fn pack_json(s: String) -> u64 {
    let bytes = s.into_bytes();
    let len = bytes.len();
    let ptr = interval_alloc(len);
    if ptr.is_null() {
        return 0;
    }
    // SAFETY: interval_alloc returned a non-null buffer of `len` bytes.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, len);
    }
    ((ptr as u64) << 32) | (len as u64)
}
