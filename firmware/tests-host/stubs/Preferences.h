#pragma once
// Host-test stub (TEST-ONLY — kabhi firmware build mein istemal nahi
// hota): ESP32 Preferences (NVS) ka in-memory naql. keystore.cpp isi
// API se baat karta hai (begin/isKey/getBytes/putBytes/getBool/
// putBool/remove/end) — ye stub tamam namespaces ki keys ek shared
// map mein rakhta hai, taake begin()/set()/clear() sequences asal
// NVS jaisi behave karen (alag Preferences instance bhi wahi state
// dekhta hai). Test hooks (test_reset/test_put/test_has) se NVS ki
// corrupt state seed ki ja sakti hai (R3-FW-03 test).
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <map>
#include <string>
#include <vector>

class Preferences {
 public:
  bool begin(const char *name, bool readOnly) {
    (void)readOnly;
    ns_ = name ? name : "";
    return true;
  }
  void end() {}
  bool isKey(const char *key) {
    return store_.count(ns_ + "/" + key) != 0;
  }
  size_t getBytes(const char *key, void *buf, size_t maxLen) {
    auto it = store_.find(ns_ + "/" + key);
    if (it == store_.end()) return 0;
    size_t n = it->second.size() < maxLen ? it->second.size() : maxLen;
    if (buf && n > 0) memcpy(buf, it->second.data(), n);
    return n;
  }
  size_t putBytes(const char *key, const void *value, size_t len) {
    const uint8_t *p = (const uint8_t *)value;
    store_[ns_ + "/" + key] = std::vector<uint8_t>(p, p + len);
    return len;
  }
  bool getBool(const char *key, bool dflt) {
    auto it = store_.find(ns_ + "/" + key);
    if (it == store_.end() || it->second.empty()) return dflt;
    return it->second[0] != 0;
  }
  bool putBool(const char *key, bool value) {
    store_[ns_ + "/" + key] =
        std::vector<uint8_t>{(uint8_t)(value ? 1 : 0)};
    return true;
  }
  bool remove(const char *key) { return store_.erase(ns_ + "/" + key) > 0; }

  // Test hooks (sirf host tests istemal karte hain).
  static void test_reset() { store_.clear(); }
  static void test_put(const char *ns, const char *key,
                       const uint8_t *data, size_t len) {
    store_[std::string(ns) + "/" + key] =
        std::vector<uint8_t>(data, data + len);
  }
  static void test_put_bool(const char *ns, const char *key, bool value) {
    store_[std::string(ns) + "/" + key] =
        std::vector<uint8_t>{(uint8_t)(value ? 1 : 0)};
  }
  static bool test_has(const char *ns, const char *key) {
    return store_.count(std::string(ns) + "/" + key) != 0;
  }

 private:
  std::string ns_;
  inline static std::map<std::string, std::vector<uint8_t>> store_;
};
