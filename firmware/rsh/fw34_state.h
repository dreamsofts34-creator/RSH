#pragma once
// FW-3/FW-4 state accessors (FW-7/8 wiring follow-up).
// State khud rsh.ino mein rehti hai (boot counter / safe mode / config
// txn — FW-3/FW-4 Workstream 1); net.cpp aur keystore.cpp is header se
// us tak pahunchte hain. Ye header Arduino-free hai (sirf declarations).
// Definitions rsh.ino mein hain (non-static). BSS zero-init ki wajah se
// setup() se pehle bhi safe hain (safeMode.active = 0 → not armed).
#include <stdint.h>
#include "config_transaction.h"

// Safe mode active hai? (HELLO flags bit2 + GET_CAPS capsFlags bit1.)
uint8_t rsh_fw34_safe_mode_active(void);

// Key-reset-armed hai? (BOOT-hold >= 10 s, 120 s window ke andar.)
// HELLO/GET_CAPS flags + KEY_RESET gate — yahi ek source of truth hai.
uint8_t rsh_fw34_key_reset_armed(void);

// Live config transaction (rsh.ino ka cfgTxn). FW-8 handler is par
// stage/commit/confirm/revert chalata hai.
rsh_cfg_txn_t *rsh_fw34_cfg_txn(void);

// Config persist karo (NVS "rsh-cfg"): current hamesha, previous sirf
// jab has_previous ho (COMMIT ke baad), pend flag caller ke mutabiq.
// COMMIT ke baad pending=1; CONFIRM/REVERT/rollback ke baad pending=0.
void rsh_fw34_cfg_persist(uint8_t pending);
