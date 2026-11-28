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
; Audit (v2.0.13, attempt 5): VersionMajor/Minor are filled by the
; release workflow too (makensis /DVERSIONMAJOR=<n>
; /DVERSIONMINOR=<n>, split from the tag version) — the old
; WordFind parse returned the wrong minor (WordFind "2.0.13" "."
; "2" gave 2, not 0), so the installer no longer parses the
; version string itself. 0 here means "unknown", as above.
!ifndef VERSIONMAJOR
  !define VERSIONMAJOR 0
!endif
!ifndef VERSIONMINOR
  !define VERSIONMINOR 0
!endif
; ---- Modern UI 2 (MUI2) -------------------------------------
; MUI2 wizard (welcome / license / directory / components /
; install / finish) + branding. Silent installs (/S) skip ALL pages,
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
; The main RSH section is required (SectionIn RO).
!insertmacro MUI_PAGE_COMPONENTS
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
; no backend is deferred at startup in v2.0.19).
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
    MessageBox MB_ICONSTOP "RSH needs 64-bit Windows 10 or later. This PC is 32-bit Windows, so RSH cannot run here." /SD IDOK
    Abort
  ${EndIf}
  ${IfNot} ${AtLeastWin10}
    MessageBox MB_ICONSTOP "RSH needs Windows 10 or later. This PC runs an older Windows version, so RSH cannot run here." /SD IDOK
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
FunctionEnd

; P2-2 (v2.0.13 polish): copy with one automatic kill-loop retry,
; then one user Retry/Cancel naming the file. ${__LINE__} makes
; the retry/abort labels unique per expansion (each !insertmacro
; call sits on its own line). A locked file used to abort into a
; half-installed state with a generic NSIS error.
; Unique retry/abort labels come from the N parameter (1..5, one
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
      MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "Could not write $\"${SRC}$\" — it is still in use (another RSH process or antivirus may hold it). Click Retry after closing RSH, or Cancel to stop the install." /SD IDCANCEL IDRETRY RSH_File_retry_${N} IDCANCEL RSH_File_abort_${N}
      RSH_File_abort_${N}:
        Abort
      RSH_File_retry_${N}:
        Call KillRSHProcesses
        ClearErrors
        File ${OPT} "${SRC}"
        ${If} ${Errors}
          MessageBox MB_ICONSTOP "Could not write $\"${SRC}$\" even after retrying. Close RSH (or restart the PC if a process is stuck) and run the installer again." /SD IDOK
          Abort
        ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend

; FH4 fix #1 (v2.0.13): reconfigure vJoy device 1 to the EmuWheel
; envelope (8 axes, 128 buttons, 1× 4-direction POV, all FFB) via
; vJoy's own CLI — see crates/rsh-output/src/vjoy_config.rs for
; the full derivation (the argv spelling lives there as the single
; source of truth; this call mirrors it exactly).
; v2.0.19: the GameInput RacingWheel mappings are GONE (the
; wheel-tab route they served did not work on the user's
; Microsoft Store build of FH4) — the envelope stays: it is
; the vJoy device-1 shape the app's capabilities check
; verifies, and the 7-input mapping in vjoy.rs feeds it.
;   vJoyConfig.exe 1 -f -a x y z rx ry rz sl0 sl1 -b 128 -s 1 -e All
; Runs on EVERY install (fresh and update — `-f` replaces the
; existing device-1 config). vJoy missing → LOUD (v2.0.14 — a
; MessageBox, not a silent DetailPrint): the wizard installs vJoy
; later, and the app's post-install path (rsh-app-core
; run_vjoy_install_inner) configures device 1 then — but a skipped
; envelope must be visible at install time, not discovered later
; in a game. Best-effort, never fatal: a failure warns and the
; app's connect-time capabilities check is the real verification
; — vJoyConfig exits 0
; even when its registry write fails, so the exit code only proves
; the tool ran. The installer additionally reads back
; HidReportDescriptorSize for Device01 (non-zero = a descriptor
; landed). vJoyConfig restarts the vJoy devices itself
; (refresh_vjoy), so no reboot is needed in the normal case.
Function ConfigureVJoyDevice1
  StrCpy $R0 ""
  IfFileExists "$PROGRAMFILES64\vJoy\x64\vJoyConfig.exe" vjoycfg_found
  IfFileExists "$PROGRAMFILES32\vJoy\x64\vJoyConfig.exe" vjoycfg_found_x86
  ; FH4 fix (v2.0.14, B3): a missing vJoyConfig.exe used to skip
  ; silently (DetailPrint only) — a skipped envelope is invisible
  ; at install time and only shows up later as "no Wheel tab".
  ; Make it LOUD, same pattern as the failure boxes below
  ; (/SD IDOK so silent installs never hang).
  MessageBox MB_ICONEXCLAMATION "RSH installed, but vJoy is not installed yet, so vJoy device 1 could not be configured as a wheel for Windows.Gaming.Input games. Wheel games (for example Forza Horizon 4) may not show a Wheel tab until vJoy is installed. Next step: install vJoy from the RSH wizard (Settings / Output) — the app configures device 1 right after the install — or run the RSH installer again after installing vJoy." /SD IDOK
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
    MessageBox MB_ICONEXCLAMATION "RSH installed, but vJoy device 1 could not be configured as a wheel for Windows.Gaming.Input games (vJoyConfig exit $R1). Wheel games (for example Forza Horizon 4) may not show a Wheel tab. Next step: open vJoyConf, set device 1 to 8 axes (X Y Z Rx Ry Rz Slider Dial), 128 buttons, 1 POV hat, all FFB effects — or run the RSH installer again." /SD IDOK
    Return
  ${EndIf}
  ; Verify the descriptor landed: Device01 must exist with a
  ; non-zero HidReportDescriptorSize (SetRegView 64 — the key
  ; lives in the native view).
  SetRegView 64
  ClearErrors
  ReadRegDWORD $R3 HKLM "SYSTEM\CurrentControlSet\Services\vjoy\Parameters\Device01" "HidReportDescriptorSize"
  SetRegView 32
  ${If} ${Errors}
  ${OrIf} $R3 == 0
    MessageBox MB_ICONEXCLAMATION "RSH installed, but the vJoy device-1 configuration could not be verified (no descriptor found for device 1). Wheel games (for example Forza Horizon 4) may not show a Wheel tab. Next step: open vJoyConf, set device 1 to 8 axes (X Y Z Rx Ry Rz Slider Dial), 128 buttons, 1 POV hat, all FFB effects — or run the RSH installer again." /SD IDOK
  ${EndIf}
FunctionEnd

; v2.0.19: delete the stale HKLM OEMData value the Steam FH4 mode
; (v2.0.15) wrote on the vJoy key (VID_1234&PID_BEAD), on EVERY
; install, unconditionally — the mode is deleted, and a leftover
; value can confuse the Microsoft Store build of FH4. Only the
; VALUE is ever deleted, never the OEM key (it is vJoy's).
; SetRegView 64 is SCOPED: the key lives in the native view.
; A missing value is fine (DeleteRegValue is a no-op then).
Function DeleteStaleOemData
  SetRegView 64
  DeleteRegValue HKLM "SYSTEM\CurrentControlSet\Control\MediaProperties\PrivateProperties\Joystick\OEM\VID_1234&PID_BEAD" "OEMData"
  SetRegView 32
FunctionEnd

Section "RSH" SEC_RSH
  ; Required: the app itself is never optional.
  SectionIn RO
  SetOutPath "$INSTDIR"
  Call KillRSHProcesses
  ; P2-2: file copies go through RSH_File (below) — one
  ; automatic kill-loop retry, then a user Retry/Cancel
  ; naming the file, instead of aborting into a
  ; half-installed state on a locked file.

  !insertmacro RSH_File "" "..\target\release\rsh-daemon.exe" 1
  !insertmacro RSH_File "" "..\target\release\rsh-ui.exe" 2
  ; ViGEmBus client DLL (v2.0.19): MUST sit beside
  ; rsh-daemon.exe — the Xbox backend loads it from its
  ; own exe dir (crates/rsh-output/src/vigem.rs,
  ; resolve_dll_path). See tools/vigem/README.md for
  ; provenance + SHA-256; makensis fails closed here if
  ; it is missing.
  !insertmacro RSH_File "" "..\tools\vigem\ViGEmClient.dll" 3
  ; ViGEmBus driver setup (v2.0.19, official 1.22.0): the
  ; app launches it interactively when the driver is
  ; missing — there is no documented silent switch, so
  ; it is bundled, never run silently. Makensis fails
  ; closed here if it is missing.
  !insertmacro RSH_File "" "..\tools\vigem\ViGEmBus_1.22.0_x64_x86_arm64.exe" 4
  ; Rename cleanup: drop stale sidecar files from a
  ; <= v2.0.18 install (Delete on a locked file fails
  ; silently). The HIDMaestro backend + sidecar were
  ; removed in v2.0.19.
  Delete "$INSTDIR\RshCoreHiDcr.exe"
  Delete "$INSTDIR\rsh-hm-sidecar.exe"
  Delete "$INSTDIR\HIDMaestro.Core.dll"
  Delete "$INSTDIR\HIDMaestro-LICENSE.txt"
  ; Signed OTA image, bundled so the app can flash the
  ; board with no download: the daemon resolves it as
  ; <own exe dir>\firmware-signed.bin. Makensis fails
  ; closed here if it is missing.
  !insertmacro RSH_File "" "..\firmware\.pio\build\esp32dev\firmware-signed.bin" 5
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
  ; VersionMajor/Minor from the makensis defines (see above) —
  ; same tag version the smoke test asserts, so write and check
  ; can never disagree.
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RSH" "VersionMajor" ${VERSIONMAJOR}
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\RSH" "VersionMinor" ${VERSIONMINOR}
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
    MessageBox MB_ICONEXCLAMATION "RSH installed, but the Windows Firewall rule could not be created (netsh exit $0). The wheel may not connect over Wi-Fi. Next step: Windows Security -> Firewall & network protection -> Allow an app through firewall -> add rsh-daemon.exe; or run the installer again as administrator." /SD IDOK
  ${EndIf}
  ; v2.0.19: the GameInput RacingWheel mappings are gone — only
  ; the device-1 envelope runs now (every install, fresh and
  ; update). Then the stale-OEMData cleanup below: the Steam FH4
  ; mode (v2.0.15) wrote an OEMData value on the vJoy key that
  ; can confuse the Microsoft Store build of FH4, so every
  ; install deletes it unconditionally (HKLM half; the daemon
  ; deletes the HKCU half on first start).
  Call ConfigureVJoyDevice1
  Call DeleteStaleOemData
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
  ; Stale sidecar exes from a <= v2.0.18 install can still be
  ; running — kill them too so the old files are never locked
  ; during uninstall (the sidecar was removed in v2.0.19).
  nsExec::Exec 'taskkill /F /IM RshCoreHiDcr.exe'
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
  ; v2.0.19: remove the GameInput RacingWheel mappings this
  ; installer wrote in <= v2.0.18 — a stale mapping would
  ; classify a device that no longer has RSH behind it.
  DeleteRegKey HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1234BEAD00010004"
  DeleteRegKey HKLM "SYSTEM\CurrentControlSet\Control\GameInput\Devices\1209525300010004"
  ; v2.0.19: remove the stale OEMData value unconditionally —
  ; the Steam FH4 mode (v2.0.15) is deleted, and its marker
  ; (HKLM\Software\RSH "SteamFh4OemData") is gone with it.
  ; Only the VALUE is deleted, never the OEM key (it is
  ; vJoy's). The HKCU delete is best-effort in the same-user
  ; context only: an elevated uninstaller cannot reach a
  ; standard user's hive (P1-4). A missing value is fine
  ; (DeleteRegValue on an absent value is a no-op).
  DeleteRegValue HKLM "SYSTEM\CurrentControlSet\Control\MediaProperties\PrivateProperties\Joystick\OEM\VID_1234&PID_BEAD" "OEMData"
  DeleteRegValue HKCU "SYSTEM\CurrentControlSet\Control\MediaProperties\PrivateProperties\Joystick\OEM\VID_1234&PID_BEAD" "OEMData"
  ClearErrors
  SetRegView 32
  Delete "$INSTDIR\rsh-daemon.exe"
  Delete "$INSTDIR\rsh-ui.exe"
  Delete "$INSTDIR\ViGEmClient.dll"
  Delete "$INSTDIR\ViGEmBus_1.22.0_x64_x86_arm64.exe"
  ; Stale sidecar files from a <= v2.0.18 install (removed
  ; in v2.0.19) — delete them too so no stale sidecar
  ; survives an upgrade.
  Delete "$INSTDIR\RshCoreHiDcr.exe"
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
  Delete "$INSTDIR\ViGEmClient.dll"
  Delete "$INSTDIR\ViGEmBus_1.22.0_x64_x86_arm64.exe"
  ; Stale sidecar files from a <= v2.0.18 install, retry pass.
  Delete "$INSTDIR\RshCoreHiDcr.exe"
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
  MessageBox MB_ICONEXCLAMATION "RSH could not be fully removed: rsh-daemon.exe is still in use (the app may still be running). Close RSH and run the uninstaller again." /SD IDOK
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

; Components-page descriptions (MUI2): shown when a section is
; selected on the components page.
!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_RSH} "RSH — the wheel app, daemon, and firmware."
!insertmacro MUI_FUNCTION_DESCRIPTION_END
