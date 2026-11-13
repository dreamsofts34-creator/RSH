# Performance baseline — OPTIONAL (not required)

> **Decision:** the owner cannot take measurements and the dev sandbox cannot run .NET/Windows, so no numeric baseline is required. Performance is protected by construction (software plan §0.3, firmware plan §0). This file and `tools-perf/` are kept only in case someone wants numbers later.

**Why:** v1.2.2 is the reference ("tested, works, optimized"). Every later phase is compared to these numbers (software plan §0.3, firmware plan §0). The sandbox used for development has no .NET SDK or Windows, so **these numbers must be measured on your PC**. Nothing below is measured yet.

## A. App (Windows PC) — 3 commands

1. **Engine + pipeline benchmark** (needs .NET 10 SDK):
   `dotnet run -c Release --project software/tools/RSH.Benchmarks -- --soak-sec 600`
   Copy the printed lines (Parse, Pipe, e2e loopback, soak memory).
2. **Idle footprint** (run v1.2.2 as you normally use it, wheel connected if possible):
   `powershell -ExecutionPolicy Bypass -File tools-perf\Measure-RSH.ps1 -ExePath "<path>\RSH.exe" -IdleSeconds 120`
   Produces `perf-baseline-<time>.txt` (startup ms, idle CPU, memory, threads).
3. **While driving/moving the wheel** (30 s): note Task Manager CPU % and whether any UI hitch/lag is visible on Dashboard/Monitor pages (yes/no).

## B. Firmware (real board)

Set `RSH_DEBUG 1` in `config.h`, flash, open serial monitor 115200 for 10 min with the app connected. Record from the 1 Hz line: `sent/s` (should be ≈200), `overrun` per window, `maxCycleUs`, steering std-dev. Put debug back to 0 afterwards.

## C. Results table (fill in)

| Metric | v1.2.2 baseline | v2 budget |
|---|---|---|
| Engine tick avg / max (µs) | | avg ≤ +15 % |
| ns per packet parse / pipeline tick | | ≤ +15 % |
| Allocations per tick | | **0** |
| Loopback loss (%) | | 0 |
| 10-min soak memory growth | | flat |
| Startup to window (ms) | | ≤ +300 ms |
| Idle CPU (% total) | | ≤ baseline + 0.5 pt |
| Working set (MB) | | ≤ +25 % (or +40 MB) |
| UI hitches while driving | | none > 50 ms |
| Firmware sent/s, overruns/10 min, maxCycleUs | | no worse |
| Firmware flash % / RAM % (v3.3: 71.4 % / 14.5 %) | | flash < 85 % |
