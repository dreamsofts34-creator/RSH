"""PlatformIO post-build script: sign the app image for WebOTA (SEC-C-06).

Runs after `pio run`. Output (sirf jab key configure ho):
  .pio/build/<env>/firmware-signed.bin   (firmware.bin + 68 B trailer)

Key kahan se aati hai:
  RSH_SIGNING_KEY=<path to Ed25519 private key PEM>   (env var)
  - Var set nahi: signing SKIP hoti hai (dev build unsigned rehta hai —
    board + app unsigned image ko reject karte hain, yehi sahi hai).
    Build fail NAHI hota — LEKIN purana `firmware-signed.bin` (agar
    pichle keyed build se bacha ho) DELETE kar diya jata hai (BUG-W1-1):
    stale signed bin kabhi na rahe — warna keyless build ke baad bhi
    purani version ki signed image build dir mein pari rehti aur release
    mein ghalat image ja sakti hai. Note: ye delete sirf tab chalta hai
    jab post-action chale (yaani `firmware.bin` dobara bana ho); no-op
    `pio run` par SCons post-action nahi chalata — is liye release.yml
    mein alag se fail-closed verify hai (signature + signed-region hash
    vs `firmware.bin`, dekho release.yml).
  - Var set hai lekin key file nahi / sign fail: build ZOR SE FAIL hota
    hai — release build khamoshi se unsigned nahi rehni chahiye.
  - Key kabhi repo mein nahi aati. Dev keypair:
      python3 sign-image.py --gen-keypair /tmp/rsh-signing
      python3 sign-image.py --pubkey-header /tmp/rsh-signing/dev-key.pem
    phir header ko firmware/rsh/signing_pubkey.h mein copy karo.
    Production (Phase H): offline release keypair, private key secure
    storage mein, public key release build mein bake hoti hai.
    Detail: docs/SIGNED-OTA.md
"""
import os
import subprocess

Import("env")  # noqa: F821  (provided by PlatformIO/SCons)


def sign_firmware(source, target, env):  # noqa: F811
    build_dir = env.subst("$BUILD_DIR")
    app = os.path.join(build_dir, "firmware.bin")
    out = os.path.join(build_dir, "firmware-signed.bin")
    key = os.environ.get("RSH_SIGNING_KEY", "").strip()

    if not key:
        # BUG-W1-1 (Phase H W10): keyless build par purana signed bin
        # DELETE karo — stale signed image kabhi build dir mein na rahe.
        # (Keyed build ke baad version badal kar keyless build karne par
        # purana signed bin reh jata tha — QA repro.)
        if os.path.isfile(out):
            os.remove(out)
            print("sign_firmware: RSH_SIGNING_KEY nahi — purana "
                  "firmware-signed.bin DELETE kar diya (stale signed "
                  "image nahi rehni chahiye)")
        else:
            print("sign_firmware: RSH_SIGNING_KEY nahi — firmware-signed.bin "
                  "nahi bana (dev build unsigned rehta hai; SEC-C-06 ke tehat "
                  "unsigned OTA reject hoti hai)")
        return
    if not os.path.isfile(key):
        raise RuntimeError(
            "sign_firmware: RSH_SIGNING_KEY set hai lekin key file nahi: %s"
            % key)
    if not os.path.isfile(app):
        raise RuntimeError("sign_firmware: missing input: %s" % app)

    tool = os.path.join(env.subst("$PROJECT_DIR"), "sign-image.py")
    cmd = [
        env.subst("$PYTHONEXE"),
        tool,
        "--sign", key, app,
        "--out", out,
    ]
    subprocess.run(cmd, check=True, stdin=subprocess.DEVNULL)
    if not os.path.isfile(out):
        raise RuntimeError("sign_firmware: output nahi bana: %s" % out)
    if os.path.getsize(out) != os.path.getsize(app) + 68:
        raise RuntimeError(
            "sign_firmware: output size ghalat (%d, expected %d + 68): %s"
            % (os.path.getsize(out), os.path.getsize(app), out))
    print("sign_firmware: wrote %s" % out)


env.AddPostAction("$BUILD_DIR/${PROGNAME}.bin", sign_firmware)  # noqa: F821
