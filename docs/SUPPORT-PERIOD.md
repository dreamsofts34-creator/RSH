# RSH — Support period statement (v2.0.0)

**Version 2.0.0 — app v2.0.0 · firmware v4.0.0 · protocol v2.1**
Authority: Plan v2.3 §5 Phase H + §6 (support period likho).

## Statement

- **RSH v2.0.0** (app v2.0.0 + firmware v4.0.0) ko release date se **12
  mahine** tak support milega: bug fixes aur security fixes is version
  line (v2.0.x / fw v4.0.x) mein aayenge.
- **Security fixes** (jaise signing-key ya protocol issue) ko priority
  milegi — agar signing key compromise ho to response
  `docs/KEY-COMPROMISE-RESPONSE.md` (OPEN-19, Phase H) ke mutabiq hoga.
- **12 mahine ke baad:** nayi features sirf agli major/minor line mein
  aayengi; v2.0.x ko sirf critical security fix mil sakta hai (agar
  mile to release notes mein likha jayega).
- **.NET 10 LTS** 14 Nov 2028 tak supported hai; RSH us ke baad runtime
  update karega (agli release mein).
- **Windows floor:** Windows 11 (64-bit, x64). Windows 10 EOL hai
  (14 Oct 2025) — Windows 10 par best-effort, koi guarantee nahi.
- **Firmware v3.3 (purani line):** v4.0.0 aate hi v3.3 ko koi update nahi
  milega — upgrade order: pehle app v2.0.0, phir firmware v4.0.0
  (signed OTA, R-M10).
- **Support channel:** GitHub Issues —
  https://github.com/dreamsofts34-creator/RSH/issues (bug reports +
  Diagnostics export `.zip` saath lagayein — USER-GUIDE §6).

Ye statement har release ke saath update hoti hai (nayi version = nayi
12-mahine window).
