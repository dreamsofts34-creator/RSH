# Anti-Cheat Note (R-L6)

> **Short version:** RSH injects nothing into any game, writes
> nothing into any game's memory, and sends no input to any game
> except through a normal virtual joystick device — the same kind of
> device any physical wheel presents. There is nothing to hide and
> nothing hidden.

## 1. What RSH does

- **Reads telemetry, read-only.** Assetto Corsa, ACC, and iRacing
  publish their physics/graphics/session pages in shared memory
  (memory-mapped files the games create for any companion app).
  RSH opens them with `OpenFileMappingW` → `MapViewOfFile` with
  `FILE_MAP_READ`, copies the structs it needs, and unmaps. The
  mapping is read-only by construction: the game memory is never
  written, and the reader never touches the game process itself
  (no `OpenProcess`, no handle to the game at all). When no game is
  running the reader idles — it never crashes, never retries
  aggressively, never scans memory.
- **Sends wheel input through a virtual joystick device.** The
  processed wheel state (steering, pedals, buttons, gears) goes to
  the output backend — vJoy or HIDMaestro — which presents a
  standard DirectInput/XInput joystick to Windows. Games see a
  joystick, exactly as they would see a physical wheel. The app
  never synthesises keystrokes, mouse input, or game-specific input
  events.
- **Talks to its own board.** The ESP32 wheel talks to the app over
  Wi-Fi (UDP, HMAC-authenticated per protocol v2.1) or USB-serial.
  That traffic is between the app and its own hardware only.

## 2. What RSH never does

- **No injection of any kind:** no DLL injection, no code injection,
  no thread creation inside a game process, no hooking of game
  functions, no patching of game code or data.
- **No memory writes to a game:** no `WriteProcessMemory`, no
  writable mapping, no modification of telemetry pages, car data,
  timing data, or any other game state.
- **No game-traffic interception:** no packet sniffing, no proxying,
  no modification of network traffic between the game and its
  servers.
- **No hiding:** no rootkit behaviour, no process hiding, no
  attempt to evade detection. The app runs as a normal user-mode
  process with a visible window, tray icon, and taskbar entry; the
  virtual joystick appears in Windows' game-controller list like
  any other device.
- **No kernel driver of its own:** RSH ships no driver. vJoy /
  HIDMaestro are the output backends and their drivers are
  installed by the user from their own vendors; RSH only calls
  their published user-mode APIs (vJoy DLL resolved from the
  registry / Program Files, SHA-256 pinned, never bundled).
- **No automation of gameplay:** no macros, no auto-driving, no
  reading of other players' data, no reaction-time or input
  automation. The wheel input is the user's hands, sampled at
  200 Hz.

## 3. What an anti-cheat sees

From an anti-cheat's point of view RSH is indistinguishable from a
normal wheel setup: a user-mode companion app reading the game's
own public shared-memory telemetry (the mechanism the games
document for companion apps) and a virtual joystick device
delivering input. There is no injected module in the game process
to detect, because there is none.

That said, RSH makes **no guarantee** about any specific game's
anti-cheat policy: some online services restrict virtual input
devices or companion apps by policy, not by detection, and those
policies change. If a game's rules forbid virtual joysticks, the
user must follow the game's rules — RSH will not help evade them,
and no "undetectable" mode exists or is planned.

## 4. If a game flags RSH

The honest report to send is: "RSH is a sim-racing wheel companion
that reads the game's public shared-memory telemetry read-only and
presents input as a standard virtual joystick (vJoy/HIDMaestro).
It does not inject code or write game memory." The diagnostics
export (see `docs/DIAGNOSTICS.md`) contains no game data and no
telemetry contents — only app versions and redacted logs — so there
is nothing in a support bundle that could be used to probe a game.
