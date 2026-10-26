#!/bin/sh
# Builds the standalone test binary on a Linux machine: ./build_standalone.sh
cd "$(dirname "$0")" && gcc -std=gnu11 -Wall -Wextra -O2 -o thermopsis_standalone thermopsis_sensors.c thermopsis_core.c thermopsis_standalone.c -ldl
