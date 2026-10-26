/* Thermopsis monitor: main loop. Sample every N seconds, report min/mean/max every M seconds, never defer, never loop without sensors. */
#define _GNU_SOURCE
#include "thermopsis.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define INV_CAP 24576
#define REPORT_CAP 60000

static int has_temperature(const ts_reading_t *r, int n)
{
    for (int i = 0; i < n; i++)
        if (r[i].key[0] == 't' && r[i].key[1] == '.') return 1;
    return 0;
}

/* checkpoint text: "<inventory hash> <seconds already run>" */
static void checkpoint_save(const char *hash, long run)
{
    char t[96];
    int n = snprintf(t, sizeof t, "%s %ld", hash, run);
    glue_checkpoint_write(t, (size_t)n);
}

/* Waits until the wall-clock deadline. A BOINC task gets timer signals that end sleep() early, so counting sleeps would make the sampling several times too fast. */
static int interruptible_sleep(unsigned seconds)
{
    time_t until = time(NULL) + (time_t)seconds;
    while (time(NULL) < until) {
        if (glue_quit_requested()) return 1;
        struct timespec ts = { 0, 500 * 1000 * 1000 };
        nanosleep(&ts, NULL);
    }
    return glue_quit_requested();
}

int thermopsis_run(const char *sysroot, int max_cycles_for_tests)
{
    ts_config_t cfg = { 60, 600, 3 * 86400 };
    static ts_reading_t r[TS_MAX_READINGS];
    static ts_window_t w;
    static char inv[INV_CAP], rep[REPORT_CAP];
    char hash[16], sent_hash[16] = "";
    long run_before = 0;

    if (glue_init(0, NULL, &cfg) != 0) glue_done(0);
    if (cfg.sample_seconds < 1) cfg.sample_seconds = 60;
    if (cfg.report_seconds < cfg.sample_seconds) cfg.report_seconds = cfg.sample_seconds;

    {
        char c[96];
        size_t n = glue_checkpoint_read(c, sizeof c);
        if (n > 0) {
            c[n < sizeof c ? n : sizeof c - 1] = 0;
            if (sscanf(c, "%15s %ld", sent_hash, &run_before) != 2) { sent_hash[0] = 0; run_before = 0; }
        }
    }

    int len = ts_inventory_json(sysroot, inv, sizeof inv);
    if (len < 0) { inv[0] = 0; len = 0; }
    snprintf(hash, sizeof hash, "%08x", ts_hash(inv, (size_t)len));

    int n = ts_read_sensors(sysroot, r, TS_MAX_READINGS);
    /* the CPU utilisation needs two reads: discard the first, read again at once */
    n = ts_read_sensors(sysroot, r, TS_MAX_READINGS);

    if (!has_temperature(r, n)) {
        /* nothing to measure here (virtual machine, container, board without sensors): say so once and end with success. No deferral. */
        snprintf(rep, sizeof rep, "{\"v\":1,\"t\":%lld,\"nosensors\":true,\"inv\":\"%s\",\"inventory\":%s}", (long long)time(NULL), hash, inv[0] ? inv : "null");
        glue_send_report(rep);
        glue_log("no temperature sensors found: reported once, finishing");
        /* the client collects a trickle message while the task is alive: leaving at once loses it, and the project then never learns that this computer has no sensors */
        interruptible_sleep(20);
        glue_done(0);
        return 0;
    }

    time_t t0 = time(NULL), now = t0;
    ts_window_reset(&w, now);
    int cycles = 0;
    int first_report = (strcmp(sent_hash, hash) != 0);   /* inventory goes with the first report when new or changed */

    for (;;) {
        n = ts_read_sensors(sysroot, r, TS_MAX_READINGS);
        ts_window_add(&w, r, n);
        now = time(NULL);

        if (now - w.start >= (time_t)cfg.report_seconds) {
            int l = ts_window_json(&w, now, hash, first_report ? inv : NULL, rep, sizeof rep);
            if (l > 0 && glue_send_report(rep) == 0) {
                first_report = 0;
                checkpoint_save(hash, run_before + (long)(now - t0));
            }
            ts_window_reset(&w, now);
            if (max_cycles_for_tests && ++cycles >= max_cycles_for_tests) return 0;
        }

        if ((long)(now - t0) + run_before >= (long)cfg.max_run_seconds) break;
        if (interruptible_sleep(cfg.sample_seconds)) {
            checkpoint_save(first_report ? "none" : hash, run_before + (long)(time(NULL) - t0));
            return 1;   /* asked to quit: the host decides what happens next (BOINC glue exits without finishing the task) */
        }
        if (max_cycles_for_tests && cycles >= max_cycles_for_tests) return 0;
    }

    /* send what is left, then end: a new task will follow */
    if (w.samples > 0) {
        int l = ts_window_json(&w, time(NULL), hash, first_report ? inv : NULL, rep, sizeof rep);
        if (l > 0) { glue_send_report(rep); interruptible_sleep(20); }      /* time for the client to take the last report */
    }
    glue_done(0);
    return 0;
}
