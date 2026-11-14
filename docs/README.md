# RSH — Documentation Index

> **Versions:** App **v2.0.4** · Firmware **v4.0.1** · Protocol **v2.1**
> (2026-11-14). Start at the repo root
> [`README.md`](../README.md), then come back here.

## Start here

| File | What it is |
|---|---|
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | **System overview** — firmware + app + output, data flow, transports, config transactions, security summary, history |
| [`FIRMWARE.md`](FIRMWARE.md) | Firmware architecture — 200 Hz loop, source map, pins, safe mode, signed OTA |
| [`SOFTWARE.md`](SOFTWARE.md) | PC app architecture — engine, pipeline, transports, profiles, UI, build |
| [`PROTOCOL.md`](PROTOCOL.md) | Protocol v2.1, readable summary (authority: `PROTOCOL-V2.1.md`) |
| [`SECURITY.md`](SECURITY.md) | Security model, key lifecycle summary, open items |
| [`TESTING.md`](TESTING.md) | Virtual Test Rig, suites, gates, CI, A7 run |

## Users

| File | What it is |
|---|---|
| [`USER-GUIDE.md`](USER-GUIDE.md) | User guide v2.0.0 (install, setup, use) |
| [`FLASH-GUIDE.md`](FLASH-GUIDE.md) | Flashing the board (first flash + updates) |
| [`RELEASE-NOTES-v2.0.4.md`](RELEASE-NOTES-v2.0.4.md) | Latest release notes — read this first after updating |
| [`RELEASE-NOTES-v2.0.0.md`](RELEASE-NOTES-v2.0.0.md) · [`RELEASE-NOTES-v2.0.1.md`](RELEASE-NOTES-v2.0.1.md) · [`RELEASE-NOTES-v2.0.2.md`](RELEASE-NOTES-v2.0.2.md) · [`RELEASE-NOTES-v2.0.3.md`](RELEASE-NOTES-v2.0.3.md) | Earlier release notes |
| [`SUPPORT-PERIOD.md`](SUPPORT-PERIOD.md) | Support period |
| [`THIRD-PARTY-LICENSES.md`](THIRD-PARTY-LICENSES.md) | Third-party licences (SBOM: `SBOM.cyclonedx.json`) |

## Reference (authority documents)

| File | What it is |
|---|---|
| [`PROTOCOL-V2.1.md`](PROTOCOL-V2.1.md) | **Wire protocol v2.1 — the authority** (1163 lines) |
| [`PIN-RULES.md`](PIN-RULES.md) | Pin capability rules + board JSON (`board-esp32-devkitv1.json`) |
| [`KEY-LIFECYCLE.md`](KEY-LIFECYCLE.md) | Per-device key lifecycle (full) |
| [`THREAT-MODEL.md`](THREAT-MODEL.md) | STRIDE threat model |
| [`DECISION-LOG.md`](DECISION-LOG.md) | Decision log D1–D35 |
| [`UI-LOCKED.md`](UI-LOCKED.md) | UI design — locked (8 screens, themes, resx) |
| [`E2E-BUDGET.md`](E2E-BUDGET.md) | Latency budget and measurements |
| [`PERF-BASELINE.md`](PERF-BASELINE.md) | Performance baseline |
| [`GAME-COMPATIBILITY.md`](GAME-COMPATIBILITY.md) | Game compatibility matrix (draft — real launches pending) |
| [`TELEMETRY-READER.md`](TELEMETRY-READER.md) | Per-car telemetry (AC / ACC / iRacing) |
| [`HANDBRAKE-MAPPING.md`](HANDBRAKE-MAPPING.md) | Handbrake → vJoy WSlider |
| [`HIDMAESTRO-EVALUATION.md`](HIDMAESTRO-EVALUATION.md) | HIDMaestro backend evaluation |
| [`AS5600-SPEC.md`](AS5600-SPEC.md) | AS5600 magnetic encoder spec |
| [`EX3-EXTENSION.md`](EX3-EXTENSION.md) | Ex3 extension (buttons 97–128 gap) |

## Operations

| File | What it is |
|---|---|
| [`A7-RUNBOOK.md`](A7-RUNBOOK.md) | Real-board measured run (A7) — the freeze gate |
| [`A7-GATE-PROCEDURE.md`](A7-GATE-PROCEDURE.md) | A7 gate procedure |
| [`BRICK-RECOVERY.md`](BRICK-RECOVERY.md) | Brick recovery |
| [`FLASH-ENCRYPTION-PROCEDURE.md`](FLASH-ENCRYPTION-PROCEDURE.md) | Flash encryption + Secure Boot (board-side) |
| [`SIGNED-OTA.md`](SIGNED-OTA.md) · [`SIGNING.md`](SIGNING.md) | Signed OTA, signing keys and process |
| [`RELEASE-CHECKLIST.md`](RELEASE-CHECKLIST.md) | Release checklist |
| [`RELEASE-WORKFLOW-CHECK.md`](RELEASE-WORKFLOW-CHECK.md) | Release workflow checks |
| [`PIN-MAP-PUSH.md`](PIN-MAP-PUSH.md) | Pushing the pin map from PC to board |
| [`SAFE-MODE-CONFIG-TXN.md`](SAFE-MODE-CONFIG-TXN.md) | Safe mode + config transactions |
| [`SETUP-GATES-DECISION.md`](SETUP-GATES-DECISION.md) · [`T9-DECISION.md`](T9-DECISION.md) | Setup-gate and T9 decisions |

## Security reviews

| File | What it is |
|---|---|
| [`SECURITY-REVIEW-PHASE-C.md`](SECURITY-REVIEW-PHASE-C.md) | Phase C security review (24 findings) |
| [`SECURITY-REVIEW-PHASE-F-W1.md`](SECURITY-REVIEW-PHASE-F-W1.md) | Phase F security review |
| [`SECURITY-RESCAN-ROUND2-W5.md`](SECURITY-RESCAN-ROUND2-W5.md) | Round 2 re-scan — CONDITIONAL PASS, no Critical/High |

## History (superseded — read as record, not authority)

| File | What it is |
|---|---|
| [`PROTOCOL-V2.md`](PROTOCOL-V2.md) | Protocol v2 draft — **superseded by v2.1** |
| [`FIRMWARE-PLAN.md`](FIRMWARE-PLAN.md) | v1-era firmware plan — superseded |
| [`SOFTWARE-PLAN.md`](SOFTWARE-PLAN.md) · [`SOFTWARE-PLAN-v2.md`](SOFTWARE-PLAN-v2.md) | v1-era software plans — superseded |
| [`PROJECT.md`](PROJECT.md) | v1.0.0-era project notes — superseded (says so in its header) |
| [`DELIVERY-NOTE.md`](DELIVERY-NOTE.md) · [`FINAL-REPORT.md`](FINAL-REPORT.md) · [`TEST-REPORT.md`](TEST-REPORT.md) | v1.x delivery records |
| [`POLISH-SWEEP-2026-11-13.md`](POLISH-SWEEP-2026-11-13.md) | Round 1 polish sweep traceability |
| [`ROUND2-W7-DATA-AI.md`](ROUND2-W7-DATA-AI.md) | Round 2 data/AI workstream |
| [`PHASE-G-WS5-HANDOFF.md`](PHASE-G-WS5-HANDOFF.md) · [`PHASE-H-WS2-HANDOFF.md`](PHASE-H-WS2-HANDOFF.md) | Phase handoff notes |
| [`sim-wheel-docs/`](sim-wheel-docs/) | Original spec set (01–06) from the first handover |
| [`ui-reference/rsh-ui-preview.html`](ui-reference/rsh-ui-preview.html) | UI preview (visual spec only) |

**Note:** the old root `HANDOVER.md` (v1.2.2-era) was removed in
v2.0.4 — its surviving content (default pin map, "firmware dumb, PC
smart", delivery history) is merged into `ARCHITECTURE.md` and
`FIRMWARE.md`. The original handover package is preserved outside
this repo.

## Where the rest lives (not in this repo)

- **Development plan v2.3** (FINAL) and the **Virtual Test Rig**
  (`test-env/`) live in the project workspace, not in this repo —
  they are working documents, not shipped code.
- **Signing keys** live outside the repo by design and are never
  committed (`.gitignore` blocks `*.pem`).
