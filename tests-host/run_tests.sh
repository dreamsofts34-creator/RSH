#!/bin/sh
# Host-side firmware logic tests (no Arduino needed). Run from anywhere.
set -e
cd "$(dirname "$0")"
g++ -std=c++17 -O1 -I ../firmware/apexlink -o run_tests test_main.cpp
./run_tests
