#pragma once
#include <stdint.h>
#include <stddef.h>

// SoftAP + HELO handshake + unicast data (docs: 02/03)
// Protocol: PC har ~1 s "HELO" bhejta hai port 4210 par; ESP bhejne wale ka IP
// yaad rakh kar data usi IP ke port 4211 par bhejta hai. HELLO_TIMEOUT_MS tak
// HELO na aye to bhejna band (fail-safe: purana PC mar gaya ho sakta hai).
//
// v2.2 (no-pairing): AP khula hai jab koi password stored nahi, warna
// WPA2-PSK (SSID "RSH-XXXX", D4). Control plain hai (9 B header + CRC16,
// reqId-only dedup); stream gate plain GET_CAPS OK hai (D3).

bool net_begin();                    // false = SoftAP start fail (error state)
void net_poll();                     // loop mein har dafa — tamam pending HELO drain karo
void net_poll_ctrl();                // FW-8: port 4212 drain — control + ping demux
void net_led_tick(uint32_t nowMs);   // Phase H WS2: LED strip driver tick (200 Hz loop se — rsh.ino)
void net_maybe_rescan(uint32_t nowMs);  // FW-7/F8: degraded link par rescan (60 s+ waqfa)
bool net_pc_active(uint32_t nowMs);  // HELO timeout ke andar hai?
bool net_pc_is_v2(uint32_t nowMs);   // aakhri HELO proto 0x02 tha (v2.2 peer)
bool net_caps_ok(uint32_t nowMs);    // D3: is session mein GET_CAPS OK hua + PC active?
bool net_send(const uint8_t *buf, size_t len);  // PC ko unicast; PC na ho to false

// ---- Wired USB-serial transport (Phase G, D26 / OPEN-20) ----
// Serial peer Wi-Fi peer se ALAG session hai: apna HELO timer, apna
// control session/cache, apna GET_CAPS-OK flag. Har transport par naya
// session (HELO sessionFlag=1) SIRF usi transport ka session reset
// karta hai (R3-FW-11: pehle dono reset hote thay). Wi-Fi par naya peer
// ya liveness timeout ke baad wapas aaya peer bhi Wi-Fi session reset
// karta hai (R3-FW-02, net_logic.h rsh_session_reset_needed).
// Ye functions sirf serial_link.cpp se bulaye jate hain; framing
// (serial_transport.h) us ki taraf hai.
void net_serial_begin();  // serial control session/cache init (net_begin se)
size_t net_serial_helo(const uint8_t *helo, size_t len, uint32_t nowMs,
                       uint8_t *hello_out);  // HELO → HELLO reply bytes (92) ya 0
bool net_serial_pc_active(uint32_t nowMs);    // serial HELO timeout ke andar?
bool net_serial_pc_is_v2(uint32_t nowMs);     // serial peer v2.2 hai?
bool net_serial_caps_ok(uint32_t nowMs);      // D3 (serial)
bool net_serial_dispatch_control(const uint8_t *frame, size_t len,
                                 uint8_t *reply_out, size_t reply_out_len,
                                 size_t *reply_len_out);  // plain control over serial
size_t net_serial_ping(const uint8_t *req, int len, uint8_t *reply_out);  // ping reply (34) ya 0 (drop)
void net_serial_after_reply();  // reply bhejne KE BAAD deferred actions (REBOOT/SET_PASSWORD)
uint32_t net_tx_fail_count();        // F3: boot se failed UDP transmissions
uint32_t net_tx_sent_count();        // F3: boot se kamyab UDP transmissions
int8_t net_rssi_dbm();               // Strongest associated station ka RSSI (dBm, R3-FW-09 — PC ka nahi); -128 = unknown
uint8_t net_channel();               // current SoftAP channel (0 = AP down)
void net_set_stream_busy(bool busy); // ping drop-when-busy ke liye (stream loop peeche ho)
uint32_t net_ping_dropped_count();   // §9.4: rate-limit/busy par drop hue pings
// v4.0.2:
// HORN_TEST (D-5) abhi active hai? (rsh.ino har sample par poochta hai.)
uint8_t net_horn_test_active(uint32_t nowMs);
// Persistent LED config (F-14) current blob se driver par lagao
// (rsh.ino dirty-flag apply se bulata hai).
void net_led_apply_persistent(const uint8_t *blob, size_t len);
