use super::credentials::publish;
use quotabar::{
    credentials::Platform,
    provider_requests::{self, Parameters},
};
use std::{ptr, slice};
/// # Safety
/// Supply length readable bytes and valid result locations. Free every returned
/// request or discovery response using qb_secret_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_provider_request(
    mode: i32,
    data: *const u8,
    length: usize,
    output: *mut *mut u8,
    out_length: *mut usize,
) -> i32 {
    let (Some(output), Some(out_length)) =
        (unsafe { output.as_mut() }, unsafe { out_length.as_mut() })
    else {
        return -1;
    };
    *output = ptr::null_mut();
    *out_length = 0;
    if data.is_null() || length == 0 {
        return -1;
    }
    let platform = if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Mac
    };
    let result = std::panic::catch_unwind(|| {
        let input = unsafe { slice::from_raw_parts(data, length) };
        match mode {
            0 => {
                let params: Parameters = serde_json::from_slice(input).ok()?;
                serde_json::to_vec(&provider_requests::build(&params, platform)).ok()
            }
            1 | 2 => {
                let response = serde_json::from_slice(input).ok()?;
                serde_json::to_vec(&provider_requests::discovery(
                    &response,
                    mode == 2,
                    platform,
                ))
                .ok()
            }
            _ => None,
        }
    });
    if let Ok(Some(value)) = result {
        publish(value, output, out_length);
        0
    } else {
        -1
    }
}
