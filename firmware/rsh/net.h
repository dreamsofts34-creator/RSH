#pragma once
#include <stdint.h>
#include <stddef.h>

// SoftAP + HELO handshake + unicast data (docs: 02/03)
// Protocol: PC har ~1 s "HELO" bhejta hai port 4210 par; ESP bhejne wale ka IP
// yaad rakh kar data usi IP ke port 4211 par bhejta hai. HELLO_TIMEOUT_MS tak
// HELO na aye to bhejna band (fail-safe: purana PC mar gaya ho sakta hai).

bool net_begin();                    // false = SoftAP start fail (error state)
void net_poll();                     // loop mein har dafa — tamam pending HELO drain karo
bool net_pc_active(uint32_t nowMs);  // HELO timeout ke andar hai?
bool net_send(const uint8_t *buf, size_t len);  // PC ko unicast; PC na ho to false
bool net_restart_requested();        // PC ne "RSTX" bheja (remote restart)
