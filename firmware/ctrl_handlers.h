#pragma once
// FW-3/FW-4 control handler (FW-7/8 wiring follow-up) — PURE (Arduino-free)
// taake host par unit test ho sake. net.cpp ka net_ctrl_handler FW-3/FW-4
// commands (SET_CFG / COMMIT / CONFIRM / REVERT / GET_CFG / SET_PASSWORD) ko
// yahan delegate karta hai; GET_CAPS / REBOOT net.cpp mein rehte hain.
//
// Handler contract (ctrl.h): status return karo; reply_body mein status
// KE BAGHAIR body bharo (dispatcher status khud prepend karta hai).
//
// Reply formats (PROTOCOL-V2.2.md §3.3/§3.5/§3.6):
//   GET_CFG   -> status, totalLen u16, offset u16, data...
//              wantLen > 1019 ho to 1019 par clamp hota hai (dispatcher ki
//              body had 1023 B hai: 4 header + 1019 data; PC baaqi offset
//              se dobara mangta hai). wantLen > 1024 -> TOO_BIG.
//   SET_CFG   -> status, stagedBytes u16, totalExpected u16 (fail par bhi
//              maujooda txn values — PC dekh sakta hai kitna stage hua).
//   COMMIT    -> status, confirmDeadlineSec u8 (= 30) [kamyabi par];
//              validation fail par status, errCount u8, errCode u8 x errCount.
//              Dobara COMMIT (retry) wahi deadline deta hai (idempotent).
//   CONFIRM   -> status (khaali body).
//   REVERT    -> status (khaali body).
//   KEY_RESET (0x0B) — v2.2 mein DELETE (wire opcode khatam; password
//              clear sirf physical BOOT-hold se hota hai, D7). Handler ko
//              0x0B mile to default UNSUPPORTED deta hai.
//   SET_PASSWORD -> status (khaali body). Payload v2.2 (D3): curLen u8 +
//              currentPw[curLen] + newLen u8 + newPw[newLen]. curLen = 0
//              matlab "koi current password nahi" (pehli setup / koi
//              password stored nahi). Koi password stored ho aur curLen
//              0 ho ya current password ghalat ho -> BAD_PASSWORD (1).
//              Naya password: 8..63 printable ASCII 0x21-0x7E (§5.2).
//              Kamyabi par password_change_request() bulaya jata hai —
//              asal NVS write + AP restart caller karta hai (reply bhejne
//              KE BAAD, §5.2: reply OK restart se PEHLE). Retry wahi
//              password se idempotent hai (dobara wahi value likhi jati
//              hai); retry alag password se naya execution hai (naya
//              reqId, §3.2).
//   LED_TEST -> status (khaali body). Payload: 8 B pattern request (§3.7).
//              Kamyabi par led_test_request() bulaya jata hai (deferred —
//              reply ke baad driver start karta hai). Callback NULL ho to
//              UNSUPPORTED (fail closed — WS2812 driver Phase H tak nahi
//              hai; test accept karke strip na chalana false PASS hota).
//              Pattern galat ho to UNSUPPORTED, R-L9 pin rule toote to
//              PIN_CONFLICT, LED module configured na ho to UNSUPPORTED.
//   FACTORY_RESET -> status (khaali body). Payload: koi nahi (len != 0
//              -> UNSUPPORTED). Config factory defaults par wapas
//              (factory blob load + persist); key/password ko haath NAHI
//              lagta (keystore alag hai — password clear BOOT-hold se hota hai, D7).
//              Staged/previous state saaf ho jata hai (load_current
//              has_previous=0, state IDLE). Reboot ki zaroorat nahi —
//              live blob badalte hi caller ka blob-change detector
//              modules + inputs dobara lagata hai.
//   Unknown cmd -> UNSUPPORTED (execute nahi hota).
//
// Malformed payload (SET_CFG len != 8+chunkLen, GET_CFG len != 4,
// COMMIT/CONFIRM/REVERT len != 0, SET_PASSWORD shape ghalat) ->
// UNSUPPORTED. Frame khud (magic/len/CRC) dispatcher pehle hi verify kar
// chuka hota hai (§3.2). Ghalat current password -> BAD_PASSWORD (1),
// payload-shape ghalat hone par nahi (wo UNSUPPORTED hai).
//
// Callbacks ctx mein hain taake host tests fake de sakein; production
// mein net.cpp asal functions deta hai (millis, NVS persist, armed state).
// now_ms NULL ho to waqt 0 mana jata hai (sirf test safety — production
// mein hamesha millis hota hai).
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include "protocol.h"
#include "config_transaction.h"
#include "password.h"   // SET_PASSWORD 0x0C validation (§5.2)
#include "led_patterns.h"  // LED_TEST 0x0A pattern parse/validate (Phase F WS5)

// GET_CFG reply body ki had: dispatcher body <= 1023 B leta hai
// (RSH_V2_CTRL_MAX_PAYLOAD - 1), is mein 4 B header + data.
#define RSH_FW34_GET_CFG_MAX_DATA 1019u

typedef struct {
  rsh_cfg_txn_t *txn;  // FW-4 transaction state (rsh.ino ka cfgTxn)
  // SET_PASSWORD (D3): current password verify karo. Callback return 1 =
  // proceed (koi password stored nahi, ya current password stored se
  // milta hai); 0 = reject (BAD_PASSWORD). net.cpp mein keystore par
  // mabni hai (has_password ? check_password : 1). Callback NULL ho to
  // handler UNSUPPORTED deta hai (fail closed).
  uint8_t (*password_verify_current)(void *user, const uint8_t *pw, uint16_t len);
  // SET_PASSWORD kamyab: caller ko naya password de (deferred — reply
  // bhejne ke baad NVS write + AP restart; upar dekho). Callback NULL ho
  // to handler UNSUPPORTED deta hai (fail closed — password accept karke
  // apply na karna us se bura hai ke caller ko pata hi na chale).
  void (*password_change_request)(void *user, const uint8_t *pw, uint16_t len);
  // F-07 (polish round): NVS write SYNCHRONOUSLY, reply se pehle.
  // Callback set ho to handler pehle is se NVS likhta hai; 0 aaye to
  // RSH_CTRL_NVS_FAIL (8) — app ko pata chalta hai ke password NAHI
  // badla (pehle reply OK jata tha, NVS fail hone par app/board ka
  // password alag ho jata tha). NULL ho to purana deferred rasta
  // (host tests / purane callers). Kamyabi par bhi password_change_request
  // bulaya jata hai — AP restart ab bhi reply ke baad deferred hota hai;
  // deferred NVS write wahi value dobara likhta hai (idempotent).
  uint8_t (*password_set_sync)(void *user, const uint8_t *pw, uint16_t len);
  // LED_TEST (Phase F WS5, §3.7): LED module ki board-side state.
  // led_pin = config blob LED_STRIP record ka pin (0xFF = configured nahi);
  // expanders = 1 jab MCP23017/PCF8574 records hon (R-L9 rule badal jata hai).
  uint8_t led_pin;
  uint8_t led_has_expanders;
  // LED_TEST kamyab: caller ko pattern de (deferred — reply bhejne ke
  // baad driver start karta hai; pattern engine rsh_led_pattern_start).
  // Callback NULL ho to handler UNSUPPORTED deta hai (fail closed —
  // test accept karke strip na chalana false PASS hota; WS2812 driver
  // Phase H tak nahi hai, is liye production mein ye NULL rehta hai).
  void (*led_test_request)(void *user, const RshLedPattern *pattern);
  // Config NVS persist (COMMIT ke baad pending=1, CONFIRM/REVERT 0).
  void (*cfg_persist)(void *user, uint8_t pending);
  // Board clock (millis) — commit/confirm deadline ke liye.
  uint32_t (*now_ms)(void *user);
  void *user;
  // Observability (tests + diagnostics): handler kitni dafa chala.
  uint32_t calls;
  uint8_t last_cmd;
} rsh_fw34_handler_ctx_t;

static inline void rsh_fw34_handler_ctx_init(rsh_fw34_handler_ctx_t *c,
                                             rsh_cfg_txn_t *txn) {
  memset(c, 0, sizeof(*c));
  c->txn = txn;
}

static inline uint8_t rsh_fw34_handler(uint8_t cmd, const uint8_t *payload, uint16_t len,
                                        uint8_t *reply_body, uint16_t *reply_body_len,
                                        void *ctx) {
  if (!reply_body || !reply_body_len) return RSH_CTRL_UNSUPPORTED;
  *reply_body_len = 0;
  rsh_fw34_handler_ctx_t *c = (rsh_fw34_handler_ctx_t *)ctx;
  if (c) {
    c->calls++;
    c->last_cmd = cmd;
  }
  uint32_t now = (c && c->now_ms) ? c->now_ms(c->user) : 0;

  switch (cmd) {
    case RSH_CMD_GET_CFG: {
      if (!c || !c->txn) return RSH_CTRL_UNSUPPORTED;
      if (len != 4) return RSH_CTRL_UNSUPPORTED;
      uint16_t offset = rsh_get_u16(payload + 0);
      uint16_t want = rsh_get_u16(payload + 2);
      if (want > RSH_V2_CTRL_MAX_PAYLOAD) return RSH_CTRL_TOO_BIG;
      uint16_t take = want > RSH_FW34_GET_CFG_MAX_DATA
                          ? (uint16_t)RSH_FW34_GET_CFG_MAX_DATA : want;
      rsh_put_u16(reply_body + 0, c->txn->current_len);
      rsh_put_u16(reply_body + 2, offset);
      size_t n = rsh_cfg_get_chunk(c->txn, offset, take, reply_body + 4);
      *reply_body_len = (uint16_t)(4 + n);
      return RSH_CTRL_OK;
    }
    case RSH_CMD_SET_CFG: {
      if (!c || !c->txn) return RSH_CTRL_UNSUPPORTED;
      // Payload: offset u16 | total u16 | chunkLen u16 | chunkCrc u16 | data.
      if (len < 8) return RSH_CTRL_UNSUPPORTED;
      uint16_t chunk_len = rsh_get_u16(payload + 4);
      // uint32 mein compare karo — chunk_len bara ho to u16 wrap na ho.
      if ((uint32_t)len != 8u + (uint32_t)chunk_len) return RSH_CTRL_UNSUPPORTED;
      uint16_t staged = 0, total_exp = 0;
      uint8_t st = rsh_cfg_stage_chunk(
          c->txn, rsh_get_u16(payload + 0), rsh_get_u16(payload + 2), chunk_len,
          rsh_get_u16(payload + 6), payload + 8, &staged, &total_exp);
      if (st != RSH_CTRL_OK) {
        // Fail par bhi maujooda values — PC dekh sakta hai kitna stage hua.
        staged = rsh_cfg_staged_bytes(c->txn);
        total_exp = c->txn->has_total ? c->txn->total : rsh_get_u16(payload + 2);
      }
      rsh_put_u16(reply_body + 0, staged);
      rsh_put_u16(reply_body + 2, total_exp);
      *reply_body_len = 4;
      return st;
    }
    case RSH_CMD_COMMIT: {
      if (!c || !c->txn) return RSH_CTRL_UNSUPPORTED;
      if (len != 0) return RSH_CTRL_UNSUPPORTED;
      uint8_t errs[8];
      uint8_t nerr = 0;
      uint8_t st = rsh_cfg_commit(c->txn, now, errs, &nerr);
      if (st == RSH_CTRL_OK) {
        reply_body[0] = RSH_CFG_CONFIRM_DEADLINE_SEC;  // 30
        *reply_body_len = 1;
        if (c->cfg_persist) c->cfg_persist(c->user, 1);
      } else if (nerr > 0) {
        // Validation fail (§3.6): errCount + errCode x errCount.
        // validate kul ghaltiyan ginta hai (255 tak), lekin errs[] mein
        // sirf pehli 8 hain — reply mein pehli 8 hi jati hain (cap).
        uint8_t ncopy = nerr > 8 ? 8 : nerr;
        reply_body[0] = ncopy;
        memcpy(reply_body + 1, errs, ncopy);
        *reply_body_len = (uint16_t)(1 + ncopy);
      }
      return st;
    }
    case RSH_CMD_CONFIRM: {
      if (!c || !c->txn) return RSH_CTRL_UNSUPPORTED;
      if (len != 0) return RSH_CTRL_UNSUPPORTED;
      uint8_t st = rsh_cfg_confirm(c->txn, now);
      if (st == RSH_CTRL_OK && c->cfg_persist) c->cfg_persist(c->user, 0);
      return st;
    }
    case RSH_CMD_REVERT: {
      if (!c || !c->txn) return RSH_CTRL_UNSUPPORTED;
      if (len != 0) return RSH_CTRL_UNSUPPORTED;
      uint8_t was_pending = (c->txn->state != RSH_TXN_IDLE || c->txn->has_previous)
                                ? 1 : 0;
      uint8_t st = rsh_cfg_revert(c->txn);
      if (st == RSH_CTRL_OK && was_pending && c->cfg_persist)
        c->cfg_persist(c->user, 0);
      return st;
    }
    case RSH_CMD_FACTORY_RESET: {
      // M-5: config factory defaults par wapas — key/password ko haath
      // nahi lagta (keystore alag hai; password clear BOOT-hold se hota hai, D7).
      // Factory blob load hota hai (validate hota hai — ghalat blob
      // load nahi hota, caller factory fallback le); load_current
      // staged/previous saaf kar deta hai (state IDLE,
      // has_previous=0). Persist karo taake reboot ke baad bhi
      // defaults rahen; live blob badalte hi rsh.ino ka detector
      // modules + inputs dobara lagata hai (reboot nahi chahiye).
      // F-17 (polish round): safe mode mein FACTORY_RESET bhi BUSY hai —
      // safe mode ka matlab hai "config apply nahi hoti", aur factory
      // reset config replace + apply karta hai. Safe mode se nikalne ka
      // rasta reboot hai (factory reset safe-mode exit nahi hai).
      if (!c || !c->txn) return RSH_CTRL_UNSUPPORTED;
      if (rsh_cfg_is_safe_mode(c->txn)) return RSH_CTRL_BUSY;
      if (len != 0) return RSH_CTRL_UNSUPPORTED;
      uint8_t factory[RSH_CFG_MAX_TOTAL];
      size_t flen = rsh_cfg_factory_blob(factory);
      uint8_t st = rsh_cfg_txn_load_current(c->txn, factory, flen);
      if (st != RSH_CTRL_OK) return st;
      if (c->cfg_persist) c->cfg_persist(c->user, 0);
      return RSH_CTRL_OK;
    }
    case RSH_CMD_LED_TEST: {
      // Payload: 8 B pattern request (§3.7). Dispatcher CRC pehle hi
      // verify kar chuka hai. Validate order: payload -> module configured
      // -> patternId/reserved/period -> R-L9 pin rule (PIN_CONFLICT).
      // Kamyabi par led_test_request() bulaya jata hai (deferred — reply
      // bhejne ke baad driver start karta hai; SET_PASSWORD wala pattern).
      // Callback NULL ho to UNSUPPORTED (fail closed — driver Phase H tak
      // nahi hai; test accept karke strip na chalana false PASS hota).
      if (!c) return RSH_CTRL_UNSUPPORTED;
      RshLedPattern pattern;
      if (!rsh_led_pattern_parse(payload, len, &pattern))
        return RSH_CTRL_UNSUPPORTED;
      // led_pin 0xFF ya 0 = LED module configured nahi (0 kabhi LED pin
      // nahi ho sakta — GPIO0 BOOT button hai; memset-initialized ctx mein
      // bhi 0 hi hota hai, is liye dono ko not-configured mano).
      uint8_t led_configured = (c->led_pin != 0xFF && c->led_pin != 0) ? 1 : 0;
      uint8_t st = rsh_led_pattern_validate(&pattern, c->led_pin,
                                            c->led_has_expanders, led_configured);
      if (st != RSH_CTRL_OK) return st;
      if (!c->led_test_request) return RSH_CTRL_UNSUPPORTED;
      c->led_test_request(c->user, &pattern);
      return RSH_CTRL_OK;
    }
    case RSH_CMD_SET_PASSWORD: {
      // Payload v2.2 (D3): curLen u8 + currentPw[curLen] + newLen u8 +
      // newPw[newLen]. curLen = 0 matlab koi current password nahi.
      // Validation order: shape -> naya password (length/charset) ->
      // current password verify (BAD_PASSWORD). Shape ghalat ho to
      // UNSUPPORTED; current ghalat ho to BAD_PASSWORD (1) — dono alag
      // hain (PC pehla "request ghalat" aur doosra "password ghalat"
      // samajhta hai).
      if (len < 2) return RSH_CTRL_UNSUPPORTED;
      uint8_t cur_len = payload[0];
      if ((uint32_t)len < 1u + (uint32_t)cur_len + 1u) return RSH_CTRL_UNSUPPORTED;
      const uint8_t *cur_pw = payload + 1;
      const uint8_t *rest = payload + 1 + cur_len;
      uint8_t new_len = rest[0];
      if ((uint32_t)len != 1u + (uint32_t)cur_len + 1u + (uint32_t)new_len)
        return RSH_CTRL_UNSUPPORTED;
      if (cur_len > RSH_PASSWORD_MAX_LEN) return RSH_CTRL_UNSUPPORTED;
      if (cur_len > 0) {
        for (uint8_t i = 0; i < cur_len; i++)
          if (cur_pw[i] < 0x21 || cur_pw[i] > 0x7E) return RSH_CTRL_UNSUPPORTED;
      }
      if (rsh_password_validate(rest + 1, new_len) != RSH_CTRL_OK)
        return RSH_CTRL_UNSUPPORTED;  // length/charset fail (§5.2)
      if (!c || !c->password_verify_current) return RSH_CTRL_UNSUPPORTED;
      if (!c->password_verify_current(c->user, cur_pw, cur_len))
        return RSH_CTRL_BAD_PASSWORD;  // D3: ghalat current password
      // F-07: NVS write reply se PEHLE (sync callback ho to) — fail
      // ho to app ko NVS_FAIL batao, OK nahi.
      if (c->password_set_sync &&
          !c->password_set_sync(c->user, rest + 1, new_len))
        return RSH_CTRL_NVS_FAIL;
      if (!c->password_change_request) return RSH_CTRL_UNSUPPORTED;
      c->password_change_request(c->user, rest + 1, new_len);
      return RSH_CTRL_OK;
    }
    default:
      return RSH_CTRL_UNSUPPORTED;
  }
}
