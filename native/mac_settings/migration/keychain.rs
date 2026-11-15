use super::LegacyItems;
use crate::codex_sessions::Secret;
use objc2::{
    class, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::AnyObject,
};
use objc2_core_foundation::{CFData, CFDictionary, CFRetained, CFType};
use objc2_foundation::{NSDictionary, NSNumber, NSString};
use objc2_security::{
    SecItemCopyMatching, SecItemDelete, SecKeychain, kSecAttrAccount, kSecAttrService, kSecClass,
    kSecClassGenericPassword, kSecMatchLimit, kSecMatchLimitOne, kSecReturnData,
    kSecUseAuthenticationContext,
};
use std::{
    ptr::{self, NonNull},
    sync::Mutex,
};
use uuid::Uuid;

#[link(name = "LocalAuthentication", kind = "framework")]
unsafe extern "C" {}

pub struct Keychain;

// The legacy Security setting is process-wide. Serialize its complete lifetime,
// and restore the previous value on every exit.
#[allow(deprecated)]
fn without_ui<T>(operation: impl FnOnce() -> T) -> Option<T> {
    static LOCK: Mutex<()> = Mutex::new(());
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut previous = 0u8;
    unsafe {
        if SecKeychain::user_interaction_allowed(NonNull::from_mut(&mut previous)) != 0
            || SecKeychain::set_user_interaction_allowed(false) != 0
        {
            return None;
        }
    }
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            unsafe {
                SecKeychain::set_user_interaction_allowed(self.0);
            }
        }
    }
    let _restore = Restore(previous != 0);
    Some(autoreleasepool(|_| operation()))
}

fn query(id: Uuid, read: bool) -> Retained<NSDictionary<NSString, AnyObject>> {
    let username = NSString::from_str(&id.hyphenated().to_string().to_uppercase());
    let service = NSString::from_str("com.aiusage.lite.credentials");
    let yes = NSNumber::numberWithBool(true);
    unsafe {
        let context: Retained<AnyObject> = msg_send![class!(LAContext), new];
        let _: () = msg_send![&*context, setInteractionNotAllowed: true];
        let class: &NSString = kSecClassGenericPassword.as_ref();
        let one: &NSString = kSecMatchLimitOne.as_ref();
        let keys: [&NSString; 6] = [
            kSecClass.as_ref(),
            kSecAttrService.as_ref(),
            kSecAttrAccount.as_ref(),
            kSecUseAuthenticationContext.as_ref(),
            kSecReturnData.as_ref(),
            kSecMatchLimit.as_ref(),
        ];
        let values: [&AnyObject; 6] = [
            class.as_ref(),
            service.as_ref(),
            username.as_ref(),
            &context,
            yes.as_ref(),
            one.as_ref(),
        ];
        let count = if read { 6 } else { 4 };
        NSDictionary::from_slices(&keys[..count], &values[..count])
    }
}

impl LegacyItems for Keychain {
    fn read(&mut self, id: Uuid) -> Option<Secret> {
        without_ui(|| {
            let query = query(id, true);
            let dictionary: &CFDictionary<NSString, AnyObject> = (*query).as_ref();
            let mut value: *const CFType = ptr::null();
            if unsafe { SecItemCopyMatching(dictionary.as_opaque(), &mut value) } != 0 {
                return None;
            }
            // CopyMatching gives us ownership of this result.
            let result = unsafe { CFRetained::from_raw(NonNull::new(value.cast_mut())?) };
            let data = result.downcast::<CFData>().ok()?;
            Some(Secret::new(data.to_vec()))
        })
        .flatten()
    }

    fn delete(&mut self, id: Uuid) {
        without_ui(|| {
            let query = query(id, false);
            let dictionary: &CFDictionary<NSString, AnyObject> = (*query).as_ref();
            unsafe {
                SecItemDelete(dictionary.as_opaque());
            }
        });
    }
}
