//! Board firmware upload over the board's WebOTA HTTP server
//! (H5-control, `upload_firmware`).
//!
//! The v4.0.1 firmware serves `http://192.168.4.1/` (its AP address)
//! with HTTP Basic Auth (`admin` / the per-device Wi-Fi password — the
//! same credential as the AP) and a per-boot CSRF token on the index
//! page. The upload is `POST /update`, multipart/form-data, field name
//! `firmware`, header `X-RSH-CSRF: <token>`; a `200` with a body
//! starting `"OK"` means the board accepted and will restart
//! (SIGNED-OTA §2–§3). ArduinoOTA is disabled — WebOTA is the only OTA
//! path; there is no serial firmware-upload protocol in v4.0.1.
//!
//! This module is deliberately dependency-free (`std::net::TcpStream`
//! + a hand-rolled base64 + a small chunked-body reader — the workspace
//! has no HTTP client crate): the board's index page is served with
//! chunked transfer encoding, so the reader must handle that.
//!
//! The caller owns the trust decisions: the image must already have
//! passed `rsh_security::verify_firmware_image` (Ed25519 signature +
//! format + anti-downgrade vs the live version) and the Wi-Fi password
//! must come from the keystore. This module only moves bytes to a
//! board that has already been authenticated to.
//!
//! The full flow lives here too ([`flash_firmware`]): validate the
//! image against the installed version, upload it with progress, then
//! confirm the board reports the new version (the confirm step is a
//! caller-supplied closure — the daemon reconnects and reads the
//! signed `GET_CAPS` version, or passes [`wait_for_board_version`],
//! which polls the board's index-page version chip). HTTP 200 + `"OK"`
//! alone is never a version confirm (SIGNED-OTA §3).
//!
//! The board is expected on its own AP, so the PC must be joined to
//! the board's Wi-Fi network for the upload to reach it; a TCP
//! connect failure says so plainly instead of pretending otherwise.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use rsh_security::firmware_verify::{
    FIRMWARE_SIGNING_PUBKEY, FirmwareVersion, verify_firmware_image_with_pubkey,
};
use serde::{Deserialize, Serialize};

use crate::secrets::ZeroizingString;

/// The `WebOTA` server port on the board (plain HTTP on its AP).
const WEB_OTA_PORT: u16 = 80;

/// Basic-Auth username for every `WebOTA` endpoint (`WEB_OTA_USER` in
/// the firmware's `config.h`).
const WEB_OTA_USER: &str = "admin";

/// Multipart boundary for the firmware upload (fixed: no RNG in `std`,
/// and the boundary only has to be absent from the image bytes).
const MULTIPART_BOUNDARY: &str = "----RSH-WebOTA-7f3a9c";

/// TCP connect timeout for the board's update server.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Read timeout for the CSRF-token fetch (the index page is tiny).
const GET_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Read timeout for the upload response: the board verifies the
/// signature and writes flash before answering, which takes seconds
/// on real hardware.
const POST_READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Write timeout for request bytes (the board aborts an upload after
/// 30 s of inactivity, so a stalled write must fail first).
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Firmware bytes written per `write_all` call during the upload.
const UPLOAD_CHUNK_SIZE: usize = 32 * 1024;

/// Maximum HTTP header block accepted (the board sends a handful of
/// short headers; anything larger is not the board).
const MAX_HEADER_BYTES: usize = 64 * 1024;

/// Base64 alphabet (RFC 4648, standard padding).
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode bytes as standard base64 with `=` padding (hand-rolled:
/// no base64 crate in the workspace).
#[must_use]
pub fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(BASE64_ALPHABET[(triple >> 18) as usize & 0x3F] as char);
        out.push(BASE64_ALPHABET[(triple >> 12) as usize & 0x3F] as char);
        if chunk.len() > 1 {
            out.push(BASE64_ALPHABET[(triple >> 6) as usize & 0x3F] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(BASE64_ALPHABET[triple as usize & 0x3F] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// A parsed HTTP response: status code + raw body bytes.
struct HttpResponse {
    /// HTTP status code (e.g. 200, 401).
    status: u16,
    /// Response body, decoded from chunked transfer encoding when used.
    body: Vec<u8>,
}

/// A buffered reader over a [`TcpStream`] that can read lines, exact
/// byte counts, and to-EOF — the pieces an HTTP response needs.
struct HttpReader {
    /// The underlying stream (already connected, timeouts set).
    stream: TcpStream,
    /// Bytes already read from the stream but not yet consumed.
    buffer: Vec<u8>,
    /// Read offset into `buffer`.
    position: usize,
}

impl HttpReader {
    /// Wrap an already-connected stream.
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            buffer: Vec::new(),
            position: 0,
        }
    }

    /// Fill the buffer from the stream. Returns `false` on EOF.
    fn fill(&mut self) -> Result<bool, String> {
        let mut chunk = [0u8; 8192];
        let read = self
            .stream
            .read(&mut chunk)
            .map_err(|error| format!("failed reading from the update server: {error}"))?;
        if read == 0 {
            return Ok(false);
        }
        self.buffer.extend_from_slice(&chunk[..read]);
        Ok(true)
    }

    /// Read one `\n`-terminated line (the trailing `\r\n` is stripped).
    /// Returns `None` on EOF before any byte of the line.
    fn read_line(&mut self) -> Result<Option<String>, String> {
        loop {
            if let Some(newline) = self.buffer[self.position..]
                .iter()
                .position(|byte| *byte == b'\n')
            {
                let end = self.position + newline;
                let mut line = self.buffer[self.position..end].to_vec();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                self.position = end + 1;
                return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
            }
            if self.position > 0 {
                self.buffer.drain(..self.position);
                self.position = 0;
            }
            if self.buffer.len() > MAX_HEADER_BYTES {
                return Err("update server sent an oversized header block".to_string());
            }
            if !self.fill()? {
                return Ok(None);
            }
        }
    }

    /// Read exactly `count` bytes, or fail on EOF.
    fn read_exact(&mut self, count: usize) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(count);
        while out.len() < count {
            if self.position < self.buffer.len() {
                let available = (count - out.len()).min(self.buffer.len() - self.position);
                out.extend_from_slice(&self.buffer[self.position..self.position + available]);
                self.position += available;
                continue;
            }
            self.buffer.clear();
            self.position = 0;
            if !self.fill()? {
                return Err(format!(
                    "update server closed the connection after {} of {count} body bytes",
                    out.len()
                ));
            }
        }
        Ok(out)
    }

    /// Read everything until the server closes the connection.
    fn read_to_eof(&mut self) -> Result<Vec<u8>, String> {
        let mut out = self.buffer[self.position..].to_vec();
        self.buffer.clear();
        self.position = 0;
        let mut chunk = [0u8; 8192];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => out.extend_from_slice(&chunk[..read]),
                Err(error) => {
                    return Err(format!("failed reading from the update server: {error}"));
                }
            }
        }
        Ok(out)
    }
}

/// Read one HTTP response from a connected stream: status line,
/// headers, then the body (chunked transfer encoding, `Content-Length`,
/// or read-to-EOF in that order — the board's index page is chunked).
fn read_response(reader: &mut HttpReader) -> Result<HttpResponse, String> {
    let status_line = reader
        .read_line()?
        .ok_or_else(|| "update server closed the connection without a response".to_string())?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| format!("update server sent an unreadable status line: {status_line}"))?;
    let mut headers: HashMap<String, String> = HashMap::new();
    loop {
        let line = reader
            .read_line()?
            .ok_or_else(|| "update server closed the connection mid-headers".to_string())?;
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let body = if headers
        .get("transfer-encoding")
        .is_some_and(|encoding| encoding.eq_ignore_ascii_case("chunked"))
    {
        read_chunked_body(reader)?
    } else if let Some(length) = headers.get("content-length") {
        let length: usize = length
            .parse()
            .map_err(|_| format!("update server sent an unreadable Content-Length: {length}"))?;
        reader.read_exact(length)?
    } else {
        reader.read_to_eof()?
    };
    Ok(HttpResponse { status, body })
}

/// Read a chunked body: hex size lines (extensions ignored), a
/// zero-size chunk, then trailers up to the empty line.
fn read_chunked_body(reader: &mut HttpReader) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    loop {
        let size_line = reader
            .read_line()?
            .ok_or_else(|| "update server closed the connection mid-chunk".to_string())?;
        let size_text = size_line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| format!("update server sent an unreadable chunk size: {size_line}"))?;
        if size == 0 {
            // Trailers up to the empty line.
            loop {
                let line = reader.read_line()?.ok_or_else(|| {
                    "update server closed the connection mid-trailers".to_string()
                })?;
                if line.is_empty() {
                    break;
                }
            }
            return Ok(body);
        }
        body.extend_from_slice(&reader.read_exact(size)?);
        // The CRLF after each chunk.
        let crlf = reader.read_exact(2)?;
        if crlf != b"\r\n" {
            return Err("update server sent a malformed chunked body".to_string());
        }
    }
}

/// Connect to the board's update server with a short timeout, and
/// set the read/write timeouts for the request that follows.
fn connect(host: IpAddr, port: u16, read_timeout: Duration) -> Result<TcpStream, String> {
    let stream =
        TcpStream::connect_timeout(&std::net::SocketAddr::new(host, port), CONNECT_TIMEOUT)
            .map_err(|error| {
                format!(
                    "could not reach the board's update server at http://{host}/ ({error}) — \
             join the board's Wi-Fi network and retry"
                )
            })?;
    stream
        .set_read_timeout(Some(read_timeout))
        .map_err(|error| format!("could not set the read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(WRITE_TIMEOUT))
        .map_err(|error| format!("could not set the write timeout: {error}"))?;
    Ok(stream)
}

/// The `Authorization: Basic …` header value for the `WebOTA` user.
fn basic_auth_header(password: &str) -> String {
    format!(
        "Basic {}",
        base64_encode(format!("{WEB_OTA_USER}:{password}").as_bytes())
    )
}

/// Fetch the board's index page (`GET /` with Basic Auth) and return
/// its body text. Shared by the CSRF-token fetch and the version-chip
/// read — one request shape, one set of error messages.
///
/// # Errors
///
/// Returns a message when the server cannot be reached, rejects the
/// password (HTTP 401), or answers with another non-200 status.
fn get_index_page(host: IpAddr, port: u16, password: &str) -> Result<String, String> {
    let stream = connect(host, port, GET_READ_TIMEOUT)?;
    let mut reader = HttpReader::new(stream);
    let request = format!(
        "GET / HTTP/1.1\r\nHost: {host}\r\nAuthorization: {}\r\nConnection: close\r\n\r\n",
        basic_auth_header(password)
    );
    reader
        .stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("failed sending the request to the update server: {error}"))?;
    let response = read_response(&mut reader)?;
    if response.status == 401 {
        return Err(
            "update server rejected the Wi-Fi password (HTTP 401) — re-pair the board and retry"
                .to_string(),
        );
    }
    if response.status != 200 {
        return Err(format!(
            "update server returned HTTP {} for the index page",
            response.status
        ));
    }
    Ok(String::from_utf8_lossy(&response.body).into_owned())
}

/// Fetch the per-boot CSRF token from the board's index page
/// (`GET /` with Basic Auth, token in
/// `<meta name="csrf-token" content="<32 hex chars>">`).
///
/// # Errors
///
/// Returns a message when the server cannot be reached, rejects the
/// password (HTTP 401), answers with another non-200 status, or sends
/// a page without a token.
fn fetch_csrf_token(host: IpAddr, port: u16, password: &str) -> Result<String, String> {
    let page = get_index_page(host, port, password)?;
    extract_csrf_token(&page)
}

/// Read the running firmware version from the board's index page
/// (the `Version <b>a.b.c</b>` chip the page renders). This is the
/// unauthenticated-session version read: the page is served after the
/// same Basic Auth as the upload, and the value is only trusted as a
/// *confirm* after an upload whose image was signature-verified —
/// never as the sole proof of what is running (the signed `GET_CAPS`
/// version is the strong confirm; the daemon passes that as the
/// `confirm_version` closure to [`flash_firmware`] when it has a
/// session).
///
/// # Errors
///
/// Returns a message when the server cannot be reached, rejects the
/// password, answers non-200, or sends a page without a version chip.
pub fn fetch_board_version(host: IpAddr, port: u16, password: &str) -> Result<String, String> {
    let page = get_index_page(host, port, password)?;
    parse_board_version_chip(&page)
}

/// Parse the `Version <b>a.b.c</b>` chip out of an index page: find
/// the marker, take the digit/dot run after it, and parse it strictly
/// as `a.b.c` (each part 1–3 digits, value ≤ 255 — the board rule in
/// `ota_validate.h`).
fn parse_board_version_chip(page: &str) -> Result<String, String> {
    let marker = "Version <b>";
    let at = page.find(marker).ok_or_else(|| {
        "update server did not return a firmware version — the board may not be running firmware v4.0.1 or newer"
            .to_string()
    })?;
    let rest = &page[at + marker.len()..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(rest.len());
    let run = &rest[..end];
    parse_version_triple(run)
        .map(|(major, minor, patch)| format!("{major}.{minor}.{patch}"))
        .ok_or_else(|| format!("update server returned an unreadable firmware version: {run}"))
}

/// Strict `a.b.c` parse (each part 1–3 digits, value ≤ 255, whole run
/// consumed), mirroring `rsh_parse_version` (`ota_validate.h`).
fn parse_version_triple(run: &str) -> Option<FirmwareVersion> {
    let bytes = run.as_bytes();
    if bytes.is_empty() || bytes.len() > 11 {
        return None;
    }
    let mut parts = [0u16; 3];
    let mut part = 0usize;
    let mut digits = 0usize;
    for &byte in bytes {
        if byte.is_ascii_digit() {
            digits += 1;
            if digits > 3 {
                return None;
            }
            parts[part] = parts[part] * 10 + u16::from(byte - b'0');
            if parts[part] > 255 {
                return None;
            }
        } else if byte == b'.' {
            if digits == 0 || part >= 2 {
                return None;
            }
            part += 1;
            digits = 0;
        } else {
            return None;
        }
    }
    if part != 2 || digits == 0 {
        return None;
    }
    Some((
        u8::try_from(parts[0]).ok()?,
        u8::try_from(parts[1]).ok()?,
        u8::try_from(parts[2]).ok()?,
    ))
}

/// Extract the CSRF token from an index page: find `csrf-token`,
/// then the `content="…"` after it. The board's token is 32 hex chars.
fn extract_csrf_token(page: &str) -> Result<String, String> {
    let marker = page.find("csrf-token").ok_or_else(|| {
        "update server did not return a CSRF token — the board may not be running firmware v4.0.1"
            .to_string()
    })?;
    let after_marker = &page[marker..];
    let content_marker = "content=\"";
    let content_start = after_marker.find(content_marker).ok_or_else(|| {
        "update server did not return a CSRF token — the board may not be running firmware v4.0.1"
            .to_string()
    })? + content_marker.len();
    let rest = &after_marker[content_start..];
    let content_end = rest.find('"').ok_or_else(|| {
        "update server did not return a CSRF token — the board may not be running firmware v4.0.1"
            .to_string()
    })?;
    let token = &rest[..content_end];
    if token.len() == 32 && token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(token.to_string())
    } else {
        Err(
            "update server returned a CSRF token in an unexpected format — the board may not be running firmware v4.0.1"
                .to_string(),
        )
    }
}

/// Upload a firmware image to the board's `WebOTA` endpoint on its
/// usual port (80) — see [`upload_firmware_image_on_port`].
///
/// # Errors
///
/// Returns a message when the server cannot be reached, the token
/// fetch fails, the board answers non-200, or the board's body does
/// not start with `"OK"`. The image itself is NOT verified here — the
/// caller must have run `rsh_security::verify_firmware_image` first.
pub fn upload_firmware_image(host: IpAddr, password: &str, image: &[u8]) -> Result<(), String> {
    upload_firmware_image_on_port(host, WEB_OTA_PORT, password, image)
}

/// Upload a firmware image to the board's `WebOTA` endpoint on an
/// explicit port (`POST /update`, multipart/form-data, field
/// `firmware`, header `X-RSH-CSRF: <token>`). The image is written in
/// 32 KB chunks with an exact `Content-Length`. Production callers use
/// [`upload_firmware_image`] (port 80); this variant exists so tests
/// can point the upload at a fake server on an ephemeral port.
///
/// A `200` with a body starting `"OK"` is success (the board restarts
/// about a second later); anything else is the board's failure reason,
/// returned verbatim so the caller can show it.
///
/// # Errors
///
/// Returns a message when the server cannot be reached, the token
/// fetch fails, the board answers non-200, or the board's body does
/// not start with `"OK"`. The image itself is NOT verified here — the
/// caller must have run `rsh_security::verify_firmware_image` first.
pub fn upload_firmware_image_on_port(
    host: IpAddr,
    port: u16,
    password: &str,
    image: &[u8],
) -> Result<(), String> {
    upload_firmware_image_on_port_with_progress(host, port, password, image, &mut |_| {})
}

/// Upload a firmware image to the board's `WebOTA` endpoint on an
/// explicit port, reporting upload progress: `progress` is called
/// after every 32 KB chunk with the percent of the image sent so far
/// (0..100, 100 once the whole image is on the wire — the board still
/// verifies the signature and writes flash before answering, so 100%
/// sent is not yet success). See [`upload_firmware_image_on_port`]
/// for the contract; production callers use [`upload_firmware_image`].
///
/// # Errors
///
/// Same as [`upload_firmware_image_on_port`].
pub fn upload_firmware_image_on_port_with_progress(
    host: IpAddr,
    port: u16,
    password: &str,
    image: &[u8],
    progress: &mut dyn FnMut(u8),
) -> Result<(), String> {
    let token = fetch_csrf_token(host, port, password)?;
    let preamble = format!(
        "--{MULTIPART_BOUNDARY}\r\nContent-Disposition: form-data; name=\"firmware\"; \
         filename=\"firmware.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    );
    let closing = format!("\r\n--{MULTIPART_BOUNDARY}--\r\n");
    let content_length = preamble.len() + image.len() + closing.len();
    let stream = connect(host, port, POST_READ_TIMEOUT)?;
    let mut reader = HttpReader::new(stream);
    let headers = format!(
        "POST /update HTTP/1.1\r\nHost: {host}\r\nAuthorization: {}\r\nX-RSH-CSRF: {token}\r\n\
         Content-Type: multipart/form-data; boundary={MULTIPART_BOUNDARY}\r\n\
         Content-Length: {content_length}\r\nConnection: close\r\n\r\n",
        basic_auth_header(password)
    );
    reader
        .stream
        .write_all(headers.as_bytes())
        .map_err(|error| format!("failed sending the upload request: {error}"))?;
    reader
        .stream
        .write_all(preamble.as_bytes())
        .map_err(|error| format!("failed sending the upload request: {error}"))?;
    let mut sent = 0usize;
    for chunk in image.chunks(UPLOAD_CHUNK_SIZE) {
        reader
            .stream
            .write_all(chunk)
            .map_err(|error| format!("failed sending the firmware image: {error}"))?;
        sent += chunk.len();
        let percent = if image.is_empty() {
            100
        } else {
            u8::try_from(sent.saturating_mul(100) / image.len()).unwrap_or(100)
        };
        progress(percent);
    }
    reader
        .stream
        .write_all(closing.as_bytes())
        .map_err(|error| format!("failed sending the upload request: {error}"))?;
    if image.is_empty() {
        progress(100);
    }
    let response = read_response(&mut reader)?;
    let body = String::from_utf8_lossy(&response.body);
    if response.status != 200 {
        return Err(format!(
            "update server returned HTTP {}: {}",
            response.status,
            body.trim()
        ));
    }
    if body.starts_with("OK") {
        Ok(())
    } else {
        Err(format!("board rejected the firmware: {}", body.trim()))
    }
}

/// Where to flash: the board's `WebOTA` server address plus the
/// per-device Wi-Fi password from the keystore. The password is a
/// [`ZeroizingString`] (Security L-2) — it is zeroed on drop and never
/// appears in a `Debug` print.
#[derive(Clone, Debug)]
pub struct WebOtaTarget {
    /// Board AP address (`192.168.4.1` in production; the IP the live
    /// session authenticated against).
    pub host: IpAddr,
    /// `WebOTA` server port (80 in production; an ephemeral port only
    /// for tests against a fake board).
    pub port: u16,
    /// Per-device Wi-Fi password (keystore; never a default).
    pub password: ZeroizingString,
}

impl WebOtaTarget {
    /// Production target: the board's `WebOTA` server on its usual port.
    #[must_use]
    pub fn new(host: IpAddr, password: impl Into<String>) -> Self {
        Self {
            host,
            port: WEB_OTA_PORT,
            password: ZeroizingString::new(password),
        }
    }

    /// Target on an explicit port (tests against a fake board only).
    #[must_use]
    pub fn on_port(host: IpAddr, port: u16, password: impl Into<String>) -> Self {
        Self {
            host,
            port,
            password: ZeroizingString::new(password),
        }
    }
}

/// One stage of a firmware flash, reported through the
/// [`flash_firmware`] progress callback.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum FlashStage {
    /// The image file is being read and signature-verified.
    Validating,
    /// Image bytes are going to the board (`percent` is set).
    Uploading,
    /// The board accepted the image and is restarting.
    WaitingForRestart,
    /// The new version is being confirmed on the rebooted board.
    Confirming,
}

/// One progress report from [`flash_firmware`]: the stage, plus the
/// upload percent (0..100) while [`FlashStage::Uploading`], `None` on
/// every other stage.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FlashProgress {
    /// The stage the flash is in.
    pub stage: FlashStage,
    /// Upload percent while uploading, `None` otherwise.
    pub percent: Option<u8>,
}

/// The outcome of [`flash_firmware`]: success with the confirmed new
/// version, or failure with the reason — never a silent or ambiguous
/// result. The caller shows `error` verbatim to the user.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FlashResult {
    /// True only when the board confirmed the new version after the
    /// restart. False on every other outcome, including an upload the
    /// board accepted but whose version could not be confirmed.
    pub success: bool,
    /// The confirmed new firmware version (`"4.0.2"`), when success.
    pub version: Option<String>,
    /// The failure reason, when not success (empty on success).
    pub error: Option<String>,
}

impl FlashResult {
    fn ok(version: String) -> Self {
        Self {
            success: true,
            version: Some(version),
            error: None,
        }
    }

    fn fail(reason: String) -> Self {
        Self {
            success: false,
            version: None,
            error: Some(reason),
        }
    }
}

/// Poll the board's index-page version chip until it reports
/// `expected`, or the timeout runs out. This is the ready-made
/// `confirm_version` closure for [`flash_firmware`] when the caller
/// has no live session to read the signed `GET_CAPS` version from:
/// connection failures while the board reboots are expected and
/// ignored until the deadline; a different version keeps polling
/// (the board may still be coming up on the old image).
///
/// # Errors
///
/// Returns a message when the board never reports `expected` in time
/// (naming the last version seen, when any was seen).
pub fn wait_for_board_version(
    target: &WebOtaTarget,
    expected: &str,
    timeout: Duration,
) -> Result<String, String> {
    let deadline = Instant::now() + timeout;
    let mut last_seen: Option<String> = None;
    loop {
        match fetch_board_version(target.host, target.port, target.password.as_str()) {
            Ok(reported) if reported == expected => return Ok(reported),
            Ok(reported) => last_seen = Some(reported),
            Err(_) => { /* board rebooting — retry until the deadline */ }
        }
        if Instant::now() >= deadline {
            return Err(match last_seen {
                Some(seen) => format!(
                    "board did not report firmware version {expected} within {} s (last seen: {seen})",
                    timeout.as_secs()
                ),
                None => format!(
                    "board did not answer within {} s after the upload — version {expected} not confirmed",
                    timeout.as_secs()
                ),
            });
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Flash a signed firmware image to the board end to end, against the
/// production release key:
///
/// 1. **Validate** — the image is read and verified (Ed25519 signature + format + anti-downgrade vs `installed_version`; an older image is rejected, an equal version is a reinstall and is allowed, matching the board rule in `ota_validate.h`). Nothing is sent before this passes.
/// 2. **Upload** — the image goes to the board's `WebOTA` endpoint with progress (stage [`FlashStage::Uploading`], percent 0..100).
/// 3. **Confirm** — after the board accepts and restarts, `confirm_version` runs: it must wait for the rebooted board and return its live firmware version (the daemon passes a closure that reconnects and reads the signed `GET_CAPS` version, or [`wait_for_board_version`]). Success needs the reported version to equal the image version — HTTP 200 + `"OK"` alone is not a confirm. A failed upload leaves the old firmware running (the
///    board only applies a fully verified image to the other OTA slot).
///
/// `image_path` is a parameter on purpose: the daemon passes
/// `<exe dir>/firmware-signed.bin` (the installer bundles it beside
/// the daemon, the sidecar pattern). `installed_version` comes from
/// `get_firmware_version` (the live session's `GET_CAPS`) before the
/// flash.
///
/// The result is a [`FlashResult`], never a panic and never a bare
/// `Ok(())` on an unconfirmed flash.
#[must_use]
pub fn flash_firmware(
    image_path: &Path,
    target: &WebOtaTarget,
    installed_version: FirmwareVersion,
    progress: &mut dyn FnMut(FlashProgress),
    confirm_version: &mut dyn FnMut() -> Result<String, String>,
) -> FlashResult {
    flash_firmware_with_pubkey(
        image_path,
        target,
        installed_version,
        &FIRMWARE_SIGNING_PUBKEY,
        progress,
        confirm_version,
    )
}

/// Same as [`flash_firmware`], but the caller supplies the Ed25519
/// public key. Production code always uses [`flash_firmware`] (the
/// embedded release key); this entry point exists so tests can flash
/// images built with a throwaway keypair without ever touching the
/// production signing key, and so a future key-migration path can
/// verify against a second key — the same pattern as
/// `rsh_security::verify_firmware_image_with_pubkey`.
#[must_use]
pub fn flash_firmware_with_pubkey(
    image_path: &Path,
    target: &WebOtaTarget,
    installed_version: FirmwareVersion,
    pubkey: &[u8; 32],
    progress: &mut dyn FnMut(FlashProgress),
    confirm_version: &mut dyn FnMut() -> Result<String, String>,
) -> FlashResult {
    progress(FlashProgress {
        stage: FlashStage::Validating,
        percent: None,
    });
    let bytes = match std::fs::read(image_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return FlashResult::fail(format!(
                "could not read the firmware image at {}: {error}",
                image_path.display()
            ));
        }
    };
    let info = match verify_firmware_image_with_pubkey(&bytes, installed_version, pubkey) {
        Ok(info) => info,
        Err(error) => {
            return FlashResult::fail(format!("firmware image failed verification: {error}"));
        }
    };
    let expected = format!("{}.{}.{}", info.version.0, info.version.1, info.version.2);
    let mut upload_progress = |percent: u8| {
        progress(FlashProgress {
            stage: FlashStage::Uploading,
            percent: Some(percent),
        });
    };
    if let Err(error) = upload_firmware_image_on_port_with_progress(
        target.host,
        target.port,
        target.password.as_str(),
        &bytes,
        &mut upload_progress,
    ) {
        return FlashResult::fail(format!(
            "firmware upload failed (old firmware still running): {error}"
        ));
    }
    progress(FlashProgress {
        stage: FlashStage::WaitingForRestart,
        percent: None,
    });
    progress(FlashProgress {
        stage: FlashStage::Confirming,
        percent: None,
    });
    match confirm_version() {
        Ok(reported) if reported == expected => FlashResult::ok(expected),
        Ok(reported) => FlashResult::fail(format!(
            "board reports firmware version {reported}, expected {expected} — the update may not have applied"
        )),
        Err(error) => FlashResult::fail(format!(
            "could not confirm the new firmware version ({expected}): {error}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsh_security::firmware_verify::{FIRMWARE_ESP_IMAGE_MAGIC, FIRMWARE_IMAGE_MIN_LEN};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// One HTTP request as the fake server saw it.
    #[derive(Debug)]
    struct CapturedRequest {
        /// Request line, e.g. `GET / HTTP/1.1`.
        request_line: String,
        /// Headers as `(lowercase name, value)` pairs.
        headers: Vec<(String, String)>,
        /// Request body bytes.
        body: Vec<u8>,
    }

    /// Read one HTTP request from a client stream (headers +
    /// `Content-Length` body).
    fn read_request(stream: &mut TcpStream) -> CapturedRequest {
        let mut raw = Vec::new();
        let mut byte = [0u8; 1];
        while !raw.ends_with(b"\r\n\r\n") {
            match stream.read(&mut byte) {
                Ok(0) | Err(_) => break,
                Ok(_) => raw.extend_from_slice(&byte),
            }
        }
        let text = String::from_utf8_lossy(&raw).into_owned();
        let mut lines = text.lines();
        let request_line = lines.next().unwrap_or("").to_string();
        let mut headers = Vec::new();
        let mut content_length = 0usize;
        for line in lines {
            if let Some((name, value)) = line.split_once(':') {
                let name = name.trim().to_ascii_lowercase();
                let value = value.trim().to_string();
                if name == "content-length" {
                    content_length = value.parse().unwrap_or(0);
                }
                headers.push((name, value));
            }
        }
        let mut body = vec![0u8; content_length];
        let _ = stream.read_exact(&mut body);
        CapturedRequest {
            request_line,
            headers,
            body,
        }
    }

    /// A fake `WebOTA` server on an ephemeral localhost port. `responses`
    /// is one canned response per request, in order; every request is
    /// sent back over `captured` for the test to assert on. Returns the
    /// port the server listens on.
    fn spawn_fake_server(responses: Vec<Vec<u8>>, captured: mpsc::Sender<CapturedRequest>) -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fake server");
        let port = listener.local_addr().expect("server addr").port();
        std::thread::spawn(move || {
            for response in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let request = read_request(&mut stream);
                let _ = captured.send(request);
                let _ = stream.write_all(&response);
            }
        });
        port
    }

    /// A `200` response with a `Content-Length` body.
    fn response_200(body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    /// A `200` response with a chunked body (what the ESP sends for
    /// the index page — `CONTENT_LENGTH_UNKNOWN`).
    fn response_200_chunked(body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
            body.len()
        )
        .into_bytes()
    }

    /// The index page the board serves, with a 32-hex CSRF token.
    fn index_page() -> String {
        format!(
            "<html><head><meta name=\"csrf-token\" content=\"{}\"></head></html>",
            "0123456789abcdef0123456789abcdef"
        )
    }

    fn header<'a>(request: &'a CapturedRequest, name: &str) -> Option<&'a str> {
        request
            .headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn base64_encode_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"admin:password"), "YWRtaW46cGFzc3dvcmQ=");
    }

    #[test]
    fn upload_succeeds_and_sends_the_signed_image() {
        let (captured_tx, captured_rx) = mpsc::channel();
        let image: Vec<u8> = (0..100_000u32).map(|n| (n % 251) as u8).collect();
        let port = spawn_fake_server(
            vec![
                response_200_chunked(&index_page()),
                response_200("OK - updated"),
            ],
            captured_tx,
        );
        let result =
            upload_firmware_image_on_port(IpAddr::from([127, 0, 0, 1]), port, "secret", &image);
        assert!(result.is_ok(), "result: {result:?}");

        let get = captured_rx.recv().expect("GET captured");
        assert_eq!(get.request_line, "GET / HTTP/1.1");
        assert_eq!(
            header(&get, "authorization"),
            Some("Basic YWRtaW46c2VjcmV0"),
            "GET carries Basic admin:secret"
        );
        let upload = captured_rx.recv().expect("POST captured");
        assert_eq!(upload.request_line, "POST /update HTTP/1.1");
        assert_eq!(
            header(&upload, "x-rsh-csrf"),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert!(
            header(&upload, "content-type")
                .is_some_and(|value| value.starts_with("multipart/form-data; boundary=")),
            "POST is multipart"
        );
        // The body is the multipart envelope around the image bytes.
        assert!(
            upload
                .body
                .windows(image.len())
                .any(|window| window == image.as_slice()),
            "POST body contains the image bytes"
        );
        assert!(
            String::from_utf8_lossy(&upload.body)
                .contains("name=\"firmware\"; filename=\"firmware.bin\""),
            "POST field is named firmware"
        );
    }

    #[test]
    fn upload_rejects_a_wrong_password() {
        let (captured_tx, _captured_rx) = mpsc::channel();
        let port = spawn_fake_server(
            vec![
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec(),
            ],
            captured_tx,
        );
        let result =
            upload_firmware_image_on_port(IpAddr::from([127, 0, 0, 1]), port, "wrong", b"image");
        assert!(result.is_err());
        let error = result.expect_err("upload must fail on 401");
        assert!(error.contains("401"), "{error}");
        assert!(error.contains("re-pair"), "{error}");
    }

    #[test]
    fn upload_rejects_a_page_without_a_csrf_token() {
        let (captured_tx, _captured_rx) = mpsc::channel();
        let port = spawn_fake_server(
            vec![response_200("<html><head></head></html>")],
            captured_tx,
        );
        let result =
            upload_firmware_image_on_port(IpAddr::from([127, 0, 0, 1]), port, "secret", b"image");
        let error = result.expect_err("upload must fail without a token");
        assert!(error.contains("CSRF"), "{error}");
    }

    #[test]
    fn upload_reports_the_boards_fail_body() {
        let (captured_tx, _captured_rx) = mpsc::channel();
        let port = spawn_fake_server(
            vec![
                response_200(&index_page()),
                response_200("FAIL - bad signature"),
            ],
            captured_tx,
        );
        let result =
            upload_firmware_image_on_port(IpAddr::from([127, 0, 0, 1]), port, "secret", b"image");
        let error = result.expect_err("upload must fail on a FAIL body");
        assert!(error.contains("bad signature"), "{error}");
    }

    #[test]
    fn upload_reports_an_unreachable_server() {
        // Bind a port, then drop the listener so nothing answers there.
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind probe");
        let port = listener.local_addr().expect("probe addr").port();
        drop(listener);
        let result =
            upload_firmware_image_on_port(IpAddr::from([127, 0, 0, 1]), port, "secret", b"image");
        let error = result.expect_err("upload must fail with no server");
        assert!(error.contains("could not reach"), "{error}");
        assert!(error.contains("Wi-Fi network"), "{error}");
    }

    // ---- flash_firmware flow (Round 2C) ----

    /// Fixed, non-secret seed for the throwaway test keypair (same
    /// pattern as `rsh_security::firmware_verify` tests). This key
    /// signs test images only; it is NOT the production signing key.
    const FLASH_TEST_SEED: [u8; 32] = [
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
        0x00, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xa0, 0xb0, 0xc0, 0xd0, 0xe0,
        0xf0, 0x0f,
    ];

    fn flash_test_pubkey() -> [u8; 32] {
        use ed25519_dalek::SigningKey;
        SigningKey::from_bytes(&FLASH_TEST_SEED)
            .verifying_key()
            .to_bytes()
    }

    /// Build a signed test image: `body_len` bytes starting with
    /// `0xE9`, the version marker at offset 1024, signed over
    /// `SHA-256(body)` with the throwaway test key.
    fn build_flash_test_image(version: &str, body_len: usize) -> Vec<u8> {
        use ed25519_dalek::{Signer, SigningKey};
        use sha2::{Digest, Sha256};
        let mut body = vec![0u8; body_len];
        body[0] = FIRMWARE_ESP_IMAGE_MAGIC;
        let marker = format!("Version <b>{version}");
        body[1024..1024 + marker.len()].copy_from_slice(marker.as_bytes());
        let digest: [u8; 32] = Sha256::digest(&body).into();
        let signature = SigningKey::from_bytes(&FLASH_TEST_SEED).sign(&digest);
        let mut image = body;
        image.extend_from_slice(&signature.to_bytes());
        image.extend_from_slice(b"RSHS");
        image
    }

    /// Write `image` to a per-test temp file and return its path.
    fn write_temp_image(name: &str, image: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rsh-webota-test-{name}-{}-{}.bin",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::write(&path, image).expect("write temp image");
        path
    }

    /// The index page with a version chip, as the board serves it.
    fn index_page_with_version(version: &str) -> String {
        format!(
            "<html><head><meta name=\"csrf-token\" content=\"0123456789abcdef0123456789abcdef\"></head>\
             <body><span class=\"chip\">Version <b>{version}</b></span></body></html>"
        )
    }

    fn run_flash(
        path: &std::path::Path,
        port: u16,
        installed: FirmwareVersion,
        confirm: &mut dyn FnMut() -> Result<String, String>,
    ) -> (FlashResult, Vec<FlashProgress>) {
        let target = WebOtaTarget::on_port(IpAddr::from([127, 0, 0, 1]), port, "secret");
        let mut seen = Vec::new();
        let result = flash_firmware_with_pubkey(
            path,
            &target,
            installed,
            &flash_test_pubkey(),
            &mut |progress| seen.push(progress),
            confirm,
        );
        (result, seen)
    }

    #[test]
    fn flash_succeeds_with_progress_and_version_confirm() {
        let (captured_tx, _captured_rx) = mpsc::channel();
        let port = spawn_fake_server(
            vec![
                response_200_chunked(&index_page()),
                response_200("OK - update ho gaya, board restart ho raha hai"),
            ],
            captured_tx,
        );
        let image = build_flash_test_image("4.0.2", FIRMWARE_IMAGE_MIN_LEN);
        let path = write_temp_image("ok", &image);
        let (result, seen) = run_flash(&path, port, (4, 0, 1), &mut || Ok("4.0.2".to_string()));
        let _ = std::fs::remove_file(&path);
        assert!(result.success, "result: {result:?}");
        assert_eq!(result.version.as_deref(), Some("4.0.2"));
        assert_eq!(result.error, None);
        // Stages in order: Validating, Uploading (percents rising to
        // 100), WaitingForRestart, Confirming.
        assert_eq!(seen.first().map(|p| p.stage), Some(FlashStage::Validating));
        let uploading: Vec<u8> = seen
            .iter()
            .filter(|p| p.stage == FlashStage::Uploading)
            .filter_map(|p| p.percent)
            .collect();
        assert_eq!(
            uploading.last(),
            Some(&100),
            "uploading percents: {uploading:?}"
        );
        assert!(
            uploading.windows(2).all(|w| w[0] <= w[1]),
            "percents never go backwards: {uploading:?}"
        );
        let stages: Vec<FlashStage> = seen.iter().map(|p| p.stage).collect();
        let waiting = stages
            .iter()
            .position(|s| *s == FlashStage::WaitingForRestart)
            .expect("WaitingForRestart reported");
        let confirming = stages
            .iter()
            .position(|s| *s == FlashStage::Confirming)
            .expect("Confirming reported");
        assert!(waiting < confirming, "stages: {stages:?}");
    }

    #[test]
    fn flash_allows_reinstall_of_the_same_version() {
        let (captured_tx, _captured_rx) = mpsc::channel();
        let port = spawn_fake_server(
            vec![
                response_200_chunked(&index_page()),
                response_200("OK - update ho gaya"),
            ],
            captured_tx,
        );
        let image = build_flash_test_image("4.0.1", FIRMWARE_IMAGE_MIN_LEN);
        let path = write_temp_image("reinstall", &image);
        let (result, _) = run_flash(&path, port, (4, 0, 1), &mut || Ok("4.0.1".to_string()));
        let _ = std::fs::remove_file(&path);
        assert!(result.success, "reinstall must succeed: {result:?}");
        assert_eq!(result.version.as_deref(), Some("4.0.1"));
    }

    #[test]
    fn flash_rejects_a_bad_signature_before_any_network() {
        let (captured_tx, captured_rx) = mpsc::channel();
        // No canned responses: any request would hang the fake server
        // thread, so a request here fails the test by timeout instead —
        // the flow must never reach the network with a bad image.
        let port = spawn_fake_server(vec![], captured_tx);
        let mut image = build_flash_test_image("4.0.2", FIRMWARE_IMAGE_MIN_LEN);
        image[2000] ^= 0x01; // tampered body byte -> bad signature
        let path = write_temp_image("badsig", &image);
        let (result, seen) = run_flash(&path, port, (4, 0, 1), &mut || Ok("4.0.2".to_string()));
        let _ = std::fs::remove_file(&path);
        assert!(!result.success, "result: {result:?}");
        let error = result.error.expect("failure names the reason");
        assert!(error.contains("signature"), "{error}");
        assert!(
            captured_rx.try_recv().is_err(),
            "no request may reach the board with a bad image"
        );
        assert_eq!(seen.len(), 1, "only Validating is reported: {seen:?}");
        assert_eq!(seen[0].stage, FlashStage::Validating);
    }

    #[test]
    fn flash_rejects_a_downgrade() {
        let (captured_tx, _captured_rx) = mpsc::channel();
        let port = spawn_fake_server(vec![], captured_tx);
        let image = build_flash_test_image("4.0.0", FIRMWARE_IMAGE_MIN_LEN);
        let path = write_temp_image("downgrade", &image);
        let (result, _) = run_flash(&path, port, (4, 0, 1), &mut || Ok("4.0.0".to_string()));
        let _ = std::fs::remove_file(&path);
        assert!(!result.success, "result: {result:?}");
        let error = result.error.expect("failure names the reason");
        assert!(error.contains("downgrade"), "{error}");
    }

    #[test]
    fn flash_reports_a_board_rejection() {
        let (captured_tx, _captured_rx) = mpsc::channel();
        let port = spawn_fake_server(
            vec![
                response_200_chunked(&index_page()),
                response_200("FAIL - signature verify fail"),
            ],
            captured_tx,
        );
        let image = build_flash_test_image("4.0.2", FIRMWARE_IMAGE_MIN_LEN);
        let path = write_temp_image("boardfail", &image);
        let (result, _) = run_flash(&path, port, (4, 0, 1), &mut || Ok("4.0.2".to_string()));
        let _ = std::fs::remove_file(&path);
        assert!(!result.success, "result: {result:?}");
        let error = result.error.expect("failure names the reason");
        assert!(error.contains("old firmware still running"), "{error}");
        assert!(error.contains("signature verify fail"), "{error}");
    }

    #[test]
    fn flash_fails_when_confirm_reports_the_old_version() {
        let (captured_tx, _captured_rx) = mpsc::channel();
        let port = spawn_fake_server(
            vec![
                response_200_chunked(&index_page()),
                response_200("OK - update ho gaya"),
            ],
            captured_tx,
        );
        let image = build_flash_test_image("4.0.2", FIRMWARE_IMAGE_MIN_LEN);
        let path = write_temp_image("oldconfirm", &image);
        // The board says OK but still reports 4.0.1 — not a confirm.
        let (result, _) = run_flash(&path, port, (4, 0, 1), &mut || Ok("4.0.1".to_string()));
        let _ = std::fs::remove_file(&path);
        assert!(!result.success, "result: {result:?}");
        let error = result.error.expect("failure names the reason");
        assert!(error.contains("4.0.1"), "{error}");
        assert!(error.contains("4.0.2"), "{error}");
    }

    #[test]
    fn flash_fails_when_confirm_errors() {
        let (captured_tx, _captured_rx) = mpsc::channel();
        let port = spawn_fake_server(
            vec![
                response_200_chunked(&index_page()),
                response_200("OK - update ho gaya"),
            ],
            captured_tx,
        );
        let image = build_flash_test_image("4.0.2", FIRMWARE_IMAGE_MIN_LEN);
        let path = write_temp_image("confirmerr", &image);
        let (result, _) = run_flash(&path, port, (4, 0, 1), &mut || {
            Err("board did not answer after the upload".to_string())
        });
        let _ = std::fs::remove_file(&path);
        assert!(!result.success, "result: {result:?}");
        let error = result.error.expect("failure names the reason");
        assert!(error.contains("could not confirm"), "{error}");
    }

    #[test]
    fn flash_fails_on_a_missing_image_file() {
        let (captured_tx, _captured_rx) = mpsc::channel();
        let port = spawn_fake_server(vec![], captured_tx);
        let missing = std::env::temp_dir().join("rsh-webota-test-definitely-missing.bin");
        let (result, _) = run_flash(&missing, port, (4, 0, 1), &mut || Ok("4.0.2".to_string()));
        assert!(!result.success, "result: {result:?}");
        assert!(
            result
                .error
                .as_ref()
                .is_some_and(|e| e.contains("could not read")),
            "result: {result:?}"
        );
    }

    #[test]
    fn fetch_board_version_reads_the_chip() {
        let (captured_tx, _captured_rx) = mpsc::channel();
        let port = spawn_fake_server(
            vec![response_200_chunked(&index_page_with_version("4.0.2"))],
            captured_tx,
        );
        let version =
            fetch_board_version(IpAddr::from([127, 0, 0, 1]), port, "secret").expect("version");
        assert_eq!(version, "4.0.2");
    }

    #[test]
    fn fetch_board_version_fails_without_a_chip() {
        let (captured_tx, _captured_rx) = mpsc::channel();
        let port = spawn_fake_server(vec![response_200_chunked(&index_page())], captured_tx);
        let error = fetch_board_version(IpAddr::from([127, 0, 0, 1]), port, "secret")
            .expect_err("no chip must fail");
        assert!(error.contains("version"), "{error}");
    }

    #[test]
    fn version_chip_parsing_is_strict() {
        assert_eq!(
            parse_board_version_chip("Version <b>4.0.2</b>").expect("4.0.2"),
            "4.0.2"
        );
        assert_eq!(
            parse_board_version_chip("xx Version <b>10.20.30</b> yy").expect("10.20.30"),
            "10.20.30"
        );
        assert!(parse_board_version_chip("no chip here").is_err());
        assert!(parse_board_version_chip("Version <b>4.0</b>").is_err());
        assert!(parse_board_version_chip("Version <b>4.0.256</b>").is_err());
        assert!(parse_board_version_chip("Version <b></b>").is_err());
    }

    #[test]
    fn wait_for_board_version_polls_until_the_new_version() {
        let (captured_tx, _captured_rx) = mpsc::channel();
        let port = spawn_fake_server(
            vec![
                response_200_chunked(&index_page_with_version("4.0.1")),
                response_200_chunked(&index_page_with_version("4.0.1")),
                response_200_chunked(&index_page_with_version("4.0.2")),
            ],
            captured_tx,
        );
        let target = WebOtaTarget::on_port(IpAddr::from([127, 0, 0, 1]), port, "secret");
        let reported = wait_for_board_version(&target, "4.0.2", Duration::from_secs(10))
            .expect("new version must appear");
        assert_eq!(reported, "4.0.2");
    }

    #[test]
    fn wait_for_board_version_times_out_on_the_old_version() {
        let (captured_tx, _captured_rx) = mpsc::channel();
        let port = spawn_fake_server(
            vec![
                response_200_chunked(&index_page_with_version("4.0.1")),
                response_200_chunked(&index_page_with_version("4.0.1")),
                response_200_chunked(&index_page_with_version("4.0.1")),
                response_200_chunked(&index_page_with_version("4.0.1")),
            ],
            captured_tx,
        );
        let target = WebOtaTarget::on_port(IpAddr::from([127, 0, 0, 1]), port, "secret");
        let error = wait_for_board_version(&target, "4.0.2", Duration::from_millis(1100))
            .expect_err("old version must time out");
        assert!(error.contains("4.0.2"), "{error}");
        assert!(error.contains("4.0.1"), "{error}");
    }
}
