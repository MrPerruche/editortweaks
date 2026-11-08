//! One HTTPS GET through WinHTTP, the HTTP client built into Windows: TLS and certificate checks
//! are Windows' own, the system's proxy settings apply. Blocking: call it off the game thread.

use core::ffi::c_void;

use windows_sys::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect,
    WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption,
    WinHttpSetTimeouts,
};

/// How long each step (name lookup, connecting, sending, each wait for data) may take.
const TIMEOUT_MS: i32 = 10_000;

/// A WinHTTP handle, closed when dropped.
struct Handle(*mut c_void);

impl Handle {
    /// `handle` if WinHTTP made one, else what `step` failed with.
    fn new(handle: *mut c_void, step: &str) -> Result<Self, String> {
        if handle.is_null() {
            return Err(last_error(step));
        }
        Ok(Self(handle))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: a handle WinHTTP gave us, closed only here.
        unsafe { WinHttpCloseHandle(self.0) };
    }
}

/// What failed, with Windows' error code.
fn last_error(step: &str) -> String {
    format!("{step} failed: {}", std::io::Error::last_os_error())
}

/// `text` as a NUL-terminated UTF-16 string, as Win32 wants.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// Gets `https://{host}{path}` with `headers` (`Name: value` lines, `\r\n` after each) and returns
/// the body, if the server answered 200 with at most `max_bytes`. Redirects aren't followed.
/// `agent` names us (the `User-Agent`).
pub fn get(
    agent: &str,
    host: &str,
    path: &str,
    headers: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    let (agent, host, path, headers) = (wide(agent), wide(host), wide(path), wide(headers));
    let verb = wide("GET");
    // SAFETY: every string is NUL-terminated and outlives the calls using it; the handles are live
    // until dropped (the request before the connection before the session, the reverse of how they
    // were declared); each buffer passed is as large as the size given with it.
    unsafe {
        let session = Handle::new(
            WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                core::ptr::null(),
                core::ptr::null(),
                0,
            ),
            "WinHttpOpen",
        )?;
        if WinHttpSetTimeouts(session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS) == 0 {
            return Err(last_error("WinHttpSetTimeouts"));
        }
        let connection = Handle::new(
            WinHttpConnect(session.0, host.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0),
            "WinHttpConnect",
        )?;
        let request = Handle::new(
            WinHttpOpenRequest(
                connection.0,
                verb.as_ptr(),
                path.as_ptr(),
                core::ptr::null(),
                core::ptr::null(),
                core::ptr::null(),
                WINHTTP_FLAG_SECURE,
            ),
            "WinHttpOpenRequest",
        )?;
        let never: u32 = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
        if WinHttpSetOption(
            request.0,
            WINHTTP_OPTION_REDIRECT_POLICY,
            (&raw const never).cast(),
            size_of::<u32>() as u32,
        ) == 0
        {
            return Err(last_error("WinHttpSetOption"));
        }
        // `u32::MAX` (-1L): the headers are NUL-terminated.
        if WinHttpSendRequest(
            request.0,
            headers.as_ptr(),
            u32::MAX,
            core::ptr::null(),
            0,
            0,
            0,
        ) == 0
        {
            return Err(last_error("WinHttpSendRequest"));
        }
        if WinHttpReceiveResponse(request.0, core::ptr::null_mut()) == 0 {
            return Err(last_error("WinHttpReceiveResponse"));
        }
        let mut status: u32 = 0;
        let mut size = size_of::<u32>() as u32;
        if WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            core::ptr::null(),
            (&raw mut status).cast(),
            &mut size,
            core::ptr::null_mut(),
        ) == 0
        {
            return Err(last_error("WinHttpQueryHeaders"));
        }
        if status != 200 {
            return Err(format!("the server answered {status}"));
        }
        let mut body = Vec::new();
        loop {
            let mut available: u32 = 0;
            if WinHttpQueryDataAvailable(request.0, &mut available) == 0 {
                return Err(last_error("WinHttpQueryDataAvailable"));
            }
            if available == 0 {
                return Ok(body);
            }
            let start = body.len();
            if start + available as usize > max_bytes {
                return Err(format!("the answer is over {max_bytes} bytes"));
            }
            body.resize(start + available as usize, 0);
            let mut read: u32 = 0;
            if WinHttpReadData(
                request.0,
                body.as_mut_ptr().add(start).cast(),
                available,
                &mut read,
            ) == 0
            {
                return Err(last_error("WinHttpReadData"));
            }
            body.truncate(start + read as usize);
        }
    }
}
