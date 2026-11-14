#pragma once
// RSH quadrature encoders — FW-12 (Plan v2.3 C3 fix: i16 cumulative, drift nahi).
// Authority: PROTOCOL-V2.1.md §1.1 — har encoder ka value boot se cumulative
// count hai (i16, wrap allowed — PC delta `new - old` ko i16 wrap ke saath
// nikale). v2 draft ke i8 detents KHATAM.
//
// Ye file Arduino se free hai taake host par test ho sake: caller A/B
// levels deta hai (digitalRead ke baad), decoder cumulative count rakhta hai.
//
// Noise/drift rule: sirf ek bit badle wali transition count hoti hai.
// Dono bits ek saath badlen (bounce / glitch / dono channel ek saath
// switch) to count NAHI badalta — is liye bounce se drift nahi aata.
// 100 turns aage + 100 turns peeche = wapas exact 0 (host test).
//
// Debounce (Phase F WS5 — reversal-cancellation, 1-tick window):
// A/B 5 ms tick par sample hote hain, is liye bounce aksar ek hi
// transition + foran ulta transition nazar aata hai (00 -> 10 -> 00).
// Aisa jora count mein mehfooz NAHI rehta: jab transition foran ulta
// ho (wapas usi state par jis se aaya tha) to pichla step UNDO ho jata
// hai (glitches++). Ye asal reversal se farq nahi karta — aur farq ki
// zaroorat bhi nahi: dono surat mein net displacement wahi hai (wapas
// origin par = net 0; phir nayi disha mein chalna sahi count hota hai).
// Tez ghumane par (musalsal ek hi disha) koi step undo nahi hota —
// sirf foran ulta hone wala step cancel hota hai, is liye fast rotation
// mein undercount nahi hota (2-sample persistence filter ulta karta —
// wo har step ko kha jata jab state har tick badle). `glitches` gin
// liye gaye cancelled/invalid transitions hain (diagnostics).
#include <stdint.h>

typedef struct {
  int16_t count;   // cumulative, i16 wrap allowed (PC wrap-aware delta nikale)
  uint8_t prev;    // pichli A/B state: (A<<1)|B, 0..3
  uint8_t seen;    // pehla sample mil gaya?
  uint8_t prevPrev;  // transition se pehle wali state (reversal pehchanne ke liye)
  int8_t lastDelta;  // aakhri accepted transition ka delta (-1/0/+1)
  uint32_t glitches;  // cancelled reversals + invalid transitions (diagnostics)
} RshEncoder;

static inline void rsh_encoder_init(RshEncoder *e) {
  e->count = 0;
  e->prev = 0;
  e->seen = 0;
  e->prevPrev = 0;
  e->lastDelta = 0;
  e->glitches = 0;
}

// Ek A/B sample do (a, b = 0/1 levels). Pehla sample sirf state set karta
// hai (count nahi badalta — boot par aadhi position se start ho to bhi
// drift nahi).
static inline void rsh_encoder_step(RshEncoder *e, uint8_t a, uint8_t b) {
  uint8_t cur = (uint8_t)(((a & 1u) << 1) | (b & 1u));
  if (!e->seen) {
    e->prev = cur;
    e->seen = 1;
    return;
  }
  if (cur == e->prev) return;   // koi harkat nahi
  // Valid single-bit transitions (quadrature cycle):
  // 00 -> 10 -> 11 -> 01 -> 00 = +1 har qadam; ulta = -1.
  // Dono bits badlen (00<->11, 01<->10) = invalid/glitch -> 0 (drift nahi).
  static const int8_t kDelta[4][4] = {
      // new: 00   01   10   11
      /*00*/ {0, -1, +1, 0},
      /*01*/ {+1, 0, 0, -1},
      /*10*/ {-1, 0, 0, +1},
      /*11*/ {0, +1, -1, 0},
  };
  int8_t d = kDelta[e->prev][cur];
  if (d == 0) {
    // Invalid transition (dono bits badle): state update karo (aage ke
    // steps sahi state se chalein), count na badlo, lastDelta bhool jao
    // (agli transition is ke reversal ke tor par na gini jaye).
    e->glitches++;
    e->prev = cur;
    e->lastDelta = 0;
    return;
  }
  if (d == (int8_t)-e->lastDelta && cur == e->prevPrev) {
    // Foran reversal (bounce jora): pichla step undo karo — net 0.
    // Misal: 00 -> 10 (+1) -> 00 (-1): count wapas 0, glitch gina gaya.
    e->count = (int16_t)(e->count - e->lastDelta);
    e->glitches++;
    e->lastDelta = 0;
    e->prev = cur;
    return;
  }
  e->prevPrev = e->prev;
  e->count = (int16_t)(e->count + d);
  e->lastDelta = d;
  e->prev = cur;
}

// i16 wrap-aware delta: new - old, -32768..32767 mein. PC side bhi yahi
// formula hai (WheelPacketV2 encoders i16 cumulative hain).
static inline int16_t rsh_encoder_delta(int16_t now, int16_t prev) {
  return (int16_t)((uint16_t)now - (uint16_t)prev);
}
