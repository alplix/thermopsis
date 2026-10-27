#!/bin/sh
# builds dist/Thermopsis-Setup-<ver>.exe: agent + window (cargo, gui feature), the sensor helper (dotnet), then NSIS
set -e
cd "$(dirname "$0")/.."
(cd agent && cargo build --release --features gui)
(cd sensor-helper && dotnet build -c Release)
rm -rf dist/app && mkdir -p dist/app
cp agent/target/release/thermopsis.exe agent/target/release/thermopsis-agent.exe dist/app/
cp -r sensor-helper/bin/Release/net8.0-windows/* dist/app/
"${MAKENSIS:-/c/Users/Alp/AppData/Local/tauri/NSIS/makensis.exe}" //V2 installer/thermopsis.nsi
