#pragma once
// Host-test stub (TEST-ONLY — kabhi firmware build mein istemal nahi
// hota): Arduino core ka chhota sa hissa jo keystore.cpp ko chahiye.
// Asli Arduino.h is build mein maujood nahi; ye stub sirf host tests
// ke liye hai (-Istubs, tests-host/README.md dekho).
#include <stdint.h>

// keystore.cpp millis() sirf diag timestamps ke liye istemal karta hai.
static inline unsigned long millis() { return 1000; }
static inline void delay(unsigned long) {}
