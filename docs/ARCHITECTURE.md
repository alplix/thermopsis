# Thermopsis: architecture

Thermopsis collects temperatures and power figures from volunteers' Linux computers. It is a non-CPU-intensive BOINC application: BOINC starts one monitor task per computer next to the volunteer's other tasks. The monitor needs no root rights and no open ports.

## What the monitor reads (all read-only, a missing file only means "no such sensor")

| Kind | Source |
|---|---|
| Temperatures | `/sys/class/hwmon/*/temp*_input` (processor, drives, board, graphics, network chips) with the chip's name and the sensor's label; `/sys/class/thermal/thermal_zone*/temp`; battery temperature. Values outside -40..150 °C are ignored. |
| Fans, power | `fan*_input`, `power*_average` / `power*_input` of hwmon; processor package power from the RAPL energy counter (`/sys/class/powercap/intel-rapl:*`, only where readable); battery discharge power while the computer runs on battery |
| Processor | package throttling counter (`thermal_throttle/core_throttle_count`), mean frequency, utilisation, load average |
| Memory | used share of RAM and swap |
| NVIDIA graphics | temperature, board power, fan, clock, utilisation, thermal-throttle flag: through NVML, or through `nvidia-smi` in the static builds |
| Battery, AC | capacity, status, adapter online |

Keys have the form `<kind>.<source>.<name>`; kinds: `t` temperature (°C), `f` fan (rpm), `p` power (W), `c` clock (MHz), `u` utilisation or load, `m` memory share, `b` battery, `x` counters and flags.

## What the monitor sends

Every sample interval (default 60 s) it reads all sensors; every report interval (default 600 s) it sends one report with the lowest, mean and highest value of every sensor in that window. The intervals can be set in the project preferences (`sample_seconds` 10..600, `report_seconds` 60..3600).

```json
{"v":1,"t":1793043979,"w":600,"s":11,"inv":"e658d60f",
 "inventory":{...},                       // only with the first report and when the hardware changed
 "m":{"t.hwmon.coretemp.Package id 0":[40,45,48], "p.nvml.0.board":[4.4,4.5,4.6], ...}}
```

The inventory describes the kind of hardware: processor model and thread count, system and board model, BIOS vendor and version, chassis type, RAM total, the number of memory modules, drive models, types and sizes, graphics chips (PCI ids, driver, name from `pci.ids`), kernel, distribution, hints for a virtual machine or container, and the list of temperature sensors with their limits.

It never contains serial numbers, MAC or IP addresses, the computer's name, UUIDs, user names or locations. Details of memory modules (vendor, size, serial) need root rights and are not read.

## Behaviour

* If the computer has no temperature sensor at all, the monitor sends one report with `"nosensors":true` and finishes successfully. It never defers, sleeps in a loop or exits with a temporary-exit code, so a client never sees a waiting task.
* A task ends by itself after three days; the next one follows. A checkpoint file keeps the inventory hash and the running time across restarts.
* Sampling is based on the wall clock, not on counting sleeps: BOINC's timer signals shorten `sleep()`.

## Server side (not part of this repository)

The reports arrive as BOINC trickle messages. A job stores one record per computer and report, builds daily summaries per computer and sensor, derives product and analysis values (idle and load temperature, throttling, power) and grants credit per valid six-hour block (at least 80 % of the expected samples). Public pages show products (processor, graphics card, computer model, drive) with the number of computers and averages; a computer is never shown.

## Limits

* Virtual machines and most containers have no sensors and are marked unsupported.
* Memory temperature exists only on boards with a DIMM sensor and a loaded driver (`jc42`, `spd5118`).
* The processor's energy counter is root-only on many systems; power figures are lower bounds that cover only the parts that can be measured, not the power taken from the wall.
* Sensor labels and accuracy differ between vendors; comparing a product with itself is more reliable than comparing different products.
