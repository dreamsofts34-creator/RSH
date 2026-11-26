#pragma once
// RSH firmware version — SINGLE SOURCE OF TRUTH (F-12, v4.0.4).
//
// Pehle version do jagah tha (config.h: FIRMWARE_VERSION "4.0.1" aur
// modules.h: RSH_CAPS_FW_* 4/0/1) aur dono ka milna sirf comment + host
// test par munhasir tha — HELLO/GET_CAPS mein ghalat 4.0.0 ka bug isi
// liye hua tha. Ab teeno ek hi macro se bante hain: version badlo to
// sirf yahan badlo; config.h ka FIRMWARE_VERSION string aur modules.h
// ke RSH_CAPS_FW_* dono yahin se aate hain. Host test
// (tests-host/test_main.cpp §34) string parse karke triple se milata hai.
#include <stdint.h>

#define RSH_FW_VERSION_MAJOR 4
#define RSH_FW_VERSION_MINOR 0
#define RSH_FW_VERSION_PATCH 6

// Stringize helpers (do-step, taake macro pehle expand ho).
#define RSH_FW_STR_(x) #x
#define RSH_FW_STR(x) RSH_FW_STR_(x)
#define RSH_FW_VERSION_STRING \
  RSH_FW_STR(RSH_FW_VERSION_MAJOR) "." RSH_FW_STR(RSH_FW_VERSION_MINOR) "." RSH_FW_STR(RSH_FW_VERSION_PATCH)

// Compile-time guard: triple hamesha 0..255 mein ho (wire bytes u8 hain).
static_assert(RSH_FW_VERSION_MAJOR <= 255 && RSH_FW_VERSION_MINOR <= 255 &&
                  RSH_FW_VERSION_PATCH <= 255,
              "RSH firmware version triple must fit in u8 wire fields");
