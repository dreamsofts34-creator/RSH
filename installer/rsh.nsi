; RSH v2.0 NSIS installer — COMMITTED source of truth (v2.0.13 polish,
; P1-1: extracted from .github/workflows/release.yml, which used to
; generate it inline and hit GitHub's 21,000-char `run:` ceiling).
; The release workflow only substitutes ${VERSION} (see the
; "Prepare NSIS script" step) and runs makensis on this file.
; Four-asset release: this installer is one of four release assets
; (exe + merged firmware + OTA firmware + SHA256SUMS.txt).
;
; Autostart owner (P2-1, v2.0.13): the daemon-owned HKCU Run key is
; the SINGLE autostart mechanism — the daemon syncs it from
; settings.autostart on every start (Settings-controlled, in the
; real user's hive). The "RSH Daemon" scheduled task is GONE:
; its AtLogOn trigger duplicated the Run key (logon race: two
; daemons, one died ~11 s after boot and looked like a crash) and
; its crash-restart duplicated the WER restart registration
; (rsh-api restart.rs). The uninstaller still removes a legacy
; task from older installs (schtasks fallback). The UI self-spawn
; stays the on-demand path while the UI is open.
; (P1-4, same change: the installer no longer writes HKCU Run or a
; task principal from its elevated token, so a standard-user install
; can no longer seed autostart into the admin's hive.)
!include "LogicLib.nsh"
!include "WordFunc.nsh"
!include "StrFunc.nsh"
!include "WinVer.nsh"
!include "x64.nsh"
${Using:StrFunc} StrStr
!define VERSION "${VERSION}"
; P3-3 (v2.0.13 polish): solid LZMA — typically 10-20% smaller
; download than the default zlib, non-solid compression.
SetCompressor /SOLID lzma
; P2-3 (v2.0.13 polish): EstimatedSize is filled by the release
; workflow (makensis /DESTIMATEDSIZEKB=<installed KB>); 0 in the
; CI compile check means "unknown", never a wrong number.
!ifndef ESTIMATEDSIZEKB
  !define ESTIMATEDSIZEKB 0
!endif
; ---- Modern UI 2 (MUI2) -------------------------------------
; MUI2 wizard (welcome / license / directory / install /
; finish) + branding. Silent installs (/S) skip ALL pages,
; so the headless smoke test is structurally unaffected.
; License file is the repo-root MIT LICENSE — makensis
; fails closed if it is missing. Bitmaps are the RSH brand
; assets in src-tauri/icons/ (header 150x57, sidebar
; 164x314 — the exact MUI sizes; icon.ico is the app icon).
!include "MUI2.nsh"
!define MUI_ABORTWARNING
!define MUI_ICON "..\src-tauri\icons\icon.ico"
!define MUI_UNICON "..\src-tauri\icons\icon.ico"
!define MUI_HEADERIMAGE
!define MUI_HEADERIMAGE_BITMAP "..\src-tauri\icons\header.bmp"
!define MUI_WELCOMEFINISHPAGE_BITMAP "..\src-tauri\icons\sidebar.bmp"
; Finish page offers "Run RSH" (checked by default): the
; checkbox runs LaunchRSH below.
; P2-5 (v2.0.13 polish): the finish page also offers a desktop
; shortcut (checked by default — the install must stay
; discoverable when the user unchecks "Run RSH"). MUI2's
; SHOWREADME checkbox is the standard finish-page checkbox
; mechanism; its function creates the shortcut. Silent installs
; skip the finish page, so no desktop shortcut is created by /S.
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "Run RSH"
!define MUI_FINISHPAGE_RUN_FUNCTION "LaunchRSH"
!define MUI_FINISHPAGE_SHOWREADME
!define MUI_FINISHPAGE_SHOWREADME_TEXT "Create a desktop shortcut"
!define MUI_FINISHPAGE_SHOWREADME_FUNCTION "CreateDesktopShortcut"
Function CreateDesktopShortcut
  CreateShortcut "$DESKTOP\\RSH.lnk" "$INSTDIR\\rsh-ui.exe"
FunctionEnd
BrandingText "RSH ${VERSION}"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH
!insertmacro MUI_LANGUAGE "English"
; P4 (v2.0.11): launch the UI NON-elevated via explorer.exe —
; the old `ExecShell "open"` inherited the elevated installer
; token (UI + daemon ran elevated; UAC prompt suppressed).
; `Exec`, not ExecShell (DS-70): no inherited-pipe hang.
Function LaunchRSH
  Exec '"$WINDIR\explorer.exe" "$INSTDIR\rsh-ui.exe"'
FunctionEnd
; Smart-installer state: set by .onInit (detection) and read
; by the install section (daemon restart after an update).
Var UpdateMode
Var InstalledVersion
Name "RSH"
; NOTE: makensis changes its working directory to the
; script's directory (installer/), so every path below is
; relative to installer/ — hence the ..\ prefix. (Release
; run 43878025293 failed here: "target\release\..." was
; resolved as installer\target\release\..., which does not
; exist.)
OutFile "..\release-staging\RSH_${VERSION}_x64-setup.exe"
InstallDir "$PROGRAMFILES64\RSH"
InstallDirRegKey HKLM "Software\RSH" "InstallDir"
RequestExecutionLevel admin
; Admin is requested ONCE here, at install time, so the firewall
; rule below can be created. Nothing at login ever prompts (D-7:
; the daemon defers the elevated HIDMaestro sidecar until
; "Send to game" is turned on).
; ---- Smart installer: detect an existing install ------------
; Reads HKLM\Software\RSH "Version" (written by this installer
; since v2.0.2) and falls back to the Uninstall key
; DisplayVersion, then to InstallDir for a v2.0.0/v2.0.1
; install (those wrote no version value).
; NOTE on registry view: this is a 32-bit installer, so
; HKLM\Software\... reads/writes land in the WOW6432Node
; view on 64-bit Windows — the SAME view v2.0.0/v2.0.1 used,
; so detection matches existing installs. Do NOT add
; SetRegView 64: it would make v2.0.2 blind to v2.0.1.
; Silent installs (/S, e.g. CI) skip the prompt and proceed
; as an update/reinstall — the daemon restart below still runs.
Function .onInit
  ; P3-2 (v2.0.13 polish): RSH needs 64-bit Windows 10 or later —
  ; without this guard a 32-bit or Win7/8 machine would install
  ; x64 binaries that fail to start with a cryptic missing-API error.
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "RSH needs 64-bit Windows 10 or later. This PC is 32-bit Windows, so RSH cannot run here."
    Abort
  ${EndIf}
  ${IfNot} ${AtLeastWin10}
    MessageBox MB_ICONSTOP "RSH needs Windows 10 or later. This PC runs an older Windows version, so RSH cannot run here."
    Abort
  ${EndIf}
  StrCpy $UpdateMode "0"
  StrCpy $InstalledVersion ""
  ReadRegStr $InstalledVersion HKLM "Software\RSH" "Version"
  ${If} $InstalledVersion == ""
    ReadRegStr $InstalledVersion HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RSH" "DisplayVersion"
  ${EndIf}
  ${If} $InstalledVersion == ""
    ReadRegStr $0 HKLM "Software\RSH" "InstallDir"
    ${If} $0 != ""
      StrCpy $InstalledVersion "unknown"
    ${EndIf}
  ${EndIf}
  ${If} $InstalledVersion == ""
    Return
  ${EndIf}
  StrCpy $UpdateMode "1"
  ; P3-1 (v2.0.13 polish): a SILENT downgrade refuses, fail closed
  ; — /S used to silently replace a newer install with an older
  ; one. Pass /ALLOWDOWNGRADE on the command line to allow it
  ; (the flag IS the consent). The interactive downgrade prompt
  ; below is unchanged; "unknown" versions cannot be compared,
  ; so they still go through the prompt.
  ${If} $InstalledVersion != "unknown"
    ${VersionCompare} $InstalledVersion "${VERSION}" $R8
    ${If} $R8 == 1
      ${StrStr} $R9 $CMDLINE "/ALLOWDOWNGRADE"
      ${If} $R9 == ""
        IfSilent silent_downgrade_refused
      ${EndIf}
    ${EndIf}
  ${EndIf}
  IfSilent done
  ${If} $InstalledVersion == "unknown"
    MessageBox MB_YESNO|MB_ICONQUESTION "RSH is already installed on this PC.$\n$\nUpdate it to ${VERSION}? Your settings and profiles are kept." IDYES done
    Abort
  ${EndIf}
  ${VersionCompare} $InstalledVersion "${VERSION}" $0
  ${If} $0 == 0
    MessageBox MB_YESNO|MB_ICONQUESTION "RSH ${VERSION} is already installed (latest).$\n$\nReinstall it?" IDYES done
    Abort
  ${ElseIf} $0 == 1
    MessageBox MB_YESNO|MB_ICONEXCLAMATION "A NEWER version of RSH ($InstalledVersion) is already installed.$\n$\nDowngrade to ${VERSION}? (Not recommended.)" IDYES done
    Abort
  ${Else}
    MessageBox MB_YESNO|MB_ICONQUESTION "RSH $InstalledVersion is installed. Update to ${VERSION}?$\n$\nYour settings and profiles are kept." IDYES done
    Abort
  ${EndIf}
  done:
    ; Audit (v2.0.13): `done:` MUST return here — without this
    ; Return, execution falls through into
    ; silent_downgrade_refused below and EVERY update/reinstall
    ; aborts with exit code 1 (release run 45782080003: the
    ; second silent install exited 1 in 0.3 s; fresh installs
    ; were unaffected because they Return early above).
    Return
  silent_downgrade_refused:
    DetailPrint "Refusing silent downgrade from $InstalledVersion to ${VERSION} (pass /ALLOWDOWNGRADE to allow it)."
    SetErrorLevel 1
    Abort
FunctionEnd
  ; Kill loop as a FUNCTION (v2.0.13 polish, P2-2): the section
; calls it once, and RSH_File calls it again on a locked-file
; retry. Stop running processes first (locked files cannot be
; overwritten on update; the old daemon holds the IPC pipe).
; Best-effort: taskkill exits non-zero when the process is not
; running; nsExec ignores that.
; DS-68: UI FIRST, then daemon — the UI respawns a dead daemon
; (budgeted, from 1 s), so daemon-first lets a fresh old daemon
; start mid-install and re-lock rsh-daemon.exe. LOOP (max 10
; passes) until NO listed process exists; a respawn racing one
; pass is caught by the next. Existence is checked with tasklist
; /NH: the image name prints only when a matching process exists,
; else the "No tasks are running" INFO line (which contains no
; exe name).
; P3-5 (v2.0.13 polish): the legacy RSH.exe is tasklist-checked
; too — it used to be taskkilled but never verified gone, so a
; hung legacy app could hold UDP 4211 forever after install.
Function KillRSHProcesses
  StrCpy $R0 0
  DS68KillLoop:
    nsExec::Exec 'taskkill /F /IM rsh-ui.exe'
    Sleep 500
    nsExec::Exec 'taskkill /F /IM rsh-daemon.exe'
    ; F21: also stop the legacy Tauri v1 app (`RSH.exe`):
    ; it binds UDP port 4211 at launch; left running, the
    ; new daemon can never bind (`AddrInUse` forever —
    ; user log: 117x). The installer does NOT uninstall
    ; the old app (own per-user install + uninstall entry)
    ; — it only stops it so the port is free.
    nsExec::Exec 'taskkill /F /IM RSH.exe'
    ; P1-B: stop the sidecar too (both names) — it locks its exe + DLL.
    nsExec::Exec 'taskkill /F /IM RshCoreHiDcr.exe'
    nsExec::Exec 'taskkill /F /IM rsh-hm-sidecar.exe'
    Sleep 500
    IntOp $R0 $R0 + 1
    nsExec::ExecToStack 'tasklist /FI "IMAGENAME eq rsh-ui.exe" /NH'
    Pop $R1  ; exit code (discard)
    Pop $R2  ; stdout: image name present only if still running
    ${StrStr} $R3 $R2 "rsh-ui.exe"
    ${If} $R3 != ""
    ${AndIf} $R0 < 10
      Goto DS68KillLoop
    ${EndIf}
    nsExec::ExecToStack 'tasklist /FI "IMAGENAME eq rsh-daemon.exe" /NH'
    Pop $R1  ; exit code (discard)
    Pop $R2  ; stdout: image name present only if still running
    ${StrStr} $R3 $R2 "rsh-daemon.exe"
    ${If} $R3 != ""
    ${AndIf} $R0 < 10
      Goto DS68KillLoop
    ${EndIf}
    nsExec::ExecToStack 'tasklist /FI "IMAGENAME eq RshCoreHiDcr.exe" /NH'
    Pop $R1
    Pop $R2
    ${StrStr} $R3 $R2 "RshCoreHiDcr.exe"
    ${If} $R3 != ""
    ${AndIf} $R0 < 10
      Goto DS68KillLoop
    ${EndIf}
    nsExec::ExecToStack 'tasklist /FI "IMAGENAME eq rsh-hm-sidecar.exe" /NH'
    Pop $R1
    Pop $R2
    ${StrStr} $R3 $R2 "rsh-hm-sidecar.exe"
    ${If} $R3 != ""
    ${AndIf} $R0 < 10
      Goto DS68KillLoop
    ${EndIf}
  nsExec::ExecToStack 'tasklist /FI "IMAGENAME eq RSH.exe" /NH'
  Pop $R1
  Pop $R2
  ${StrStr} $R3 $R2 "RSH.exe"
  ${If} $R3 != ""
  ${AndIf} $R0 < 10
    Goto DS68KillLoop
  ${EndIf}
  ; Audit (v2.0.13, C P3-3): the loop above gives up silently after
  ; 10 passes — if a copied exe is still running, fail visibly here
  ; instead of letting the File copy hit the retry dialog (or a
  ; half-installed state) with no diagnostic.
  nsExec::ExecToStack 'tasklist /FI "IMAGENAME eq rsh-daemon.exe" /NH'
  Pop $R1
  Pop $R2
  ${StrStr} $R3 $R2 "rsh-daemon.exe"
  ${If} $R3 != ""
    DetailPrint "KillRSHProcesses: rsh-daemon.exe is still running after 10 kill passes — aborting the install."
    Abort
  ${EndIf}
  nsExec::ExecToStack 'tasklist /FI "IMAGENAME eq rsh-ui.exe" /NH'
  Pop $R1
  Pop $R2
  ${StrStr} $R3 $R2 "rsh-ui.exe"
  ${If} $R3 != ""
    DetailPrint "KillRSHProcesses: rsh-ui.exe is still running after 10 kill passes — aborting the install."
    Abort
  ${EndIf}
  nsExec::ExecToStack 'tasklist /FI "IMAGENAME eq RshCoreHiDcr.exe" /NH'
  Pop $R1
  Pop $R2
  ${StrStr} $R3 $R2 "RshCoreHiDcr.exe"
  ${If} $R3 != ""
    DetailPrint "KillRSHProcesses: RshCoreHiDcr.exe is still running after 10 kill passes — aborting the install."
    Abort
  ${EndIf}
FunctionEnd

; P2-2 (v2.0.13 polish): copy with one automatic kill-loop retry,
; then one user Retry/Cancel naming the file. ${__LINE__} makes
; the retry/abort labels unique per expansion (each !insertmacro
; call sits on its own line). A locked file used to abort into a
; half-installed state with a generic NSIS error.
; Unique retry/abort labels come from the N parameter (1..6, one
; per call site below). Neither ${__LINE__} (expands to a dotted
; "line.column" value, not a valid label) nor a !define counter
; (redefinition is an error) works for this in NSIS.
!macro RSH_File OPT SRC N
  ClearErrors
  File ${OPT} "${SRC}"
  ${If} ${Errors}
    Call KillRSHProcesses
    ClearErrors
    File ${OPT} "${SRC}"
    ${If} ${Errors}
      MessageBox /SD IDCANCEL MB_RETRYCANCEL|MB_ICONEXCLAMATION "Could not write $\"${SRC}$\" — it is still in use (another RSH process or antivirus may hold it). Click Retry after closing RSH, or Cancel to stop the install." IDRETRY RSH_File_retry_${N} IDCANCEL RSH_File_abort_${N}
      RSH_File_abort_${N}:
        Abort
      RSH_File_retry_${N}:
        Call KillRSHProcesses
        ClearErrors
        File ${OPT} "${SRC}"
        ${If} ${Errors}
          MessageBox MB_ICONSTOP "Could not write $\"${SRC}$\" even after retrying. Close RSH (or restart the PC if a process is stuck) and run the installer again."
          Abort
        ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend

; FH4 fix #1 (v2.0.13): GameInput RacingWheel mapping for the vJoy
; device, so Windows.Gaming.Input wheel games (Forza Horizon 4/5,
; Forza Motorsport on the Store) show a Wheel tab for RSH.
; Wheel games classify wheels from the registry, not from
; visibility:
; mappings for VID VVVV / PID PPPP / usage page UUUU / usage XXXX
; are read from
;   HKLM\SYSTEM\CurrentControlSet\Control\GameInput\Devices\VVVVPPPPUUUUXXXX
; (Microsoft Learn, "Registry data for game controllers"). vJoy
; device 1 is VID 1234 / PID BEAD, usage page 0001 (Generic
; Desktop), usage 0004 (Joystick) — hence key 1234BEAD00010004.
; This is a Windows-level classification, not a per-game hack: no
; game is detected, no game-specific key is written.
; Axis indices are RawGameController indices, which follow the
; vJoy HID descriptor order (fixed upstream in vJoyConf's
; CreateHidReportDesc): X=0, Y=1, Z=2, Rx=3, Ry=4, Rz=5,
; Slider=6, Dial=7 — the EmuWheel envelope (see
; ConfigureVJoyDevice1) enables exactly these eight, so:
;   Wheel=X/0, Throttle=Y/1, Brake=Z/2, Handbrake=Slider/6.
; Invert is 0 everywhere: RSH's raw values RISE with input
; (steering right = max, pedals pressed = max — see
; crates/rsh-output/src/vjoy.rs), so no inversion is needed.
; (The Learn example inverts Throttle/Brake because ITS example
; device reports pedals inverted; RSH does not.)
; PreviousGear/NextGear are the sequential-shifter paddles:
; PROTOCOL-V2.2 §1.1 — stream bit 0 = shift-up, bit 1 =
; shift-down — and the output button word is 1-based (bit 0 =
; button 1), so NextGear = button 1 = ButtonIndex 0 and
; PreviousGear = button 2 = ButtonIndex 1. H-pattern gears are
; deliberately NOT mapped (FirstGear..SixthGear, ReverseGear):
; the pipeline strips gear bits 5–10 from the button word, so
; those buttons never fire — a mapping would point at dead
; indices. No Clutch subkey either: the envelope has no Clutch
; usage axis (RSH's wClutch field is ignored by the driver when
; the axis is absent), and Clutch is optional in the schema.
; MaxWheelAngle 450 = the default 900° lock-to-lock, one
; direction (the Learn example uses 0x1C2 = 450 for the same
; reason). Labels are optional in the schema and skipped.
; SetRegView 64 is SCOPED (same pattern as the WER block): the
; mapping must land in the native 64-bit view, where 64-bit games
; read it. Written on EVERY install (fresh and update — this
; section always runs), and removed by the uninstaller.
; The game must be restarted after install for the mapping to take
; effect (GameInput reads it at device enumeration). The user's
; PC retest — in Forza Horizon 4 and in a second game — is the
; only proof the Wheel tab appears; nothing here claims the wheel
; works in every game.
Function WriteGameInputVJoyMapping
  SetRegView 64
  WriteRegStr HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1234BEAD00010004" "Description" "RSH Wheel (vJoy)"
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1234BEAD00010004\RacingWheel" "MaxWheelAngle" 450
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1234BEAD00010004\RacingWheel\Wheel" "AxisIndex" 0
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1234BEAD00010004\RacingWheel\Wheel" "Invert" 0
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1234BEAD00010004\RacingWheel\Throttle" "AxisIndex" 1
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1234BEAD00010004\RacingWheel\Throttle" "Invert" 0
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1234BEAD00010004\RacingWheel\Brake" "AxisIndex" 2
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1234BEAD00010004\RacingWheel\Brake" "Invert" 0
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1234BEAD00010004\RacingWheel\Handbrake" "AxisIndex" 6
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1234BEAD00010004\RacingWheel\Handbrake" "Invert" 0
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1234BEAD00010004\RacingWheel\PreviousGear" "ButtonIndex" 1
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1234BEAD00010004\RacingWheel\NextGear" "ButtonIndex" 0
  SetRegView 32
FunctionEnd

; FH4 fix #2 (v2.0.13): the same Windows-level RacingWheel
; classification for the HIDMaestro device — the parallel /
; alternate experiment to fix #1, same mechanism, no
; game-specific behaviour. VID 1209 / PID 5253 (the pid.codes
; claim; tools/rsh-hm-sidecar/Constants.cs), usage page 0001,
; usage 0004 — hence key 1209525300010004. Axis indices are
; RawGameController indices following the custom profile's HID
; descriptor order (tools/rsh-hm-sidecar/Program.cs,
; BuildDescriptor: X, Y, Z, Rx, Slider — 5× 16-bit axes, 32
; buttons, 1 hat, 15-byte report):
;   Wheel=X/0, Throttle=Y/1, Brake=Z/2, Clutch=Rx/3,
;   Handbrake=Slider/4.
; Clutch IS mapped here (unlike the vJoy mapping): Rx is the
; clutch axis on this profile and RSH feeds it there
; (crates/rsh-output-hidmaestro/src/mapping.rs). Invert is 0
; everywhere for the same reason as fix #1 (raw values rise with
; input — see mapping.rs axis_to_u16: released = 0, pressed =
; 32767). PreviousGear/NextGear are the same sequential-shifter
; paddles as fix #1 (PROTOCOL-V2.2 §1.1: bit 0 = shift-up =
; button 1 = ButtonIndex 0; bit 1 = shift-down = button 2 =
; ButtonIndex 1). H-pattern gears are not mapped (gear bits are
; stripped from the button word — same reason as fix #1).
; MaxWheelAngle 450, same as fix #1 (default 900° lock). Written
; on EVERY install (fresh and update), removed by the
; uninstaller. The game must be restarted after install; the
; user's PC retest (FH4 and a second game) is the only proof.
Function WriteGameInputHidMapping
  SetRegView 64
  WriteRegStr HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1209525300010004" "Description" "RSH Wheel (HIDMaestro)"
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1209525300010004\RacingWheel" "MaxWheelAngle" 450
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1209525300010004\RacingWheel\Wheel" "AxisIndex" 0
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1209525300010004\RacingWheel\Wheel" "Invert" 0
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1209525300010004\RacingWheel\Throttle" "AxisIndex" 1
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1209525300010004\RacingWheel\Throttle" "Invert" 0
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1209525300010004\RacingWheel\Brake" "AxisIndex" 2
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1209525300010004\RacingWheel\Brake" "Invert" 0
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1209525300010004\RacingWheel\Clutch" "AxisIndex" 3
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1209525300010004\RacingWheel\Clutch" "Invert" 0
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1209525300010004\RacingWheel\Handbrake" "AxisIndex" 4
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1209525300010004\RacingWheel\Handbrake" "Invert" 0
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1209525300010004\RacingWheel\PreviousGear" "ButtonIndex" 1
  WriteRegDWORD HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1209525300010004\RacingWheel\NextGear" "ButtonIndex" 0
  SetRegView 32
FunctionEnd

; FH4 fix #1 (v2.0.13): reconfigure vJoy device 1 to the EmuWheel
; envelope (8 axes, 128 buttons, 1× 4-direction POV, all FFB) via
; vJoy's own CLI — see crates/rsh-output/src/vjoy_config.rs for
; the full derivation (the argv spelling lives there as the single
; source of truth; this call mirrors it exactly):
;   vJoyConfig.exe 1 -f -a x y z rx ry rz sl0 sl1 -b 128 -s 1 -e All
; Runs on EVERY install (fresh and update — `-f` replaces the
; existing device-1 config). vJoy missing → skipped silently
; (DetailPrint): the wizard installs vJoy later, and the app's
; post-install path (rsh-app-core run_vjoy_install_inner) configures
; device 1 then. Best-effort, never fatal: a failure warns (the
; mapping above is still written) and the app's connect-time
; capabilities check is the real verification — vJoyConfig exits 0
; even when its registry write fails, so the exit code only proves
; the tool ran. The installer additionally reads back
; HidReportDescriptorSize for Device01 (non-zero = a descriptor
; landed). vJoyConfig restarts the vJoy devices itself
; (refresh_vjoy), so no reboot is needed in the normal case.
Function ConfigureVJoyDevice1
  StrCpy $R0 ""
  IfFileExists "$PROGRAMFILES64\vJoy\x64\vJoyConfig.exe" vjoycfg_found
  IfFileExists "$PROGRAMFILES32\vJoy\x64\vJoyConfig.exe" vjoycfg_found_x86
  DetailPrint "FH4: vJoy not installed yet — skipping vJoy device-1 configuration (the app configures it after installing vJoy)."
  Return
  vjoycfg_found_x86:
    StrCpy $R0 "$PROGRAMFILES32\vJoy\x64\vJoyConfig.exe"
    Goto vjoycfg_run
  vjoycfg_found:
    StrCpy $R0 "$PROGRAMFILES64\vJoy\x64\vJoyConfig.exe"
  vjoycfg_run:
  nsExec::ExecToStack '"$R0" 1 -f -a x y z rx ry rz sl0 sl1 -b 128 -s 1 -e All'
  Pop $R1  ; exit code
  Pop $R2  ; stdout (discard)
  ${If} $R1 != 0
    MessageBox /SD IDOK MB_ICONEXCLAMATION "RSH installed, but vJoy device 1 could not be configured as a wheel for Windows.Gaming.Input games (vJoyConfig exit $R1). Wheel games (for example Forza Horizon 4) may not show a Wheel tab. Next step: open vJoyConf, set device 1 to 8 axes (X Y Z Rx Ry Rz Slider Dial), 128 buttons, 1 POV hat, all FFB effects — or run the RSH installer again."
    Return
  ${EndIf}
  ; Verify the descriptor landed: Device01 must exist with a
  ; non-zero HidReportDescriptorSize (SetRegView 64 — the key
  ; lives in the native view; see WriteGameInputVJoyMapping).
  SetRegView 64
  ClearErrors
  ReadRegDWORD $R3 HKLM "SYSTEM\CurrentControlSet\Services\vjoy\Parameters\Device01" "HidReportDescriptorSize"
  SetRegView 32
  ${If} ${Errors}
  ${OrIf} $R3 == 0
    MessageBox /SD IDOK MB_ICONEXCLAMATION "RSH installed, but the vJoy device-1 configuration could not be verified (no descriptor found for device 1). Wheel games (for example Forza Horizon 4) may not show a Wheel tab. Next step: open vJoyConf, set device 1 to 8 axes (X Y Z Rx Ry Rz Slider Dial), 128 buttons, 1 POV hat, all FFB effects — or run the RSH installer again."
  ${EndIf}
FunctionEnd

Section "RSH"
  SetOutPath "$INSTDIR"
  Call KillRSHProcesses
  ; P2-2: file copies go through RSH_File (below) — one
  ; automatic kill-loop retry, then a user Retry/Cancel
  ; naming the file, instead of aborting into a
  ; half-installed state on a locked file.

  !insertmacro RSH_File "" "..\target\release\rsh-daemon.exe" 1
  !insertmacro RSH_File "" "..\target\release\rsh-ui.exe" 2
  ; Sidecar MUST sit beside rsh-daemon.exe: the daemon resolves it
  ; as <own exe dir>\RshCoreHiDcr.exe (resolve_sidecar_path).
  !insertmacro RSH_File "/oname=RshCoreHiDcr.exe" "..\src-tauri\binaries\RshCoreHiDcr-x86_64-pc-windows-msvc.exe" 3
  ; Rename cleanup: drop a stale old-name sidecar from a
  ; <= v2.0.8 install (Delete on a locked file fails silently).
  Delete "$INSTDIR\rsh-hm-sidecar.exe"
  ; HIDMaestro.Core.dll (v2.0.8 retest Bug 1, H1): the
  ; sidecar SHA-256-hashes this DLL beside itself at
  ; startup (Program.cs CheckDllHash) and the Rust side
  ; refuses the hello when the hash is missing — the
  ; installer never shipped it, so the switch always
  ; failed. Pinned v2.1.0, hash-verified by the stage
  ; step; makensis fails closed here if it is missing.
  !insertmacro RSH_File "" "..\src-tauri\binaries\HIDMaestro.Core.dll" 4
  ; HIDMaestro LICENSE (GitHub issue #2): the sidecar is
  ; a NativeAOT build with HIDMaestro compiled in, so the
  ; MIT notice must ship with it — this file (pinned
  ; v2.1.0 ZIP, verbatim; THIRD-PARTY-LICENSES.md §3)
  ; installs beside the sidecar.
  !insertmacro RSH_File "" "..\tools\rsh-hm-sidecar\HIDMaestro-LICENSE.txt" 5
  ; Signed OTA image, bundled so the app can flash the
  ; board with no download: the daemon resolves it as
  ; <own exe dir>\firmware-signed.bin. Makensis fails
  ; closed here if it is missing.
  !insertmacro RSH_File "" "..\firmware\.pio\build\esp32dev\firmware-signed.bin" 6
  WriteRegStr HKLM "Software\RSH" "InstallDir" "$INSTDIR"
  ; Version values the smart installer reads back in .onInit.
  WriteRegStr HKLM "Software\RSH" "Version" "${VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RSH" "DisplayName" "RSH ${VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RSH" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RSH" "UninstallString" "$INSTDIR\uninstall.exe"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RSH" "Publisher" "Dreamsofts"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RSH" "DisplayIcon" "$INSTDIR\\rsh-ui.exe"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RSH" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RSH" "QuietUninstallString" "$INSTDIR\\uninstall.exe /S"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RSH" "URLInfoAbout" "https://github.com/dreamsofts34-creator/RSH"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RSH" "HelpLink" "https://github.com/dreamsofts34-creator/RSH"
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RSH" "NoModify" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RSH" "NoRepair" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RSH" "EstimatedSize" ${ESTIMATEDSIZEKB}
  ; VersionMajor/Minor from ${VERSION} (WordFind: "2.0.13" -> 2 / 0).
  ${WordFind} ${VERSION} "." "1" $R0
  ${WordFind} ${VERSION} "." "2" $R1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RSH" "VersionMajor" $R0
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RSH" "VersionMinor" $R1
  ; Round 3 WS-6 (R3-AK-02): WER LocalDumps — native-crash
  ; forensics with zero runtime cost. WER writes the dump
  ; out of process after any unhandled exception, including
  ; native SEH faults the Rust panic hook can never see.
  ; DumpType 1 = mini dump; DumpCount 5 caps the ring so
  ; dumps can never fill the disk. The folder is created
  ; here (WER also creates it, but explicit is fail-closed).
  ; SetRegView 64 is SCOPED: WER reads LocalDumps from the
  ; native 64-bit view; this 32-bit installer would
  ; otherwise land the keys in WOW6432Node, where WER never
  ; looks for a 64-bit exe. Restored to 32 right after, so
  ; the RSH-key detection (deliberately 32-bit — see
  ; .onInit) is unaffected. The uninstaller removes these
  ; keys (the dumps themselves are kept, like %APPDATA%\RSH).
  ; $COMMONPROGRAMDATA, NOT $PROGRAMDATA: NSIS has no
  ; $PROGRAMDATA constant (manual §4.2.3 lists
  ; $COMMONPROGRAMDATA as the all-users app-data alias) —
  ; the unknown name expands to empty, so the old path was
  ; "\RSH\dumps" and WER dumps went nowhere. Found by the
  ; makensis warning-6000 check during installer-modern.
  CreateDirectory "$COMMONPROGRAMDATA\RSH\dumps"
  SetRegView 64
  WriteRegStr HKLM "Software\Microsoft\Windows\Windows Error Reporting\LocalDumps\rsh-daemon.exe" "DumpFolder" "$COMMONPROGRAMDATA\RSH\dumps"
  WriteRegDWORD HKLM "Software\Microsoft\Windows\Windows Error Reporting\LocalDumps\rsh-daemon.exe" "DumpType" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\Windows Error Reporting\LocalDumps\rsh-daemon.exe" "DumpCount" 5
  WriteRegStr HKLM "Software\Microsoft\Windows\Windows Error Reporting\LocalDumps\rsh-ui.exe" "DumpFolder" "$COMMONPROGRAMDATA\RSH\dumps"
  WriteRegDWORD HKLM "Software\Microsoft\Windows\Windows Error Reporting\LocalDumps\rsh-ui.exe" "DumpType" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\Windows Error Reporting\LocalDumps\rsh-ui.exe" "DumpCount" 5
  ; P0 (v2.0.11): the sidecar too — its native crash left
  ; no dump (only daemon/UI were registered).
  WriteRegStr HKLM "Software\Microsoft\Windows\Windows Error Reporting\LocalDumps\RshCoreHiDcr.exe" "DumpFolder" "$COMMONPROGRAMDATA\RSH\dumps"
  WriteRegDWORD HKLM "Software\Microsoft\Windows\Windows Error Reporting\LocalDumps\RshCoreHiDcr.exe" "DumpType" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\Windows Error Reporting\LocalDumps\RshCoreHiDcr.exe" "DumpCount" 5
  SetRegView 32
  ; P2-1 (v2.0.13 polish): NO scheduled task anymore — the
  ; "RSH Daemon" task is removed (see the header comment).
  ; Autostart is the daemon-owned Run key; crash-restart is
  ; the WER restart registration. The uninstaller below
  ; still removes a legacy task from older installs.
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateDirectory "$SMPROGRAMS\RSH"
  CreateShortcut "$SMPROGRAMS\RSH\RSH.lnk" "$INSTDIR\rsh-ui.exe"
  ; Firewall rule (LOGIC-FIX-PLAN §4.3 / C-10): program-scoped
  ; inbound UDP for the daemon only (the board's 200 Hz stream
  ; arrives unsolicited from the board, source port 4211) on
  ; Private + Public. No port scope: a port-scoped rule would
  ; also allow other programs on that port. Hidden via nsExec
  ; (no console flash during install).
  ; Update-safe: remove any rule from a previous install first,
  ; otherwise every update would leave a duplicate "RSH" rule.
  nsExec::Exec 'netsh advfirewall firewall delete rule name="RSH"'
  nsExec::ExecToStack 'netsh advfirewall firewall add rule name="RSH" dir=in action=allow protocol=UDP localport=4210-4212 program="$INSTDIR\rsh-daemon.exe" profile=private,public'
  Pop $0
  Pop $1  ; DS-70: ExecToStack pushes exit code + stdout — discard both
  ${If} $0 != 0
    MessageBox /SD IDOK MB_ICONEXCLAMATION "RSH installed, but the Windows Firewall rule could not be created (netsh exit $0). The wheel may not connect over Wi-Fi. Next step: Windows Security -> Firewall & network protection -> Allow an app through firewall -> add rsh-daemon.exe; or run the installer again as administrator."
  ${EndIf}
  ; FH4 fixes #1+#2 (v2.0.13): the RacingWheel mappings FIRST, then
  ; the device-1 envelope — vJoyConfig restarts the vJoy devices,
  ; so a mapping written before it is picked up at the same
  ; re-enumeration. All run on every install (fresh and update).
  Call WriteGameInputVJoyMapping
  Call WriteGameInputHidMapping
  Call ConfigureVJoyDevice1
  ; After an update, restart the daemon (it was stopped
  ; above so its files could be replaced). Fresh installs do
  ; not start it — the UI self-spawns it on first launch.
  ; P4 (v2.0.11): restart it NON-elevated via explorer.exe
  ; (same as LaunchRSH) — the old `ExecShell "open"` ran it
  ; elevated until the next logon. No inherited pipes, so no
  ; DS-70 hang (release run 44047919933).
  ; P2-4 (v2.0.13 polish): the daemon starts after EVERY
  ; install now — fresh installs used to start nothing unless
  ; "Run RSH" stayed checked, so the install could look dead.
  ; Non-elevated via explorer.exe (P4, v2.0.11).
    Exec '"$WINDIR\explorer.exe" "$INSTDIR\rsh-daemon.exe"'
SectionEnd
Section "Uninstall"
  ; DS-10: stop the running app FIRST (UI first, then daemon —
  ; DS-68: the UI respawns a dead daemon, so daemon-first
  ; leaves a fresh process holding the files). Without this,
  ; Delete on a locked exe fails silently and RMDir fails on
  ; the non-empty dir — a partial uninstall (binaries + dir
  ; left behind, registry removed) reported as success.
  nsExec::Exec 'taskkill /F /IM rsh-ui.exe'
  Sleep 1000
  nsExec::Exec 'taskkill /F /IM rsh-daemon.exe'
  nsExec::Exec 'taskkill /F /IM RSH.exe'
  nsExec::Exec 'taskkill /F /IM RshCoreHiDcr.exe'
  ; Old exe name (<= v2.0.8): an upgrade from an old install
  ; can leave a running rsh-hm-sidecar.exe behind — kill it
  ; too so the old file is never locked during uninstall.
  nsExec::Exec 'taskkill /F /IM rsh-hm-sidecar.exe'
  Sleep 1000
  nsExec::Exec 'taskkill /F /IM rsh-ui.exe'
  nsExec::Exec 'taskkill /F /IM rsh-daemon.exe'
  ; Remove the firewall rule first (best-effort: a missing rule
  ; is fine on a machine where the install-time rule failed).
  nsExec::ExecToStack 'netsh advfirewall firewall delete rule name="RSH"'
  Pop $0
  Pop $1  ; DS-70: ExecToStack pushes exit code + stdout — discard both
  ; Round 3 WS-6 (R3-AK-01): remove the "RSH Daemon"
  ; supervisor task (best-effort — a missing task is fine).
  ; Runs before the script file itself is deleted below.
  ; The `schtasks` line is the fallback for an install
  ; whose rsh-task.ps1 is already gone.
  ; Audit (v2.0.13, C P3-5): rsh-task.ps1 is NOT shipped by this
  ; installer (no File instruction for it), so the powershell call
  ; below always errors (ignored via nsExec) — the `schtasks`
  ; line is the real legacy-task removal. Both are kept: an
  ; upgrade from an install that did ship the file still needs
  ; the Deletes below, and the call is a harmless no-op otherwise.
  nsExec::Exec 'powershell -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\rsh-task.ps1" -Uninstall'
  nsExec::Exec 'schtasks /delete /tn "RSH Daemon" /f'
  ; Round 3 WS-6 (R3-AK-02): remove the WER LocalDumps
  ; keys — same scoped SetRegView 64 as the install (the
  ; keys live in the native 64-bit view, not WOW6432Node).
  ; The dumps in %ProgramData%\RSH\dumps are deliberately
  ; KEPT (crash forensics for a later reinstall — same
  ; KEEP decision as %APPDATA%\RSH below).
  SetRegView 64
  DeleteRegKey HKLM "Software\Microsoft\Windows\Windows Error Reporting\LocalDumps\rsh-daemon.exe"
  DeleteRegKey HKLM "Software\Microsoft\Windows\Windows Error Reporting\LocalDumps\rsh-ui.exe"
  DeleteRegKey HKLM "Software\Microsoft\Windows\Windows Error Reporting\LocalDumps\RshCoreHiDcr.exe"
  ; FH4 fix #1 (v2.0.13): remove the vJoy RacingWheel mapping this
  ; installer wrote — a stale mapping would classify a device that
  ; no longer has RSH behind it.
  DeleteRegKey HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1234BEAD00010004"
  ; FH4 fix #2 (v2.0.13): same for the HIDMaestro mapping.
  DeleteRegKey HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1209525300010004"
  SetRegView 32
  Delete "$INSTDIR\rsh-daemon.exe"
  Delete "$INSTDIR\rsh-ui.exe"
  Delete "$INSTDIR\RshCoreHiDcr.exe"
  ; Old exe name (<= v2.0.8): remove it too on uninstall so
  ; no stale sidecar survives an upgrade.
  Delete "$INSTDIR\rsh-hm-sidecar.exe"
  Delete "$INSTDIR\HIDMaestro.Core.dll"
  Delete "$INSTDIR\HIDMaestro-LICENSE.txt"
  Delete "$INSTDIR\firmware-signed.bin"
  Delete "$INSTDIR\rsh-task.ps1"
  Delete "$INSTDIR\uninstall.exe"
  Delete "$SMPROGRAMS\RSH\RSH.lnk"
  Delete "$DESKTOP\\RSH.lnk"
  RMDir "$SMPROGRAMS\RSH"
  RMDir "$INSTDIR"
  ; Retry once (release run 45110475449): the uninstall
  ; exited 0 but $INSTDIR was still there — a Delete on a
  ; locked exe fails silently and RMDir fails on the
  ; non-empty dir. A process can respawn (or a handle can
  ; still be closing) between the taskkills above and the
  ; Deletes, so kill again, wait, and delete again.
  Sleep 3000
  nsExec::Exec 'taskkill /F /IM rsh-ui.exe'
  nsExec::Exec 'taskkill /F /IM rsh-daemon.exe'
  nsExec::Exec 'taskkill /F /IM RshCoreHiDcr.exe'
  nsExec::Exec 'taskkill /F /IM rsh-hm-sidecar.exe'
  nsExec::Exec 'taskkill /F /IM RSH.exe'
  Sleep 1000
  Delete "$INSTDIR\rsh-daemon.exe"
  Delete "$INSTDIR\rsh-ui.exe"
  Delete "$INSTDIR\RshCoreHiDcr.exe"
  ; Old exe name (<= v2.0.8): remove it too on the retry pass.
  Delete "$INSTDIR\rsh-hm-sidecar.exe"
  Delete "$INSTDIR\HIDMaestro.Core.dll"
  Delete "$INSTDIR\HIDMaestro-LICENSE.txt"
  Delete "$INSTDIR\firmware-signed.bin"
  Delete "$INSTDIR\rsh-task.ps1"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  ; DS-10: fail visibly if the daemon exe survived (still
  ; running/locked) instead of reporting a partial uninstall
  ; as success.
  IfFileExists "$INSTDIR\rsh-daemon.exe" 0 warn_done
  MessageBox /SD IDOK MB_ICONEXCLAMATION "RSH could not be fully removed: rsh-daemon.exe is still in use (the app may still be running). Close RSH and run the uninstaller again."
  warn_done:
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RSH"
  DeleteRegKey HKLM "Software\RSH"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "RSH"
  ; DS-69 (decided: KEEP): %APPDATA%\RSH is intentionally NOT
  ; deleted — profiles, settings, logs and the daemon status
  ; file survive an uninstall, so a reinstall keeps the user's
  ; data (same as an update install, which also keeps it).
  ; Delete %APPDATA%\RSH manually for a fully clean start.
  ; The same KEEP decision is stated in the release notes.
  DetailPrint "DS-69: your RSH data (profiles, settings, logs) and crash dumps were kept — delete %APPDATA%\RSH and %ProgramData%\RSH\dumps manually for a fully clean start."
SectionEnd
