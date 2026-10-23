use crate::json_api::solve_slices;
use std::alloc::{alloc, dealloc, Layout};

#[no_mangle]
pub extern "C" fn dws_alloc(len: usize) -> *mut u8 {
    unsafe {
        let layout = Layout::from_size_align(len.max(1), 8).unwrap();
        alloc(layout)
    }
}

#[no_mangle]
pub extern "C" fn dws_dealloc(ptr: *mut u8, len: usize) {
    unsafe {
        let layout = Layout::from_size_align(len.max(1), 8).unwrap();
        dealloc(ptr, layout);
    }
}

#[no_mangle]
pub extern "C" fn dws_solve(
    payload_ptr: *const u8,
    payload_len: usize,
    request_ptr: *const u8,
    request_len: usize,
) -> *mut u8 {
    let payload = unsafe { std::slice::from_raw_parts(payload_ptr, payload_len) };
    let request = unsafe { std::slice::from_raw_parts(request_ptr, request_len) };
    let out = match solve_slices(payload, request) {
        Ok(v) => v,
        Err(e) => serde_json::to_vec(&serde_json::json!({ "error": e })).unwrap(),
    };
    let len = out.len();
    let total = len + 4;
    let ptr = dws_alloc(total);
    if ptr.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        let len_bytes = (len as u32).to_le_bytes();
        std::ptr::copy_nonoverlapping(len_bytes.as_ptr(), ptr, 4);
        std::ptr::copy_nonoverlapping(out.as_ptr(), ptr.add(4), len);
    }
    ptr
}
