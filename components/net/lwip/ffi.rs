// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Private declarations for the C socket bridge.

use crate::Endpoint;

unsafe extern "C" {
    /// # Safety
    ///
    /// Call once before other bridge operations; seed must contain 32 readable
    /// unpredictable bytes for the duration of the call.
    pub fn arona_lwip_start(seed: *const u8) -> i32;
    /// # Safety
    ///
    /// The calling Arona thread must have active runtime TLS.
    pub fn arona_lwip_last_error() -> i32;
    /// # Safety
    ///
    /// Startup must have completed; kind must be SOCK_STREAM or SOCK_DGRAM.
    pub fn arona_lwip_socket(kind: i32) -> i32;
    /// # Safety
    ///
    /// fd must be live and exclusively accessed; endpoint must be readable for
    /// this call, after successful stack startup.
    pub fn arona_lwip_bind(fd: i32, endpoint: *const Endpoint) -> i32;
    /// # Safety
    ///
    /// fd must be live and exclusively accessed after successful stack startup.
    pub fn arona_lwip_listen(fd: i32, backlog: i32) -> i32;
    /// # Safety
    ///
    /// fd must be live and exclusively accessed; endpoint must be readable for
    /// this call, after successful stack startup.
    pub fn arona_lwip_connect(fd: i32, endpoint: *const Endpoint) -> i32;
    /// # Safety
    ///
    /// fd must be live and exclusively accessed; peer must be exclusively
    /// writable for this call. Success transfers ownership of the new socket.
    pub fn arona_lwip_accept(fd: i32, peer: *mut Endpoint) -> i32;
    /// # Safety
    ///
    /// fd must be live and exclusively accessed; endpoint must be exclusively
    /// writable for this call. peer must be zero or one.
    pub fn arona_lwip_endpoint(fd: i32, endpoint: *mut Endpoint, peer: i32) -> i32;
    /// # Safety
    ///
    /// fd must be live and exclusively accessed. bytes must be readable for
    /// length bytes, and peer must be null or readable for this call.
    pub fn arona_lwip_send(
        fd: i32,
        bytes: *const u8,
        length: usize,
        peer: *const Endpoint,
    ) -> isize;
    /// # Safety
    ///
    /// fd must be live and exclusively accessed. bytes must be exclusively
    /// writable for length bytes; peer must be null or exclusively writable.
    /// The output regions must not overlap and must remain valid for this call.
    pub fn arona_lwip_receive(fd: i32, bytes: *mut u8, length: usize, peer: *mut Endpoint)
    -> isize;
    /// # Safety
    ///
    /// fd must be live and exclusively accessed after successful stack startup.
    pub fn arona_lwip_take_error(fd: i32) -> i32;
    /// # Safety
    ///
    /// fd must be live and exclusively accessed; direction must be SHUT_RD,
    /// SHUT_WR, or SHUT_RDWR. The socket must remain owned after this call.
    pub fn arona_lwip_shutdown(fd: i32, direction: i32) -> i32;
    /// # Safety
    ///
    /// fd must be live and exclusively owned. Success invalidates the socket;
    /// failure retains ownership. No other operation may race with closing.
    pub fn arona_lwip_close(fd: i32) -> i32;
}
