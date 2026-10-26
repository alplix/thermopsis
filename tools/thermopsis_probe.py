#!/usr/bin/env python3
"""Thermopsis probe (P0): what does THIS Linux machine expose? Read-only, no root needed, standard library only.

Prints one JSON document: the inventory (hardware kind, no serial numbers, no MAC, no host name, no IP) and one sample of every temperature,
fan, power and load value it can read. Use it to see what real hardware exposes before the monitor is written:

    python3 thermopsis_probe.py            # JSON on stdout
    python3 thermopsis_probe.py --text     # a short human-readable summary

The probe never writes anything and never calls a command that needs root. NVIDIA values come from `nvidia-smi` if it exists
(the monitor itself will use NVML through dlopen).
"""
import glob
import json
import os
import platform
import shutil
import subprocess
import sys
import time


def rd(path, default=None):
    try:
        with open(path, "r", errors="replace") as f:
            return f.read().strip()
    except OSError:
        return default


def num(path, scale=1.0):
    v = rd(path)
    try:
        return float(v) / scale
    except (TypeError, ValueError):
        return None


def cpu_info():
    model, threads, mhz = None, 0, []
    for line in (rd("/proc/cpuinfo", "") or "").splitlines():
        if line.startswith("model name") and model is None:
            model = line.split(":", 1)[1].strip()
        elif line.startswith("Hardware") and model is None:      # many ARM boards
            model = line.split(":", 1)[1].strip()
        elif line.startswith("processor"):
            threads += 1
        elif line.startswith("cpu MHz"):
            try:
                mhz.append(float(line.split(":", 1)[1]))
            except ValueError:
                pass
    cores = len({(rd(p + "/topology/physical_package_id"), rd(p + "/topology/core_id"))
                 for p in glob.glob("/sys/devices/system/cpu/cpu[0-9]*")})
    return {"model": model, "threads": threads, "cores": cores or None, "mhz_mean": round(sum(mhz) / len(mhz), 1) if mhz else None,
            "microcode": next((l.split(":", 1)[1].strip() for l in (rd("/proc/cpuinfo", "") or "").splitlines() if l.startswith("microcode")), None)}


def dmi():
    d = "/sys/class/dmi/id/"
    return {k: rd(d + k) for k in ("sys_vendor", "product_name", "board_vendor", "board_name", "bios_vendor", "bios_version", "chassis_type")}


def meminfo():
    m = {}
    for line in (rd("/proc/meminfo", "") or "").splitlines():
        k, _, v = line.partition(":")
        if k in ("MemTotal", "MemAvailable", "SwapTotal", "SwapFree"):
            m[k] = int(v.split()[0]) if v.split() else None
    return m


def hwmon():
    out = []
    for d in sorted(glob.glob("/sys/class/hwmon/hwmon*")):
        chip = {"name": rd(d + "/name"), "temps": [], "fans": [], "power": []}
        for f in sorted(glob.glob(d + "/temp*_input")):
            base = f[:-6]
            chip["temps"].append({"label": rd(base + "_label") or os.path.basename(base), "c": num(f, 1000.0),
                                  "max": num(base + "_max", 1000.0), "crit": num(base + "_crit", 1000.0)})
        for f in sorted(glob.glob(d + "/fan*_input")):
            chip["fans"].append({"label": rd(f[:-6] + "_label") or os.path.basename(f[:-6]), "rpm": num(f)})
        for f in sorted(glob.glob(d + "/power*_average") + glob.glob(d + "/power*_input")):
            chip["power"].append({"label": os.path.basename(f), "w": num(f, 1e6)})
        out.append(chip)
    return out


def thermal_zones():
    return [{"type": rd(z + "/type"), "c": num(z + "/temp", 1000.0)} for z in sorted(glob.glob("/sys/class/thermal/thermal_zone*"))]


def throttle_counts():
    tot = 0
    for f in glob.glob("/sys/devices/system/cpu/cpu*/thermal_throttle/core_throttle_count"):
        v = num(f)
        tot += int(v) if v else 0
    return tot


def storage():
    out = []
    for b in sorted(glob.glob("/sys/block/*")):
        n = os.path.basename(b)
        if n.startswith(("loop", "ram", "zram", "dm-", "md", "sr", "fd")):
            continue
        sectors = num(b + "/size")
        out.append({"dev": n, "model": rd(b + "/device/model") and " ".join(rd(b + "/device/model").split()),
                    "rotational": rd(b + "/queue/rotational"), "gb": round(sectors * 512 / 1e9, 1) if sectors else None})
    return out


def gpus():
    out = []
    for c in sorted(glob.glob("/sys/class/drm/card[0-9]")):
        dev = c + "/device"
        out.append({"card": os.path.basename(c), "vendor": rd(dev + "/vendor"), "device": rd(dev + "/device"),
                    "driver": os.path.basename(os.path.realpath(dev + "/driver")) if os.path.exists(dev + "/driver") else None})
    nv = []
    if shutil.which("nvidia-smi"):
        try:
            r = subprocess.run(["nvidia-smi", "--query-gpu=name,driver_version,memory.total,temperature.gpu,power.draw,fan.speed,clocks.sm,utilization.gpu",
                                "--format=csv,noheader,nounits"], capture_output=True, text=True, timeout=10)
            for line in r.stdout.splitlines():
                p = [x.strip() for x in line.split(",")]
                if len(p) == 8:
                    nv.append(dict(zip(("name", "driver", "vram_mb", "temp_c", "power_w", "fan_pct", "clock_mhz", "util_pct"), p)))
        except (OSError, subprocess.SubprocessError):
            pass
    return {"drm": out, "nvidia_smi": nv}


def power_supply():
    return [{"name": os.path.basename(p), "type": rd(p + "/type"), "status": rd(p + "/status"), "online": rd(p + "/online"),
             "temp_c": num(p + "/temp", 10.0)} for p in sorted(glob.glob("/sys/class/power_supply/*"))]


def root_only_facts():
    """What exists but cannot be read without root: the monitor will list it in the sensor inventory so that 'not readable' is not mistaken for 'absent'."""
    facts = {}
    facts["smbios_memory_entries"] = len(glob.glob("/sys/firmware/dmi/entries/17-*")) or 0
    facts["smbios_memory_readable"] = bool(glob.glob("/sys/firmware/dmi/entries/17-*/raw")) and os.access(next(iter(glob.glob("/sys/firmware/dmi/entries/17-*/raw")), "/nonexistent"), os.R_OK)
    facts["rapl_present"] = os.path.exists("/sys/class/powercap/intel-rapl") or bool(glob.glob("/sys/class/powercap/*"))
    facts["rapl_energy_readable"] = any(os.access(f, os.R_OK) for f in glob.glob("/sys/class/powercap/*/energy_uj"))
    facts["i2c_spd_eeprom_devices"] = len(glob.glob("/sys/bus/i2c/devices/*-005[0-7]"))
    facts["dimm_temp_sensor_hwmon"] = any((c.get("name") or "").startswith(("jc42", "spd5118")) for c in hwmon())
    return facts


def virtual_environment():
    vendor = (rd("/sys/class/dmi/id/sys_vendor", "") or "").lower()
    return {"vm_vendor_hint": any(x in vendor for x in ("vmware", "qemu", "innotek", "microsoft", "xen", "amazon", "google", "kvm")),
            "container_hint": os.path.exists("/.dockerenv") or "container" in (rd("/proc/1/environ", "") or "")}


def main():
    t0 = time.time()
    probe = {
        "probe_version": 1,
        "inventory": {
            "cpu": cpu_info(), "dmi": dmi(), "memory": meminfo(), "storage": storage(), "gpus": gpus(),
            "kernel": platform.release(), "machine": platform.machine(), "os": rd("/etc/os-release", "").split("PRETTY_NAME=")[-1].split("\n")[0].strip('"') if "PRETTY_NAME" in (rd("/etc/os-release", "") or "") else None,
            "environment": virtual_environment(), "root_only": root_only_facts(),
        },
        "sample": {
            "hwmon": hwmon(), "thermal_zones": thermal_zones(), "cpu_throttle_count_total": throttle_counts(),
            "loadavg": (rd("/proc/loadavg", "") or "").split()[:3], "power_supply": power_supply(),
        },
    }
    probe["probe_seconds"] = round(time.time() - t0, 3)
    if "--text" in sys.argv:
        inv, s = probe["inventory"], probe["sample"]
        print("CPU:", inv["cpu"]["model"], "|", inv["cpu"]["cores"], "cores /", inv["cpu"]["threads"], "threads")
        print("System:", inv["dmi"].get("product_name"), "| board", inv["dmi"].get("board_name"), "| RAM", round((inv["memory"].get("MemTotal") or 0) / 1048576, 1), "GiB")
        print("GPUs:", [g["name"] for g in inv["gpus"]["nvidia_smi"]] or [g["driver"] for g in inv["gpus"]["drm"]])
        print("Storage:", [(d["dev"], d["model"]) for d in inv["storage"]])
        print("Root-only/absent:", inv["root_only"])
        n = 0
        for c in s["hwmon"]:
            for t in c["temps"]:
                n += 1
                print(f"  hwmon {c['name']:10s} {t['label']:22s} {t['c']} C (max {t['max']}, crit {t['crit']})")
            for f in c["fans"]:
                print(f"  hwmon {c['name']:10s} {f['label']:22s} {f['rpm']} rpm")
        for z in s["thermal_zones"]:
            n += 1
            print(f"  zone  {z['type']:14s} {z['c']} C")
        print(f"{n} temperature readings in {probe['probe_seconds']} s")
    else:
        print(json.dumps(probe, indent=1))


if __name__ == "__main__":
    main()
