// RSH — release image-signing public key (Ed25519, SEC-C-06 / SEC-D-04).
// Board OTA image apply karne se PEHLE is key se signature verify karta hai
// (ota_validate.h: rsh_verify_signature). Signature ke baghair / ghalat
// signature wali image kabhi active nahi hoti.
//
// *** RELEASE KEY — Phase H, 2026-11-13 ***
// Ye public key RSH release signing keypair ki hai (Ed25519).
//   Public-key SHA-256 fingerprint:
//     387764a4529337c437b233a148b48f9b2ffe3f3b5015e14c95737cb3a67a6411
//   Public key (hex):
//     ba6f53e0adaf98295c86fef5819e85028def02b320b89955d9dee09afdce2ce7
// Private key OFFLINE hai: ~/workspace/projects/rsh/signing-keys/
// (signing-key.pem, 0600) — repo ke BAHAR, kabhi git mein nahi, kabhi
// image mein nahi. Release signing sirf GitHub secret RSH_SIGNING_KEY_PEM
// ke zariye hoti hai (docs/FLASH-ENCRYPTION-PROCEDURE.md §7, OPEN-19).
//
// Is se pehle yahan DEV public key thi (private key discard — us se koi
// image sign nahi ho sakti thi). Ab is key se signed images hi board
// accept karta hai; purani (dev-key ya unsigned) images reject hoti hain.
//
// Public key badalna = planned migration: naya keypair banao, ye file
// dobara likho (sign-image.py --pubkey-header), rebuild karo, aur har
// board ko USB se dobara flash karo — OTA se nahi (purani key se signed
// image hi OTA ho sakti hai). Detail: docs/SIGNED-OTA.md §5.
#pragma once

#include <stdint.h>

// Ed25519 public key (32 bytes) — RELEASE KEY, dekho upar.
static const uint8_t kRshSigningPubkey[32] = {
  0xba, 0x6f, 0x53, 0xe0, 0xad, 0xaf, 0x98, 0x29,
  0x5c, 0x86, 0xfe, 0xf5, 0x81, 0x9e, 0x85, 0x02,
  0x8d, 0xef, 0x02, 0xb3, 0x20, 0xb8, 0x99, 0x55,
  0xd9, 0xde, 0xe0, 0x9a, 0xfd, 0xce, 0x2c, 0xe7
};
