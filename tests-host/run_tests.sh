#!/bin/sh
# Host-side firmware logic tests (no Arduino/board needed). Run from anywhere.
# FW-13 (T6): sakht flags — g++ -Wall -Wextra -Werror -fsanitize=address,undefined.
# Python reference scripts bhi yahin se chalte hain (pehle sirf CI mein thay).
set -e
cd "$(dirname "$0")"

echo "== 1. C++ host tests (test_main.cpp: protocol + FW-3..FW-12 logic) =="
# FW-9 §20: Ed25519 host tests ko libsodium chahiye (verify primitive —
# firmware wahi library istemal karta hai). Dev symlink (-lsodium) aksar
# nahi hota; versioned .so dhoondho (ye machine: libsodium.so.23).
SODIUM_LIB=""
for d in /lib/x86_64-linux-gnu /usr/lib/x86_64-linux-gnu /usr/lib /lib; do
  for f in "$d"/libsodium.so*; do
    [ -e "$f" ] && SODIUM_LIB="$f" && break 2
  done
done
[ -n "$SODIUM_LIB" ] || { echo "libsodium nahi mila — Ed25519 host tests ke liye chahiye"; exit 1; }
g++ -std=c++17 -O1 -g -Wall -Wextra -Werror -fsanitize=address,undefined \
  -I ../firmware/rsh -o run_tests test_main.cpp "$SODIUM_LIB"
./run_tests

echo "== 2. Firmware .cpp on host (Arduino stub: inputs.cpp + status_led.cpp) =="
g++ -std=c++17 -O1 -g -Wall -Wextra -Werror -fsanitize=address,undefined \
  -I ../firmware/rsh -I arduino_stub -o run_tests_host_cpp test_host_cpp.cpp
./run_tests_host_cpp mv-ok
./run_tests_host_cpp mv-fallback

echo "== 3. Python: pin rules reference =="
python3 -I pin_rules_ref.py

echo "== 4. Python: protocol v2.1 golden vectors =="
python3 -I protocol_v2_vectors.py

echo "== 5. Python: packet cross-verify (3-language match) =="
python3 -I verify_packet.py

echo "== 6. Python: merge_bin failure mock (F15: merge fail = build fail) =="
python3 -I test_merge_bin_mock.py

echo "== HOST TESTS: ALL PASS =="
