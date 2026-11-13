# T9 — Half-range decision: 32767 lost vs reboot

**Date:** 2026-11-13 (Phase C, Researchers team)
**Code:** `software/src/RSH.Core/Protocol/SequenceTracker.cs`
(`RebaselineThreshold`, `RebootWindow`)
**Related:** P3 (re-baseline, Phase B), `docs/PROTOCOL-V2.1.md` §1,
plan v2.3 §0 D10 / §5 Phase C.

## Masla

Sequence numbers u16 hain (0–65535). Tracker signed delta istemal karta
hai: `delta = (short)(seq - _last)`. Is ki had **half-range 32768**
hai:

- Aage ki chhalang `1..32767` → delta positive → `delta - 1` lost gina jata hai.
- Aage ki chhalang **bilkul 32768** → `(short)32768 = -32768` → negative
  delta → reboot samajh kar re-baseline hoti hai, loss nahi gina jata.
- Peeche ki chhalang `delta < -32` → reboot (P3, Phase B).

Sawal: 32768 ke aas-paas ki chhalang **asal loss** hai ya **reboot**?
Dono ka farq sirf yahi hai ke reboot par sequence 0 se shuru hoti hai
aur loss par sequence wahi se aage barhti hai — aik packet dekh kar
farq nahi bataya ja sakta. Faisla zaroori tha, warna aik case mein
LossPercent explode karta hai aur doosre mein reboot pehchanna nahi jata.

## Faisla

1. **Half-range par reboot jeetta hai.** Aage ki chhalang 32768 (delta
   −32768) **reboot** hai — re-baseline, 0 lost. Chhalang 32767 tak
   **loss** hai (`delta − 1` lost).
2. **Reboot-window heuristic (Phase C mein naya).** Aage ki chhalang
   `delta > 1024` jo sequence `≤ 1024` par utre, woh bhi **reboot**
   hai — chahe delta half-range se chhota ho. Reboot hamesha 0 se
   shuru hota hai; aam loss 0 ke qareeb utarne ke liye wrap ke paar
   girna chahiye.
3. **Asal loss ka tareeqa nahi badla.** Bari chhalang jo 0 ke qareeb
   na utre (masalan `_last=100` → `seq=2000`, 1899 lost) pehle jaisi
   hi loss gini jati hai.

## Reasoning

- **32768 lost packets na-mumkin hain jab baseline zinda ho.**
  200 Hz par 32768 packets = **~164 s** ka waqfa. Itni lambi
  khamoshi par `UdpWheelSource` ka **5 s backup reset** pehle hi
  baseline saaf kar chuka hota hai — tracker tak aisi chhalang kabhi
  pohanchti hi nahi. Is liye 32768 ki chhalang dekh kar "reboot" kehna
  ghalat nahi ho sakta; "32767 lost" kehna ghalat hota.
- **Reboot ka waahid seedha nishan 0 hai.** Board reboot par sequence
  0 se shuru karta hai aur PC 200 Hz par ~5 s ke andar dobara sunne
  lagta hai — is liye pehla post-reboot packet `seq ≤ 1024` hota hai
  (1024 packets ≈ 5.1 s). Yehi `RebootWindow = 1024` hai.
- **Ye heuristic kyun zaroori tha — mila hua bug.** Baseline jab
  oopri half-range mein ho (masalan `_last=40000`), to reboot par
  `seq=0` ka signed delta **+25536** banta hai — negative-delta wali
  P3 shart nahi pakarti aur **25535 phantom lost** gin liye jate thay.
  Long run (seq har ~327 s mein wrap hota hai) mein har ~doosre
  reboot par LossPercent explode karta. Heuristic ye case pakarta hai.
- **Heuristic ki had janti hui hai (neeche).** Ulta case — asal loss
  jo 0 ke qareeb utre — ke liye ≥ 5 s ki khamoshi chahiye hoti hai,
  aur woh 5 s backup reset pehle hi pakar leta hai. Dono dafa (backup
  + heuristic) mil kar har case cover karte hain.

## Asar aur residual (qubool-shuda)

| Case | Nateeja |
|---|---|
| Reboot, `_last ≤ 32768` (aam) | Negative delta → re-baseline, 0 lost, 0 duplicate (P3 jaisa) |
| Reboot, `_last > 32768`, pehla packet `seq ≤ 1024` | Heuristic → re-baseline, 0 lost (Phase C fix) |
| Reboot, `_last` wrap ke 1024 ke andar (masalan 65530 → 0) | Delta chhota (+6) → **5 phantom lost** gine jate hain; silsila khud theek ho jata hai |
| Reboot, pehla packet `seq > 1024` mile | 5 s backup reset (UdpWheelSource) baseline saaf karta hai — tracker tak nahi pohanchta |
| Asal loss `1..32767`, 0 se door utre | Loss gina jata hai (pehle jaisa) |
| Asal loss bilkul 32768 | Reboot samajha jata hai — lekin 164 s waqfa backup se pehle hi reset ho chuka hota hai, is liye kabhi hota nahi |
| Wrap (65534 → 65535 → 0 → 1) | 0 lost, 0 duplicate — sahi handle hota hai (test se verify) |

Phantom loss ki oopri had: reboot par zyada se zyada **1024** (jab
`_last` wrap ke 1024 ke andar ho); aam tor par 0. LossPercent (last
1000 ki sliding window) par is ka asar na hone ke barabar hai.

## Tests

`software/tests/RSH.Tests/SequenceTrackerTests.cs`:

- `Wraparound_NotCountedAsLossOrDuplicate` — 65534 → 65535 → 0 → 1 clean.
- `Wraparound_WithLossStillCounts` — wrap ke paar gap (65500 → 100) = 135 lost.
- `Wraparound_DuplicateAfterWrap_CountsAsDuplicate` — wrap ke paar duplicate/late.
- `Reboot_UpperHalfBaseline_RebaselinesNotPhantomLoss` — bug repro: 40000 → 0, 0 lost (pehle 25535).
- `Reboot_UpperHalfBaseline_MissedFirstPackets_StillRebaselines` — 40000 → 500, phir bhi reboot.
- `Reboot_NearWrap_SmallPhantomLoss_AcceptedResidual` — 65530 → 0, 5 phantom lost (qubool-shuda).
- `HalfRange_Exact32768_RebaselinesNotLoss` — had: 32768 → reboot, 0 lost.
- `HalfRange_JustBelow32767_CountsAsLoss` — had ke neeche: 32767 → 32766 lost.
- `GenuineLoss_LandingAwayFromZero_CountsAsLoss` — 100 → 2000, 1899 lost.
- Purane P3 tests (`Reboot_SequenceRestartsAtZero_*`,
  `SmallBackwardJump_*`, `BackwardJumpBeyondThreshold_*`) waisi hi
  pass hote hain — faisle se un ka behaviour nahi badla.

## Code change

Sirf `SequenceTracker.Observe` mein aik shart barhi (heuristic);
negative-delta P3 rule aur loss gin'na waisa hi hai. Koi API change
nahi — `LinkStatistics.Record` aur `UdpWheelSource` ko chhede baghair
fix hua.
