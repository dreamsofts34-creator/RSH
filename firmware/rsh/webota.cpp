#include "webota.h"
#include "config.h"

#if RSH_WEB_OTA

#include <WebServer.h>
#include <Update.h>
#include "status_led.h"

static WebServer server(80);
static bool active = false;          // upload chal raha hai
static bool updateOk = false;        // is upload ka result
static bool restartPending = false;  // success ke baad restart schedule
static uint32_t restartAtMs = 0;

static bool authed() {
  if (!server.authenticate(WEB_OTA_USER, OTA_PASSWORD)) {
    server.requestAuthentication();
    return false;
  }
  return true;
}

// Poora page ek hi inline HTML — AP par internet nahi hota, is liye
// koi external CSS/JS nahi. Design v3.3: app ke naye light theme jaisi — pastel background, safed card, violet accent (#6C4CF1).
static void handleRoot() {
  if (!authed()) return;
  String html = R"HTML(<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
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
<div class="meta"><span class="chip">Version <b>)HTML";
  html += FIRMWARE_VERSION;
  html += "</b></span><span class='chip'>Free heap <b>";
  html += String(ESP.getFreeHeap() / 1024);
  html += " KB</b></span></div>";
  html += R"HTML(
<form id="f" method="POST" action="/update" enctype="multipart/form-data">
<label class="drop" id="drop"><p class="fname" id="fname">Tap karke .bin file chuno</p><p>ya file yahan drag &amp; drop karo</p>
<input type="file" id="file" name="firmware" accept=".bin" required></label>
<button type="submit" id="btn" disabled>Upload &amp; Update</button>
</form>
<div class="barwrap" id="barwrap"><div class="bar" id="bar"></div></div>
<div class="status" id="status"></div>
<p class="note">Upload ke doran wheel ka data ruk jayega. File pehle board ki
dusri partition mein likhi jati hai aur verify hone par hi active hoti hai &mdash;
update fail ho ya Wi-Fi gir jaye to purana firmware hi chalta rahega.</p>
</div>
<script>
var file=document.getElementById('file'),fname=document.getElementById('fname'),
btn=document.getElementById('btn'),drop=document.getElementById('drop'),
barwrap=document.getElementById('barwrap'),bar=document.getElementById('bar'),
status=document.getElementById('status');
file.addEventListener('change',function(){if(file.files.length){fname.textContent=file.files[0].name;btn.disabled=false;}});
['dragover','dragleave','drop'].forEach(function(ev){drop.addEventListener(ev,function(e){e.preventDefault();drop.classList.toggle('over',ev==='dragover');if(ev==='drop'&&e.dataTransfer.files.length){file.files=e.dataTransfer.files;file.dispatchEvent(new Event('change'));}});});
document.getElementById('f').addEventListener('submit',function(e){
e.preventDefault();if(!file.files.length)return;
btn.disabled=true;file.disabled=true;barwrap.style.display='block';
status.className='status';status.textContent='Upload ho raha hai...';
var fd=new FormData();fd.append('firmware',file.files[0]);
var xhr=new XMLHttpRequest();xhr.open('POST','/update');
xhr.upload.onprogress=function(ev){if(ev.lengthComputable){var p=Math.round(ev.loaded/ev.total*100);bar.style.width=p+'%';status.textContent='Upload ho raha hai... '+p+'%';}};
xhr.onload=function(){if(xhr.status===200&&xhr.responseText.indexOf('OK')===0){bar.style.width='100%';status.className='status ok';status.textContent='Update ho gaya. Board restart ho raha hai...';}else{status.className='status err';status.textContent='Update fail. Purana firmware chal raha hai.';btn.disabled=false;file.disabled=false;}};
xhr.onerror=function(){status.className='status ok';status.textContent='Connection band. Shayad board restart ho gaya hai (update kamyab).';};
xhr.send(fd);});
</script></body></html>)HTML";
  server.send(200, "text/html", html);
}

// Upload khatam hone ke BAAD chalta hai (response + restart schedule)
static void handleUpdateDone() {
  bool ok = updateOk && !Update.hasError();
  server.send(200, "text/plain",
              ok ? "OK - update ho gaya, board restart ho raha hai"
                 : "FAIL - update nahi hua, purana firmware chal raha hai");
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
    if (!server.authenticate(WEB_OTA_USER, OTA_PASSWORD)) {
      updateOk = false;              // baghair login ke kuch nahi likhte
      return;
    }
    active = true;
    led_set(LedState::Updating);
    updateOk = Update.begin(UPDATE_SIZE_UNKNOWN);
  } else if (up.status == UPLOAD_FILE_WRITE) {
    if (updateOk && Update.write(up.buf, up.currentSize) != up.currentSize)
      updateOk = false;
  } else if (up.status == UPLOAD_FILE_END) {
    if (updateOk) updateOk = Update.end(true);   // checksum verify yahin hoti hai
  } else if (up.status == UPLOAD_FILE_ABORTED) {
    Update.abort();
    updateOk = false;
    active = false;
  }
}

void webota_begin() {
  server.on("/", HTTP_GET, handleRoot);
  server.on("/update", HTTP_POST, handleUpdateDone, handleUpload);
  server.onNotFound([]() { server.send(404, "text/plain", "Not found"); });
  server.begin();
}

void webota_handle() {
  server.handleClient();
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
