// Minimal Arduino stub — SIRF host tests ke liye (tests-host/arduino_stub/).
// Firmware build (pio run) isay kabhi nahi dekhta: PlatformIO ka src_dir
// firmware/rsh hai, ye file tests-host/ mein rehti hai aur sirf
// test_host_cpp.cpp ke -I flag se milti hai. Asal Arduino API ki naqal
// nahi — sirf woh functions jo inputs.cpp / status_led.cpp istemal karte
// hain, test-controlled fakes ke saath (definitions test_host_cpp.cpp mein).
#pragma once
#include <stdint.h>

#define HIGH 1
#define LOW 0
#define INPUT_PULLUP 2
#define OUTPUT 1
#define ADC_11db 0

void pinMode(uint8_t pin, uint8_t mode);
void digitalWrite(uint8_t pin, uint8_t value);
int digitalRead(uint8_t pin);
int analogRead(uint8_t pin);
uint32_t analogReadMilliVolts(uint8_t pin);
void analogReadResolution(uint8_t bits);
void analogSetAttenuation(int attenuation);
uint32_t millis();
uint32_t micros();
void delay(uint32_t ms);
