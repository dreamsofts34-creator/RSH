// RSH WebOTA — browser se firmware update (FW-9 / SEC-C-02 / SEC-C-05 /
// SEC-C-06 / SEC-D-02 / SEC-D-04 / SEC-D-12).
// Board ke AP par http://192.168.4.1 par chhota sa page milta hai:
// login (user: WEB_OTA_USER, password: per-device password —
// keystore.h (FW-8), kabhi default nahi) ke baad .bin file upload karo.
//
// Security (har rule ka test tests-host/test_main.cpp mein hai):
// - F1/SEC-D-02: HAR endpoint par authenticate() — root, /update
//   (completion handler samet), not-found. Ghalat koshishon par lockout:
//   5 fail -> 60 s tak 401 (TM-O03, brute force).
// - CSRF (SEC-D-12): /update ko per-boot random token header
//   "X-RSH-CSRF" chahiye (page ke <meta> se milta hai). Token ke baghair
//   POST reject (403) — browser mein login hone ke baad bhi malicious
//   page se cross-origin upload nahi ho sakta.
// - F9: pehle chunk mein magic 0xE9; size cap = app partition
//   (incremental — multipart totalSize running total hai, declared size
//   nahi, is liye har chunk par cap check hota hai); updateOk har upload
//   ke shuru mein reset (SEC-C-10).
// - Anti-downgrade (SEC-C-05): image ke andar "Version <b>" marker se
//   version nikalo; running version se kam ho to reject, barabar =
//   reinstall (allowed). Marker sirf version batata hai — authenticity
//   nahi (SEC-D-04).
// - Signed OTA (SEC-C-06/SEC-D-04): image ke aakhir mein 68-byte
//   trailer [Ed25519 sig 64 B]["RSHS"]; signature SHA-256(image,
//   trailer ke baghair) par verify hoti hai — verify-before-apply:
//   Update.end() SIRF tab jab signature theek ho, warna Update.abort()
//   aur purana firmware chalta rehta hai. (docs/SIGNED-OTA.md)
// - SEC-C-02: page JS connection tootne par kabhi "kamyab" nahi kehta —
//   UNKNOWN dikhata hai; kamyabi sirf HTTP 200 + "OK" par, aur version
//   verify karne ki hidayat ke saath (GET_CAPS, app Link step).
// - Page PROGMEM se stream hota hai (chunked, F9) — heap String nahi.
//   NOTE: version chip ek hi string literal hai ("Version <b>"
//   FIRMWARE_VERSION "</b>") taake marker aur version digits flashed
//   binary mein aas-paas rahein — scanner aur C# validator isi par
//   munhasir hain; is literal ko toro mat.
// - ArduinoOTA (SEC-D-02, doosra OTA rasta) config.h mein BAND hai
//   (RSH_OTA 0) — WebOTA hi waahid OTA rasta hai. Agar kabhi dobara
//   kholo to ota_begin() runtime password istemal karta hai (rsh.ino).
#include "webota.h"
#include "config.h"

#if RSH_WEB_OTA

#include <WebServer.h>
#include <Update.h>
#include <esp_random.h>
#include <sodium.h>
#include "status_led.h"
#include "keystore.h"  // FW-8: per-device password (WebOTA auth, SEC-C-04)
#include "ota_validate.h"
#include "signing_pubkey.h"

static WebServer server(80);
static bool active = false;          // upload chal raha hai
static bool updateOk = false;        // is upload ka result
static bool restartPending = false;  // success ke baad restart schedule
static uint32_t restartAtMs = 0;

// ---- Auth rate limit (SEC-D-02 / TM-O03) ----
static uint8_t authFails = 0;        // lagatar ghalat koshishein
static uint32_t authLockUntilMs = 0; // lockout khatam hone ka waqt (0 = koi lockout nahi)
static constexpr uint8_t kAuthMaxFails = 5;
static constexpr uint32_t kAuthLockoutMs = 60000;

static bool authed() {
  uint32_t now = millis();
  // Lockout ke doran password check hi nahi karte (koi oracle nahi).
  if ((int32_t)(now - authLockUntilMs) < 0) {
    server.requestAuthentication();
    return false;
  }
  if (!server.authenticate(WEB_OTA_USER, keystore_password())) {
    server.requestAuthentication();
    if (++authFails >= kAuthMaxFails) {
      authLockUntilMs = now + kAuthLockoutMs;
      authFails = 0;
    }
    return false;
  }
  authFails = 0;
  return true;
}

// ---- CSRF token (SEC-D-12): per-boot random, 16 bytes -> 32 hex chars ----
static char csrfToken[33] = {0};

static bool csrf_ok() {
  if (!server.hasHeader("X-RSH-CSRF")) return false;
  String h = server.header("X-RSH-CSRF");
  if (h.length() != 32) return false;
  // Constant-time compare (timing oracle nahi).
  uint8_t diff = 0;
  for (int i = 0; i < 32; i++) diff |= (uint8_t)(h[i] ^ csrfToken[i]);
  return diff == 0;
}

// ---- Upload state (har upload ke shuru mein reset — SEC-C-10) ----
static size_t uploadBytes = 0;       // ab tak likhe gaye bytes
static size_t uploadCap = 0;         // size cap (app partition)
static const char *uploadError = ""; // FAIL response ki wajah (literal, heap nahi)
static RshVersionScan versionScan;   // image version (marker scanner)
static RshSignHash signHash;         // signed-region digest (streaming)
static crypto_hash_sha256_state shaState;
static RshVersion currentVersion{};
static bool currentVersionOk = false;
// Round 2 W3 (FIX-3): upload inactivity timeout — adhoora/ruka hua
// upload `active` latka kar sampling hamesha ke liye band kar deta tha
// (webota_active() par loop early-return karta hai). 30 s mein koi
// chunk na aaye to upload abort + sampling wapas. Aakhri chunk ka waqt
// START + har WRITE par update hota hai (webota_handle mein check).
static uint32_t lastUploadMs = 0;
static constexpr uint32_t kUploadTimeoutMs = 30000;

static void sha_sink(void *ctx, const uint8_t *data, size_t len) {
  crypto_hash_sha256_update((crypto_hash_sha256_state *)ctx, data, (unsigned long long)len);
}

// ---- Page (PROGMEM, chunked stream — heap String nahi, F9) ----
// kPageA: head + styles + body shuru + meta csrf (token yahin insert hota hai).
static const char kPageA[] PROGMEM = R"HTML(<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="csrf-token" content=")HTML";
// kPageB: meta band + baqi head + body + version chip (marker + version
// EK literal mein — dekho file header NOTE, isay toro mat).
static const char kPageB[] PROGMEM = R"HTML(">
<title>RSH Update</title>
<style>
*{box-sizing:border-box}body{margin:0;font-family:'Segoe UI',system-ui,sans-serif;background:#e9edfb;color:#1b2340;min-height:100vh;display:flex;align-items:center;justify-content:center;padding:20px}
.card{background:#ffffff;border:1px solid #dfe4f2;border-radius:20px;padding:28px;max-width:440px;width:100%;box-shadow:0 18px 50px rgba(74,85,165,.18)}
.logo{display:flex;align-items:center;gap:12px;margin-bottom:6px}
.ring{width:40px;height:40px;border:5px solid #6c4cf1;border-radius:50%;position:relative;flex:none}
.ring:after{content:'';position:absolute;inset:11px;background:#6c4cf1;border-radius:50%}
h1{font-size:20px;margin:0;font-weight:600}
.sub{color:#5a6480;font-size:13px;margin:2px 0 18px}
.meta{display:flex;gap:8px;margin-bottom:20px}
.chip{background:#f1f3fc;border:1px solid #dfe4f2;border-radius:8px;padding:6px 10px;font-size:12px;color:#5a6480}
.chip b{color:#1b2340}
.drop{border:2px dashed #c3cbe4;border-radius:12px;padding:22px;text-align:center;cursor:pointer;transition:.15s}
.drop:hover,.drop.over{border-color:#6c4cf1;background:rgba(108,76,241,.06)}
.drop p{margin:0;color:#5a6480;font-size:13px}
.drop .fname{color:#1b2340;font-size:14px;word-break:break-all}
input[type=file]{display:none}
button{width:100%;margin-top:16px;background:#6c4cf1;color:#ffffff;border:0;border-radius:12px;padding:13px;font-size:15px;font-weight:600;cursor:pointer}
button:disabled{opacity:.45;cursor:default}
.barwrap{display:none;margin-top:18px;background:#eef1fa;border-radius:8px;height:12px;overflow:hidden}
.bar{height:100%;width:0;background:#6c4cf1;transition:width .15s}
.status{margin-top:12px;font-size:13px;color:#5a6480;min-height:18px;text-align:center}
.status.ok{color:#1f9d55}.status.err{color:#d92d20}
.note{margin-top:18px;font-size:12px;color:#64748b;line-height:1.5}
</style></head><body>
<div class="card">
<div class="logo"><div class="ring"></div><h1>RSH</h1></div>
<p class="sub">Firmware Update &mdash; ESP32 DevKit V1</p>
<div class="meta"><span class="chip">Version <b>)HTML" FIRMWARE_VERSION R"HTML(</b></span><span class="chip">Free heap <b>)HTML";
// kPageC: heap ke baad se form + script + close tak.
static const char kPageC[] PROGMEM = R"HTML( KB</b></span></div>
<form id="f" method="POST" action="/update" enctype="multipart/form-data">
<label class="drop" id="drop"><p class="fname" id="fname">Tap karke .bin file chuno</p><p>ya file yahan drag &amp; drop karo</p>
<input type="file" id="file" name="firmware" accept=".bin" required></label>
<button type="submit" id="btn" disabled>Upload &amp; Update</button>
</form>
<div class="barwrap" id="barwrap"><div class="bar" id="bar"></div></div>
<div class="status" id="status"></div>
<p class="note">Sirf signed official RSH release upload karo — board image ki
signature verify karta hai; ghalat, unsigned ya purani image reject ho
jati hai. Upload ke doran wheel ka data ruk jayega. File pehle board ki
dusri partition mein likhi jati hai aur verify hone par hi active hoti hai &mdash;
update fail ho ya Wi-Fi gir jaye to purana firmware hi chalta rahega.</p>
</div>
<script>
var file=document.getElementById('file'),fname=document.getElementById('fname'),
btn=document.getElementById('btn'),drop=document.getElementById('drop'),
barwrap=document.getElementById('barwrap'),bar=document.getElementById('bar'),
status=document.getElementById('status'),
token=document.querySelector('meta[name="csrf-token"]').getAttribute('content');
file.addEventListener('change',function(){if(file.files.length){fname.textContent=file.files[0].name;btn.disabled=false;}});
['dragover','dragleave','drop'].forEach(function(ev){drop.addEventListener(ev,function(e){e.preventDefault();drop.classList.toggle('over',ev==='dragover');if(ev==='drop'&&e.dataTransfer.files.length){file.files=e.dataTransfer.files;file.dispatchEvent(new Event('change'));}});});
document.getElementById('f').addEventListener('submit',function(e){
e.preventDefault();if(!file.files.length)return;
btn.disabled=true;file.disabled=true;barwrap.style.display='block';
status.className='status';status.textContent='Upload ho raha hai...';
var fd=new FormData();fd.append('firmware',file.files[0]);
var xhr=new XMLHttpRequest();xhr.open('POST','/update');
xhr.setRequestHeader('X-RSH-CSRF',token);
xhr.upload.onprogress=function(ev){if(ev.lengthComputable){var p=Math.round(ev.loaded/ev.total*100);bar.style.width=p+'%';status.textContent='Upload ho raha hai... '+p+'%';}};
xhr.onload=function(){if(xhr.status===200&&xhr.responseText.indexOf('OK')===0){bar.style.width='100%';status.className='status ok';status.textContent='Update ho gaya. Board restart ho raha hai — version verify karo (app Link step ya GET_CAPS se).';}else{status.className='status err';status.textContent='Update fail. Purana firmware chal raha hai.';btn.disabled=false;file.disabled=false;}};
xhr.onerror=function(){status.className='status err';status.textContent='Connection band — update VERIFY NAHI HUA. Board restart ho raha ho sakta hai ya upload fail hua ho. Dobara connect ho kar version check karo; version purana ho to dobara koshish karo.';btn.disabled=false;file.disabled=false;};
xhr.send(fd);});
</script></body></html>)HTML";

// Poora page ek hi inline HTML — AP par internet nahi hota, is liye
// koi external CSS/JS nahi. Design v3.3: app ke naye light theme jaisi — pastel background, safed card, violet accent (#6C4CF1).
static void handleRoot() {
  if (!authed()) return;
  char heap[16];
  snprintf(heap, sizeof(heap), "%u", (unsigned)(ESP.getFreeHeap() / 1024));
  server.setContentLength(CONTENT_LENGTH_UNKNOWN);
  server.send(200, "text/html", "");
  server.sendContent_P(kPageA, sizeof(kPageA) - 1);
  server.sendContent(csrfToken, 32);
  server.sendContent_P(kPageB, sizeof(kPageB) - 1);
  server.sendContent(heap, strlen(heap));
  server.sendContent_P(kPageC, sizeof(kPageC) - 1);
  // Chunked terminator _finalizeResponse() bhejta hai (handler return par).
}

// Upload khatam hone ke BAAD chalta hai (response + restart schedule)
static void handleUpdateDone() {
  if (!authed()) return;  // F1/SEC-D-02: completion handler par bhi auth
  if (!csrf_ok()) {
    server.send(403, "text/plain", "FAIL - bad CSRF token");
    updateOk = false;
    active = false;
    return;
  }
  bool ok = updateOk && !Update.hasError();
  if (ok) {
    server.send(200, "text/plain", "OK - update ho gaya, board restart ho raha hai");
  } else {
    char body[160];
    snprintf(body, sizeof(body), "FAIL - %s, purana firmware chal raha hai",
             uploadError[0] ? uploadError : "update nahi hua");
    server.send(200, "text/plain", body);
  }
  // State consume karo — stale updateOk kabhi dobara istemal na ho (SEC-C-10).
  updateOk = false;
  active = false;
  if (ok) {
    restartPending = true;
    restartAtMs = millis() + 1000;   // response nikal jane ke baad restart
  }
}

// File ke hisse aate hue chalta hai (multipart upload stream)
static void handleUpload() {
  HTTPUpload& up = server.upload();
  if (up.status == UPLOAD_FILE_START) {
    // State reset PEHLE (SEC-C-10) — phir auth/CSRF; fail ho to kuch
    // likhe baghair wapas (bina auth ke restart/DoS nahi hota).
    updateOk = false;
    active = false;
    uploadBytes = 0;
    uploadError = "";
    rsh_version_scan_init(&versionScan);
    crypto_hash_sha256_init(&shaState);
    rsh_sign_hash_init(&signHash, sha_sink, &shaState);
    if (!authed()) { uploadError = "unauthorized"; return; }
    if (!csrf_ok()) { uploadError = "bad CSRF token"; return; }
    if (!currentVersionOk) { uploadError = "bad current version"; return; }
    if (up.filename.length() == 0) { uploadError = "no file"; return; }
    uploadCap = ESP.getFreeSketchSpace();
    if (uploadCap == 0 || uploadCap > kOtaMaxBytes) uploadCap = kOtaMaxBytes;
    active = true;
    lastUploadMs = millis();  // FIX-3: timeout clock shuru
    led_set(LedState::Updating);
    updateOk = Update.begin(uploadCap);
    if (!updateOk) uploadError = "update begin failed";
  } else if (up.status == UPLOAD_FILE_WRITE) {
    if (!updateOk) return;  // pehle hi reject — bytes drain karo, kuch na likho
    if (up.currentSize == 0) return;
    // Magic: pehle chunk ka pehla byte 0xE9 (F9).
    if (uploadBytes == 0 && up.buf[0] != kEspImageMagic) {
      Update.abort();
      updateOk = false;
      uploadError = "bad magic (0xE9 nahi)";
      return;
    }
    // Size cap — incremental (multipart totalSize running total hai,
    // declared size nahi, is liye har chunk par check lazmi hai).
    if (uploadBytes + up.currentSize > uploadCap) {
      Update.abort();
      updateOk = false;
      uploadError = "image bohat bari hai (size cap)";
      return;
    }
    rsh_version_scan_feed(&versionScan, up.buf, up.currentSize);
    rsh_sign_hash_feed(&signHash, up.buf, up.currentSize);
    lastUploadMs = millis();  // FIX-3: chunk mila — timeout clock dobara
    if (Update.write(up.buf, up.currentSize) != up.currentSize) {
      updateOk = false;
      uploadError = "update write failed";
      return;
    }
    uploadBytes += up.currentSize;
  } else if (up.status == UPLOAD_FILE_END) {
    if (!updateOk) { active = false; return; }
    // Verify-before-apply (SEC-C-06 / SEC-D-04) — Update.end() SIRF tab
    // jab tamam checks pass hon; warna abort, purana firmware rehta hai.
    if (uploadBytes < kOtaMinBytes) {
      Update.abort();
      updateOk = false;
      uploadError = "image bohat chhoti hai";
      active = false;
      return;
    }
    uint8_t tail[kOtaTrailerSize];
    size_t tailLen = 0;
    rsh_sign_hash_final(&signHash, tail, &tailLen);
    uint8_t digest[32];
    crypto_hash_sha256_final(&shaState, digest);
    if (!ota_trailer_has_magic(tail, tailLen)) {
      Update.abort();
      updateOk = false;
      uploadError = "image signed nahi hai (signature trailer nahi)";
      active = false;
      return;
    }
    if (!versionScan.found) {
      Update.abort();
      updateOk = false;
      uploadError = "image mein version nahi mila";
      active = false;
      return;
    }
    if (rsh_version_compare(versionScan.version, currentVersion) < 0) {
      Update.abort();
      updateOk = false;
      uploadError = "downgrade reject (purani version)";
      active = false;
      return;
    }
    if (!rsh_verify_signature(ota_trailer_sig(tail), digest, kRshSigningPubkey)) {
      Update.abort();
      updateOk = false;
      uploadError = "signature verify fail";
      active = false;
      return;
    }
    updateOk = Update.end(true);  // ESP image SHA bhi yahin verify hoti hai
    if (!updateOk) uploadError = "update end failed";
    active = false;
  } else if (up.status == UPLOAD_FILE_ABORTED) {
    Update.abort();
    updateOk = false;
    active = false;
    uploadError = "upload aborted";
  }
}

void webota_begin() {
  // CSRF token: per-boot random 16 bytes -> 32 hex chars (SEC-D-12).
  static const char kHex[] = "0123456789abcdef";
  for (int i = 0; i < 16; i++) {
    uint8_t b = (uint8_t)esp_random();
    csrfToken[i * 2] = kHex[b >> 4];
    csrfToken[i * 2 + 1] = kHex[b & 0x0F];
  }
  csrfToken[32] = '\0';
  currentVersionOk = rsh_parse_version(FIRMWARE_VERSION, strlen(FIRMWARE_VERSION), &currentVersion);
  const char *csrfHeaders[] = {"X-RSH-CSRF"};
  server.collectHeaders(csrfHeaders, 1);
  server.on("/", HTTP_GET, handleRoot);
  server.on("/update", HTTP_POST, handleUpdateDone, handleUpload);
  server.onNotFound([]() {
    if (!authed()) return;  // F1: unknown path par bhi auth (koi info leak nahi)
    server.send(404, "text/plain", "Not found");
  });
  server.begin();
}

void webota_handle() {
  server.handleClient();
  // FIX-3: ruka hua upload (30 s koi chunk nahi) abort karo — warna
  // `active` latka rehta aur rsh.ino loop sampling kabhi wapas nahi
  // karta (webota_active() early-return). Abort ke baad purana firmware
  // chalta rehta hai (verify-before-apply — Update.end kabhi nahi hua).
  if (active && (uint32_t)(millis() - lastUploadMs) >= kUploadTimeoutMs) {
    Update.abort();
    updateOk = false;
    active = false;
    uploadError = "upload timeout (30 s koi data nahi)";
  }
  if (restartPending && (int32_t)(millis() - restartAtMs) >= 0) {
    restartPending = false;
    ESP.restart();
  }
}

bool webota_active() { return active; }

#else

void webota_begin() {}
void webota_handle() {}
bool webota_active() { return false; }

#endif
