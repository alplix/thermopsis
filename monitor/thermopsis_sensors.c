/* Thermopsis monitor: reading the sensors and describing the hardware. Linux. See docs/DESIGN.md.
 * Everything is read-only, needs no root, and a missing file only means "no such sensor". */
#define _GNU_SOURCE
#include "thermopsis.h"

#include <ctype.h>
#include <dirent.h>
#ifndef TS_STATIC_BUILD
#include <dlfcn.h>
#endif
#include <glob.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/utsname.h>
#include <unistd.h>

/* ---------------------------------------------------------------- small helpers */

static const char *root_of(const char *sysroot) { return (sysroot && sysroot[0] && strcmp(sysroot, "/") != 0) ? sysroot : ""; }
static int is_real(const char *sysroot) { return root_of(sysroot)[0] == 0; }

static void path_join(char *out, size_t cap, const char *root, const char *rel) { snprintf(out, cap, "%s%s", root, rel); }

/* read a small text file, trimmed; returns 0 on success */
static int read_file(const char *path, char *buf, size_t cap)
{
    FILE *f = fopen(path, "r");
    if (!f) return -1;
    size_t n = fread(buf, 1, cap - 1, f);
    fclose(f);
    buf[n] = 0;
    while (n > 0 && (buf[n - 1] == '\n' || buf[n - 1] == '\r' || buf[n - 1] == ' ')) buf[--n] = 0;
    return 0;
}

static int read_num(const char *path, double *v)
{
    char b[64];
    if (read_file(path, b, sizeof b) != 0 || !b[0]) return -1;
    char *end;
    double x = strtod(b, &end);
    if (end == b) return -1;
    *v = x;
    return 0;
}

static int add_reading(ts_reading_t *out, int *n, int max, double v, const char *fmt, ...)
{
    if (*n >= max) return 0;
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(out[*n].key, TS_KEY_LEN, fmt, ap);
    va_end(ap);
    out[*n].v = v;
    (*n)++;
    return 1;
}

/* a key part: no control characters, no dots (the dot separates the parts) */
static void clean(char *s)
{
    for (; *s; s++) {
        if ((unsigned char)*s < 32 || *s == '.' || *s == '"' || *s == '\\') *s = '_';
    }
}

/* the same chip name can appear twice (two NVMe drives): the second is "name#2" (order of the hwmon numbers; stable between boots) */
typedef struct { char name[48]; int count; } seen_t;
static int occurrence(seen_t *seen, int *nseen, const char *name)
{
    for (int i = 0; i < *nseen; i++)
        if (strcmp(seen[i].name, name) == 0) return ++seen[i].count;
    if (*nseen < 64) {
        snprintf(seen[*nseen].name, sizeof seen[*nseen].name, "%s", name);
        seen[*nseen].count = 1;
        (*nseen)++;
    }
    return 1;
}

static int globber(const char *pattern, glob_t *g) { return glob(pattern, 0, NULL, g); }

/* ---------------------------------------------------------------- GPU library (NVIDIA), loaded at run time */

__attribute__((unused)) static int smi_fields(char *line, char **f, int maxf)
{
    int k = 0;
    line[strcspn(line, "\r\n")] = 0;
    char *p = line;
    while (k < maxf) {
        f[k++] = p;
        char *c = strstr(p, ", ");
        if (!c) break;
        *c = 0;
        p = c + 2;
    }
    return k;
}
__attribute__((unused)) static int smi_num(const char *s, double *v)       /* "[N/A]", "[Not Supported]" and empty fields have no value */
{
    if (!s || !s[0] || s[0] == '[') return -1;
    char *end;
    *v = strtod(s, &end);
    return end == s ? -1 : 0;
}
#ifdef TS_STATIC_BUILD
/* A static binary cannot dlopen the driver library reliably (glibc mismatch), so the driver's own tool is asked instead (it is only run when it exists). */
static void nvml_read(ts_reading_t *out, int *n, int max)
{
    static int have = -1;
    if (have == 0) return;
    FILE *f = popen("nvidia-smi --query-gpu=index,temperature.gpu,power.draw,fan.speed,clocks.sm,utilization.gpu,"
                    "clocks_throttle_reasons.hw_thermal_slowdown,clocks_throttle_reasons.sw_thermal_slowdown,"
                    "memory.used,memory.total,clocks.mem,clocks.max.mem,clocks.max.sm,power.limit,pstate,"
                    "clocks_throttle_reasons.sw_power_cap,clocks_throttle_reasons.hw_slowdown,clocks_throttle_reasons.hw_power_brake_slowdown,"
                    "pcie.link.gen.current,pcie.link.gen.max,pcie.link.width.current,pcie.link.width.max --format=csv,noheader,nounits 2>/dev/null", "r");
    if (!f) return;
    char line[600];
    int rows = 0;
    while (fgets(line, sizeof line, f)) {
        char *fl[24];
        double v, t, mu = -1, mt = -1;
        if (smi_fields(line, fl, 24) < 6 || smi_num(fl[0], &v) != 0) continue;
        int i = (int)v;
        rows++;
        if (smi_num(fl[1], &v) == 0) add_reading(out, n, max, v, "t.nvml.%d.gpu", i);
        if (smi_num(fl[2], &v) == 0) add_reading(out, n, max, v, "p.nvml.%d.board", i);
        if (smi_num(fl[3], &v) == 0) add_reading(out, n, max, v, "f.nvml.%d.fan_percent", i);
        if (smi_num(fl[4], &v) == 0) add_reading(out, n, max, v, "c.nvml.%d.sm_mhz", i);
        if (smi_num(fl[5], &v) == 0) add_reading(out, n, max, v, "u.nvml.%d.gpu_percent", i);
        #define ACTIVE(x) ((x) && strstr((x), "Active") && !strstr((x), "Not"))
        t = (ACTIVE(fl[6]) || ACTIVE(fl[7])) ? 1.0 : 0.0;
        if (fl[6][0] && fl[7][0]) add_reading(out, n, max, t, "x.nvml.%d.thermal_throttle", i);
        if (smi_num(fl[8], &mu) == 0 && smi_num(fl[9], &mt) == 0 && mt > 0) {
            add_reading(out, n, max, 100.0 * mu / mt, "m.nvml.%d.vram_used_percent", i);
            add_reading(out, n, max, mu, "x.nvml.%d.vram_used_mb", i);
        }
        if (smi_num(fl[10], &v) == 0) add_reading(out, n, max, v, "c.nvml.%d.mem_mhz", i);
        if (smi_num(fl[11], &v) == 0) add_reading(out, n, max, v, "c.nvml.%d.mem_max_mhz", i);
        if (smi_num(fl[12], &v) == 0) add_reading(out, n, max, v, "c.nvml.%d.sm_max_mhz", i);
        if (smi_num(fl[13], &v) == 0) add_reading(out, n, max, v, "p.nvml.%d.limit", i);
        if (fl[14][0] == 'P' && smi_num(fl[14] + 1, &v) == 0) add_reading(out, n, max, v, "x.nvml.%d.pstate", i);
        if (fl[15][0]) add_reading(out, n, max, ACTIVE(fl[15]) ? 1.0 : 0.0, "x.nvml.%d.thr_power", i);
        if (fl[16][0] && fl[17][0]) add_reading(out, n, max, (ACTIVE(fl[16]) || ACTIVE(fl[17])) ? 1.0 : 0.0, "x.nvml.%d.thr_hardware", i);
        if (smi_num(fl[18], &v) == 0) add_reading(out, n, max, v, "c.nvml.%d.pcie_gen", i);
        if (smi_num(fl[19], &v) == 0) add_reading(out, n, max, v, "c.nvml.%d.pcie_gen_max", i);
        if (smi_num(fl[20], &v) == 0) add_reading(out, n, max, v, "c.nvml.%d.pcie_width", i);
        if (smi_num(fl[21], &v) == 0) add_reading(out, n, max, v, "c.nvml.%d.pcie_width_max", i);
        #undef ACTIVE
    }
    int st = pclose(f);
    if (rows == 0 && st != 0) have = 0;   /* no such tool: do not try again */
}
#else
typedef struct { unsigned int gpu, memory; } nvml_util_t;
static void *nvml_lib;
static int nvml_ready = -1;   /* -1 not tried, 0 unavailable, 1 ready */
static int (*p_init)(void), (*p_count)(unsigned int *), (*p_handle)(unsigned int, void **);
static int (*p_temp)(void *, int, unsigned int *), (*p_power)(void *, unsigned int *), (*p_fan)(void *, unsigned int *);
static int (*p_clock)(void *, int, unsigned int *), (*p_util)(void *, nvml_util_t *), (*p_thr)(void *, unsigned long long *);

static int nvml_load(void)
{
    if (nvml_ready >= 0) return nvml_ready;
    nvml_ready = 0;
    nvml_lib = dlopen("libnvidia-ml.so.1", RTLD_NOW);
    if (!nvml_lib) return 0;
    *(void **)&p_init = dlsym(nvml_lib, "nvmlInit_v2");
    *(void **)&p_count = dlsym(nvml_lib, "nvmlDeviceGetCount_v2");
    *(void **)&p_handle = dlsym(nvml_lib, "nvmlDeviceGetHandleByIndex_v2");
    *(void **)&p_temp = dlsym(nvml_lib, "nvmlDeviceGetTemperature");
    *(void **)&p_power = dlsym(nvml_lib, "nvmlDeviceGetPowerUsage");
    *(void **)&p_fan = dlsym(nvml_lib, "nvmlDeviceGetFanSpeed");
    *(void **)&p_clock = dlsym(nvml_lib, "nvmlDeviceGetClockInfo");
    *(void **)&p_util = dlsym(nvml_lib, "nvmlDeviceGetUtilizationRates");
    *(void **)&p_thr = dlsym(nvml_lib, "nvmlDeviceGetCurrentClocksThrottleReasons");
    if (!p_init || !p_count || !p_handle || !p_temp || p_init() != 0) return 0;
    nvml_ready = 1;
    return 1;
}

static void nvml_read(ts_reading_t *out, int *n, int max)
{
    unsigned int count = 0;
    if (!nvml_load() || p_count(&count) != 0) return;
    for (unsigned int i = 0; i < count && i < 8; i++) {
        void *h;
        unsigned int t, w, fan, mhz;
        nvml_util_t u;
        unsigned long long thr;
        if (p_handle(i, &h) != 0) continue;
        if (p_temp(h, 0, &t) == 0) add_reading(out, n, max, (double)t, "t.nvml.%u.gpu", i);
        if (p_power && p_power(h, &w) == 0) add_reading(out, n, max, w / 1000.0, "p.nvml.%u.board", i);
        if (p_fan && p_fan(h, &fan) == 0) add_reading(out, n, max, (double)fan, "f.nvml.%u.fan_percent", i);
        if (p_clock && p_clock(h, 1, &mhz) == 0) add_reading(out, n, max, (double)mhz, "c.nvml.%u.sm_mhz", i);
        if (p_util && p_util(h, &u) == 0) add_reading(out, n, max, (double)u.gpu, "u.nvml.%u.gpu_percent", i);
        /* throttle reasons: 0x08 hardware slowdown, 0x20 software thermal slowdown, 0x40 hardware thermal slowdown; the mean of this 0/1 value is the share of time spent throttled */
        if (p_thr && p_thr(h, &thr) == 0) add_reading(out, n, max, (thr & (0x08ULL | 0x20ULL | 0x40ULL)) ? 1.0 : 0.0, "x.nvml.%u.thermal_throttle", i);
    }
}

#endif

/* ---------------------------------------------------------------- the readings */

/* ---------------------------------------------------------------- CPU package power (RAPL) and PCI names */

#include <time.h>
#define RAPL_MAX 8
static double rapl_prev_uj[RAPL_MAX], rapl_prev_t[RAPL_MAX];
static int rapl_have_prev[RAPL_MAX];

static double mono_seconds(void)
{
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double)t.tv_sec + t.tv_nsec / 1e9;
}

/* energy counters of the package domains (intel-rapl:N, also used by AMD): watts = counter difference / time. Needs a readable energy_uj (often root only):
 * where it is not readable, nothing is reported. */
static void rapl_read(const char *root, ts_reading_t *out, int *n, int max)
{
    glob_t g;
    char pat[512];
    snprintf(pat, sizeof pat, "%s/sys/class/powercap/intel-rapl:[0-9]", root);
    if (glob(pat, 0, NULL, &g) != 0) return;
    double now = mono_seconds();
    for (size_t i = 0; i < g.gl_pathc && i < RAPL_MAX; i++) {
        char p[600], name[64];
        double e, range = 0;
        snprintf(p, sizeof p, "%s/name", g.gl_pathv[i]);
        if (read_file(p, name, sizeof name) != 0) snprintf(name, sizeof name, "package-%zu", i);
        clean(name);
        if (strncmp(name, "package", 7) != 0) continue;
        for (int c = 0; c < 3; c++) {                 /* power limits (long term = PL1, short term = PL2): readable without root */
            char cn[32], pl[600];
            double lim;
            snprintf(p, sizeof p, "%s/constraint_%d_name", g.gl_pathv[i], c);
            snprintf(pl, sizeof pl, "%s/constraint_%d_power_limit_uw", g.gl_pathv[i], c);
            if (read_file(p, cn, sizeof cn) == 0 && read_num(pl, &lim) == 0 && lim > 0) { clean(cn); add_reading(out, n, max, lim / 1e6, "p.rapl.%s.limit_%s_w", name, cn); }
        }
        snprintf(p, sizeof p, "%s/energy_uj", g.gl_pathv[i]);
        if (read_num(p, &e) != 0) continue;
        snprintf(p, sizeof p, "%s/max_energy_range_uj", g.gl_pathv[i]);
        read_num(p, &range);
        if (rapl_have_prev[i] && now > rapl_prev_t[i]) {
            double d = e - rapl_prev_uj[i];
            if (d < 0 && range > 0) d += range;
            double w = d / 1e6 / (now - rapl_prev_t[i]);
            if (w >= 0 && w < 2000) add_reading(out, n, max, w, "p.rapl.%s", name);
        }
        rapl_prev_uj[i] = e;
        rapl_prev_t[i] = now;
        rapl_have_prev[i] = 1;
    }
    globfree(&g);
}

/* the name of a PCI device from the system's pci.ids (the text in [] if there is one, as in "GA106 [GeForce RTX 3050 8GB]"); empty if the file or the entry is missing */
static void pci_name(const char *root, const char *ven, const char *dev, char *out, size_t cap)
{
    static const char *files[] = { "/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids", "/usr/share/pci.ids" };
    char v[8] = "", d[8] = "", path[512], line[320];
    out[0] = 0;
    if (strncmp(ven, "0x", 2) == 0) ven += 2;
    if (strncmp(dev, "0x", 2) == 0) dev += 2;
    snprintf(v, sizeof v, "%.4s", ven);
    snprintf(d, sizeof d, "%.4s", dev);
    for (size_t f = 0; f < sizeof files / sizeof files[0]; f++) {
        snprintf(path, sizeof path, "%s%s", root, files[f]);
        FILE *fp = fopen(path, "r");
        if (!fp) continue;
        int in_vendor = 0;
        while (fgets(line, sizeof line, fp)) {
            if (line[0] == '#' || line[0] == '\n') continue;
            if (line[0] != '\t') {
                if (in_vendor) break;                        /* the next vendor: the device was not found */
                in_vendor = strncasecmp(line, v, 4) == 0 && line[4] == ' ';
            } else if (in_vendor && line[1] != '\t' && strncasecmp(line + 1, d, 4) == 0 && line[5] == ' ') {
                char *name = line + 6;
                while (*name == ' ') name++;
                name[strcspn(name, "\n")] = 0;
                char *a = strchr(name, '['), *b = a ? strrchr(a, ']') : NULL;
                if (a && b && b > a + 1) { *b = 0; name = a + 1; }
                snprintf(out, cap, "%s", name);
                fclose(fp);
                return;
            }
        }
        fclose(fp);
        if (out[0]) return;
    }
}

/* ---------------------------------------------------------------- PCIe links, disk counters, context (all readable without root) */

/* "8.0 GT/s PCIe" -> 8.0 ; the link of a device: speed and width now and at most (a graphics card slows its link down when idle: the maximum shows what it can do) */
static void pcie_one(const char *dir, const char *label, ts_reading_t *out, int *n, int max)
{
    char p[640], b[64];
    static const char *files[4] = { "current_link_speed", "current_link_width", "max_link_speed", "max_link_width" };
    static const char *names[4] = { "speed_gts", "width", "max_speed_gts", "max_width" };
    for (int i = 0; i < 4; i++) {
        snprintf(p, sizeof p, "%s/%s", dir, files[i]);
        if (read_file(p, b, sizeof b) == 0 && b[0] >= '0' && b[0] <= '9') add_reading(out, n, max, atof(b), "c.pcie.%s.%s", label, names[i]);
    }
}

static void pcie_read(const char *root, ts_reading_t *out, int *n, int max)
{
    glob_t g;
    char pat[512], dir[640], lab[32];
    snprintf(pat, sizeof pat, "%s/sys/class/drm/card[0-9]", root);
    if (glob(pat, 0, NULL, &g) == 0) {
        for (size_t i = 0; i < g.gl_pathc; i++) {
            snprintf(dir, sizeof dir, "%s/device", g.gl_pathv[i]);
            snprintf(lab, sizeof lab, "gpu%s", strrchr(g.gl_pathv[i], '/') + 5);          /* card0 -> gpu0 */
            pcie_one(dir, lab, out, n, max);
        }
        globfree(&g);
    }
    snprintf(pat, sizeof pat, "%s/sys/class/nvme/nvme[0-9]*", root);
    if (glob(pat, 0, NULL, &g) == 0) {
        for (size_t i = 0; i < g.gl_pathc; i++) {
            snprintf(dir, sizeof dir, "%s/device", g.gl_pathv[i]);
            snprintf(lab, sizeof lab, "%s", strrchr(g.gl_pathv[i], '/') + 1);
            clean(lab);
            pcie_one(dir, lab, out, n, max);
        }
        globfree(&g);
    }
}

/* bytes read and written since boot per whole disk (the minimum and maximum of a window give the rate); the life-long totals need root (SMART) */
static void disk_read(const char *root, ts_reading_t *out, int *n, int max)
{
    glob_t g;
    char pat[512];
    snprintf(pat, sizeof pat, "%s/sys/block/*/stat", root);
    if (glob(pat, 0, NULL, &g) != 0) return;
    for (size_t i = 0; i < g.gl_pathc; i++) {
        char dev[64], line[400];
        const char *e = strstr(g.gl_pathv[i], "/sys/block/");
        if (!e) continue;
        snprintf(dev, sizeof dev, "%s", e + 11);
        char *sl = strchr(dev, '/');
        if (sl) *sl = 0;
        if (!strncmp(dev, "loop", 4) || !strncmp(dev, "ram", 3) || !strncmp(dev, "zram", 4) || !strncmp(dev, "dm-", 3) || !strncmp(dev, "md", 2) || !strncmp(dev, "sr", 2) || !strncmp(dev, "fd", 2) || !strncmp(dev, "nbd", 3)) continue;
        if (read_file(g.gl_pathv[i], line, sizeof line) != 0) continue;
        unsigned long long f[11] = { 0 };
        if (sscanf(line, "%llu %llu %llu %llu %llu %llu %llu", &f[0], &f[1], &f[2], &f[3], &f[4], &f[5], &f[6]) < 7) continue;
        clean(dev);
        add_reading(out, n, max, (double)f[2] * 512.0, "x.disk.%s.read_bytes", dev);
        add_reading(out, n, max, (double)f[6] * 512.0, "x.disk.%s.written_bytes", dev);
    }
    globfree(&g);
}

static void context_read(const char *root, ts_reading_t *out, int *n, int max)
{
    char p[512], b[128];
    snprintf(p, sizeof p, "%s/proc/uptime", root);
    if (read_file(p, b, sizeof b) == 0) add_reading(out, n, max, atof(b), "x.ctx.uptime_s");
    time_t t = time(NULL);
    struct tm lt;
    if (localtime_r(&t, &lt)) add_reading(out, n, max, (double)lt.tm_gmtoff / 60.0, "x.ctx.tz_min");      /* UTC offset only: no city, no country */
}

static double prev_idle = -1, prev_total = -1;   /* for the CPU utilisation between two samples */

int ts_read_sensors(const char *sysroot, ts_reading_t *out, int max)
{
    const char *root = root_of(sysroot);
    char pat[512], b[256], name[64], label[96];
    int n = 0;
    glob_t g;

    /* hwmon: temperatures, fans, power */
    seen_t seen[64];
    int nseen = 0;
    path_join(pat, sizeof pat, root, "/sys/class/hwmon/hwmon*");
    if (globber(pat, &g) == 0) {
        for (size_t i = 0; i < g.gl_pathc; i++) {
            const char *d = g.gl_pathv[i];
            snprintf(pat, sizeof pat, "%s/name", d);
            if (read_file(pat, name, sizeof name) != 0) snprintf(name, sizeof name, "hwmon");
            clean(name);
            int occ = occurrence(seen, &nseen, name);
            char chip[96];
            if (occ > 1) snprintf(chip, sizeof chip, "%s#%d", name, occ);
            else snprintf(chip, sizeof chip, "%s", name);
            glob_t t;
            snprintf(pat, sizeof pat, "%s/temp*_input", d);
            if (globber(pat, &t) == 0) {
                for (size_t k = 0; k < t.gl_pathc; k++) {
                    double v;
                    if (read_num(t.gl_pathv[k], &v) != 0) continue;
                    v /= 1000.0;
                    if (v <= -40 || v >= 150) continue;                    /* broken sensor values (-128, 255, 65535 ...) */
                    char base[512];
                    snprintf(base, sizeof base, "%s", t.gl_pathv[k]);
                    base[strlen(base) - 6] = 0;                             /* strip "_input" */
                    snprintf(pat, sizeof pat, "%s_label", base);
                    if (read_file(pat, label, sizeof label) != 0 || !label[0]) snprintf(label, sizeof label, "%s", strrchr(base, '/') + 1);
                    clean(label);
                    add_reading(out, &n, max, v, "t.hwmon.%s.%s", chip, label);
                }
                globfree(&t);
            }
            snprintf(pat, sizeof pat, "%s/fan*_input", d);
            if (globber(pat, &t) == 0) {
                for (size_t k = 0; k < t.gl_pathc; k++) {
                    double v;
                    if (read_num(t.gl_pathv[k], &v) != 0) continue;
                    snprintf(label, sizeof label, "%s", strrchr(t.gl_pathv[k], '/') + 1);
                    label[strlen(label) - 6] = 0;
                    add_reading(out, &n, max, v, "f.hwmon.%s.%s", chip, label);
                }
                globfree(&t);
            }
            snprintf(pat, sizeof pat, "%s/power*_average", d);
            if (globber(pat, &t) != 0 || t.gl_pathc == 0) {
                globfree(&t);
                snprintf(pat, sizeof pat, "%s/power*_input", d);
                globber(pat, &t);
            }
            for (size_t k = 0; k < t.gl_pathc; k++) {
                double v;
                if (read_num(t.gl_pathv[k], &v) != 0) continue;
                snprintf(label, sizeof label, "%s", strrchr(t.gl_pathv[k], '/') + 1);
                char *us = strchr(label, '_');
                if (us) *us = 0;
                add_reading(out, &n, max, v / 1e6, "p.hwmon.%s.%s", chip, label);
            }
            globfree(&t);
        }
        globfree(&g);
    }

    /* ACPI and platform thermal zones */
    nseen = 0;
    path_join(pat, sizeof pat, root, "/sys/class/thermal/thermal_zone*");
    if (globber(pat, &g) == 0) {
        for (size_t i = 0; i < g.gl_pathc; i++) {
            double v;
            snprintf(pat, sizeof pat, "%s/temp", g.gl_pathv[i]);
            if (read_num(pat, &v) != 0) continue;
            v /= 1000.0;
            if (v <= -40 || v >= 150) continue;
            snprintf(pat, sizeof pat, "%s/type", g.gl_pathv[i]);
            if (read_file(pat, name, sizeof name) != 0) snprintf(name, sizeof name, "zone");
            clean(name);
            int occ = occurrence(seen, &nseen, name);
            if (occ > 1) add_reading(out, &n, max, v, "t.zone.%s#%d", name, occ);
            else add_reading(out, &n, max, v, "t.zone.%s", name);
        }
        globfree(&g);
    }

    /* thermal-throttle counter of the CPU (monotonic; the minimum and maximum of a window show how many events happened in it) */
    double thr = 0;
    int have_thr = 0;
    path_join(pat, sizeof pat, root, "/sys/devices/system/cpu/cpu*/thermal_throttle/core_throttle_count");
    if (globber(pat, &g) == 0) {
        for (size_t i = 0; i < g.gl_pathc; i++) {
            double v;
            if (read_num(g.gl_pathv[i], &v) == 0) { thr += v; have_thr = 1; }
        }
        globfree(&g);
    }
    if (have_thr) add_reading(out, &n, max, thr, "x.cpu.thermal_throttle_count");

    /* CPU frequency (mean over the cores) */
    double fsum = 0, fmin = 1e12, fmax = 0;
    int fn = 0;
    path_join(pat, sizeof pat, root, "/sys/devices/system/cpu/cpu[0-9]*/cpufreq/scaling_cur_freq");
    if (globber(pat, &g) == 0) {
        for (size_t i = 0; i < g.gl_pathc; i++) {
            double v;
            if (read_num(g.gl_pathv[i], &v) == 0) { fsum += v / 1000.0; fn++; if (v / 1000.0 < fmin) fmin = v / 1000.0; if (v / 1000.0 > fmax) fmax = v / 1000.0; }
        }
        globfree(&g);
    }
    if (fn) {
        add_reading(out, &n, max, fsum / fn, "c.cpu.mhz_mean");
        if (fn > 1) { add_reading(out, &n, max, fmin, "c.cpu.mhz_min"); add_reading(out, &n, max, fmax, "c.cpu.mhz_max"); }     /* slowest and fastest core */
    }

    /* CPU utilisation between two samples, load, memory */
    path_join(pat, sizeof pat, root, "/proc/stat");
    FILE *f = fopen(pat, "r");
    if (f) {
        unsigned long long u, ni, s, idle, io = 0, irq = 0, sirq = 0, steal = 0;
        if (fscanf(f, "cpu %llu %llu %llu %llu %llu %llu %llu %llu", &u, &ni, &s, &idle, &io, &irq, &sirq, &steal) >= 4) {
            double total = (double)(u + ni + s + idle + io + irq + sirq + steal), idl = (double)(idle + io);
            if (prev_total >= 0 && total > prev_total) add_reading(out, &n, max, 100.0 * (1.0 - (idl - prev_idle) / (total - prev_total)), "u.cpu.percent");
            prev_idle = idl;
            prev_total = total;
        }
        fclose(f);
    }
    path_join(pat, sizeof pat, root, "/proc/loadavg");
    if (read_file(pat, b, sizeof b) == 0) {
        double l1;
        if (sscanf(b, "%lf", &l1) == 1) add_reading(out, &n, max, l1, "u.load.1min");
    }
    path_join(pat, sizeof pat, root, "/proc/meminfo");
    f = fopen(pat, "r");
    if (f) {
        char line[160];
        double mt = 0, ma = 0, st = 0, sf = 0;
        while (fgets(line, sizeof line, f)) {
            if (!strncmp(line, "MemTotal:", 9)) mt = atof(line + 9);
            else if (!strncmp(line, "MemAvailable:", 13)) ma = atof(line + 13);
            else if (!strncmp(line, "SwapTotal:", 10)) st = atof(line + 10);
            else if (!strncmp(line, "SwapFree:", 9)) sf = atof(line + 9);
        }
        fclose(f);
        if (mt > 0) add_reading(out, &n, max, 100.0 * (1.0 - ma / mt), "m.mem.used_percent");
        if (st > 0) add_reading(out, &n, max, 100.0 * (1.0 - sf / st), "m.swap.used_percent");
    }

    /* battery and AC adapter */
    path_join(pat, sizeof pat, root, "/sys/class/power_supply/*");
    if (globber(pat, &g) == 0) {
        for (size_t i = 0; i < g.gl_pathc; i++) {
            const char *p = g.gl_pathv[i];
            char type[32], nm[64];
            snprintf(pat, sizeof pat, "%s/type", p);
            if (read_file(pat, type, sizeof type) != 0) continue;
            snprintf(nm, sizeof nm, "%s", strrchr(p, '/') + 1);
            clean(nm);
            double v;
            if (!strcmp(type, "Battery")) {
                snprintf(pat, sizeof pat, "%s/temp", p);
                if (read_num(pat, &v) == 0 && v / 10.0 > -40 && v / 10.0 < 150) add_reading(out, &n, max, v / 10.0, "t.battery.%s", nm);
                snprintf(pat, sizeof pat, "%s/capacity", p);
                if (read_num(pat, &v) == 0) add_reading(out, &n, max, v, "b.battery.%s.percent", nm);
                {   /* wear: full-charge capacity against the design capacity, and the charge cycles */
                    double full = -1, design = -1, cyc;
                    snprintf(pat, sizeof pat, "%s/energy_full", p);
                    if (read_num(pat, &full) != 0) { snprintf(pat, sizeof pat, "%s/charge_full", p); if (read_num(pat, &full) != 0) full = -1; }
                    snprintf(pat, sizeof pat, "%s/energy_full_design", p);
                    if (read_num(pat, &design) != 0) { snprintf(pat, sizeof pat, "%s/charge_full_design", p); if (read_num(pat, &design) != 0) design = -1; }
                    if (full > 0 && design > 0 && full / design < 1.5) add_reading(out, &n, max, 100.0 * full / design, "b.battery.%s.health_percent", nm);
                    snprintf(pat, sizeof pat, "%s/cycle_count", p);
                    if (read_num(pat, &cyc) == 0 && cyc >= 0 && cyc < 100000) add_reading(out, &n, max, cyc, "x.battery.%s.cycles", nm);
                }
                {   /* power the battery delivers while the computer runs on it (a figure of the whole computer, laptops only) */
                    char st[24];
                    snprintf(pat, sizeof pat, "%s/status", p);
                    double pw = -1;
                    if (read_file(pat, st, sizeof st) == 0 && strcmp(st, "Discharging") == 0) {
                        snprintf(pat, sizeof pat, "%s/power_now", p);
                        if (read_num(pat, &pw) == 0 && pw > 0) {
                            add_reading(out, &n, max, pw / 1e6, "p.battery.%s.discharge_w", nm);
                        } else {
                            double cu, vo;
                            snprintf(pat, sizeof pat, "%s/current_now", p);
                            int c1 = read_num(pat, &cu);
                            snprintf(pat, sizeof pat, "%s/voltage_now", p);
                            if (c1 == 0 && read_num(pat, &vo) == 0 && cu > 0 && vo > 0) add_reading(out, &n, max, (cu / 1e6) * (vo / 1e6), "p.battery.%s.discharge_w", nm);
                        }
                    }
                }
            } else if (!strcmp(type, "Mains")) {
                snprintf(pat, sizeof pat, "%s/online", p);
                if (read_num(pat, &v) == 0) add_reading(out, &n, max, v, "b.ac.%s.online", nm);
            }
        }
        globfree(&g);
    }

    pcie_read(root, out, &n, max);
    disk_read(root, out, &n, max);
    context_read(root, out, &n, max);
    if (is_real(sysroot)) rapl_read(root, out, &n, max);
    if (is_real(sysroot)) nvml_read(out, &n, max);
    return n;
}

/* ---------------------------------------------------------------- JSON text */

typedef struct { char *p; size_t cap, len; int over; } sb_t;

static void sb_raw(sb_t *s, const char *t)
{
    size_t l = strlen(t);
    if (s->len + l + 1 > s->cap) { s->over = 1; return; }
    memcpy(s->p + s->len, t, l + 1);
    s->len += l;
}

static void sb_fmt(sb_t *s, const char *fmt, ...)
{
    char tmp[256];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(tmp, sizeof tmp, fmt, ap);
    va_end(ap);
    sb_raw(s, tmp);
}

static void sb_str(sb_t *s, const char *t)    /* a quoted, escaped JSON string */
{
    sb_raw(s, "\"");
    for (; *t; t++) {
        unsigned char c = (unsigned char)*t;
        char e[8];
        if (c == '"' || c == '\\') { e[0] = '\\'; e[1] = (char)c; e[2] = 0; }
        else if (c < 32) snprintf(e, sizeof e, "\\u%04x", c);
        else { e[0] = (char)c; e[1] = 0; }
        sb_raw(s, e);
    }
    sb_raw(s, "\"");
}

static void sb_kv_str(sb_t *s, const char *k, const char *v, int *first)
{
    if (!*first) sb_raw(s, ",");
    *first = 0;
    sb_str(s, k);
    sb_raw(s, ":");
    if (v && v[0]) sb_str(s, v); else sb_raw(s, "null");
}

static void sb_kv_num(sb_t *s, const char *k, double v, int *first)
{
    if (!*first) sb_raw(s, ",");
    *first = 0;
    sb_str(s, k);
    sb_fmt(s, ":%.6g", v);
}

/* NVIDIA cards as the driver tool describes them (once per inventory): name, driver version, memory in MB, PCIe maximum */
static int smi_inv_n;
static char smi_inv_name[8][80], smi_inv_drv[8][32];
static double smi_inv_vram[8], smi_inv_gen[8], smi_inv_wid[8];
static void smi_inventory(void)
{
    smi_inv_n = 0;
    FILE *f = popen("nvidia-smi --query-gpu=name,driver_version,memory.total,pcie.link.gen.max,pcie.link.width.max --format=csv,noheader,nounits 2>/dev/null", "r");
    if (!f) return;
    char line[400];
    while (smi_inv_n < 8 && fgets(line, sizeof line, f)) {
        char *fl[8];
        if (smi_fields(line, fl, 8) < 5) continue;
        int k = smi_inv_n++;
        snprintf(smi_inv_name[k], sizeof smi_inv_name[k], "%s", fl[0]);
        snprintf(smi_inv_drv[k], sizeof smi_inv_drv[k], "%s", fl[1]);
        if (smi_num(fl[2], &smi_inv_vram[k]) != 0) smi_inv_vram[k] = 0;
        if (smi_num(fl[3], &smi_inv_gen[k]) != 0) smi_inv_gen[k] = 0;
        if (smi_num(fl[4], &smi_inv_wid[k]) != 0) smi_inv_wid[k] = 0;
    }
    pclose(f);
}

uint32_t ts_hash(const char *s, size_t n)    /* FNV-1a */
{
    uint32_t h = 2166136261u;
    for (size_t i = 0; i < n; i++) { h ^= (unsigned char)s[i]; h *= 16777619u; }
    return h;
}

/* ---------------------------------------------------------------- inventory */

static void file_field(sb_t *s, const char *root, const char *rel, const char *key, int *first)
{
    char p[512], v[160];
    snprintf(p, sizeof p, "%s%s", root, rel);
    if (read_file(p, v, sizeof v) != 0) v[0] = 0;
    sb_kv_str(s, key, v, first);
}

int ts_inventory_json(const char *sysroot, char *buf, size_t cap)
{
    const char *root = root_of(sysroot);
    sb_t s = { buf, cap, 0, 0 };
    char p[512], v[256];
    buf[0] = 0;
    sb_raw(&s, "{");

    /* CPU */
    sb_raw(&s, "\"cpu\":{");
    snprintf(p, sizeof p, "%s/proc/cpuinfo", root);
    {
        FILE *f = fopen(p, "r");
        char line[256], model[160] = "", mc[32] = "";
        int threads = 0;
        if (f) {
            while (fgets(line, sizeof line, f)) {
                char *c = strchr(line, ':');
                if (!c) continue;
                c++;
                while (*c == ' ' || *c == '\t') c++;
                c[strcspn(c, "\n")] = 0;
                if (!model[0] && (!strncmp(line, "model name", 10) || !strncmp(line, "Hardware", 8))) snprintf(model, sizeof model, "%s", c);
                else if (!strncmp(line, "processor", 9)) threads++;
                else if (!mc[0] && !strncmp(line, "microcode", 9)) snprintf(mc, sizeof mc, "%s", c);
            }
            fclose(f);
        }
        int cf = 1;
        char gv[64], ep[64], nt[16], mx[32], mn[32];
        snprintf(p, sizeof p, "%s/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor", root);
        if (read_file(p, gv, sizeof gv) != 0) gv[0] = 0;
        snprintf(p, sizeof p, "%s/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference", root);
        if (read_file(p, ep, sizeof ep) != 0) ep[0] = 0;
        snprintf(p, sizeof p, "%s/sys/devices/system/cpu/intel_pstate/no_turbo", root);
        if (read_file(p, nt, sizeof nt) != 0) nt[0] = 0;
        snprintf(p, sizeof p, "%s/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq", root);
        if (read_file(p, mx, sizeof mx) != 0) mx[0] = 0;
        snprintf(p, sizeof p, "%s/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_min_freq", root);
        if (read_file(p, mn, sizeof mn) != 0) mn[0] = 0;
        sb_kv_str(&s, "model", model, &cf);
        sb_kv_num(&s, "threads", threads, &cf);
        sb_kv_str(&s, "microcode", mc, &cf);
        sb_kv_str(&s, "governor", gv, &cf);
        sb_kv_str(&s, "epp", ep, &cf);
        if (nt[0]) sb_kv_num(&s, "turbo", nt[0] == '0' ? 1 : 0, &cf);
        if (mx[0]) sb_kv_num(&s, "max_mhz", atof(mx) / 1000.0, &cf);
        if (mn[0]) sb_kv_num(&s, "min_mhz", atof(mn) / 1000.0, &cf);
    }
    sb_raw(&s, "}");

    /* system board, BIOS (no serial numbers, no UUID) */
    sb_raw(&s, ",\"dmi\":{");
    {
        int df = 1;
        file_field(&s, root, "/sys/class/dmi/id/sys_vendor", "sys_vendor", &df);
        file_field(&s, root, "/sys/class/dmi/id/product_name", "product", &df);
        file_field(&s, root, "/sys/class/dmi/id/board_vendor", "board_vendor", &df);
        file_field(&s, root, "/sys/class/dmi/id/board_name", "board", &df);
        file_field(&s, root, "/sys/class/dmi/id/bios_vendor", "bios_vendor", &df);
        file_field(&s, root, "/sys/class/dmi/id/bios_version", "bios_version", &df);
        file_field(&s, root, "/sys/class/dmi/id/chassis_type", "chassis_type", &df);
    }
    sb_raw(&s, "}");

    /* memory */
    snprintf(p, sizeof p, "%s/proc/meminfo", root);
    {
        FILE *f = fopen(p, "r");
        char line[160];
        double mt = 0;
        if (f) {
            while (fgets(line, sizeof line, f))
                if (!strncmp(line, "MemTotal:", 9)) { mt = atof(line + 9); break; }
            fclose(f);
        }
        sb_fmt(&s, ",\"mem_kb\":%.0f", mt);
    }

    /* RAM modules: the SMBIOS entries and the SPD EEPROM are root-only; only the fact that they exist is recorded */
    {
        glob_t g;
        int smbios = 0, spd = 0;
        snprintf(p, sizeof p, "%s/sys/firmware/dmi/entries/17-*", root);
        if (globber(p, &g) == 0) { smbios = (int)g.gl_pathc; globfree(&g); }
        snprintf(p, sizeof p, "%s/sys/bus/i2c/devices/*-005[0-7]", root);
        if (globber(p, &g) == 0) { spd = (int)g.gl_pathc; globfree(&g); }
        sb_fmt(&s, ",\"ram_modules\":{\"smbios_entries\":%d,\"spd_eeprom_devices\":%d,\"details_readable\":false}", smbios, spd);
    }

    /* storage: kind and capacity, no serial numbers */
    sb_raw(&s, ",\"storage\":[");
    {
        glob_t g;
        int firstd = 1;
        snprintf(p, sizeof p, "%s/sys/block/*", root);
        if (globber(p, &g) == 0) {
            for (size_t i = 0; i < g.gl_pathc; i++) {
                const char *nm = strrchr(g.gl_pathv[i], '/') + 1;
                if (!strncmp(nm, "loop", 4) || !strncmp(nm, "ram", 3) || !strncmp(nm, "zram", 4) || !strncmp(nm, "dm-", 3) || !strncmp(nm, "md", 2) || !strncmp(nm, "sr", 2) || !strncmp(nm, "fd", 2)) continue;
                char model[128], rot[8], sz[32];
                snprintf(p, sizeof p, "%s/device/model", g.gl_pathv[i]);
                if (read_file(p, model, sizeof model) != 0) model[0] = 0;
                snprintf(p, sizeof p, "%s/queue/rotational", g.gl_pathv[i]);
                if (read_file(p, rot, sizeof rot) != 0) rot[0] = 0;
                snprintf(p, sizeof p, "%s/size", g.gl_pathv[i]);
                double sectors = 0;
                if (read_num(p, &sectors) != 0) sectors = 0;
                (void)sz;
                if (!firstd) sb_raw(&s, ",");
                firstd = 0;
                int ff = 1;
                sb_raw(&s, "{");
                sb_kv_str(&s, "kind", !strncmp(nm, "nvme", 4) ? "nvme" : (rot[0] == '1' ? "hdd" : "ssd"), &ff);
                sb_kv_str(&s, "model", model, &ff);
                sb_kv_num(&s, "gb", sectors * 512.0 / 1e9, &ff);
                int ctl = -1;
                if (sscanf(nm, "nvme%d", &ctl) == 1) {          /* the PCIe link of the controller: speed and width now and at most */
                    static const char *lf[4] = { "current_link_speed", "current_link_width", "max_link_speed", "max_link_width" };
                    static const char *ln[4] = { "link_gts", "link_width", "link_max_gts", "link_max_width" };
                    for (int q = 0; q < 4; q++) {
                        char lp[600], lb[48];
                        snprintf(lp, sizeof lp, "%s/sys/class/nvme/nvme%d/device/%s", root, ctl, lf[q]);
                        if (read_file(lp, lb, sizeof lb) == 0 && lb[0] >= '0' && lb[0] <= '9') sb_kv_num(&s, ln[q], atof(lb), &ff);
                    }
                }
                sb_raw(&s, "}");
            }
            globfree(&g);
        }
    }
    sb_raw(&s, "]");

    /* GPUs by PCI id and driver */
    sb_raw(&s, ",\"gpus\":[");
    {
        glob_t g;
        int firstg = 1, nvidia_k = 0;
        if (is_real(sysroot)) smi_inventory(); else smi_inv_n = 0;
        snprintf(p, sizeof p, "%s/sys/class/drm/card[0-9]", root);
        if (globber(p, &g) == 0) {
            for (size_t i = 0; i < g.gl_pathc; i++) {
                char ven[16], dev[16], drv[256] = "";
                snprintf(p, sizeof p, "%s/device/vendor", g.gl_pathv[i]);
                if (read_file(p, ven, sizeof ven) != 0) continue;
                snprintf(p, sizeof p, "%s/device/device", g.gl_pathv[i]);
                if (read_file(p, dev, sizeof dev) != 0) dev[0] = 0;
                snprintf(p, sizeof p, "%s/device/driver", g.gl_pathv[i]);
                ssize_t l = readlink(p, v, sizeof v - 1);
                if (l > 0) { v[l] = 0; snprintf(drv, sizeof drv, "%s", strrchr(v, '/') ? strrchr(v, '/') + 1 : v); }
                if (!firstg) sb_raw(&s, ",");
                firstg = 0;
                int ff = 1;
                sb_raw(&s, "{");
                sb_kv_str(&s, "vendor", ven, &ff);
                sb_kv_str(&s, "device", dev, &ff);
                sb_kv_str(&s, "driver", drv, &ff);
                {
                    char gname[160];
                    pci_name(root, ven, dev, gname, sizeof gname);
                    if (!strcmp(ven, "0x10de") && is_real(sysroot) && nvidia_k < smi_inv_n) {
                        if (smi_inv_name[nvidia_k][0]) snprintf(gname, sizeof gname, "%s", smi_inv_name[nvidia_k]);
                        sb_kv_str(&s, "name", gname, &ff);
                        sb_kv_str(&s, "driver_version", smi_inv_drv[nvidia_k], &ff);
                        if (smi_inv_vram[nvidia_k] > 0) sb_kv_num(&s, "vram_mb", smi_inv_vram[nvidia_k], &ff);
                        if (smi_inv_gen[nvidia_k] > 0) sb_kv_num(&s, "pcie_gen_max", smi_inv_gen[nvidia_k], &ff);
                        if (smi_inv_wid[nvidia_k] > 0) sb_kv_num(&s, "pcie_width_max", smi_inv_wid[nvidia_k], &ff);
                        nvidia_k++;
                    } else {
                        sb_kv_str(&s, "name", gname, &ff);
                    }
                }
                sb_raw(&s, "}");
            }
            globfree(&g);
        }
    }
    sb_raw(&s, "]");

    /* kernel, machine, distribution, virtualisation hints */
    {
        struct utsname u;
        if (uname(&u) == 0) sb_fmt(&s, ",\"kernel\":\"%.60s\",\"machine\":\"%.30s\"", u.release, u.machine);
        snprintf(p, sizeof p, "%s/etc/os-release", root);
        FILE *f = fopen(p, "r");
        char line[200], pretty[200] = "";
        if (f) {
            while (fgets(line, sizeof line, f))
                if (!strncmp(line, "PRETTY_NAME=", 12)) {
                    snprintf(pretty, sizeof pretty, "%s", line + 12);
                    pretty[strcspn(pretty, "\n")] = 0;
                    if (pretty[0] == '"') { memmove(pretty, pretty + 1, strlen(pretty)); char *q = strrchr(pretty, '"'); if (q) *q = 0; }
                    break;
                }
            fclose(f);
        }
        sb_raw(&s, ",\"os\":");
        if (pretty[0]) sb_str(&s, pretty); else sb_raw(&s, "null");
        snprintf(p, sizeof p, "%s/sys/class/dmi/id/sys_vendor", root);
        char ven[80] = "";
        read_file(p, ven, sizeof ven);
        for (char *c = ven; *c; c++) *c = (char)tolower((unsigned char)*c);
        int vm = strstr(ven, "vmware") || strstr(ven, "qemu") || strstr(ven, "innotek") || strstr(ven, "microsoft") || strstr(ven, "xen") || strstr(ven, "amazon") || strstr(ven, "google") || strstr(ven, "kvm");
        snprintf(p, sizeof p, "%s/.dockerenv", root);
        int ctr = access(p, F_OK) == 0;
        sb_fmt(&s, ",\"virtual_machine_hint\":%s,\"container_hint\":%s", vm ? "true" : "false", ctr ? "true" : "false");
    }

    /* the sensor inventory: every hwmon chip and zone with its labels and limits, so that "absent" and "not read" can be told apart */
    sb_raw(&s, ",\"sensors\":[");
    {
        glob_t g;
        int firsts = 1;
        snprintf(p, sizeof p, "%s/sys/class/hwmon/hwmon*", root);
        if (globber(p, &g) == 0) {
            for (size_t i = 0; i < g.gl_pathc; i++) {
                char name[64];
                snprintf(p, sizeof p, "%s/name", g.gl_pathv[i]);
                if (read_file(p, name, sizeof name) != 0) snprintf(name, sizeof name, "hwmon");
                glob_t t;
                snprintf(p, sizeof p, "%s/temp*_input", g.gl_pathv[i]);
                if (globber(p, &t) != 0) { globfree(&t); continue; }
                for (size_t k = 0; k < t.gl_pathc; k++) {
                    char base[512], label[96], lim[32];
                    snprintf(base, sizeof base, "%s", t.gl_pathv[k]);
                    base[strlen(base) - 6] = 0;
                    snprintf(p, sizeof p, "%s_label", base);
                    if (read_file(p, label, sizeof label) != 0 || !label[0]) snprintf(label, sizeof label, "%s", strrchr(base, '/') + 1);
                    double mx = -1, cr = -1;
                    snprintf(p, sizeof p, "%s_max", base);
                    if (read_num(p, &mx) != 0) mx = -1;
                    snprintf(p, sizeof p, "%s_crit", base);
                    if (read_num(p, &cr) != 0) cr = -1;
                    (void)lim;
                    if (!firsts) sb_raw(&s, ",");
                    firsts = 0;
                    int ff = 1;
                    sb_raw(&s, "{");
                    sb_kv_str(&s, "chip", name, &ff);
                    sb_kv_str(&s, "label", label, &ff);
                    if (mx > 0) sb_kv_num(&s, "max", mx / 1000.0, &ff);
                    if (cr > 0) sb_kv_num(&s, "crit", cr / 1000.0, &ff);
                    sb_raw(&s, "}");
                }
                globfree(&t);
            }
            globfree(&g);
        }
    }
    sb_raw(&s, "]");

    sb_raw(&s, "}");
    return s.over ? -1 : (int)s.len;
}

/* ---------------------------------------------------------------- window aggregation and the report text */

void ts_window_reset(ts_window_t *w, time_t now)
{
    w->count = 0;
    w->samples = 0;
    w->start = now;
}

void ts_window_add(ts_window_t *w, const ts_reading_t *r, int n)
{
    w->samples++;
    for (int i = 0; i < n; i++) {
        int j;
        for (j = 0; j < w->count; j++)
            if (strcmp(w->a[j].key, r[i].key) == 0) break;
        if (j == w->count) {
            if (w->count >= TS_MAX_READINGS) continue;
            snprintf(w->a[j].key, TS_KEY_LEN, "%s", r[i].key);
            w->a[j].mn = w->a[j].mx = r[i].v;
            w->a[j].sum = 0;
            w->a[j].n = 0;
            w->count++;
        }
        ts_agg_t *a = &w->a[j];
        if (r[i].v < a->mn) a->mn = r[i].v;
        if (r[i].v > a->mx) a->mx = r[i].v;
        a->sum += r[i].v;
        a->n++;
    }
}

int ts_window_json(const ts_window_t *w, time_t now, const char *inv_hash, const char *inventory_json, char *buf, size_t cap)
{
    sb_t s = { buf, cap, 0, 0 };
    buf[0] = 0;
    sb_fmt(&s, "{\"v\":1,\"t\":%lld,\"w\":%lld,\"s\":%d,\"inv\":", (long long)now, (long long)(now - w->start), w->samples);
    sb_str(&s, inv_hash ? inv_hash : "");
    if (inventory_json && inventory_json[0]) {
        sb_raw(&s, ",\"inventory\":");
        sb_raw(&s, inventory_json);
    }
    sb_raw(&s, ",\"m\":{");
    for (int i = 0; i < w->count; i++) {
        if (i) sb_raw(&s, ",");
        sb_str(&s, w->a[i].key);
        sb_fmt(&s, ":[%.4g,%.4g,%.4g]", w->a[i].mn, w->a[i].n ? w->a[i].sum / w->a[i].n : 0.0, w->a[i].mx);
    }
    sb_raw(&s, "}}");
    return s.over ? -1 : (int)s.len;
}
