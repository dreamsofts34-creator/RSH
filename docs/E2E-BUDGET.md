# RSH — E2E Latency Budget (SW-13, Phase C)

Status: **CONTRACT — Phase C deliverable.** Authority: Plan v2.3 §3 (SLOs),
`docs/PROTOCOL-V2.1.md` §9 (ping frame + security rules).
E2E latency **sirf NTP-style ping se** measure hoti hai (R-H2). Jab tak ping
na ho, metric **"unmeasured"** hai — jhoota number kabhi nahi.

## 1. SLO

| SLO | Value | Kaise maapa jayega |
|---|---|---|
| End-to-end latency (Wi-Fi) | p99 ≤ 10 ms | NTP-style ping delay, p99 (aakhri ≤128 samples) |
| End-to-end latency (wired, Phase G) | p99 ≤ 5 ms | Wahi ping, wired link par |
| Packet inter-arrival jitter (PC receive) | p99 ≤ 1 ms | Receive thread timestamps (ping se nahi — ye alag metric hai) |
| Output update latency (packet-in → vJoy-out) | p99 ≤ 2 ms | Engine timestamps (ping se nahi) |

**"E2E" ka matlab yahan:** ping delay = `(t3 − t0) − (t2 − t1)` — network
round-trip minus board processing. Ye **network** ka E2E hai (PC → board →
PC). Poora input chain (neeche §2) is se bara hota hai — ping sirf network
hissa naapta hai; baqi hisse apne SLOs se naapte hain. Ping delay ko poora
chain samajhna ghalti hogi.

## 2. Per-stage latency budget (200 Hz input chain)

| Stage | Budget | Notes |
|---|---|---|
| Firmware tick (ADC read → packet send) | ≤ 5 ms (1 tick @ 200 Hz) | `maxCycleUs < 3500 µs` (D15) — tick ke andar cycle 3.5 ms se kam |
| Network (board → PC, ek taraf) | Wi-Fi p99 ≤ 5 ms; wired p99 ≤ 2.5 ms | Ping delay ka aadha (symmetric maan kar); SLO §1 se: ping p99 ≤ 10 ms Wi-Fi |
| Engine tick (receive → pipeline → output) | p99 ≤ 2 ms | Output latency SLO (§1) — engine 200 Hz, 5 ms tick |
| vJoy feed → game DirectInput poll | game par munhasir (typically 60 Hz ≈ 16.7 ms) | RSH ke control se bahir — budget mein nahi, sirf note |
| **Ping-measured E2E (network round-trip)** | **p99 ≤ 10 ms Wi-Fi / ≤ 5 ms wired** | §1 SLO — isi par gate lagta hai |

Per-stage budget ka matlab: agar ping p99 SLO se upar jaye to pehle network
hissa (Wi-Fi latency/loss/reorder — rig `chaos-net` scenario) dekho; agar
engine tick p99 SLO se upar jaye to pipeline/output dekho. Dono ko ek
number mein mila kar "E2E kharab hai" kehna ghalat diagnosis hai.

## 3. Ping se measure karne ka tareeqa

1. `NtpPingClient` (1 Hz loop, `PingLoopAsync`) board ko ping bhejta hai —
   port 4212, magic `"RSHN"`, fixed 34 B (PROTOCOL-V2.1.md §9).
2. Har reply se `PingResult` banta hai: `delay = (t3−t0) − (t2−t1)`,
   `offset = ((t1−t0) + (t2−t3)) / 2`.
3. `E2ELatency` (client ki `Latency` property) aakhri ≤128 delays rakhta hai;
   p50/p99 usi se nikalte hain. Panel sirf `DisplayText` parhta hai:
   `"unmeasured"` ya `"p99 X.X ms (n=N)"`.
4. SLO check: `MeetsSlo(wired)` — unmeasured ho to hamesha `false`.
5. Board firmware abhi Phase D mein aayega — tab tak virtual wheel
   (`test-env/virtual-wheel/wheel.py`) ping reply deta hai aur rig scenario
   `ntp-ping` isay assert karta hai (delay p99 ≤ 10 ms loopback par,
   samples ≥ 25, timeouts = 0). **Rig note:** asal (production) ping port
   4212 hai, lekin rig har run mein wheel + harness dono ko ek ephemeral
   ping port deta hai (`run_rig.py` — stream port jaisa, T10), taake
   concurrent rig runs 4212 par na takraein.

**Kya measure NAHI hota:** ping firmware tick time nahi naapta (board
processing `t2−t1` sirf diagnostics hai, SLO mein nahi), na game-side poll
delay. Inter-arrival jitter ping se nahi nikalta — wo receive timestamps se hai.

## 4. Security (SEC-C-21/22/24/25 — gate ka hissa)

Ping unauthenticated UDP hai (measurement hai, control nahi) — is liye ye
rules contract ka hissa hain, optional nahi (poori tafseel PROTOCOL-V2.1.md §9):

- **Board rate limit (SEC-C-21, High):** per-source token bucket —
  max 10 replies/s, burst 5; zyada ping khamoshi se drop (koi error reply
  nahi). Key = source IP. Table bounded (64 sources). Reference:
  `RSH.Core/Transport/PingRateLimiter.cs` (wahi numbers, unit tests).
  Firmware Phase D mein is se match karega; virtual wheel mein wahi rule hai.
- **Reflection (SEC-C-22, Medium):** reply hamesha ≤ request (dono fixed
  34 B — amplification ≤ 1); reply sirf packet ke asal source address/port
  par (payload mein address ka field hai hi nahi). HELO-gating **nahi** li
  gayi (ping pairing se pehle bhi chalna hai) — faisla §9.5 mein darj hai.
- **PC flood guard (SEC-C-24, Low):** sender 1–4 Hz (loop 1 Hz; client
  250 ms min interval enforce karta hai — tez koshish throttle hoti hai,
  packet bhejta hi nahi). Ephemeral port; timeout 500 ms, koi retry nahi;
  ping UI thread par kabhi nahi; raw payload kabhi log nahi hota.
- **Offset untrusted (SEC-C-25, Low):** clock offset **sirf display** ke
  liye hai — is se kabhi auth / replay / expiry / koi security faisla nahi
  hoga. Ping HELO ka 2 s silence timer reset nahi karta aur koi state/config
  nahi badalta. Ping timestamps untrusted measurement hain.
- **Fixed size, koi secret nahi (SEC-C-23):** reply mein sirf seq + 4
  timestamps — koi key, nonce, password, uptime nahi; koi variable-size
  token nahi (seq u16 hi echo hai).

## 5. Honest limits

- Loopback/virtual-wheel numbers **real Wi-Fi nahi hain** — asal 2.4 GHz
  interference, RSSI, router behaviour sirf real board + real Wi-Fi par
  naapte hain (final production release par).
- Sandbox VM mein jitter/latency par scheduling ka asar hota hai — rig
  thresholds is liye sandbox ke mutabiq hain; real target §1 mein hai.
- Offset ka matlab sirf "board clock PC clock se kitna aage/peeche hai" —
  is se board ka wall-clock waqt nahi nikalta aur na ye NTP sync hai.
