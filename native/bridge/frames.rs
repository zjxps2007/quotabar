use quotabar::dashboard::{Decoder, MAX_FRAME, write_frame};
use std::{ffi::c_void, ptr, slice};

#[unsafe(no_mangle)]
pub extern "C" fn qb_frame_decoder_new() -> *mut Decoder {
    Box::into_raw(Box::new(Decoder::default()))
}
/// # Safety
/// The decoder must remain live and unmodified during the copy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_frame_decoder_copy(decoder: *const Decoder) -> *mut Decoder {
    unsafe { decoder.as_ref() }.map_or(ptr::null_mut(), |d| Box::into_raw(Box::new(d.clone())))
}
/// # Safety
/// Release a uniquely owned decoder once, outside an active callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_frame_decoder_free(decoder: *mut Decoder) {
    if !decoder.is_null() {
        drop(unsafe { Box::from_raw(decoder) });
    }
}
/// # Safety
/// Supply exclusive access to the decoder and length readable bytes. The callback
/// may copy its frame during the call. It must not retain the pointer or reenter.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_frame_append(
    decoder: *mut Decoder,
    data: *const u8,
    length: usize,
    callback: Option<extern "C" fn(*mut c_void, *const u8, usize)>,
    context: *mut c_void,
) -> i32 {
    let (Some(decoder), Some(callback)) = (unsafe { decoder.as_mut() }, callback) else {
        return -1;
    };
    if length == 0 {
        return 0;
    }
    if data.is_null() {
        return -1;
    }
    match decoder.append(unsafe { slice::from_raw_parts(data, length) }, |frame| {
        callback(context, frame.as_ptr(), frame.len())
    }) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}
/// # Safety
/// Pass a live decoder, with no concurrent append.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_frame_finish(decoder: *const Decoder) -> i32 {
    if unsafe { decoder.as_ref() }.is_some_and(|d| d.finish().is_ok()) {
        0
    } else {
        -1
    }
}
/// # Safety
/// Input is length readable bytes. Release a returned frame with qb_secret_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_frame_encode(
    data: *const u8,
    length: usize,
    output: *mut *mut u8,
    output_length: *mut usize,
) -> i32 {
    let (Some(output), Some(output_length)) = (unsafe { output.as_mut() }, unsafe {
        output_length.as_mut()
    }) else {
        return -1;
    };
    *output = ptr::null_mut();
    *output_length = 0;
    if data.is_null() || !(1..=MAX_FRAME).contains(&length) {
        return -1;
    }
    let mut buffer = Vec::with_capacity(length + 4);
    if write_frame(&mut buffer, unsafe { slice::from_raw_parts(data, length) }).is_err() {
        return -1;
    }
    *output_length = buffer.len();
    *output = Box::into_raw(buffer.into_boxed_slice()).cast();
    0
}
