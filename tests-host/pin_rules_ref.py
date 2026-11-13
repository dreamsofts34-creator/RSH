"""Reference pin validator + tests (S0/F0, FW-1 WP1/WP2). Firmware (C++) and app (C#) must match these rules.
Run (in the Virtual Test Rig only): python3 -I tests-host/pin_rules_ref.py

Kinds (T7): axis | button | encoder | led | battery | axis_ads1115 | button_matrix | expander_button.
Limits (D6): axes <= 8, buttons <= 128, encoders <= 4; LED strips <= 1 (60 LEDs/strip);
one battery module -- and a battery is NOT an axis (T7 fix for the old
`n[...] += 1 if kind in n else 0` line, which counted a battery as 0 axes
through an unreadable expression instead of counting it as a battery).
"""
import copy, json, os
BOARD = json.load(open(os.path.join(os.path.dirname(__file__), "..", "docs", "board-esp32-devkitv1.json")))
PINS = BOARD["pins"]; RES = BOARD["reserved"]; LIM = BOARD["limits"]

KINDS = ("axis", "button", "encoder", "led", "battery",
         "axis_ads1115", "button_matrix", "expander_button")

def _pin_errs(p, kind, m, used):
    """Per-GPIO checks shared by every kind that uses ESP32 pins.
    Returns the error list for this pin (empty = pin usable for this kind).
    Order matches the original validator: forbidden/avoid first, then
    already-used, then kind-specific electrical rules."""
    info = PINS.get(str(p))
    if info is None:
        return [f"GPIO{p}: does not exist"]
    c = info.get("class")
    if c == "forbidden":
        return [f"GPIO{p}: forbidden ({info['reason']})"]
    if c == "avoid":
        return [f"GPIO{p}: avoided ({info['reason']})"]
    if p in used:
        return [f"GPIO{p}: already used by {used[p]}"]
    errs = []
    if kind in ("axis", "battery") and not info.get("adc"):
        errs.append(f"GPIO{p}: not ADC1 — unusable for analog with Wi-Fi")
    if kind in ("button", "encoder", "button_matrix") and not info.get("pullup") and not m.get("ext_pullup"):
        errs.append(f"GPIO{p}: no internal pull-up (set external pull-up)")
    if kind == "led" and not info.get("digital_out"):
        errs.append(f"GPIO{p}: cannot drive output")
    return errs

def validate(modules, expanders=False):
    """modules: list of dicts {kind: ..., pins: [...], ...}.
    Returns list of error strings (empty = valid).
    expanders=True reserves GPIO21/22 for I2C and is required by any I2C kind
    (axis_ads1115, expander_button)."""
    errs, used = [], {}
    if not modules:
        errs.append("at least one input module required")
    if expanders:
        used[RES["i2c_sda"]] = "i2c"
        used[RES["i2c_scl"]] = "i2c"
    used[RES["status_led"]] = "status LED"
    n = {"axis": 0, "button": 0, "encoder": 0, "led": 0, "battery": 0}
    ads_used, exp_used = set(), set()
    for m in modules:
        kind = m.get("kind")
        if kind not in KINDS:
            errs.append(f"unknown kind '{kind}'")
            continue
        pins = m.get("pins")
        if not isinstance(pins, list):
            errs.append(f"{kind}: pins must be a list")
            continue
        if kind == "axis_ads1115":
            # I2C ADC channel: no ESP32 GPIO, but the I2C bus must be on.
            if pins:
                errs.append("axis_ads1115: uses I2C, no GPIO pin")
            if not expanders:
                errs.append("axis_ads1115: requires I2C expander (pass expanders=True)")
            ch = m.get("channel", 0)
            addr = m.get("addr", 0x48)
            if not isinstance(ch, int) or isinstance(ch, bool) or ch < 0 or ch > 3:
                errs.append(f"axis_ads1115: channel {ch} out of range (0-3)")
            elif (addr, ch) in ads_used:
                errs.append(f"axis_ads1115: channel {ch} on addr {addr} already used")
            else:
                ads_used.add((addr, ch))
            n["axis"] += 1
            continue
        if kind == "expander_button":
            # MCP23017 / PCF8574 input: no ESP32 GPIO, but the I2C bus must be on.
            if pins:
                errs.append("expander_button: uses no ESP32 GPIO pin")
            if not expanders:
                errs.append("expander_button: requires I2C expander (pass expanders=True)")
            if "expander_pin" in m:
                exp = m.get("expander", "MCP23017")
                addr = m.get("addr", 0x20)
                ep = m["expander_pin"]
                hi = 7 if exp == "PCF8574" else 15
                if not isinstance(ep, int) or isinstance(ep, bool) or ep < 0 or ep > hi:
                    errs.append(f"expander_button: pin {ep} out of range for {exp} (0-{hi})")
                elif (exp, addr, ep) in exp_used:
                    errs.append(f"expander_button: {exp} addr {addr} pin {ep} already used")
                else:
                    exp_used.add((exp, addr, ep))
            n["button"] += 1
            continue
        if kind == "button_matrix":
            rows = m.get("rows", 4)
            cols = m.get("cols", 4)
            if (not isinstance(rows, int) or isinstance(rows, bool)
                    or not isinstance(cols, int) or isinstance(cols, bool)
                    or rows < 1 or rows > 8 or cols < 1 or cols > 8):
                errs.append(f"button_matrix: rows/cols must be 1-8 (got {rows}x{cols})")
                continue
            if len(pins) != rows + cols:
                errs.append(f"button_matrix: needs {rows + cols} pins (rows+cols), got {len(pins)}")
                continue
            for p in pins:
                perr = _pin_errs(p, kind, m, used)
                errs.extend(perr)
                if not perr:
                    used[p] = kind
            n["button"] += rows * cols
            continue
        if kind == "encoder" and len(pins) != 2:
            errs.append(f"encoder: needs exactly 2 pins (A, B), got {len(pins)}")
            continue
        if kind in ("axis", "battery", "button", "led") and len(pins) != 1:
            errs.append(f"{kind}: needs exactly 1 pin, got {len(pins)}")
            continue
        for p in pins:
            perr = _pin_errs(p, kind, m, used)
            errs.extend(perr)
            if not perr:
                used[p] = kind
        if kind == "axis":
            n["axis"] += 1
        elif kind == "battery":
            n["battery"] += 1  # T7 fix: a battery is counted as a battery, never as an axis
        elif kind == "button":
            n["button"] += 1
        elif kind == "encoder":
            n["encoder"] += 1
        elif kind == "led":
            n["led"] += 1
            if "count" in m:
                cnt = m["count"]
                if (not isinstance(cnt, int) or isinstance(cnt, bool)
                        or cnt < 1 or cnt > LIM["leds_per_strip"]):
                    errs.append(f"led: count {cnt} out of range (1-{LIM['leds_per_strip']})")
    if n["axis"] > LIM["axes"]:
        errs.append(f"too many axes ({n['axis']} > {LIM['axes']})")
    if n["button"] > LIM["buttons"]:
        errs.append(f"too many buttons ({n['button']} > {LIM['buttons']})")
    if n["encoder"] > LIM["encoders"]:
        errs.append(f"too many encoders ({n['encoder']} > {LIM['encoders']})")
    if n["led"] > LIM["led_strips"]:
        errs.append(f"too many LED strips ({n['led']} > {LIM['led_strips']})")
    if n["battery"] > LIM["batteries"]:
        errs.append(f"too many battery modules ({n['battery']} > {LIM['batteries']})")
    return errs

def free_pins(modules, expanders=False):
    used = {RES["status_led"]} | ({RES["i2c_sda"], RES["i2c_scl"]} if expanders else set())
    for m in modules:
        used |= set(m.get("pins") or [])
    ok = [int(k) for k, v in PINS.items() if v.get("class") == "ok"]
    # T8 fix: the old `not (p in (21, 22) and False)` was dead code (always True).
    # GPIO21/22 need no special case here: with expanders they are already in
    # `used` (I2C reserved), without expanders they are free digital pins.
    # Sorted numerically: JSON key order is not numeric ("10" < "2"), so the
    # helper must not depend on it.
    return sorted(p for p in ok if p not in used)

def T(name, cond):
    global fails; fails += 0 if cond else 1; print(("PASS " if cond else "FAIL ") + name)
fails = 0
if __name__ == "__main__":
    A = lambda p: {"kind": "axis", "pins": [p]}
    B = lambda p, **k: {"kind": "button", "pins": [p], **k}
    E = lambda a, b: {"kind": "encoder", "pins": [a, b]}
    ADS = lambda ch, **k: {"kind": "axis_ads1115", "pins": [], "channel": ch, **k}
    XB = lambda **k: {"kind": "expander_button", "pins": [], **k}
    MX = lambda pins, **k: {"kind": "button_matrix", "pins": pins, **k}
    LED = lambda p, **k: {"kind": "led", "pins": [p], **k}
    BAT = lambda p: {"kind": "battery", "pins": [p]}

    def make_full_rig():
        """Fresh full rig on every call (T8 fix: the old shared `full` list was
        mutated between tests -- LED swapped for battery -- so tests were not
        independent). 5 axes + 7 buttons + 2 encoders + battery = 17 pins."""
        return [A(32), A(33), A(34), A(35), A(36),
                B(4), B(13), B(14), B(16), B(17), B(18), B(19),
                E(23, 25), E(26, 27),
                BAT(39)]

    # --- original S0/F0 checks (unchanged behaviour) ---
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
    rig = make_full_rig()
    rig[-1] = LED(39)
    T("LED on input-only pin rejected", any("cannot drive" in e for e in validate(rig)))
    T("full rig (5 axes, 7 buttons, 2 encoders, battery) is valid", validate(make_full_rig()) == [])
    # T8 fix: the old `len(free_pins(full)) >= 0` was vacuous (always true).
    # Real assertions: exact free-pin sets.
    T("full rig leaves exactly GPIO21+22 free", free_pins(make_full_rig()) == [21, 22])
    T("full rig leaves nothing free when I2C reserved", free_pins(make_full_rig(), expanders=True) == [])
    # R-L9: in the direct full rig the LED fits only on a spare I2C pin.
    rig = make_full_rig()
    rig.append(LED(21))
    T("full rig + LED on GPIO21 is valid", validate(rig) == [])
    T("full rig + LED leaves exactly GPIO22 free", free_pins(rig) == [22])
    T("full rig + LED on used pin rejected", any("already used" in e for e in validate(make_full_rig() + [LED(4)])))
    T("full rig + LED on GPIO21 rejected with expanders (I2C)", any("already used" in e for e in validate(make_full_rig() + [LED(21)], expanders=True)))
    rig = make_full_rig()
    rig.append(LED(21))
    rig.append(E(22, 4))
    T("3rd encoder does not fit in full rig + LED (pin budget)", any("already used" in e for e in validate(rig)))

    # --- presets from docs/board-esp32-devkitv1.json (single source of truth) ---
    for pname, preset in BOARD["presets"].items():
        mods = copy.deepcopy(preset["modules"])
        T(f"preset {pname} is valid", validate(mods, expanders=preset["expanders"]) == [])
        T(f"preset {pname} free pins exact",
          sorted(free_pins(mods, expanders=preset["expanders"])) == sorted(preset["expected_free_pins"]))

    # --- T7 limit tests (D6 bounds: axes <= 8, buttons <= 128, encoders <= 4) ---
    T("9 axes rejected (6 direct + 3 ADS1115)",
      any("too many axes" in e for e in validate([A(32), A(33), A(34), A(35), A(36), A(39), ADS(0), ADS(1), ADS(2)], expanders=True)))
    T("8 axes valid (6 direct + 2 ADS1115, boundary)",
      validate([A(32), A(33), A(34), A(35), A(36), A(39), ADS(0), ADS(1)], expanders=True) == [])
    T("5 encoders rejected (limit + 1)",
      any("too many encoders" in e for e in validate([E(4, 13), E(14, 16), E(17, 18), E(19, 23), E(25, 26)])))
    T("4 encoders valid (boundary, D6)",
      validate([E(4, 13), E(14, 16), E(17, 18), E(19, 23)]) == [])
    # NOTE on the plan text "3-encoder reject": D6 / FW-2 bound is nEnc <= 4, so a
    # standalone 3-encoder config is VALID. Only the 3rd encoder *inside the
    # full rig + LED* is rejected (pin budget -- see the test above).
    T("3 encoders valid standalone (D6: nEnc <= 4)",
      validate([E(4, 13), E(14, 16), E(17, 18)]) == [])
    T("129 buttons rejected (limit + 1)", any("too many buttons" in e for e in validate([XB() for _ in range(129)], expanders=True)))
    T("128 buttons valid (boundary, D6)", validate([XB() for _ in range(128)], expanders=True) == [])
    T("button_matrix 4x4 valid (16 buttons on 8 pins)",
      validate([MX([4, 13, 14, 16, 17, 18, 19, 23])]) == [])
    T("button_matrix wrong pin count rejected",
      any("needs 8 pins" in e for e in validate([MX([4, 13, 14, 16, 17, 18, 19])])))
    T("button_matrix 2x3 valid (6 buttons on 5 pins)",
      validate([MX([4, 13, 14, 16, 17], rows=2, cols=3)]) == [])
    T("button_matrix counts toward button limit (16 + 113 = 129 rejected)",
      any("too many buttons" in e for e in validate([MX([4, 13, 14, 16, 17, 18, 19, 23])] + [XB() for _ in range(113)], expanders=True)))
    T("button_matrix counts toward button limit (16 + 112 = 128 valid)",
      validate([MX([4, 13, 14, 16, 17, 18, 19, 23])] + [XB() for _ in range(112)], expanders=True) == [])

    # --- T7 battery fix: a battery is NOT an axis ---
    T("battery on non-ADC pin rejected", any("ADC1" in e for e in validate([BAT(25)])))
    T("two batteries rejected", any("too many battery" in e for e in validate([BAT(39), BAT(36)])))
    T("battery not counted as axis (8 axes + battery valid)",
      validate([A(32), A(33), A(34), A(35), A(36), ADS(0), ADS(1), ADS(2), BAT(39)], expanders=True) == [])

    # --- T7 new kinds: ADS1115 / expander / LED / encoder shape / unknown kinds ---
    T("axis_ads1115 without expanders rejected", any("requires I2C" in e for e in validate([ADS(0)])))
    T("axis_ads1115 with GPIO pin rejected", any("no GPIO pin" in e for e in validate([{"kind": "axis_ads1115", "pins": [32], "channel": 0}], expanders=True)))
    T("axis_ads1115 channel out of range rejected", any("out of range" in e for e in validate([ADS(4)], expanders=True)))
    T("axis_ads1115 duplicate channel rejected",
      any("already used" in e for e in validate([ADS(0), ADS(0)], expanders=True)))
    T("axis_ads1115 same channel on other addr valid",
      validate([ADS(0), ADS(0, addr=0x49)], expanders=True) == [])
    T("expander_button without expanders rejected", any("requires I2C" in e for e in validate([XB()])))
    T("expander_button with GPIO pin rejected", any("no ESP32 GPIO pin" in e for e in validate([{"kind": "expander_button", "pins": [4]}], expanders=True)))
    T("expander_button duplicate pin rejected",
      any("already used" in e for e in validate([XB(expander_pin=0), XB(expander_pin=0)], expanders=True)))
    T("expander_button same pin on other addr valid",
      validate([XB(expander_pin=0), XB(expander_pin=0, addr=0x21)], expanders=True) == [])
    T("expander_button PCF8574 pin 8 rejected", any("out of range" in e for e in validate([XB(expander="PCF8574", expander_pin=8)], expanders=True)))
    T("expander_button MCP23017 pin 16 rejected", any("out of range" in e for e in validate([XB(expander_pin=16)], expanders=True)))
    T("2 LED strips rejected", any("too many LED strips" in e for e in validate([LED(4), LED(13)])))
    T("LED count 61 rejected", any("out of range" in e for e in validate([LED(4, count=61)])))
    T("LED count 60 valid (boundary)", validate([LED(4, count=60)]) == [])
    T("encoder with 1 pin rejected", any("exactly 2 pins" in e for e in validate([{"kind": "encoder", "pins": [23]}])))
    T("encoder with 3 pins rejected (push is a separate button)",
      any("exactly 2 pins" in e for e in validate([{"kind": "encoder", "pins": [23, 25, 26]}])))
    T("unknown kind rejected, no crash", any("unknown kind" in e for e in validate([{"kind": "teleport", "pins": [4]}])))
    T("axis_as5600 not yet a validator kind (D34, future)",
      any("unknown kind" in e for e in validate([{"kind": "axis_as5600", "pins": []}])))
    print(f"\n{'ALL PASS' if not fails else str(fails)+' FAILED'}"); raise SystemExit(1 if fails else 0)
