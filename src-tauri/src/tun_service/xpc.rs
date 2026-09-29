use block2::Block;
use std::ffi::{c_char, c_void};
use std::sync::OnceLock;

pub type XpcObject = *mut c_void;
pub type XpcConnection = *mut c_void;
pub type XpcType = *const c_void;

pub const XPC_CONNECTION_MACH_SERVICE_LISTENER: u64 = 1 << 0;
pub const XPC_CONNECTION_MACH_SERVICE_PRIVILEGED: u64 = 1 << 1;

extern "C" {
    pub static _xpc_type_connection: u8;
    pub static _xpc_type_dictionary: u8;
    pub static _xpc_type_error: u8;

    pub fn xpc_get_type(object: XpcObject) -> XpcType;
    pub fn xpc_release(object: XpcObject);
    pub fn xpc_connection_create_mach_service(
        name: *const c_char,
        target_queue: *mut c_void,
        flags: u64,
    ) -> XpcConnection;
    pub fn xpc_connection_set_event_handler(
        connection: XpcConnection,
        handler: &Block<dyn Fn(XpcObject)>,
    );
    pub fn xpc_connection_resume(connection: XpcConnection);
    pub fn xpc_connection_cancel(connection: XpcConnection);
    pub fn xpc_connection_send_message(connection: XpcConnection, message: XpcObject);
    pub fn xpc_connection_send_message_with_reply(
        connection: XpcConnection,
        message: XpcObject,
        queue: *mut c_void,
        handler: &Block<dyn Fn(XpcObject)>,
    );
    pub fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut c_void;
    pub fn xpc_connection_get_euid(connection: XpcConnection) -> libc::uid_t;

    pub fn xpc_dictionary_create(
        keys: *const *const c_char,
        values: *const XpcObject,
        count: usize,
    ) -> XpcObject;
    pub fn xpc_dictionary_create_reply(original: XpcObject) -> XpcObject;
    pub fn xpc_dictionary_get_remote_connection(dictionary: XpcObject) -> XpcConnection;
    pub fn xpc_dictionary_set_string(
        dictionary: XpcObject,
        key: *const c_char,
        value: *const c_char,
    );
    pub fn xpc_dictionary_get_string(dictionary: XpcObject, key: *const c_char) -> *const c_char;
    pub fn xpc_dictionary_set_int64(dictionary: XpcObject, key: *const c_char, value: i64);
    pub fn xpc_dictionary_get_int64(dictionary: XpcObject, key: *const c_char) -> i64;
    pub fn xpc_dictionary_set_uint64(dictionary: XpcObject, key: *const c_char, value: u64);
    pub fn xpc_dictionary_get_uint64(dictionary: XpcObject, key: *const c_char) -> u64;
    pub fn xpc_dictionary_set_bool(dictionary: XpcObject, key: *const c_char, value: bool);
    pub fn xpc_dictionary_get_bool(dictionary: XpcObject, key: *const c_char) -> bool;
    pub fn xpc_dictionary_set_data(
        dictionary: XpcObject,
        key: *const c_char,
        bytes: *const c_void,
        length: usize,
    );
    pub fn xpc_dictionary_get_data(
        dictionary: XpcObject,
        key: *const c_char,
        length: *mut usize,
    ) -> *const c_void;

    pub fn dispatch_main() -> !;
}

pub fn set_peer_requirement(connection: XpcConnection, requirement: *const c_char) -> i32 {
    type SetRequirement = unsafe extern "C" fn(XpcConnection, *const c_char) -> i32;
    const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
    static SYMBOL: OnceLock<Option<SetRequirement>> = OnceLock::new();
    let symbol = SYMBOL.get_or_init(|| unsafe {
        for name in [
            c"xpc_connection_set_peer_code_signing_requirement".as_ptr(),
            c"xpc_connection_set_codesigning_requirement".as_ptr(),
        ] {
            let pointer = libc::dlsym(RTLD_DEFAULT, name);
            if !pointer.is_null() {
                return Some(std::mem::transmute::<*mut c_void, SetRequirement>(pointer));
            }
        }
        None
    });
    match symbol {
        Some(function) => unsafe { function(connection, requirement) },
        None => -1,
    }
}

pub fn type_connection() -> *const u8 {
    std::ptr::addr_of!(_xpc_type_connection)
}

pub fn type_dictionary() -> *const u8 {
    std::ptr::addr_of!(_xpc_type_dictionary)
}

pub fn type_error() -> *const u8 {
    std::ptr::addr_of!(_xpc_type_error)
}

pub unsafe fn is_type(object: XpcObject, expected: *const u8) -> bool {
    !object.is_null() && xpc_get_type(object) == expected.cast()
}
