//! Fake `WebOTA` board for the OTA end-to-end rig scenarios (Round 2C).
//!
//! `FakeBoard` behaves like the real board's `WebOTA` server
//! (`firmware/webota.cpp`) closely enough to prove the app-core flash
//! flow end to end, over real loopback TCP:
//!
//! * `GET /` needs Basic Auth (`admin` / the per-device password) and
//!   serves an index page with a per-boot CSRF token and a
//!   `Version <b>a.b.c</b>` chip for the *active* slot's version.
//! * `POST /update` needs the same auth plus the `X-RSH-CSRF` header,
//!   parses the multipart body, and verifies the image exactly like
//!   the board: ESP `0xE9` magic, size bounds, `RSHS` trailer,
//!   Ed25519 signature over `SHA-256(body)` against the *test* key,
//!   version marker, anti-downgrade vs the active version. A bad image
//!   gets `200 FAIL - …` and the active slot is untouched.
//! * A good image is "flashed" to the inactive slot; the board answers
//!   `200 OK …` and "restarts" ~200 ms later, after which the active
//!   version is the new one — the same verify-before-apply +
//!   other-slot behaviour as the real dual-OTA-partition board.
//! * [`FailMode::DropMidUpload`] closes the connection mid-body, like
//!   a Wi-Fi drop: the upload fails and the old firmware keeps
//!   running (rollback).
//!
//! Test images are signed with a throwaway keypair (never the
//! production key); the board verifies against the matching throwaway
//! public key via `rsh_security::verify_firmware_image_with_pubkey`.
//! The production key never enters this module.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use ed25519_dalek::{Signer, SigningKey};
use rsh_security::firmware_verify::{
    FIRMWARE_ESP_IMAGE_MAGIC, FirmwareVerifyError, verify_firmware_image_with_pubkey,
};
use sha2::{Digest, Sha256};

/// Multipart boundary the app-core uploader uses
/// (`webota.rs::MULTIPART_BOUNDARY` — the value must match, the
/// constant itself is private there).
const MULTIPART_BOUNDARY: &str = "----RSH-WebOTA-7f3a9c";

/// CSRF token the fake board serves (32 hex chars, like the real
/// board's per-boot token).
const CSRF_TOKEN: &str = "0123456789abcdef0123456789abcdef";

/// Fixed, non-secret seed for the throwaway test keypair. This key
/// signs test images only; it is NOT the production signing key and
/// must never be used to sign a real firmware image.
pub const TEST_SEED: [u8; 32] = [
    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00,
    0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xa0, 0xb0, 0xc0, 0xd0, 0xe0, 0xf0, 0x0f,
];

/// A second throwaway seed, for images whose signature is valid under
/// a *different* key than the board trusts (app-side verify passes,
/// board-side verify must still reject).
pub const OTHER_TEST_SEED: [u8; 32] = [
    0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
    0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
];

/// Public key of the throwaway test keypair — the key the fake board
/// trusts.
#[must_use]
pub fn test_pubkey() -> [u8; 32] {
    SigningKey::from_bytes(&TEST_SEED)
        .verifying_key()
        .to_bytes()
}

/// Build a signed test image: `body_len` bytes starting with `0xE9`,
/// the version marker at offset 1024, signed over `SHA-256(body)`
/// with the throwaway test key. Returns the image plus the public key
/// it verifies against.
#[must_use]
pub fn build_test_image(version: &str, body_len: usize) -> (Vec<u8>, [u8; 32]) {
    build_test_image_with_seed(version, body_len, &TEST_SEED)
}

/// Same as [`build_test_image`], but signed with an explicit seed
/// (for the wrong-key case).
#[must_use]
pub fn build_test_image_with_seed(
    version: &str,
    body_len: usize,
    seed: &[u8; 32],
) -> (Vec<u8>, [u8; 32]) {
    let mut body = vec![0u8; body_len];
    body[0] = FIRMWARE_ESP_IMAGE_MAGIC;
    let marker = format!("Version <b>{version}");
    body[1024..1024 + marker.len()].copy_from_slice(marker.as_bytes());
    let digest: [u8; 32] = Sha256::digest(&body).into();
    let key = SigningKey::from_bytes(seed);
    let signature = key.sign(&digest);
    let mut image = body;
    image.extend_from_slice(&signature.to_bytes());
    image.extend_from_slice(b"RSHS");
    (image, key.verifying_key().to_bytes())
}

/// A firmware version triple.
pub type VersionTriple = (u8, u8, u8);

/// Failure injection for the fake board.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FailMode {
    /// Normal behaviour.
    #[default]
    None,
    /// Close the connection after `after_bytes` of the POST body have
    /// been read (Wi-Fi drop mid-upload).
    DropMidUpload {
        /// Body bytes to accept before dropping.
        after_bytes: usize,
    },
}

/// Shared fake-board state (the serving thread + the checks read it).
struct BoardState {
    /// Version the active slot reports (`GET /` chip).
    active_version: VersionTriple,
    /// Image staged in the inactive slot, waiting for the "restart".
    staged: Option<(VersionTriple, Vec<u8>)>,
    /// Current failure injection.
    fail_mode: FailMode,
}

/// A fake `WebOTA` board on 127.0.0.1 (ephemeral port), serving real
/// HTTP until dropped.
pub struct FakeBoard {
    port: u16,
    state: Arc<Mutex<BoardState>>,
    password: String,
    shutdown: Arc<AtomicBool>,
    listener: Option<TcpListener>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeBoard {
    /// Start a fake board reporting `initial_version`, accepting the
    /// given Wi-Fi password (`"secret"` in the checks).
    #[must_use]
    pub fn start(password: &str, initial_version: VersionTriple) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("fake board binds loopback");
        let port = listener.local_addr().expect("fake board addr").port();
        let state = Arc::new(Mutex::new(BoardState {
            active_version: initial_version,
            staged: None,
            fail_mode: FailMode::None,
        }));
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_state = Arc::clone(&state);
        let thread_password = password.to_string();
        let thread_shutdown = Arc::clone(&shutdown);
        let thread_listener = listener.try_clone().expect("clone fake board listener");
        let thread = std::thread::spawn(move || {
            serve(
                &thread_listener,
                &thread_state,
                &thread_password,
                &thread_shutdown,
            );
        });
        Self {
            port,
            state,
            password: password.to_string(),
            shutdown,
            listener: Some(listener),
            thread: Some(thread),
        }
    }

    /// The loopback port the board listens on.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The version the active slot currently reports.
    #[must_use]
    pub fn version(&self) -> VersionTriple {
        self.state.lock().expect("fake board state").active_version
    }

    /// The password the board accepts (for building the target).
    #[must_use]
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Set failure injection (takes effect on the next request).
    pub fn set_fail_mode(&self, mode: FailMode) {
        self.state.lock().expect("fake board state").fail_mode = mode;
    }
}

impl Drop for FakeBoard {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Wake the accept loop, then join the thread.
        if let Ok(mut wake) = TcpStream::connect(("127.0.0.1", self.port)) {
            let _ = wake.write_all(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n");
        }
        drop(self.listener.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Accept loop: one connection at a time (the flash flow is
/// sequential — GET, then POST, then polling GETs).
fn serve(
    listener: &TcpListener,
    state: &Arc<Mutex<BoardState>>,
    password: &str,
    shutdown: &Arc<AtomicBool>,
) {
    while !shutdown.load(Ordering::SeqCst) {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        if shutdown.load(Ordering::SeqCst) {
            return;
        }
        handle_connection(stream, state, password);
    }
}

/// One HTTP request: headers + `Content-Length` body, then the
/// `GET /` or `POST /update` behaviour.
fn handle_connection(mut stream: TcpStream, state: &Arc<Mutex<BoardState>>, password: &str) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let mut raw = Vec::new();
    let mut byte = [0u8; 1];
    while !raw.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => return,
            Ok(_) => raw.extend_from_slice(&byte),
        }
        if raw.len() > 64 * 1024 {
            return;
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let mut lines = text.lines();
    let request_line = lines.next().unwrap_or("").to_string();
    let mut content_length = 0usize;
    let mut authorization = String::new();
    let mut csrf = String::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() {
                "content-length" => content_length = value.trim().parse().unwrap_or(0),
                "authorization" => authorization = value.trim().to_string(),
                "x-rsh-csrf" => csrf = value.trim().to_string(),
                _ => {}
            }
        }
    }
    let expected_auth = format!(
        "Basic {}",
        rsh_app_core::webota::base64_encode(format!("admin:{password}").as_bytes())
    );
    if authorization != expected_auth {
        let _ = stream.write_all(
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        return;
    }
    // Read the body in 8 KB chunks so DropMidUpload can cut the
    // connection part-way through (the client then fails the upload).
    let fail_after = state.lock().expect("fake board state").fail_mode;
    let mut body = Vec::with_capacity(content_length);
    let mut chunk = [0u8; 8192];
    while body.len() < content_length {
        let want = (content_length - body.len()).min(chunk.len());
        match stream.read(&mut chunk[..want]) {
            Ok(0) | Err(_) => return,
            Ok(read) => body.extend_from_slice(&chunk[..read]),
        }
        if let FailMode::DropMidUpload { after_bytes } = fail_after
            && body.len() >= after_bytes
        {
            return; // connection dropped — no response, upload fails
        }
    }
    if request_line.starts_with("GET / ") {
        let version = state.lock().expect("fake board state").active_version;
        let page = format!(
            "<html><head><meta name=\"csrf-token\" content=\"{CSRF_TOKEN}\"></head>\
             <body><span class=\"chip\">Version <b>{}.{}.{}</b></span></body></html>",
            version.0, version.1, version.2
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
            page.len()
        );
        let _ = stream.write_all(response.as_bytes());
    } else if request_line.starts_with("POST /update ") {
        if csrf != CSRF_TOKEN {
            respond_fail(&mut stream, "bad CSRF token");
            return;
        }
        let Some(image) = parse_multipart(&body) else {
            respond_fail(&mut stream, "bad multipart body");
            return;
        };
        let current = state.lock().expect("fake board state").active_version;
        match verify_firmware_image_with_pubkey(&image, current, &test_pubkey()) {
            Ok(info) => {
                // Staged in the inactive slot; the "restart" ~200 ms
                // later makes it active (verify-before-apply).
                state.lock().expect("fake board state").staged = Some((info.version, image));
                let state_for_restart = Arc::clone(state);
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(200));
                    let mut locked = state_for_restart.lock().expect("fake board state");
                    if let Some((version, _)) = locked.staged.take() {
                        locked.active_version = version;
                    }
                });
                let body = "OK - update ho gaya, board restart ho raha hai";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
            Err(error) => {
                // Board-style reason; the active slot is untouched.
                let reason = match error {
                    FirmwareVerifyError::InvalidSignature => "signature verify fail",
                    FirmwareVerifyError::Downgrade { .. } => "downgrade reject (purani version)",
                    FirmwareVerifyError::BadTrailerMagic => {
                        "image signed nahi hai (signature trailer nahi)"
                    }
                    FirmwareVerifyError::BadEspMagic => "bad magic (0xE9 nahi)",
                    FirmwareVerifyError::TooSmall => "image bohat chhoti hai",
                    FirmwareVerifyError::TooLarge => "image bohat bari hai (size cap)",
                    FirmwareVerifyError::TruncatedTrailer => "image truncated",
                    FirmwareVerifyError::VersionNotFound => "image mein version nahi mila",
                };
                respond_fail(&mut stream, reason);
            }
        }
    } else {
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    }
}

/// `200 FAIL - <reason>` (the real board answers failures this way).
fn respond_fail(stream: &mut TcpStream, reason: &str) {
    let body = format!("FAIL - {reason}, purana firmware chal raha hai");
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

/// Extract the `firmware` field bytes from a multipart body: the bytes
/// between the first blank line and the closing boundary.
fn parse_multipart(body: &[u8]) -> Option<Vec<u8>> {
    let header_end = find_subslice(body, b"\r\n\r\n")? + 4;
    let closing = format!("\r\n--{MULTIPART_BOUNDARY}--").into_bytes();
    let image_end = find_subslice(&body[header_end..], &closing)? + header_end;
    Some(body[header_end..image_end].to_vec())
}

/// First index of `needle` in `haystack`, or `None`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
