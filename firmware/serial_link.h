#pragma once
// RSH wired USB-serial link — Arduino glue (Phase G, D26 / OPEN-20).
// Framing (serial_transport.h) pure hai; yahan sirf Serial I/O aur
// net.cpp ke serial entry points se wiring hai.
//
// - begin: Serial.begin(RSH_SERIAL_BAUD) + net_serial_begin().
// - poll: Serial se bytes parho, decoder feed karo, poora frame mile to
//   net_serial_helo / dispatch_control / ping par bhejo, reply frame
//   wapas Serial par likho.
// - send_frame: stream/status emission (rsh.ino) ke liye — payload ko
//   "RSHS" frame mein lapet kar Serial par likho.
//
// Loop timing: poll har call mein zyada se zyada 512 bytes drain karta
// hai (flood se 200 Hz loop starve na ho). Serial.write TX buffer bhar
// jane par block kar sakta hai — 921600 baud par 45 B frame ~0.5 ms mein
// drain hota hai; cable unplugged ho to CP2102 bytes yunhi leta hai
// (koi flow control nahi), is liye block nahi hota.
#include <stdint.h>
#include <stddef.h>
#include "serial_transport.h"  // RSH_SERIAL_KIND_* + frame sizes (rsh.ino bhi yahin se leta hai)

void serial_link_begin();
void serial_link_poll(uint32_t nowMs);
// Payload ko serial frame mein lapet kar bhejo. Return true = poora frame likha gaya.
bool serial_link_send_frame(uint8_t kind, const uint8_t *payload, uint16_t payload_len);
// Diagnostics (decoder counters + gira hua inbound frame).
uint32_t serial_link_frames_ok();
uint32_t serial_link_bad_frames();
uint32_t serial_link_dropped_inbound();
