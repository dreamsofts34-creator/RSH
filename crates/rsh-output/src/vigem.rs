//! Xbox 360 output via ViGEmBus (v2.0.19).
//!
//! The fallback backend for games that do not accept a vJoy wheel:
//! RSH presents as a wired Xbox 360 controller through the
//! ViGEmBus kernel driver, using the ViGEmClient API directly.
//! No `vigem` crate (unverified, single maintainer) — raw FFI
//! through `libloading`, the same rule as the vJoy backend.
//!
//! # Driver vs DLL
//!
//! `ViGEmClient.dll` is NOT the driver — it is the user-mode
//! client library, shipped beside `rsh-daemon.exe` (never
//! installed system-wide). The driver is ViGEmBus itself; the
//! authoritative "is it installed" check is `vigem_connect()`
//! returning `VIGEM_ERROR_BUS_NOT_FOUND` (`0xE0000001`). The
//! wizard offers the official ViGEmBus 1.22.0 setup EXE for an
//! interactive install when the driver is missing.
//!
//! # Thread safety
//!
//! The ViGEmBus client library is NOT thread-safe — one client
//! only, guarded by the [`Mutex`] in [`VigemOutput`]. Every FFI
//! call goes through that lock (and the trait hands out `&mut
//! self`, so calls are serialized twice over). `vigem_connect`
//! is documented "expensive": it runs once at connect, never
//! per tick.
//!
//! # Lifecycle
//!
//! `alloc → connect → x360_alloc → add → update (200 Hz) →
//! remove → target_free → disconnect → free`. `Drop` runs the
//! teardown best-effort, so a dropped backend never leaves an
//! orphaned target behind until process exit.
//!
//! # Mapping (coordinator decision, v2.0.19)
//!
//! Only the 7 surviving inputs are mapped (see [`build_report`]):
//! steering → `sThumbLX`, accelerator → `bRightTrigger`, brake →
//! `bLeftTrigger`, clutch → `sThumbRY` (pressed = −32768, rest 0),
//! handbrake → A (`0x1000`), horn → X (`0x4000`), shifter up/down
//! → D-pad Up (`0x0001`) / Down (`0x0002`). The game sees the
//! Controller tab, never the Wheel tab — this backend is a
//! drive-it-anyway fallback, not a wheel.
//!
//! # Error convention
//!
//! ViGEm success is `0x20000000`, NOT 0. All errors are
//! `0xE00000xx`. [`VigemError`] converts into the trait's
//! [`VJoyError`](crate::vjoy::VJoyError) so the selector's
//! fallback wording stays the same shape as vJoy's.

use std::ffi::c_void;
use std::fmt;
use std::mem::{offset_of, size_of};
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::sync::Mutex;

use libloading::{Library, Symbol};

use crate::vjoy::VJoyError;
use crate::{IJoystickOutput, ProcessedState};

/// File name of the ViGEmBus client library. Shipped beside
/// `rsh-daemon.exe` by the RSH installer (same rule as the vJoy
/// DLL: never bundled into the repo, resolved at runtime).
pub const VIGEM_DLL_FILE_NAME: &str = "ViGEmClient.dll";

/// ViGEm success — `0x20000000`, NOT 0.
pub const VIGEM_ERROR_NONE: u32 = 0x2000_0000;
/// No compatible bus driver on the system — the "driver not
/// installed" signal.
pub const VIGEM_ERROR_BUS_NOT_FOUND: u32 = 0xE000_0001;
/// All device slots occupied.
pub const VIGEM_ERROR_NO_FREE_SLOT: u32 = 0xE000_0002;
/// Invalid target device.
pub const VIGEM_ERROR_INVALID_TARGET: u32 = 0xE000_0003;
/// Update on an unplugged target.
pub const VIGEM_ERROR_TARGET_NOT_PLUGGED_IN: u32 = 0xE000_0007;
/// Incompatible driver version.
pub const VIGEM_ERROR_BUS_VERSION_MISMATCH: u32 = 0xE000_0008;

/// D-pad Up button bit (`XINPUT_GAMEPAD_DPAD_UP`).
pub const XUSB_DPAD_UP: u16 = 0x0001;
/// D-pad Down button bit (`XINPUT_GAMEPAD_DPAD_DOWN`).
pub const XUSB_DPAD_DOWN: u16 = 0x0002;
/// A button bit (`XINPUT_GAMEPAD_A`) — the handbrake.
pub const XUSB_BUTTON_A: u16 = 0x1000;
/// X button bit (`XINPUT_GAMEPAD_X`) — the horn.
pub const XUSB_BUTTON_X: u16 = 0x4000;

/// `XUSB_REPORT` — the 12-byte Xbox 360 report, identical to
/// `XINPUT_GAMEPAD` (ViGEm README, Microsoft Learn, ViGEm SDK
/// `BusShared.h`). `#[repr(C)]`, no padding: `size_of` must be
/// exactly 12 (asserted below).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XusbReport {
    /// Button bits, offset 0.
    pub w_buttons: u16,
    /// Left trigger, 0..=255, offset 2 — the brake.
    pub b_left_trigger: u8,
    /// Right trigger, 0..=255, offset 3 — the accelerator.
    pub b_right_trigger: u8,
    /// Left stick X, −32768..=32767, offset 4 — the steering
    /// (positive = right, XInput convention).
    pub s_thumb_lx: i16,
    /// Left stick Y, offset 6 — unused, always 0.
    pub s_thumb_ly: i16,
    /// Right stick X, offset 8 — unused, always 0.
    pub s_thumb_rx: i16,
    /// Right stick Y, −32768..=32767, offset 10 — the clutch
    /// (pressed = −32768, rest 0).
    pub s_thumb_ry: i16,
}

const _: () = assert!(size_of::<XusbReport>() == 12);
const _: () = assert!(offset_of!(XusbReport, w_buttons) == 0);
const _: () = assert!(offset_of!(XusbReport, b_left_trigger) == 2);
const _: () = assert!(offset_of!(XusbReport, b_right_trigger) == 3);
const _: () = assert!(offset_of!(XusbReport, s_thumb_lx) == 4);
const _: () = assert!(offset_of!(XusbReport, s_thumb_ly) == 6);
const _: () = assert!(offset_of!(XusbReport, s_thumb_rx) == 8);
const _: () = assert!(offset_of!(XusbReport, s_thumb_ry) == 10);

/// An opaque ViGEmBus handle (client or target) as returned by
/// `vigem_alloc` / `vigem_target_x360_alloc`.
///
/// `*mut c_void` is not `Send` (the extern type carries no
/// auto-trait), but the handle is only ever touched while holding
/// the [`VigemOutput`] mutex, on the thread that owns the backend —
/// the same exclusivity the `IJoystickOutput` `&mut self` contract
/// already gives. Hence `Send` is sound here; the `unsafe impl`
/// documents exactly that reasoning. The inner pointer is never
/// dereferenced from Rust — it is only handed back to the API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VigemHandle(*mut c_void);
unsafe impl Send for VigemHandle {}

/// Map one processed tick to an [`XusbReport`] (pure, testable on
/// any OS — no driver, no DLL).
///
/// * steering (−1..=1) → `sThumbLX` (−32767..=32767, right positive)
/// * accelerator (0..=1) → `bRightTrigger` (0..=255)
/// * brake (0..=1) → `bLeftTrigger` (0..=255)
/// * clutch (0..=1) → `sThumbRY` (0 at rest, −32768 fully pressed)
/// * handbrake (0..=1, analog) → A while pressed at all (the
///   pipeline deadzones the rest value to exactly 0, so any
///   non-zero value is a real pull)
/// * horn → X
/// * shifter up/down (`buttons` bits 0/1) → D-pad Up/Down
///
/// Everything else is 0: `sThumbLY`/`sThumbRX` are never driven,
/// and no other button is ever set (only the 7 surviving inputs
/// exist in v2.0.19).
#[must_use]
pub fn build_report(state: &ProcessedState) -> XusbReport {
    let mut buttons: u16 = 0;
    if state.buttons & 0b01 != 0 {
        buttons |= XUSB_DPAD_UP;
    }
    if state.buttons & 0b10 != 0 {
        buttons |= XUSB_DPAD_DOWN;
    }
    if state.handbrake > 0.0 {
        buttons |= XUSB_BUTTON_A;
    }
    if state.horn {
        buttons |= XUSB_BUTTON_X;
    }
    XusbReport {
        w_buttons: buttons,
        b_left_trigger: (state.brake.clamp(0.0, 1.0) * 255.0) as u8,
        b_right_trigger: (state.accelerator.clamp(0.0, 1.0) * 255.0) as u8,
        s_thumb_lx: (state.steering.clamp(-1.0, 1.0) * 32767.0) as i16,
        s_thumb_ly: 0,
        s_thumb_rx: 0,
        s_thumb_ry: (-(state.clutch.clamp(0.0, 1.0) * 32768.0)) as i16,
    }
}

/// Everything that can go wrong in the Xbox (ViGEmBus) backend.
///
/// Display strings are the human-readable reason: the caller shows
/// them verbatim, so they name the fix ("install the ViGEmBus
/// driver …"), never just a code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VigemError {
    /// `ViGEmClient.dll` was not found beside the daemon exe.
    DllNotFound,
    /// `libloading` could not load the DLL or resolve a symbol.
    DllLoadFailed(String),
    /// `vigem_connect` returned `VIGEM_ERROR_BUS_NOT_FOUND` —
    /// the ViGEmBus driver is not installed.
    BusNotFound,
    /// `vigem_connect` failed with a non-bus error (raw code).
    ConnectFailed(u32),
    /// `vigem_alloc` returned NULL (OOM only).
    ClientAllocFailed,
    /// `vigem_target_x360_alloc` returned NULL (OOM only).
    TargetAllocFailed,
    /// `vigem_target_add` refused the target (raw code).
    TargetAddFailed(u32),
    /// `vigem_target_x360_update` refused a connected target
    /// (raw code).
    UpdateFailed(u32),
    /// `update` called while not connected.
    NotConnected,
}

impl fmt::Display for VigemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DllNotFound => write!(
                f,
                "ViGEmClient.dll was not found beside rsh-daemon.exe — re-run the RSH installer."
            ),
            Self::DllLoadFailed(msg) => write!(f, "could not load ViGEmClient.dll: {msg}"),
            Self::BusNotFound => write!(
                f,
                "ViGEmBus driver not found (0xE0000001) - install the ViGEmBus driver and retry."
            ),
            Self::ConnectFailed(code) => write!(
                f,
                "could not connect to the ViGEmBus driver (0x{code:08X}) - install the ViGEmBus driver and retry."
            ),
            Self::ClientAllocFailed => {
                write!(f, "could not allocate the ViGEmBus client (out of memory).")
            }
            Self::TargetAllocFailed => {
                write!(f, "could not allocate the Xbox 360 target (out of memory).")
            }
            Self::TargetAddFailed(code) => write!(
                f,
                "could not plug in the Xbox 360 controller (0x{code:08X}) - install the ViGEmBus driver and retry."
            ),
            Self::UpdateFailed(code) => write!(
                f,
                "the Xbox 360 controller refused an update (0x{code:08X})."
            ),
            Self::NotConnected => {
                write!(f, "the Xbox 360 controller is not connected.")
            }
        }
    }
}

impl std::error::Error for VigemError {}

impl From<VigemError> for VJoyError {
    fn from(error: VigemError) -> Self {
        match &error {
            VigemError::DllNotFound | VigemError::BusNotFound => {
                VJoyError::DriverNotFound(error.to_string())
            }
            VigemError::DllLoadFailed(msg) => VJoyError::LoadFailed(msg.clone()),
            VigemError::TargetAddFailed(code) => VJoyError::AcquireFailed(*code),
            VigemError::UpdateFailed(code) => VJoyError::UpdateFailed(*code),
            VigemError::NotConnected => VJoyError::NotConnected,
            VigemError::ConnectFailed(_)
            | VigemError::ClientAllocFailed
            | VigemError::TargetAllocFailed => VJoyError::LoadFailed(error.to_string()),
        }
    }
}

// ---------------------------------------------------------------------
// FFI seam
// ---------------------------------------------------------------------

type VigemAllocFn = unsafe extern "C" fn() -> *mut c_void;
type VigemFreeFn = unsafe extern "C" fn(*mut c_void);
type VigemConnectFn = unsafe extern "C" fn(*mut c_void) -> u32;
type VigemDisconnectFn = unsafe extern "C" fn(*mut c_void);
type VigemTargetX360AllocFn = unsafe extern "C" fn() -> *mut c_void;
type VigemTargetAddFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32;
type VigemTargetRemoveFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32;
type VigemTargetFreeFn = unsafe extern "C" fn(*mut c_void);
/// NOTE: the report is passed BY VALUE, not by pointer — the FFI
/// signature must match (`XUSB_REPORT`, 12 bytes, by value).
type VigemTargetX360UpdateFn = unsafe extern "C" fn(*mut c_void, *mut c_void, XusbReport) -> u32;

/// The ViGEmBus client calls RSH needs, behind a trait so the
/// whole backend (except the literal `Library::new`) runs on a
/// mock in tests with no driver installed.
pub trait VigemApi: Send {
    /// `vigem_alloc` — NULL on OOM only, NOT on a missing driver.
    fn alloc(&mut self) -> *mut c_void;
    /// `vigem_free`.
    fn free(&mut self, client: *mut c_void);
    /// `vigem_connect` — expensive, call once at connect.
    fn connect(&mut self, client: *mut c_void) -> u32;
    /// `vigem_disconnect` — destroys attached targets, does NOT
    /// free target objects.
    fn disconnect(&mut self, client: *mut c_void);
    /// `vigem_target_x360_alloc`.
    fn target_x360_alloc(&mut self) -> *mut c_void;
    /// `vigem_target_add` — blocks until fully operational.
    fn target_add(&mut self, client: *mut c_void, target: *mut c_void) -> u32;
    /// `vigem_target_remove` — unplug; the target object is
    /// reusable afterwards.
    fn target_remove(&mut self, client: *mut c_void, target: *mut c_void) -> u32;
    /// `vigem_target_free` — removing first is the caller's job.
    fn target_free(&mut self, target: *mut c_void);
    /// `vigem_target_x360_update` — report BY VALUE.
    fn target_x360_update(
        &mut self,
        client: *mut c_void,
        target: *mut c_void,
        report: XusbReport,
    ) -> u32;
}

/// The real API, loaded with `libloading`.
///
/// The function pointers are copied out of the `libloading` symbols
/// at load time, so the `Library` only needs to stay alive (it is
/// held here) — no symbol lookup happens per tick.
pub struct RealVigemApi {
    _library: Library,
    alloc: VigemAllocFn,
    free: VigemFreeFn,
    connect: VigemConnectFn,
    disconnect: VigemDisconnectFn,
    target_x360_alloc: VigemTargetX360AllocFn,
    target_add: VigemTargetAddFn,
    target_remove: VigemTargetRemoveFn,
    target_free: VigemTargetFreeFn,
    target_x360_update: VigemTargetX360UpdateFn,
}

impl RealVigemApi {
    /// Load `ViGEmClient.dll` from `path` and resolve every
    /// symbol RSH needs. `None` when the DLL cannot be loaded or
    /// any symbol is missing.
    fn load(path: &Path) -> Option<Self> {
        // SAFETY: `Library::new`/`get` are `unsafe` in libloading 0.7
        // because a wrong signature is UB at the call site — every
        // signature here is copied from ViGEmClient `Client.h`
        // (research §1.1), and the report-by-value update signature
        // is pinned by the `XusbReport` layout asserts above.
        let library = unsafe { Library::new(path) }.ok()?;
        let alloc: Symbol<'_, VigemAllocFn> = unsafe { library.get(b"vigem_alloc\0") }.ok()?;
        let free: Symbol<'_, VigemFreeFn> = unsafe { library.get(b"vigem_free\0") }.ok()?;
        let connect: Symbol<'_, VigemConnectFn> =
            unsafe { library.get(b"vigem_connect\0") }.ok()?;
        let disconnect: Symbol<'_, VigemDisconnectFn> =
            unsafe { library.get(b"vigem_disconnect\0") }.ok()?;
        let target_x360_alloc: Symbol<'_, VigemTargetX360AllocFn> =
            unsafe { library.get(b"vigem_target_x360_alloc\0") }.ok()?;
        let target_add: Symbol<'_, VigemTargetAddFn> =
            unsafe { library.get(b"vigem_target_add\0") }.ok()?;
        let target_remove: Symbol<'_, VigemTargetRemoveFn> =
            unsafe { library.get(b"vigem_target_remove\0") }.ok()?;
        let target_free: Symbol<'_, VigemTargetFreeFn> =
            unsafe { library.get(b"vigem_target_free\0") }.ok()?;
        let target_x360_update: Symbol<'_, VigemTargetX360UpdateFn> =
            unsafe { library.get(b"vigem_target_x360_update\0") }.ok()?;
        // Copy the function pointers out of the symbols FIRST —
        // the symbols borrow `library`, so it cannot move into the
        // struct while they are still alive.
        let alloc_fn: VigemAllocFn = *alloc;
        let free_fn: VigemFreeFn = *free;
        let connect_fn: VigemConnectFn = *connect;
        let disconnect_fn: VigemDisconnectFn = *disconnect;
        let target_x360_alloc_fn: VigemTargetX360AllocFn = *target_x360_alloc;
        let target_add_fn: VigemTargetAddFn = *target_add;
        let target_remove_fn: VigemTargetRemoveFn = *target_remove;
        let target_free_fn: VigemTargetFreeFn = *target_free;
        let target_x360_update_fn: VigemTargetX360UpdateFn = *target_x360_update;
        Some(Self {
            _library: library,
            alloc: alloc_fn,
            free: free_fn,
            connect: connect_fn,
            disconnect: disconnect_fn,
            target_x360_alloc: target_x360_alloc_fn,
            target_add: target_add_fn,
            target_remove: target_remove_fn,
            target_free: target_free_fn,
            target_x360_update: target_x360_update_fn,
        })
    }
}

impl VigemApi for RealVigemApi {
    fn alloc(&mut self) -> *mut c_void {
        // SAFETY: the pointer comes from the loaded `vigem_alloc`.
        unsafe { (self.alloc)() }
    }
    fn free(&mut self, client: *mut c_void) {
        // SAFETY: `client` was allocated by this API's `alloc`.
        unsafe { (self.free)(client) }
    }
    fn connect(&mut self, client: *mut c_void) -> u32 {
        // SAFETY: `client` was allocated by this API's `alloc`.
        unsafe { (self.connect)(client) }
    }
    fn disconnect(&mut self, client: *mut c_void) {
        // SAFETY: `client` was allocated by this API's `alloc`.
        unsafe { (self.disconnect)(client) }
    }
    fn target_x360_alloc(&mut self) -> *mut c_void {
        // SAFETY: the pointer comes from the loaded
        // `vigem_target_x360_alloc`.
        unsafe { (self.target_x360_alloc)() }
    }
    fn target_add(&mut self, client: *mut c_void, target: *mut c_void) -> u32 {
        // SAFETY: both handles come from this API.
        unsafe { (self.target_add)(client, target) }
    }
    fn target_remove(&mut self, client: *mut c_void, target: *mut c_void) -> u32 {
        // SAFETY: both handles come from this API.
        unsafe { (self.target_remove)(client, target) }
    }
    fn target_free(&mut self, target: *mut c_void) {
        // SAFETY: `target` was allocated by this API's
        // `target_x360_alloc` and removed first by the caller.
        unsafe { (self.target_free)(target) }
    }
    fn target_x360_update(
        &mut self,
        client: *mut c_void,
        target: *mut c_void,
        report: XusbReport,
    ) -> u32 {
        // SAFETY: both handles come from this API; the report is
        // passed BY VALUE exactly as `Client.h` declares it.
        unsafe { (self.target_x360_update)(client, target, report) }
    }
}

/// Resolve `ViGEmClient.dll` without ever bundling it into the
/// repo: the directory of the running exe (the installer ships
/// the DLL beside `rsh-daemon.exe`). Returns `None` when the
/// file is not there.
#[must_use]
pub fn resolve_dll_path() -> Option<PathBuf> {
    resolve_dll_path_from_exe(std::env::current_exe().ok().as_deref())
}

/// Pure half of [`resolve_dll_path`]: the DLL beside an explicit
/// exe path (testable on any OS — no process query).
#[must_use]
pub fn resolve_dll_path_from_exe(exe: Option<&Path>) -> Option<PathBuf> {
    let dir = exe?.parent()?;
    let candidate = dir.join(VIGEM_DLL_FILE_NAME);
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

// ---------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------

/// FFI state, behind the [`Mutex`] — the ViGEmBus client is not
/// thread-safe, so every call below locks first.
struct VigemInner {
    api: Option<Box<dyn VigemApi>>,
    client: VigemHandle,
    target: VigemHandle,
    connected: bool,
}

/// Xbox 360 output via ViGEmBus.
///
/// `new()` never fails: a missing DLL just means [`try_connect`](IJoystickOutput::try_connect)
/// fails later with [`VJoyError::DriverNotFound`] naming the fix —
/// the selector's factory signature is infallible, same as vJoy.
pub struct VigemOutput {
    inner: Mutex<VigemInner>,
}

impl VigemOutput {
    /// Build the backend, loading `ViGEmClient.dll` when it is
    /// beside the running exe. A missing DLL is NOT an error
    /// here — [`try_connect`](IJoystickOutput::try_connect)
    /// reports it.
    #[must_use]
    pub fn new() -> Self {
        let api = resolve_dll_path().and_then(|path| RealVigemApi::load(&path));
        Self::with_api(api.map(|real| Box::new(real) as Box<dyn VigemApi>))
    }

    fn with_api(api: Option<Box<dyn VigemApi>>) -> Self {
        Self {
            inner: Mutex::new(VigemInner {
                api,
                client: VigemHandle(null_mut()),
                target: VigemHandle(null_mut()),
                connected: false,
            }),
        }
    }

    fn lock_inner(&self) -> std::sync::MutexGuard<'_, VigemInner> {
        // A poisoned lock means a previous holder panicked mid-FFI
        // — recover the state instead of crashing the daemon.
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Teardown, best-effort, never panics. Called by
    /// [`disconnect`](IJoystickOutput::disconnect) and by `Drop`.
    fn teardown_locked(inner: &mut VigemInner) {
        if !inner.connected {
            return;
        }
        if let Some(api) = inner.api.as_mut() {
            // Order matters: remove the target first (else it is
            // orphaned until process exit), then disconnect the
            // client, then free both objects. Every result is
            // ignored on purpose — teardown must not fail.
            let _ = api.target_remove(inner.client.0, inner.target.0);
            api.target_free(inner.target.0);
            api.disconnect(inner.client.0);
            api.free(inner.client.0);
        }
        inner.client = VigemHandle(null_mut());
        inner.target = VigemHandle(null_mut());
        inner.connected = false;
    }
}

impl Drop for VigemOutput {
    fn drop(&mut self) {
        let mut inner = self.lock_inner();
        Self::teardown_locked(&mut inner);
    }
}

impl IJoystickOutput for VigemOutput {
    fn name(&self) -> &'static str {
        "Xbox"
    }

    fn is_connected(&self) -> bool {
        self.lock_inner().connected
    }

    fn try_connect(&mut self) -> Result<(), VJoyError> {
        let mut inner = self.lock_inner();
        if inner.connected {
            return Ok(());
        }
        if inner.api.is_none() {
            return Err(VigemError::DllNotFound.into());
        }
        // The borrow of `inner.api` must end before the
        // `inner.client`/`inner.target` writes below, so the whole
        // driver conversation happens in this scope and only plain
        // handles escape it. (No `unsafe` here: the calls go
        // through the safe `VigemApi` trait — the raw FFI lives
        // in `RealVigemApi`.)
        let outcome = {
            let api = inner.api.as_mut().expect("checked above");
            let client = VigemHandle(api.alloc());
            if client.0.is_null() {
                // `vigem_alloc` returns NULL on OOM only.
                Err(VigemError::ClientAllocFailed)
            } else {
                let rc = api.connect(client.0);
                if rc != VIGEM_ERROR_NONE {
                    api.free(client.0);
                    if rc == VIGEM_ERROR_BUS_NOT_FOUND {
                        Err(VigemError::BusNotFound)
                    } else {
                        Err(VigemError::ConnectFailed(rc))
                    }
                } else {
                    let target = VigemHandle(api.target_x360_alloc());
                    if target.0.is_null() {
                        api.disconnect(client.0);
                        api.free(client.0);
                        Err(VigemError::TargetAllocFailed)
                    } else {
                        // `vigem_target_add` BLOCKS until the
                        // target is fully operational.
                        let rc = api.target_add(client.0, target.0);
                        if rc != VIGEM_ERROR_NONE {
                            api.target_free(target.0);
                            api.disconnect(client.0);
                            api.free(client.0);
                            Err(VigemError::TargetAddFailed(rc))
                        } else {
                            Ok((client, target))
                        }
                    }
                }
            }
        };
        match outcome {
            Ok((client, target)) => {
                inner.client = client;
                inner.target = target;
                inner.connected = true;
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    fn disconnect(&mut self) {
        let mut inner = self.lock_inner();
        Self::teardown_locked(&mut inner);
    }

    fn update(&mut self, state: &ProcessedState) -> Result<(), VJoyError> {
        let report = build_report(state);
        let mut inner = self.lock_inner();
        if !inner.connected {
            return Err(VigemError::NotConnected.into());
        }
        let client = inner.client.0;
        let target = inner.target.0;
        let api = inner.api.as_mut().expect("connected implies an API");
        // `client`/`target` are the handles this API plugged in;
        // the report goes BY VALUE.
        let rc = api.target_x360_update(client, target, report);
        if rc != VIGEM_ERROR_NONE {
            return Err(VigemError::UpdateFailed(rc).into());
        }
        Ok(())
    }

    fn send_neutral(&mut self) -> Result<(), VJoyError> {
        // A no-op while disconnected; best-effort while connected
        // (a failed neutral send is swallowed, never fatal).
        if !self.is_connected() {
            return Ok(());
        }
        let _ = self.update(&ProcessedState::default());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- mock API ---------------------------------------------------

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum MockCall {
        Alloc,
        Free,
        Connect,
        Disconnect,
        TargetAlloc,
        TargetAdd,
        TargetRemove,
        TargetFree,
        Update(XusbReport),
    }

    struct MockVigemApi {
        calls: Vec<MockCall>,
        connect_rc: u32,
        target_add_rc: u32,
        update_rc: u32,
        alloc_null: bool,
        target_alloc_null: bool,
    }

    impl MockVigemApi {
        fn new() -> Self {
            Self {
                calls: Vec::new(),
                connect_rc: VIGEM_ERROR_NONE,
                target_add_rc: VIGEM_ERROR_NONE,
                update_rc: VIGEM_ERROR_NONE,
                alloc_null: false,
                target_alloc_null: false,
            }
        }
    }

    impl VigemApi for MockVigemApi {
        fn alloc(&mut self) -> *mut c_void {
            self.calls.push(MockCall::Alloc);
            if self.alloc_null {
                null_mut()
            } else {
                0x1000 as *mut c_void
            }
        }
        fn free(&mut self, _client: *mut c_void) {
            self.calls.push(MockCall::Free);
        }
        fn connect(&mut self, _client: *mut c_void) -> u32 {
            self.calls.push(MockCall::Connect);
            self.connect_rc
        }
        fn disconnect(&mut self, _client: *mut c_void) {
            self.calls.push(MockCall::Disconnect);
        }
        fn target_x360_alloc(&mut self) -> *mut c_void {
            self.calls.push(MockCall::TargetAlloc);
            if self.target_alloc_null {
                null_mut()
            } else {
                0x2000 as *mut c_void
            }
        }
        fn target_add(&mut self, _client: *mut c_void, _target: *mut c_void) -> u32 {
            self.calls.push(MockCall::TargetAdd);
            self.target_add_rc
        }
        fn target_remove(&mut self, _client: *mut c_void, _target: *mut c_void) -> u32 {
            self.calls.push(MockCall::TargetRemove);
            VIGEM_ERROR_NONE
        }
        fn target_free(&mut self, _target: *mut c_void) {
            self.calls.push(MockCall::TargetFree);
        }
        fn target_x360_update(
            &mut self,
            _client: *mut c_void,
            _target: *mut c_void,
            report: XusbReport,
        ) -> u32 {
            self.calls.push(MockCall::Update(report));
            self.update_rc
        }
    }

    fn output_with(mock: MockVigemApi) -> (VigemOutput, std::sync::Arc<Mutex<MockVigemApi>>) {
        // The mock must be shared to inspect calls after the
        // backend owns it — wrap it in Arc<Mutex<>> via a shim.
        struct SharedApi(std::sync::Arc<Mutex<MockVigemApi>>);
        impl VigemApi for SharedApi {
            fn alloc(&mut self) -> *mut c_void {
                self.0.lock().unwrap().alloc()
            }
            fn free(&mut self, client: *mut c_void) {
                self.0.lock().unwrap().free(client);
            }
            fn connect(&mut self, client: *mut c_void) -> u32 {
                self.0.lock().unwrap().connect(client)
            }
            fn disconnect(&mut self, client: *mut c_void) {
                self.0.lock().unwrap().disconnect(client);
            }
            fn target_x360_alloc(&mut self) -> *mut c_void {
                self.0.lock().unwrap().target_x360_alloc()
            }
            fn target_add(&mut self, client: *mut c_void, target: *mut c_void) -> u32 {
                self.0.lock().unwrap().target_add(client, target)
            }
            fn target_remove(&mut self, client: *mut c_void, target: *mut c_void) -> u32 {
                self.0.lock().unwrap().target_remove(client, target)
            }
            fn target_free(&mut self, target: *mut c_void) {
                self.0.lock().unwrap().target_free(target);
            }
            fn target_x360_update(
                &mut self,
                client: *mut c_void,
                target: *mut c_void,
                report: XusbReport,
            ) -> u32 {
                self.0
                    .lock()
                    .unwrap()
                    .target_x360_update(client, target, report)
            }
        }
        let shared = std::sync::Arc::new(Mutex::new(mock));
        let out = VigemOutput::with_api(Some(Box::new(SharedApi(shared.clone()))));
        (out, shared)
    }

    fn full_state() -> ProcessedState {
        ProcessedState {
            steering: 1.0,
            accelerator: 1.0,
            brake: 0.5,
            clutch: 1.0,
            handbrake: 1.0,
            pedals_combined: 0.0,
            buttons: 0b01, // shifter up
            horn: true,
        }
    }

    // ---- report layout ----------------------------------------------

    #[test]
    fn xusb_report_is_12_bytes_no_padding() {
        assert_eq!(size_of::<XusbReport>(), 12);
        assert_eq!(offset_of!(XusbReport, w_buttons), 0);
        assert_eq!(offset_of!(XusbReport, b_left_trigger), 2);
        assert_eq!(offset_of!(XusbReport, b_right_trigger), 3);
        assert_eq!(offset_of!(XusbReport, s_thumb_lx), 4);
        assert_eq!(offset_of!(XusbReport, s_thumb_ly), 6);
        assert_eq!(offset_of!(XusbReport, s_thumb_rx), 8);
        assert_eq!(offset_of!(XusbReport, s_thumb_ry), 10);
    }

    // ---- mapping ------------------------------------------------------

    #[test]
    fn build_report_maps_the_seven_inputs() {
        let report = build_report(&full_state());
        assert_eq!(report.s_thumb_lx, 32767); // steering full right
        assert_eq!(report.b_right_trigger, 255); // accel full
        assert_eq!(report.b_left_trigger, 127); // brake half (0.5*255)
        assert_eq!(report.s_thumb_ry, -32768); // clutch fully pressed
        assert_eq!(report.s_thumb_ly, 0);
        assert_eq!(report.s_thumb_rx, 0);
        // handbrake -> A, horn -> X, shifter up -> D-pad Up.
        assert_eq!(
            report.w_buttons,
            XUSB_BUTTON_A | XUSB_BUTTON_X | XUSB_DPAD_UP
        );
    }

    #[test]
    fn build_report_shifter_down_is_dpad_down() {
        let mut state = ProcessedState::default();
        state.buttons = 0b10;
        assert_eq!(build_report(&state).w_buttons, XUSB_DPAD_DOWN);
    }

    #[test]
    fn build_report_clutch_is_analog() {
        let mut state = ProcessedState::default();
        state.clutch = 0.5;
        assert_eq!(build_report(&state).s_thumb_ry, -16384);
        state.clutch = 0.0;
        assert_eq!(build_report(&state).s_thumb_ry, 0);
    }

    #[test]
    fn build_report_steering_left_is_negative() {
        let mut state = ProcessedState::default();
        state.steering = -1.0;
        assert_eq!(build_report(&state).s_thumb_lx, -32767);
        state.steering = 0.0;
        assert_eq!(build_report(&state).s_thumb_lx, 0);
    }

    #[test]
    fn build_report_clamps_out_of_range_inputs() {
        let mut state = ProcessedState::default();
        state.steering = 2.0;
        state.accelerator = 2.0;
        state.brake = -1.0;
        state.clutch = -1.0;
        let report = build_report(&state);
        assert_eq!(report.s_thumb_lx, 32767);
        assert_eq!(report.b_right_trigger, 255);
        assert_eq!(report.b_left_trigger, 0);
        assert_eq!(report.s_thumb_ry, 0);
    }

    #[test]
    fn build_report_neutral_is_all_zero() {
        assert_eq!(
            build_report(&ProcessedState::default()),
            XusbReport::default()
        );
    }

    #[test]
    fn build_report_handbrake_needs_a_real_pull() {
        let mut state = ProcessedState::default();
        state.handbrake = 0.0;
        assert_eq!(build_report(&state).w_buttons & XUSB_BUTTON_A, 0);
        state.handbrake = 0.01;
        assert_ne!(build_report(&state).w_buttons & XUSB_BUTTON_A, 0);
    }

    // ---- lifecycle (mock) ---------------------------------------------

    #[test]
    fn connect_plugs_in_and_update_sends_the_report() {
        let (mut out, shared) = output_with(MockVigemApi::new());
        assert!(!out.is_connected());
        out.try_connect().expect("mock connect succeeds");
        assert!(out.is_connected());
        out.update(&full_state()).expect("mock update succeeds");
        let calls = shared.lock().unwrap().calls.clone();
        assert!(calls.contains(&MockCall::Alloc));
        assert!(calls.contains(&MockCall::Connect));
        assert!(calls.contains(&MockCall::TargetAlloc));
        assert!(calls.contains(&MockCall::TargetAdd));
        assert!(calls.contains(&MockCall::Update(build_report(&full_state()))));
        out.disconnect();
        assert!(!out.is_connected());
        let calls = shared.lock().unwrap().calls.clone();
        assert!(calls.contains(&MockCall::TargetRemove));
        assert!(calls.contains(&MockCall::TargetFree));
        assert!(calls.contains(&MockCall::Disconnect));
        assert!(calls.contains(&MockCall::Free));
    }

    #[test]
    fn connect_without_driver_reports_bus_not_found() {
        let mut mock = MockVigemApi::new();
        mock.connect_rc = VIGEM_ERROR_BUS_NOT_FOUND;
        let (mut out, shared) = output_with(mock);
        let error = out.try_connect().expect_err("bus missing must fail");
        assert!(matches!(error, VJoyError::DriverNotFound(_)));
        assert!(error.to_string().contains("ViGEmBus driver not found"));
        assert!(!out.is_connected());
        // Failed connect frees the client it allocated.
        assert!(shared.lock().unwrap().calls.contains(&MockCall::Free));
    }

    #[test]
    fn connect_target_add_failure_cleans_up() {
        let mut mock = MockVigemApi::new();
        mock.target_add_rc = VIGEM_ERROR_NO_FREE_SLOT;
        let (mut out, shared) = output_with(mock);
        let error = out.try_connect().expect_err("add failure must fail");
        assert!(matches!(error, VJoyError::AcquireFailed(_)));
        assert!(!out.is_connected());
        let calls = shared.lock().unwrap().calls.clone();
        assert!(calls.contains(&MockCall::TargetFree));
        assert!(calls.contains(&MockCall::Disconnect));
        assert!(calls.contains(&MockCall::Free));
    }

    #[test]
    fn connect_without_dll_reports_driver_not_found() {
        let mut out = VigemOutput::with_api(None);
        let error = out.try_connect().expect_err("no DLL must fail");
        assert!(matches!(error, VJoyError::DriverNotFound(_)));
        assert!(error.to_string().contains("ViGEmClient.dll"));
    }

    #[test]
    fn update_while_disconnected_is_not_connected() {
        let (mut out, _) = output_with(MockVigemApi::new());
        assert!(matches!(
            out.update(&ProcessedState::default()),
            Err(VJoyError::NotConnected)
        ));
    }

    #[test]
    fn send_neutral_while_disconnected_is_a_noop() {
        let (mut out, shared) = output_with(MockVigemApi::new());
        out.send_neutral()
            .expect("neutral while disconnected is Ok");
        assert!(
            !shared
                .lock()
                .unwrap()
                .calls
                .contains(&MockCall::Update(XusbReport::default()))
        );
    }

    #[test]
    fn send_neutral_sends_zeros_while_connected() {
        let (mut out, shared) = output_with(MockVigemApi::new());
        out.try_connect().expect("mock connect succeeds");
        out.send_neutral().expect("neutral send succeeds");
        assert!(
            shared
                .lock()
                .unwrap()
                .calls
                .contains(&MockCall::Update(XusbReport::default()))
        );
    }

    #[test]
    fn update_failure_surfaces_the_code() {
        let mut mock = MockVigemApi::new();
        mock.update_rc = VIGEM_ERROR_TARGET_NOT_PLUGGED_IN;
        let (mut out, _) = output_with(mock);
        out.try_connect().expect("mock connect succeeds");
        assert!(matches!(
            out.update(&ProcessedState::default()),
            Err(VJoyError::UpdateFailed(VIGEM_ERROR_TARGET_NOT_PLUGGED_IN))
        ));
    }

    #[test]
    fn vigem_error_messages_name_the_fix() {
        assert!(
            VigemError::BusNotFound
                .to_string()
                .contains("install the ViGEmBus driver")
        );
        assert!(
            VigemError::DllNotFound
                .to_string()
                .contains("ViGEmClient.dll")
        );
        assert_eq!(
            VJoyError::from(VigemError::BusNotFound).to_string(),
            VigemError::BusNotFound.to_string()
        );
    }

    #[test]
    fn resolve_dll_path_from_exe_finds_the_dll_beside_the_exe() {
        let base =
            std::env::temp_dir().join(format!("rsh-vigem-test-{}-resolve", std::process::id()));
        std::fs::create_dir_all(&base).expect("create fake install dir");
        let dll = base.join(VIGEM_DLL_FILE_NAME);
        std::fs::write(&dll, b"fake").expect("write fake ViGEmClient.dll");
        let exe = base.join("rsh-daemon.exe");
        assert_eq!(resolve_dll_path_from_exe(Some(&exe)), Some(dll));
        assert_eq!(
            resolve_dll_path_from_exe(Some(&base.join("other.exe"))),
            Some(base.join(VIGEM_DLL_FILE_NAME))
        );
        assert_eq!(resolve_dll_path_from_exe(None), None);
        let empty = base.join("empty");
        std::fs::create_dir_all(&empty).expect("create empty dir");
        assert_eq!(
            resolve_dll_path_from_exe(Some(&empty.join("rsh-daemon.exe"))),
            None
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
