// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use objc2::rc::Retained;
use objc2_foundation::NSObject;
use std::ffi::{CStr, CString, c_char, c_int, c_void};

use crate::{Error, proxy::ProxyEndpoint};

#[allow(non_camel_case_types)]
pub type nw_endpoint_t = *mut NSObject;
#[allow(non_camel_case_types)]
pub type nw_protocol_options_t = *mut NSObject;
#[allow(non_camel_case_types)]
pub type nw_proxy_config_t = *mut NSObject;

unsafe extern "C" {
  fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
  fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
}

/// `RTLD_DEFAULT` on Apple systems.
const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
const RTLD_LAZY: c_int = 1;

// Nezzel patch: the proxy constructors exist from macOS 14 only, and the Network framework
// itself from 10.14. Linked by name they are strong imports (and a load command), so the app
// would not launch on an older macOS; they are looked up at run time, and a system without
// them gets an error.
fn symbol(name: &CStr) -> Result<*mut c_void, Error> {
  let mut found = unsafe { dlsym(RTLD_DEFAULT, name.as_ptr()) };
  if found.is_null() {
    // WebKit loads the framework itself; this is for a process where nothing has yet.
    let network =
      unsafe { dlopen(c"/System/Library/Frameworks/Network.framework/Network".as_ptr(), RTLD_LAZY) };
    if !network.is_null() {
      found = unsafe { dlsym(network, name.as_ptr()) };
    }
  }
  if found.is_null() {
    Err(Error::ProxyEndpointCreationFailed)
  } else {
    Ok(found)
  }
}

// Nezzel patch: the `nw_*_create` functions return an object the caller owns (+1). `Retained`
// releases it when it goes out of scope, on the error paths too; upstream leaked both.
fn owned(object: *mut NSObject) -> Result<Retained<NSObject>, Error> {
  unsafe { Retained::from_raw(object) }.ok_or(Error::ProxyEndpointCreationFailed)
}

pub fn nw_proxy_config_create_socksv5(
  proxy_endpoint: &NSObject,
) -> Result<Retained<NSObject>, Error> {
  let found = symbol(c"nw_proxy_config_create_socksv5")?;
  let create: unsafe extern "C" fn(nw_endpoint_t) -> nw_proxy_config_t =
    unsafe { std::mem::transmute(found) };
  owned(unsafe { create(std::ptr::from_ref(proxy_endpoint).cast_mut()) })
}

/// # Safety
/// `proxy_tls_options` must be a valid `nw_protocol_options_t` or null.
pub unsafe fn nw_proxy_config_create_http_connect(
  proxy_endpoint: &NSObject,
  proxy_tls_options: nw_protocol_options_t,
) -> Result<Retained<NSObject>, Error> {
  let found = symbol(c"nw_proxy_config_create_http_connect")?;
  let create: unsafe extern "C" fn(nw_endpoint_t, nw_protocol_options_t) -> nw_proxy_config_t =
    unsafe { std::mem::transmute(found) };
  owned(unsafe {
    create(
      std::ptr::from_ref(proxy_endpoint).cast_mut(),
      proxy_tls_options,
    )
  })
}

// Nezzel patch: `port` is the one to connect to (`ProxyConfig::port`), and an IPv6 host goes in
// without its brackets: with them the framework takes it for a name to look up.
pub fn nw_endpoint(endpoint: &ProxyEndpoint, port: &str) -> Result<Retained<NSObject>, Error> {
  let host = CString::new(endpoint.bare_host()).map_err(|_| Error::ProxyEndpointCreationFailed)?;
  let port = CString::new(port).map_err(|_| Error::ProxyEndpointCreationFailed)?;
  let found = symbol(c"nw_endpoint_create_host")?;
  let create: unsafe extern "C" fn(*const c_char, *const c_char) -> nw_endpoint_t =
    unsafe { std::mem::transmute(found) };
  owned(unsafe { create(host.as_ptr(), port.as_ptr()) })
}
