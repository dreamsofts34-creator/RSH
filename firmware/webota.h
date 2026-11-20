#pragma once
// RSH Web OTA — browser se firmware update (ESPHome/Supla style).
// Board ke AP par http://192.168.4.1 par chhota sa page milta hai:
// login (user: WEB_OTA_USER, password: user-set Wi-Fi password —
// keystore_password(); koi compile-time/default password nahi hai,
// OTA_PASSWORD naam ka koi define nahi) ke baad .bin file
// D9 (v2.2): jab tak koi password set nahi, WebOTA DISABLED hai —
// server tabhi start hota hai jab keystore_has_password() true ho.
// upload karo, board verify karke khud restart ho jata hai.
// Dual OTA partition ki wajah se adhoora/failed upload par purana
// firmware hi chalta rehta hai — board brick nahi hota.
void webota_begin();
void webota_handle();
bool webota_active();   // upload chal raha ho to true (sampling rokne ke liye)
