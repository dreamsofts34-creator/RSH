# Firmware host tests (vendored reference copies — M9)

Vendored from `rust-recreation/reference/tests-host/` (the reference
copies used to build the firmware — kept here so the repo is
self-contained and CI can run them without the handover folder):

- `protocol_v2_vectors.py` — protocol v2.1 golden vectors:
  `VECTORS` (10) + `NEGATIVE` (6). Run: `python3 protocol_v2_vectors.py`
- `verify_packet.py` — packet cross-verify (Python vs Rust vs C++
  must agree). Run: `python3 verify_packet.py`
- `test_main.cpp` — C++ host tests for the firmware logic in
  `firmware/` (protocol + FW-3..FW-12 + Phase 2 `FACTORY_RESET`).
  No Arduino/board needed; headers are included by bare name and
  resolve from `firmware/` (so `-I..` from this directory).

## Run

```sh
cd firmware/tests-host

# 1. C++ host tests (needs libsodium for the Ed25519 §20 tests)
SODIUM=$(ls /lib/x86_64-linux-gnu/libsodium.so* /usr/lib/x86_64-linux-gnu/libsodium.so* 2>/dev/null | head -1)
g++ -std=c++17 -O1 -g -Wall -Wextra -Werror -fsanitize=address,undefined \
  -I.. -o run_tests test_main.cpp "$SODIUM"
./run_tests

# 2. Python golden vectors
python3 -I protocol_v2_vectors.py
python3 -I verify_packet.py
```

(The original `run_tests.sh` in the reference folder does the same
steps plus `test_host_cpp.cpp` / `pin_rules_ref.py` /
`test_merge_bin_mock.py`, which live outside this repo; its
`-I ../firmware/rsh` include path is `-I..` here because the repo's
firmware headers live directly in `firmware/`, not `firmware/rsh/`.)

## Vector counts (corrected — M9)

- Python reference: 10 positive + 6 negative (`protocol_v2_vectors.py`).
- Rust `crates/rsh-protocol/tests/golden_vectors.rs`: **18 tests — 11
  positive + 7 negative**.
- These C++ host tests mirror the same vectors; the count lives in
  `test_main.cpp` section headers, not here.
