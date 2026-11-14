// RSH WS2812 strip — RMT hardware sink implementation (Phase H WS2).
// ESP-IDF legacy RMT driver (driver/rmt.h, installed Arduino core
// 2.0.17 / IDF 4.4). Channel 0, clk_div 8 -> 100 ns tick
// (ws2812_rmt.h timing), mem_block_num 8 -> 512 items/channel memory.
#include "led_sink_rmt.h"
#include "ws2812_rmt.h"
#include <Arduino.h>
#include <driver/rmt.h>

#define RSH_LED_RMT_CHANNEL RMT_CHANNEL_0
// Ek RMT write mein zyada se zyada items (mem_block_num 8 x 64).
#define RSH_LED_RMT_CHUNK_ITEMS 512

static uint8_t rmt_installed = 0;
static uint8_t rmt_pin = 0xFF;
static uint8_t rmt_ready = 0;
// Pichli frame kab tak bhej raha hoga (micros) — in-flight guard.
static uint32_t rmt_busy_until_us = 0;

// Encode output (static — RMT memory mein copy hoti hai, lekin
// chunked write ke doran buffer zinda rehna chahiye).
static RshWs2812Item rmt_items[RSH_WS2812_MAX_ITEMS];

uint8_t rsh_led_rmt_sink_ready(void) { return rmt_ready; }
uint8_t rsh_led_rmt_sink_pin(void) { return rmt_pin; }

uint8_t rsh_led_rmt_sink_init(uint8_t pin) {
  if (pin > 39) return 0;  // ESP32 GPIO range (0..39)
  if (rmt_installed && pin == rmt_pin && rmt_ready) return 1;
  if (rmt_installed) {
    rmt_driver_uninstall(RSH_LED_RMT_CHANNEL);
    rmt_installed = 0;
    rmt_ready = 0;
  }
  rmt_config_t cfg = RMT_DEFAULT_CONFIG_TX((gpio_num_t)pin, RSH_LED_RMT_CHANNEL);
  cfg.clk_div = 8;        // 80 MHz APB / 8 = 10 MHz -> 100 ns tick
  cfg.mem_block_num = 8;  // 8 x 64 = 512 items/channel memory
  cfg.flags = 0;
  cfg.tx_config.carrier_en = false;
  cfg.tx_config.loop_en = false;
  cfg.tx_config.idle_output_en = true;
  cfg.tx_config.idle_level = RMT_IDLE_LEVEL_LOW;
  if (rmt_config(&cfg) != ESP_OK) return 0;
  if (rmt_driver_install(RSH_LED_RMT_CHANNEL, 0, 0) != ESP_OK) return 0;
  rmt_installed = 1;
  rmt_pin = pin;
  rmt_ready = 1;
  rmt_busy_until_us = 0;
  return 1;
}

void rsh_led_rmt_sink(void *user, const RshLedRgb *pixels, uint8_t count) {
  (void)user;
  if (!rmt_ready || !rmt_installed || !pixels || count == 0 ||
      count > RSH_WS2812_MAX_LEDS)
    return;
  // In-flight guard: pichli frame abhi line par ho to skip karo
  // (memory overwrite se behtar — agla badla frame phir bhi aayega).
  // micros() wrap-safe: signed difference.
  if ((int32_t)(micros() - rmt_busy_until_us) < 0) return;
  uint16_t n = rsh_ws2812_encode((const uint8_t *)pixels, count, rmt_items,
                                 RSH_WS2812_MAX_ITEMS);
  if (n == 0) return;  // encode fail — kabhi hona nahi chahiye (upar guards)
  // rmt_item32_t mein badlo (layout: duration0:15, level0:1,
  // duration1:15, level1:1) aur chunked likho.
  static rmt_item32_t hw[RSH_LED_RMT_CHUNK_ITEMS];
  uint16_t off = 0;
  while (off < n) {
    uint16_t chunk = (uint16_t)(n - off > RSH_LED_RMT_CHUNK_ITEMS
                                    ? RSH_LED_RMT_CHUNK_ITEMS
                                    : n - off);
    for (uint16_t i = 0; i < chunk; i++) {
      hw[i].duration0 = rmt_items[off + i].duration0;
      hw[i].level0 = rmt_items[off + i].level0;
      hw[i].duration1 = rmt_items[off + i].duration1;
      hw[i].level1 = rmt_items[off + i].level1;
    }
    uint8_t last = (off + chunk >= n) ? 1 : 0;
    // Beech wale chunks wait karte hain (chunk khatam hone ka intezar —
    // warna agla chunk memory overwrite karta); aakhri chunk
    // non-blocking jata hai (RMT hardware bhejta hai, CPU loop mein).
    if (rmt_write_items(RSH_LED_RMT_CHANNEL, hw, (int)chunk,
                        last ? false : true) != ESP_OK) {
      rmt_ready = 0;  // write error — dobara hammer nahi (A7 visual check)
      return;
    }
    off += chunk;
  }
  // Poori frame ka takhmina waqt — is ke baad channel dobara free hai.
  rmt_busy_until_us = micros() + rsh_ws2812_frame_us(count);
}
