//! Windows background resilience for the RSH daemon (Scope C).
//!
//! The user asked that Windows not close, kill, or throttle the RSH
//! software while a game is running. This module applies the
//! evidence-based, documented mitigations for that, and nothing
//! speculative: no registry hacks, no changes to game processes, no
//! touching the Game Mode setting.
//!
//! # What Windows actually does to a background app while a game runs
//!
//! Researched 2026-11-18; the claims below are what the mitigations
//! in this module are built on.
//!
//! - **Game Mode** (Settings > Gaming > Game Mode, on by default):
//!   when a game is in the foreground with focus, Windows gives the
//!   game priority/exclusive access to hardware resources (CPU core
//!   exclusivity, increased GPU prioritization) and lowers the
//!   priority of background work; it also prevents Windows Update
//!   from doing driver installs and restart notifications while a
//!   game runs. It does **not** terminate background processes, and
//!   there is no per-process opt-out API for a background app — the
//!   defence is to make our own process look like work the system
//!   should not throttle (see below), not to fight Game Mode.
//!   Sources: Microsoft Support, "Use Game Mode while gaming on your
//!   Windows device"; Microsoft Learn, "Game Mode" (archived
//!   developer documentation — its Game Mode APIs are deprecated
//!   since Windows 10 1809, but the behaviour description stands).
//! - **Power throttling / `EcoQoS` / efficiency mode:** a background
//!   process can be classified `EcoQoS`, which drops its base priority
//!   and runs its threads at reduced clocks / on efficiency cores.
//!   A process opts **out** by calling [`SetProcessInformation`
//!   with `ProcessPowerThrottling`](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-setprocessinformation)
//!   and a [`PROCESS_POWER_THROTTLING_STATE`](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/ns-processthreadsapi-process_power_throttling_state)
//!   whose `ControlMask` selects `EXECUTION_SPEED` and whose
//!   `StateMask` clears it (0 = `HighQoS`, do not throttle). Windows
//!   10 1709+. This is the single most important mitigation here:
//!   a headless daemon with no window is exactly the kind of
//!   process the heuristics classify as background.
//! - **Execution state / power requests** (preventing idle sleep):
//!   [`SetThreadExecutionState`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setthreadexecutionstate?redirectedfrom=MSDN)
//!   (`ES_CONTINUOUS | ES_SYSTEM_REQUIRED`) is per-thread and must
//!   be set/cleared from the same thread, which is awkward for a
//!   multi-threaded Tokio daemon. The newer power-request API —
//!   `PowerCreateRequest` / `PowerSetRequest` / `PowerClearRequest`
//!   (see [`Power Management Functions`](https://learn.microsoft.com/en-us/windows/win32/power/power-management-functions))
//!   — is handle-based, works across threads, and takes a reason
//!   string that shows up in `powercfg /requests`, which is why
//!   this module uses it. Two request types are held:
//!   `PowerRequestSystemRequired` (no automatic idle sleep) and
//!   `PowerRequestExecutionRequired` (the process keeps running
//!   instead of being suspended by process lifetime management on a
//!   Modern Standby machine). Limits, from the docs: neither can
//!   stop a **user-initiated** sleep (power button, lid close,
//!   Start > Sleep) — "The `SetThreadExecutionState` function cannot
//!   be used to prevent the user from putting the computer to
//!   sleep"; on Modern Standby machines on battery, system/execution
//!   requests are terminated 5 minutes after the sleep timeout would
//!   otherwise have expired. They prevent sleep, not process kill.
//!   This module deliberately does **not** take
//!   `ES_DISPLAY_REQUIRED` / `PowerRequestDisplayRequired`: the
//!   daemon must never keep the user's display awake. It also does
//!   **not** use `ES_AWAYMODE_REQUIRED`: away mode is for
//!   media-recording/distribution apps on traditional S3 sleep only.
//! - **Memory priority / working-set trimming:** a process's memory
//!   priority decides which of its pages the memory manager trims
//!   first under pressure; trimming pages a process back out to the
//!   standby list, it does not kill the process. This module
//!   (re-)asserts `MEMORY_PRIORITY_NORMAL` via
//!   `SetProcessInformation` with `ProcessMemoryPriority`
//!   ([`PROCESS_INFORMATION_CLASS`](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/ne-processthreadsapi-process_information_class))
//!   so a stray efficiency-mode classification cannot leave the
//!   daemon's pages first in line to be trimmed. A minimum working
//!   set via `SetProcessWorkingSetSizeEx` is deliberately **not**
//!   set: it needs `SeIncreaseWorkingSetPrivilege`, pins RAM the
//!   game could use, and does not protect against anything that can
//!   actually happen to this daemon.
//! - **What Windows cannot be stopped from doing:** desktop Windows
//!   has no Linux-style OOM killer for Win32 processes — process
//!   lifetime management (suspend/terminate of background apps) is a
//!   UWP/Store-app mechanism (see Microsoft Learn, ["Optimize
//!   suspend/resume - UWP applications"](https://learn.microsoft.com/en-us/windows/uwp/debug-test-perf/optimize-suspend-resume))
//!   and does not apply to a Win32 desktop daemon. Nothing in this
//!   module can stop a user or admin killing the process, a system
//!   shutdown/restart, or an explicit user sleep — and it does not
//!   try. If the daemon process dies for any of those reasons, the
//!   recovery path is restart, not prevention (see below).
//!
//! # What this module does (Windows only)
//!
//! [`BackgroundGuard::acquire`] — call once at daemon start —
//! applies, best-effort, in order:
//!
//! 1. Power-throttling opt-out (`HighQoS`, see above).
//! 2. Memory priority re-asserted to `MEMORY_PRIORITY_NORMAL`.
//! 3. Priority class restored to `NORMAL_PRIORITY_CLASS` (undoes a
//!    stale efficiency-mode `IDLE_PRIORITY_CLASS` left by the user
//!    or by Task Manager; the `EcoQoS` opt-out alone does not restore
//!    the base priority).
//! 4. A power request (`SystemRequired` + `ExecutionRequired`) with
//!    the reason string `"RSH wheel daemon — keeping wheel link alive"`,
//!    held for the daemon's whole lifetime and released on drop.
//!
//! [`BackgroundGuard::escalate`] — call while a game is running or
//! the wheel is connected — raises the priority class to
//! `ABOVE_NORMAL_PRIORITY_CLASS` (one step above normal: enough to
//! keep the input pipeline scheduled ahead of other background work
//! without starving the game, which `HIGH_PRIORITY_CLASS` or
//! `REALTIME_PRIORITY_CLASS` would risk) and re-applies steps 1–2
//! in case the user toggled efficiency mode mid-session.
//! [`BackgroundGuard::relax`] restores `NORMAL_PRIORITY_CLASS`
//! when the game/wheel session ends. Both are idempotent.
//!
//! Every step is best-effort and reported, never fatal: a failed
//! mitigation is logged by the caller from the returned report and
//! the daemon keeps running without it.
//!
//! # Keep-alive / respawn — what actually covers a dead daemon
//!
//! A watchdog thread *inside* the daemon cannot restart the daemon
//! if the OS kills the process — the watchdog dies with it. So this
//! module deliberately contains no in-process watchdog. The real
//! coverage is outside the process and already exists: the UI
//! respawns the daemon when it finds IPC dead (F8/v2.0.3 detached
//! spawn + UI-side respawn), and autostart re-launches it at login.
//! What this module adds is making the kill/throttle *less likely*
//! in the first place; it does not pretend to survive one.
//!
//! # Platform behaviour
//!
//! On non-Windows platforms every function is a no-op stub with the
//! same signature ([`BackgroundGuard::supported`] returns `false`),
//! so the module compiles on Linux and the daemon's cross-platform
//! gate stays green.

/// One best-effort mitigation step and whether it took effect.
///
/// `name` is a stable short identifier for logs; `detail` is a
/// human-readable note (success note, or the OS error on failure).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mitigation {
    /// Stable short identifier, e.g. `"power-throttling opt-out"`.
    pub name: &'static str,
    /// Whether the mitigation is in effect.
    pub applied: bool,
    /// Human-readable note: success note, or the OS error.
    pub detail: String,
}

impl Mitigation {
    /// A mitigation that took effect.
    ///
    /// Both constructors are only called from the `#[cfg(windows)]`
    /// mitigation functions and from tests, so a Linux check sees
    /// them as dead — the allow keeps the `-D warnings` gate green;
    /// on Windows they are genuinely used.
    #[allow(dead_code)]
    fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            applied: true,
            detail: detail.into(),
        }
    }

    /// A mitigation that did not take effect, with the reason.
    /// Same Linux-dead-code note as [`Mitigation::ok`].
    #[allow(dead_code)]
    fn failed(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            applied: false,
            detail: detail.into(),
        }
    }
}

/// One line per mitigation, e.g.
/// `power-throttling opt-out (HighQoS): applied — EcoQoS disabled`.
/// Empty report renders as `"(no background mitigations)"`.
#[must_use]
pub fn summarize(report: &[Mitigation]) -> String {
    if report.is_empty() {
        return "(no background mitigations)".to_owned();
    }
    report
        .iter()
        .map(|m| {
            format!(
                "{}: {} — {}",
                m.name,
                if m.applied { "applied" } else { "FAILED" },
                m.detail
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Guard holding the daemon's background-resilience state.
///
/// Acquire once at daemon start with [`BackgroundGuard::acquire`]
/// and keep the guard alive for the daemon's whole lifetime: on
/// Windows it owns the power-request handle, which is cleared and
/// closed when the guard drops. Escalate while a game is running or
/// the wheel is connected ([`BackgroundGuard::escalate`]), relax
/// afterwards ([`BackgroundGuard::relax`]).
/// A power-request handle that is safe to share across threads.
///
/// `windows::Win32::Foundation::HANDLE` is `!Send` only because it
/// wraps a raw pointer. A power-request handle is a process-wide
/// kernel-object reference: `PowerSetRequest`, `PowerClearRequest`
/// and `CloseHandle` on it are thread-safe, and the guard only ever
/// calls those three, always under a mutex. Sharing it is therefore
/// sound.
#[cfg(windows)]
struct SharedHandle(windows::Win32::Foundation::HANDLE);

// SAFETY: see the doc comment on `SharedHandle` — the wrapped handle
// is only ever used for thread-safe Win32 calls, always under mutex
// protection.
#[cfg(windows)]
unsafe impl Send for SharedHandle {}
#[cfg(windows)]
unsafe impl Sync for SharedHandle {}

// BackgroundGuard is shared with the manager-loop task via
// `Arc<Mutex<..>>`, so it must be `Send`. The only `!Send` field
// would be the raw `HANDLE` — wrapped in `SharedHandle` above.
// This fails Windows CI (not Linux, where the stub is a unit
// struct) if a future field breaks `Send`.
#[cfg(windows)]
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<BackgroundGuard>();
};

#[cfg(windows)]
pub struct BackgroundGuard {
    /// Power-request handle from `PowerCreateRequest`, if held.
    power_request: Option<SharedHandle>,
    /// Whether [`BackgroundGuard::escalate`] is currently in effect.
    escalated: bool,
}

/// Non-Windows stub: no state, every method is a no-op.
#[cfg(not(windows))]
pub struct BackgroundGuard;

impl BackgroundGuard {
    /// Whether this platform supports any background mitigation
    /// (`true` on Windows, `false` elsewhere). Read by the status /
    /// diagnostics reporting wired at consolidation; until then it
    /// is intentionally unused — the allow keeps the `-D warnings`
    /// gate green and is removed once it is read.
    #[allow(dead_code)]
    #[must_use]
    pub const fn supported() -> bool {
        cfg!(windows)
    }

    /// Whether a power request is currently held (`false` off
    /// Windows, and on Windows until [`BackgroundGuard::acquire`]
    /// succeeds in creating one). Same wiring note as
    /// [`BackgroundGuard::supported`].
    #[allow(dead_code)]
    #[must_use]
    #[cfg(windows)]
    pub fn power_request_active(&self) -> bool {
        self.power_request.is_some()
    }

    /// Whether a power request is currently held; always `false`
    /// off Windows. Same wiring note as
    /// [`BackgroundGuard::supported`]. The `&self` is kept so the
    /// signature matches the Windows version (which reads the
    /// handle); the stub has no state to read.
    #[allow(dead_code)]
    #[allow(clippy::unused_self)]
    #[must_use]
    #[cfg(not(windows))]
    pub const fn power_request_active(&self) -> bool {
        false
    }
}

/// Reason string shown for the daemon in `powercfg /requests`.
#[cfg(windows)]
const POWER_REQUEST_REASON: &str = "RSH wheel daemon — keeping wheel link alive";

#[cfg(windows)]
impl BackgroundGuard {
    /// Apply the baseline mitigations at daemon start and return the
    /// guard plus a per-step report. Best-effort: a failed step is
    /// reported, never fatal, and the remaining steps still run.
    #[must_use]
    pub fn acquire() -> (Self, Vec<Mitigation>) {
        let mut report = vec![
            opt_out_of_power_throttling(),
            set_memory_priority_normal(),
            restore_priority_class_normal(),
        ];
        let (power_request, power_mitigation) = acquire_power_request();
        report.push(power_mitigation);
        (
            Self {
                power_request,
                escalated: false,
            },
            report,
        )
    }

    /// Escalate while a game is running or the wheel is connected:
    /// raise the priority class to `ABOVE_NORMAL_PRIORITY_CLASS`
    /// and re-apply the power-throttling opt-out and memory
    /// priority (in case efficiency mode was toggled mid-session).
    /// Idempotent: a second call while already escalated reports
    /// nothing to do. Returns a per-step report.
    ///
    /// Wired to the wheel-Active transition in `manager_loop`
    /// (`main.rs`): escalate when the session becomes Active,
    /// relax when it leaves Active.
    #[must_use]
    pub fn escalate(&mut self) -> Vec<Mitigation> {
        if self.escalated {
            return Vec::new();
        }
        let mut report = vec![opt_out_of_power_throttling(), set_memory_priority_normal()];
        report.push(set_priority_class(
            "priority class raised (above normal)",
            windows::Win32::System::Threading::ABOVE_NORMAL_PRIORITY_CLASS,
        ));
        if report.iter().all(|m| m.applied) {
            self.escalated = true;
        }
        report
    }

    /// Undo [`BackgroundGuard::escalate`]: restore
    /// `NORMAL_PRIORITY_CLASS`. Idempotent: a call while not
    /// escalated reports nothing to do. Returns a per-step report.
    /// Same wiring note as [`BackgroundGuard::escalate`].
    #[must_use]
    pub fn relax(&mut self) -> Vec<Mitigation> {
        if !self.escalated {
            return Vec::new();
        }
        let report = vec![restore_priority_class_normal()];
        if report.iter().all(|m| m.applied) {
            self.escalated = false;
        }
        report
    }
}

#[cfg(not(windows))]
impl BackgroundGuard {
    /// No-op stub: no mitigations exist off Windows.
    #[must_use]
    pub fn acquire() -> (Self, Vec<Mitigation>) {
        (Self, Vec::new())
    }

    /// No-op stub off Windows. Same wiring note as the Windows
    /// [`BackgroundGuard::escalate`].
    /// The `&mut self` is kept so the signature matches the
    /// Windows version; the stub has no state to change.
    #[allow(clippy::unused_self)]
    #[must_use]
    pub fn escalate(&mut self) -> Vec<Mitigation> {
        Vec::new()
    }

    /// No-op stub off Windows. Same wiring note as the Windows
    /// [`BackgroundGuard::escalate`].
    /// The `&mut self` is kept so the signature matches the
    /// Windows version; the stub has no state to change.
    #[allow(clippy::unused_self)]
    #[must_use]
    pub fn relax(&mut self) -> Vec<Mitigation> {
        Vec::new()
    }
}

/// Release the power request on drop: clear both request types,
/// then close the handle. Best-effort — drop cannot report.
#[cfg(windows)]
impl Drop for BackgroundGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.power_request.take() {
            use windows::Win32::System::Power::{
                PowerClearRequest, PowerRequestExecutionRequired, PowerRequestSystemRequired,
            };
            unsafe {
                let _ = PowerClearRequest(handle.0, PowerRequestSystemRequired);
                let _ = PowerClearRequest(handle.0, PowerRequestExecutionRequired);
                let _ = windows::Win32::Foundation::CloseHandle(handle.0);
            }
        }
    }
}

#[cfg(windows)]
fn opt_out_of_power_throttling() -> Mitigation {
    use windows::Win32::System::Threading::{
        GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
        ProcessPowerThrottling, SetProcessInformation,
    };
    // ControlMask selects EXECUTION_SPEED, StateMask 0 clears it:
    // HighQoS — do not throttle execution speed (EcoQoS off).
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: 0,
    };
    let size = u32::try_from(size_of::<PROCESS_POWER_THROTTLING_STATE>()).unwrap_or(u32::MAX);
    let result = unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            std::ptr::addr_of!(state).cast::<std::ffi::c_void>(),
            size,
        )
    };
    match result {
        Ok(()) => Mitigation::ok(
            "power-throttling opt-out",
            "HighQoS — EcoQoS execution-speed throttling disabled",
        ),
        Err(error) => Mitigation::failed(
            "power-throttling opt-out",
            format!("SetProcessInformation failed: {error}"),
        ),
    }
}

#[cfg(windows)]
fn set_memory_priority_normal() -> Mitigation {
    use windows::Win32::System::Threading::{
        GetCurrentProcess, MEMORY_PRIORITY_INFORMATION, MEMORY_PRIORITY_NORMAL,
        ProcessMemoryPriority, SetProcessInformation,
    };
    let info = MEMORY_PRIORITY_INFORMATION {
        MemoryPriority: MEMORY_PRIORITY_NORMAL,
    };
    let size = u32::try_from(size_of::<MEMORY_PRIORITY_INFORMATION>()).unwrap_or(u32::MAX);
    let result = unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessMemoryPriority,
            std::ptr::addr_of!(info).cast::<std::ffi::c_void>(),
            size,
        )
    };
    match result {
        Ok(()) => Mitigation::ok(
            "memory priority",
            "MEMORY_PRIORITY_NORMAL — daemon pages are not first in line to be trimmed",
        ),
        Err(error) => Mitigation::failed(
            "memory priority",
            format!("SetProcessInformation failed: {error}"),
        ),
    }
}

#[cfg(windows)]
fn set_priority_class(
    name: &'static str,
    class: windows::Win32::System::Threading::PROCESS_CREATION_FLAGS,
) -> Mitigation {
    use windows::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass};
    match unsafe { SetPriorityClass(GetCurrentProcess(), class) } {
        Ok(()) => Mitigation::ok(name, "SetPriorityClass succeeded"),
        Err(error) => Mitigation::failed(name, format!("SetPriorityClass failed: {error}")),
    }
}

#[cfg(windows)]
fn restore_priority_class_normal() -> Mitigation {
    use windows::Win32::System::Threading::NORMAL_PRIORITY_CLASS;
    set_priority_class("priority class", NORMAL_PRIORITY_CLASS)
}

/// Create the power request and set both request types. On any
/// failure the handle is cleared and closed and the mitigation is
/// reported failed. The wide reason string is built fresh for the
/// `PowerCreateRequest` call and dropped with this function: the
/// system copies it into the power-request object at creation
/// (Chromium's `power_save_blocker_win.cc` and the `insomnia`
/// crate both drop the buffer right after the call and keep only
/// the handle), so the guard does not own it.
#[cfg(windows)]
fn acquire_power_request() -> (Option<SharedHandle>, Mitigation) {
    use windows::Win32::System::Power::{
        PowerCreateRequest, PowerRequestExecutionRequired, PowerRequestSystemRequired,
        PowerSetRequest,
    };
    use windows::Win32::System::Threading::{
        POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
    };
    use windows::core::PWSTR;

    let mut reason_wide: Vec<u16> = POWER_REQUEST_REASON
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // `REASON_CONTEXT.Version` is `POWER_REQUEST_CONTEXT_VERSION`
    // (= 0); `Flags` selects the simple-string reason form.
    // Initialising a union field in a struct literal is safe —
    // only *reading* a union field is unsafe — so no `unsafe`
    // block is needed here (clippy `unused_unsafe`).
    let context = REASON_CONTEXT {
        Version: 0,
        Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
        Reason: REASON_CONTEXT_0 {
            SimpleReasonString: PWSTR(reason_wide.as_mut_ptr()),
        },
    };
    let handle = match unsafe { PowerCreateRequest(std::ptr::addr_of!(context)) } {
        Ok(handle) => handle,
        Err(error) => {
            return (
                None,
                Mitigation::failed(
                    "power request",
                    format!("PowerCreateRequest failed: {error}"),
                ),
            );
        }
    };
    // Both types must hold; a half-set request is torn down.
    let set = unsafe {
        PowerSetRequest(handle, PowerRequestSystemRequired)
            .and_then(|()| PowerSetRequest(handle, PowerRequestExecutionRequired))
    };
    match set {
        Ok(()) => (
            Some(SharedHandle(handle)),
            Mitigation::ok(
                "power request",
                "SystemRequired + ExecutionRequired — visible in powercfg /requests; prevents automatic idle sleep and Modern Standby suspension, not user-initiated sleep",
            ),
        ),
        Err(error) => {
            unsafe {
                use windows::Win32::System::Power::PowerClearRequest;
                let _ = PowerClearRequest(handle, PowerRequestSystemRequired);
                let _ = PowerClearRequest(handle, PowerRequestExecutionRequired);
                let _ = windows::Win32::Foundation::CloseHandle(handle);
            }
            (
                None,
                Mitigation::failed("power request", format!("PowerSetRequest failed: {error}")),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_renders_applied_and_failed() {
        let report = vec![
            Mitigation::ok("power-throttling opt-out", "HighQoS"),
            Mitigation::failed("power request", "boom"),
        ];
        let text = summarize(&report);
        assert!(text.contains("power-throttling opt-out: applied — HighQoS"));
        assert!(text.contains("power request: FAILED — boom"));
        assert_eq!(summarize(&[]), "(no background mitigations)");
    }

    #[test]
    fn mitigation_constructors_agree_with_fields() {
        assert!(Mitigation::ok("x", "y").applied);
        assert!(!Mitigation::failed("x", "y").applied);
    }

    // Off Windows the whole module is a no-op stub: acquire returns
    // an empty report, escalate/relax do nothing, and nothing is
    // supported. These are the only tests that can run on Linux.
    #[cfg(not(windows))]
    #[test]
    fn acquire_is_a_noop_off_windows() {
        assert!(!BackgroundGuard::supported());
        let (mut guard, report) = BackgroundGuard::acquire();
        assert_eq!(report.len(), 0);
        assert!(!guard.power_request_active());
        assert_eq!(guard.escalate().len(), 0);
        assert_eq!(guard.relax().len(), 0);
    }

    // Windows-only: the baseline must actually apply on a normal
    // desktop session, and escalate/relax must round-trip the
    // priority class. Runs on Windows CI; compiled out on Linux.
    #[cfg(windows)]
    #[test]
    fn acquire_applies_baseline_on_windows() {
        assert!(BackgroundGuard::supported());
        let (mut guard, report) = BackgroundGuard::acquire();
        for step in &report {
            assert!(step.applied, "mitigation failed: {step:?}");
        }
        assert!(guard.power_request_active());
        assert_ne!(guard.escalate(), [] as [Mitigation; 0]);
        assert!(guard.escalate().is_empty(), "escalate is idempotent");
        assert_ne!(guard.relax(), [] as [Mitigation; 0]);
        assert!(guard.relax().is_empty(), "relax is idempotent");
    }
}
