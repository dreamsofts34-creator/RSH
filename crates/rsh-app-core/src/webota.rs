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
//! The board is expected on its own AP, so the PC must be joined to
//! the board's Wi-Fi network for the upload to reach it; a TCP
//! connect failure says so plainly instead of pretending otherwise.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpStream};
use std::time::Duration;

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
    let page = String::from_utf8_lossy(&response.body);
    extract_csrf_token(&page)
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
    for chunk in image.chunks(UPLOAD_CHUNK_SIZE) {
        reader
            .stream
            .write_all(chunk)
            .map_err(|error| format!("failed sending the firmware image: {error}"))?;
    }
    reader
        .stream
        .write_all(closing.as_bytes())
        .map_err(|error| format!("failed sending the upload request: {error}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
