# Thermopsis

Thermopsis measures the temperatures and the power draw of volunteers' computers for the [BitBOINC](https://bitboinc.athena.org.tr/) project and analyses them together:
which hardware stays cool under sustained load, which parts throttle, how seasons and ageing change it. Linux only (x86-64 and ARM64), opt-in, no personal data.

Results and the volunteer-facing description: <https://bitboinc.athena.org.tr/thermopsis.php>

## Contents

| Path | What it is |
|---|---|
| `monitor/` | The monitor, written in C (BOINC glue in C++). Reads the sensors once a minute and sends a short summary every ten minutes. |
| `tools/thermopsis_probe.py` | Shows what a Linux machine exposes: hardware inventory and one sample of every sensor, as JSON. Read-only, no root, standard library only. |
| `docs/ARCHITECTURE.md` | What is collected, the report format, privacy, data flow. |

## Try it without BOINC

```sh
cd monitor
./build_standalone.sh
./thermopsis_standalone --cycles 1 --sample 1 --report 3   # one report as JSON on stdout
./thermopsis_standalone --inventory                        # the hardware inventory that would be sent
```

`--sysroot DIR` reads a recorded `/sys` and `/proc` tree instead of the real system. `tools/thermopsis_probe.py --text` prints a readable summary of what the machine exposes.

`monitor/build_thermopsis_monitor.sh` builds the static BOINC applications against per-target BOINC builds (edit the paths at its top).

## License

Proprietary, all rights reserved. The source is published for inspection; see `LICENSE`.
