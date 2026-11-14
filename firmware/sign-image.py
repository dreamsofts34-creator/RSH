#!/usr/bin/env python3
"""RSH — offline firmware-image signing tool (SEC-C-06 / SEC-D-04, Ed25519).

Signed-image format (docs/SIGNED-OTA.md — authority):
    [ESP32 app image, apne trailing SHA-256 digest samet]
    [Ed25519 signature, 64 B] [magic "RSHS", 4 B]
Signature = Ed25519(secret_key, SHA-256(image bytes)) — hash-then-sign.
Board (webota.cpp) image apply karne se pehle verify karta hai; unsigned
ya ghalat-signature wali image reject hoti hai.

Signing key OFFLINE rakho — kabhi repo/git/image mein nahi
(KEY-LIFECYCLE.md §9). Ye tool sirf openssl CLI + hashlib istemal karta
hai (koi nayi dependency nahi).

Usage:
  sign-image.py --gen-keypair <dir>
      Nayi Ed25519 keypair banao: <dir>/signing-key.pem (0600, OFFLINE
      rakho) + <dir>/signing_pubkey.h (firmware/rsh/ mein copy karo).
  sign-image.py --sign <key.pem> <firmware.bin> [--out <signed.bin>]
      Image sign karo. Default out: <firmware>-signed.bin.
      Pehle se signed image dobara sign nahi hoti (error).
  sign-image.py --verify <key.pem|pub.pem> <signed.bin>
      Signature verify karo (openssl se, board wali construction).
  sign-image.py --pubkey-header <key.pem> [--out <signing_pubkey.h>]
      Private key se public-key header banao (stdout, ya file mein).
"""
import argparse
import hashlib
import os
import subprocess
import sys
import tempfile

SIG_LEN = 64
MAGIC = b"RSHS"
TRAILER_LEN = SIG_LEN + len(MAGIC)
MIN_IMAGE = 65536
MAX_IMAGE = 0x140000


def run(cmd, **kw):
    r = subprocess.run(cmd, capture_output=True, **kw)
    if r.returncode != 0:
        sys.exit(f"openssl fail ({' '.join(cmd)}): {r.stderr.decode().strip()}")
    return r


def _temp_file(data: bytes) -> str:
    # SEC-H-09: fixed /tmp paths (predictable, dusre user/process ke
    # saath clash + symlink risk) ki jagah mkstemp — unique, 0600.
    fd, path = tempfile.mkstemp(prefix="rsh-sign-")
    try:
        os.write(fd, data)
    finally:
        os.close(fd)
    return path


def _remove_quietly(path: str) -> None:
    try:
        os.unlink(path)
    except OSError:
        pass


def raw_pubkey_from_der(der: bytes) -> bytes:
    # DER SubjectPublicKeyInfo (Ed25519, 44 bytes): aakhri 32 bytes raw key.
    if len(der) != 44 or der[-33] != 0x00:
        sys.exit("unexpected Ed25519 public-key DER length")
    return der[-32:]


def pubkey_der_from_key(key_pem: str) -> bytes:
    return run(["openssl", "pkey", "-in", key_pem, "-pubout",
                "-outform", "DER"]).stdout


def pubkey_pem_from_key(key_pem: str, tmp: str) -> str:
    run(["openssl", "pkey", "-in", key_pem, "-pubout", "-out", tmp])
    return tmp


def read_image(path: str) -> bytes:
    with open(path, "rb") as f:
        data = f.read()
    if len(data) < MIN_IMAGE:
        sys.exit(f"{path}: image bohat chhoti hai ({len(data)} bytes)")
    if len(data) > MAX_IMAGE:
        sys.exit(f"{path}: image app partition se bari hai ({len(data)} bytes)")
    if data[0] != 0xE9:
        sys.exit(f"{path}: magic 0xE9 nahi — ye app-only firmware.bin nahi lagti "
                  "(merged/factory image ko /update par mat bhejo)")
    if data[-4:] == MAGIC:
        sys.exit(f"{path}: image pehle se signed hai — dobara sign nahi hoti")
    return data


def sign_image(key_pem: str, image_path: str, out_path: str) -> None:
    data = read_image(image_path)
    digest = hashlib.sha256(data).digest()
    digest_path = _temp_file(digest)
    sig_path = _temp_file(b"")
    try:
        run(["openssl", "pkeyutl", "-sign", "-inkey", key_pem,
             "-rawin", "-in", digest_path,
             "-out", sig_path])
        with open(sig_path, "rb") as f:
            sig = f.read()
    finally:
        _remove_quietly(digest_path)
        _remove_quietly(sig_path)
    if len(sig) != SIG_LEN:
        sys.exit(f"openssl ne {len(sig)}-byte signature di (64 chahiye)")
    with open(out_path, "wb") as f:
        f.write(data)
        f.write(sig)
        f.write(MAGIC)
    print(f"signed: {out_path} ({len(data)} + {TRAILER_LEN} trailer bytes)")
    print(f"image SHA-256: {digest.hex()}")


def verify_image(key_or_pub: str, image_path: str) -> None:
    with open(image_path, "rb") as f:
        data = f.read()
    if len(data) < TRAILER_LEN or data[-4:] != MAGIC:
        sys.exit(f"{image_path}: signature trailer nahi — unsigned image")
    image, sig = data[:-TRAILER_LEN], data[-TRAILER_LEN:-4]
    digest = hashlib.sha256(image).digest()
    pub_pem = None
    digest_path = _temp_file(digest)
    sig_path = _temp_file(sig)
    try:
        if "PRIVATE KEY" in open(key_or_pub, "rb").read(64).decode("ascii", "ignore"):
            pub_pem = _temp_file(b"")
            pubkey_pem_from_key(key_or_pub, pub_pem)
        else:
            pub_pem = key_or_pub
        r = subprocess.run(["openssl", "pkeyutl", "-verify", "-pubin",
                            "-inkey", pub_pem, "-rawin",
                            "-in", digest_path,
                            "-sigfile", sig_path],
                           capture_output=True)
    finally:
        _remove_quietly(digest_path)
        _remove_quietly(sig_path)
        if pub_pem is not None and pub_pem != key_or_pub:
            _remove_quietly(pub_pem)
    if r.returncode != 0:
        sys.exit(f"VERIFY FAIL: {image_path} ({r.stderr.decode().strip()})")
    print(f"VERIFY OK: {image_path} (signature theek, image SHA-256 {digest.hex()[:16]}…)")


def pubkey_header(key_pem: str) -> str:
    pk = raw_pubkey_from_der(pubkey_der_from_key(key_pem))
    rows = []
    for i in range(0, 32, 8):
        rows.append("  " + ", ".join(f"0x{b:02x}" for b in pk[i:i + 8]) + ",")
    return ("// RSH — release image-signing public key (Ed25519, SEC-C-06).\n"
            "// sign-image.py --pubkey-header se bana hai. Private key kabhi\n"
            "// repo mein nahi aati (KEY-LIFECYCLE.md §9).\n"
            "#pragma once\n\n"
            "#include <stdint.h>\n\n"
            "static const uint8_t kRshSigningPubkey[32] = {\n" +
            "\n".join(rows) + "\n};\n")


def gen_keypair(outdir: str) -> None:
    os.makedirs(outdir, exist_ok=True)
    key = os.path.join(outdir, "signing-key.pem")
    if os.path.exists(key):
        sys.exit(f"{key} pehle se maujood hai — overwrite nahi karte")
    run(["openssl", "genpkey", "-algorithm", "ED25519", "-out", key])
    os.chmod(key, 0o600)
    header = os.path.join(outdir, "signing_pubkey.h")
    with open(header, "w") as f:
        f.write(pubkey_header(key))
    print(f"keypair ban gaya:\n  private (OFFLINE rakho): {key}\n  header (firmware/rsh/ mein copy karo): {header}")
    print("Private key ko kabhi repo/git/image mein mat dalo.")


def main() -> None:
    ap = argparse.ArgumentParser(description="RSH firmware image signing (Ed25519)")
    ap.add_argument("--gen-keypair", metavar="DIR")
    ap.add_argument("--sign", nargs=2, metavar=("KEY", "IMAGE"))
    ap.add_argument("--out", metavar="PATH")
    ap.add_argument("--verify", nargs=2, metavar=("KEY_OR_PUB", "IMAGE"))
    ap.add_argument("--pubkey-header", metavar="KEY")
    a = ap.parse_args()
    if a.gen_keypair:
        gen_keypair(a.gen_keypair)
    elif a.sign:
        key, image = a.sign
        out = a.out or (os.path.splitext(image)[0] + "-signed.bin")
        sign_image(key, image, out)
    elif a.verify:
        verify_image(a.verify[0], a.verify[1])
    elif a.pubkey_header:
        hdr = pubkey_header(a.pubkey_header)
        if a.out:
            with open(a.out, "w") as f:
                f.write(hdr)
            print(f"header likha: {a.out}")
        else:
            print(hdr, end="")
    else:
        ap.print_help()
        sys.exit(2)


if __name__ == "__main__":
    main()
