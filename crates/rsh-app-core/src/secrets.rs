//! Zeroizing secret strings (Security L-2).
//!
//! Wi-Fi passwords and other short-lived secrets must not linger in
//! memory after use: a plain `String` keeps its bytes on the heap
//! until the allocator reuses the page, and a clone in a log line or
//! an error message can copy them anywhere. [`ZeroizingString`] wraps
//! a `String` and overwrites its bytes with zeros on drop (and on
//! explicit [`ZeroizingString::zeroize`]), and it deliberately has no
//! `Display` / `Debug` that prints the value — `Debug` shows only the
//! length — so a secret can never leak through a `{:?}` in a log.
//!
//! This is a local type (the `zeroize` crate is only a transitive
//! dependency; adding a workspace dependency for one wrapper would
//! pull a new crate into the release build for no gain).

use std::fmt;

/// A heap string whose bytes are zeroed when it is dropped.
///
/// Construct with [`ZeroizingString::new`] / `From<&str>`; read the
/// value only through [`ZeroizingString::as_str`], at the exact call
/// site that needs it. Cloning copies the secret (unavoidable when a
/// value must move into a session), but the clone is zeroed on its
/// own drop too. Never log the value: [`fmt::Debug`] prints
/// `ZeroizingString(len = N)` and there is no `Display` impl.
#[derive(Clone)]
pub struct ZeroizingString {
    inner: String,
}

impl ZeroizingString {
    /// Wraps a secret string.
    #[must_use]
    pub fn new(secret: impl Into<String>) -> Self {
        Self {
            inner: secret.into(),
        }
    }

    /// Borrows the secret value. Keep the borrow as short as possible
    /// — copy it into the call that needs it and drop the borrow.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.inner
    }

    /// True when the wrapped string is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Length of the secret in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Overwrites the bytes with zeros immediately (the same thing
    /// [`Drop`] does). The string stays allocated but empty of secret
    /// data; call this as soon as the value is no longer needed
    /// instead of waiting for the drop.
    pub fn zeroize(&mut self) {
        // SAFETY: overwriting the string's bytes in place with zeros
        // keeps valid UTF-8 (NUL is valid UTF-8) and never changes the
        // length, capacity, or allocation.
        unsafe {
            let bytes = self.inner.as_bytes_mut();
            std::ptr::write_bytes(bytes.as_mut_ptr(), 0, bytes.len());
        }
    }
}

impl Drop for ZeroizingString {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl fmt::Debug for ZeroizingString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZeroizingString")
            .field("len", &self.inner.len())
            .finish()
    }
}

impl From<&str> for ZeroizingString {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for ZeroizingString {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl AsRef<str> for ZeroizingString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq for ZeroizingString {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl Eq for ZeroizingString {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_the_secret() {
        let secret = ZeroizingString::new("super-secret-password");
        let rendered = format!("{secret:?}");
        assert!(!rendered.contains("super-secret-password"), "{rendered}");
        assert!(rendered.contains("len"), "{rendered}");
    }

    #[test]
    fn zeroize_clears_bytes_without_changing_length() {
        let mut secret = ZeroizingString::new("abc123");
        secret.zeroize();
        assert_eq!(secret.as_str().as_bytes(), &[0, 0, 0, 0, 0, 0]);
        assert_eq!(secret.len(), 6);
    }
}
