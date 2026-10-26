#!/bin/bash
# Builds the Thermopsis monitor as static BOINC apps against per-target BOINC builds in /opt/bitboinc-src/boinc-<target> (edit the path to your own build trees).
#   ./build_thermopsis_monitor.sh [linux64 aarch64]     Output: out/thermopsis_v<VER>_<platform>
VER=3
mkdir -p out
SRC="thermopsis_sensors.c thermopsis_core.c"
build() {  # tag cxx boinc-dir output
    local tag=$1 cxx=$2 B=/opt/bitboinc-src/$3 outf=out/$4 o=obj_$1 ok=1 objs=""
    mkdir -p $o; echo "=== $tag ==="
    for f in $SRC; do $cxx -O2 -x c -std=gnu11 -Wall -Wextra -DTS_STATIC_BUILD -I . -c $f -o $o/${f%.c}.o || ok=0; objs="$objs $o/${f%.c}.o"; done
    $cxx -O2 -std=c++17 -I . -I $B/api -I $B/lib -c thermopsis_boinc.cpp -o $o/boinc.o || ok=0
    if [ $ok = 1 ] && $cxx -static $objs $o/boinc.o $B/api/libboinc_api.a $B/lib/libboinc.a -lpthread -lrt -lm -o $outf; then file $outf; else echo "$tag FAILED"; fi
}
sel=("$@")
run() { local tag=$1; shift; if [ ${#sel[@]} -eq 0 ] || [[ " ${sel[*]} " == *" $tag "* ]]; then build "$tag" "$@"; fi; }
run linux64 g++ boinc thermopsis_v${VER}_linux_x86_64
run aarch64 aarch64-linux-gnu-g++ boinc-aarch64 thermopsis_v${VER}_linux_aarch64
ls -la out/
