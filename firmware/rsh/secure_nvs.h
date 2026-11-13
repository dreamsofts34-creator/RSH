#pragma once
// RSH secure-boot posture check (SEC-E-01 / OPEN-3, Phase H).
//
// Sirf SECURE build mein active hai (build flag RSH_SECURE_NVS —
// platformio.ini [env:esp32dev-secure] dekho). Normal (dev) build mein
// rsh_secure_boot_check() hamesha true deta hai — koi behaviour change
// nahi, koi nayi dependency nahi.
//
// Kya check hota hai (boot par, pehli Preferences/NVS use se PEHLE):
//   1. Flash encryption waqai enabled hai
//      (esp_flash_encryption_enabled() — eFuse FLASH_CRYPT_CNT).
//   2. NVS-keys partition maujood hai (data / nvs_keys, subtype 0x04 —
//      partitions-secure.csv) — is ke baghair encrypted NVS ban hi nahi
//      sakta.
//
// Kya check NAHI hota (jaan-boojh kar):
// - NVS init khud yahan NAHI hota. Arduino core initArduino() mein
//   nvs_flash_init() bulata hai (esp32-hal-misc.c:249); jab
//   CONFIG_NVS_ENCRYPTION=y ho to wahi call secure flow chalati hai
//   (nvs_keys partition se keys parho, khaali ho to generate karo, phir
//   nvs_flash_secure_init — nvs_flash.h doc, "NVS_ENCRYPTION enabled"
//   wala paragraph). Dobara init bulana ghalat hota (double-init), is
//   liye ye file sirf VERIFY karti hai, init nahi.
// - Device key / password ka plaintext check nahi — wo flash encryption
//   ke neeche hardware se encrypted hain; dump se ciphertext milta hai.
//
// Fail = fail closed: caller (rsh.ino setup) Error LED ke saath halt
// karta hai — secure build kabhi plaintext flash par normal boot nahi
// karta. Ye check sirf tab chalta hai jab secure env se build hua ho;
// agar koi secure-build binary ghalti se bina-encryption wale board par
// flash ho jaye to board halt ho kar ghalti batata hai, khamoshi se
// plaintext mein nahi chalta.
//
// Procedure + eFuse steps: docs/FLASH-ENCRYPTION-PROCEDURE.md (§2–§5).
// Secure env sandbox mein build-verify NAHI hua (framework =
// arduino,espidf — IDF source build; procedure ka step 0 us ka build
// check hai, kisi bhi irreversible eFuse step se pehle).
#if defined(RSH_SECURE_NVS)
#include "esp_flash_encrypt.h"  // esp_flash_encryption_enabled()
#include "esp_partition.h"      // esp_partition_find_first()

static inline bool rsh_secure_boot_check(void) {
  // 1. Flash encryption enabled honi chahiye (pehle encrypted boot par
  //    bootloader ne key generate kar ke flash encrypt ki hoti hai).
  if (!esp_flash_encryption_enabled()) return false;
  // 2. NVS-keys partition honi chahiye (encrypted NVS ki keys ka ghar).
  const esp_partition_t *keys = esp_partition_find_first(
      ESP_PARTITION_TYPE_DATA, ESP_PARTITION_SUBTYPE_DATA_NVS_KEYS, NULL);
  if (keys == NULL) return false;
  return true;
}
#else
static inline bool rsh_secure_boot_check(void) { return true; }
#endif
