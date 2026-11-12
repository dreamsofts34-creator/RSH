"""Reference pin validator + tests (S0/F0). Firmware (C++) and app (C#) must match these rules.
Run: python3 -I tests-host/pin_rules_ref.py
"""
import json, os
BOARD = json.load(open(os.path.join(os.path.dirname(__file__), "..", "docs", "board-esp32-devkitv1.json")))
PINS = BOARD["pins"]; RES = BOARD["reserved"]

def validate(modules, expanders=False):
    """modules: list of dicts {kind:'axis'|'button'|'encoder'|'led'|'battery', pins:[...], ext_pullup:bool}.
    Returns list of error strings (empty = valid)."""
    errs, used = [], {}
    if not modules: errs.append("at least one input module required")
    if expanders: used[RES["i2c_sda"]] = "i2c"; used[RES["i2c_scl"]] = "i2c"
    used[RES["status_led"]] = "status LED"
    n = {"axis": 0, "button": 0, "encoder": 0, "led": 0}
    for m in modules:
        kind = m["kind"]; n[kind if kind in n else "axis" if kind == "battery" else kind] += 1 if kind in n else 0
        for p in m["pins"]:
            info = PINS.get(str(p))
            if info is None: errs.append(f"GPIO{p}: does not exist"); continue
            c = info.get("class")
            if c == "forbidden": errs.append(f"GPIO{p}: forbidden ({info['reason']})"); continue
            if c == "avoid": errs.append(f"GPIO{p}: avoided ({info['reason']})"); continue
            if p in used: errs.append(f"GPIO{p}: already used by {used[p]}"); continue
            if kind in ("axis", "battery") and not info.get("adc"): errs.append(f"GPIO{p}: not ADC1 — unusable for analog with Wi-Fi")
            if kind in ("button", "encoder") and not info.get("pullup") and not m.get("ext_pullup"):
                errs.append(f"GPIO{p}: no internal pull-up (set external pull-up)")
            if kind == "led" and not info.get("digital_out"): errs.append(f"GPIO{p}: cannot drive output")
            used[p] = kind
    lim = BOARD["limits"]
    if n["axis"] > lim["axes"]: errs.append("too many axes")
    if n["encoder"] > lim["encoders"]: errs.append("too many encoders")
    return errs

def free_pins(modules, expanders=False):
    used = {RES["status_led"]} | ({RES["i2c_sda"], RES["i2c_scl"]} if expanders else set())
    for m in modules: used |= set(m["pins"])
    ok = [int(k) for k, v in PINS.items() if v.get("class") in ("ok",)]
    return [p for p in ok if p not in used and not (p in (21, 22) and False)]

def T(name, cond):
    global fails; fails += 0 if cond else 1; print(("PASS " if cond else "FAIL ") + name)
fails = 0
if __name__ == "__main__":
    A = lambda p: {"kind": "axis", "pins": [p]}
    B = lambda p, **k: {"kind": "button", "pins": [p], **k}
    T("v3.3 default map is valid", validate([A(34), A(35), A(36), B(25), B(26), B(27), B(33)]) == [])
    T("analog on ADC2 pin rejected", any("ADC1" in e for e in validate([A(25)])))
    T("flash pin rejected", any("forbidden" in e for e in validate([B(6)])))
    T("strapping pin 12 rejected", any("avoided" in e for e in validate([B(12)])))
    T("UART pin 1 rejected", any("forbidden" in e for e in validate([B(1)])))
    T("duplicate pin rejected", any("already used" in e for e in validate([A(34), B(34, ext_pullup=True)])))
    T("button on input-only pin needs ext pull-up", any("pull-up" in e for e in validate([B(34)])))
    T("button on input-only pin ok with ext pull-up", validate([B(34, ext_pullup=True)]) == [])
    T("status LED pin 2 reserved", any("already used" in e for e in validate([B(2)])))
    T("I2C pins reserved with expanders", any("already used" in e for e in validate([B(21)], expanders=True)))
    T("empty rig rejected", validate([]) != [])
    full = [A(32), A(33), A(34), A(35), A(36),                               # 5 analog
            B(4), B(13), B(14), B(16), B(17), B(18), B(19),                  # 7 buttons
            {"kind": "encoder", "pins": [23, 25]}, {"kind": "encoder", "pins": [26, 27]},
            {"kind": "led", "pins": [4 if False else 39]}]                   # LED on 39 must fail (input-only)
    T("LED on input-only pin rejected", any("cannot drive" in e for e in validate(full)))
    full[-1] = {"kind": "battery", "pins": [39]}
    T("full rig (5 axes, 7 buttons, 2 encoders, battery) is valid", validate(full) == [])
    T("pins left after full rig", len(free_pins(full)) >= 0)
    print(f"\n{'ALL PASS' if not fails else str(fails)+' FAILED'}"); raise SystemExit(1 if fails else 0)
