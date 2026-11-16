# RSH firmware — CMakeLists for the esp32dev-secure env
# (PlatformIO `arduino, espidf` IDF source build; the default esp32dev env
# uses precompiled Arduino and never reads this file).
#
# This file has TWO roles (src_dir = . in platformio.ini, so the project
# dir and the component dir are the same — PlatformIO's normal two-file
# template collapses into one):
#
#   1. Top-level project file: include project.cmake + project(firmware).
#   2. IDF component file: IDF includes this file a second time as the
#      `firmware` component (EXTRA_COMPONENT_DIRS contains the project
#      src dir) with __idf_component_context == 1 — then it must call
#      idf_component_register, otherwise the __idf_firmware library
#      never exists and PlatformIO fails with "Couldn't find the main
#      target of the project!".
#
# Script-mode note (v4.0.2): IDF's component_get_requirements.cmake
# includes this file with `cmake -P` (script mode) — `project()` /
# `add_library` are not scriptable, and IDF 4.4.7's own idf.cmake calls
# __build_init (add_library) unconditionally at include time, so the
# project branch below is guarded. Lekin component registration script
# mode mein bhi lazmi hai (Role 2) — sirf project() guarded hai,
# register nahi.
cmake_minimum_required(VERSION 3.16.0)

# --- Role 2: IDF firmware component --------------------------------------
# Do contexts mein chalta hai — DONO mein idf_component_register lazmi hai:
#   * Project mode, component inclusion (__idf_component_context == 1):
#     asal register — __idf_firmware library banti hai.
#   * Script mode (CMAKE_SCRIPT_MODE_FILE, IDF ki requirements pass):
#     yahan idf_component_register IDF ka STUB macro hai (sirf REQUIRES
#     record karta hai). Agar yahan register na ho to component ki
#     __COMPONENT_REGISTERED property 0 rehti hai aur __build_expand_requirements
#     use build list se nikaal deta hai — phir project mode mein bhi
#     component kabhi add nahi hota (yehi wajah thi ke pehle firmware
#     build_components mein nahi aata tha).
# Dono branches ke baad return — neeche wala project code yahan nahi chalna.
if(__idf_component_context OR CMAKE_SCRIPT_MODE_FILE)
  file(GLOB app_sources
    "${CMAKE_CURRENT_LIST_DIR}/*.cpp"
    "${CMAKE_CURRENT_LIST_DIR}/*.c")
  # rsh.ino.cpp: PlatformIO converts rsh.ino before CMake runs (transient
  # file, deleted at pio exit) — the glob above picks it up when present.
  # tests-host/ is a subdirectory, is liye glob mein nahi aata.
  idf_component_register(
    SRCS ${app_sources}
    INCLUDE_DIRS "."
    REQUIRES framework-arduinoespressif32 driver esp_wifi spi_flash)
  return()
endif()

# --- Role 1: top-level project file --------------------------------------
if(NOT CMAKE_SCRIPT_MODE_FILE)
  include($ENV{IDF_PATH}/tools/cmake/project.cmake)
  project(firmware)
endif()
