/* Thermopsis monitor: shared declarations. C99, POSIX (Linux). See docs/DESIGN.md. */
#ifndef THERMOPSIS_H
#define THERMOPSIS_H

#include <stddef.h>
#include <stdint.h>
#include <time.h>

#ifdef __cplusplus
extern "C" {
#endif

#define TS_MAX_READINGS 256
#define TS_KEY_LEN 96

/* One sensor value. key = "<kind>.<source>.<name>", kind: t temperature (C), f fan, p power (W), c clock (MHz), u utilisation or load,
 * m memory (percent), b battery, x other (counters, flags). Missing sensors are simply absent; nothing is ever guessed. */
typedef struct {
    char key[TS_KEY_LEN];
    double v;
} ts_reading_t;

/* sysroot: NULL or "" for the real system; a directory for tests (the tree below it mimics /sys and /proc; the GPU library is then not used). */
int ts_read_sensors(const char *sysroot, ts_reading_t *out, int max);

/* The hardware inventory as a JSON object (kinds of hardware, no serial numbers, MAC, host name, UUID or address). Returns the length, or -1 if it did not fit. */
int ts_inventory_json(const char *sysroot, char *buf, size_t cap);

uint32_t ts_hash(const char *s, size_t n);

/* Aggregation over one reporting window: min / mean / max per key. */
typedef struct {
    char key[TS_KEY_LEN];
    double mn, mx, sum;
    int n;
} ts_agg_t;

typedef struct {
    ts_agg_t a[TS_MAX_READINGS];
    int count;
    int samples;
    time_t start;
} ts_window_t;

void ts_window_reset(ts_window_t *w, time_t now);
void ts_window_add(ts_window_t *w, const ts_reading_t *r, int n);
/* {"v":1,"t":<unix>,"w":<window seconds>,"s":<samples>,"inv":"<hash>"[,"inventory":{...}],"m":{"key":[min,mean,max],...}} Returns the length, or -1. */
int ts_window_json(const ts_window_t *w, time_t now, const char *inv_hash, const char *inventory_json, char *buf, size_t cap);

/* What the monitor needs from its host (BOINC or a plain program): see thermopsis_boinc.cpp and thermopsis_standalone.c. */
typedef struct {
    unsigned sample_seconds;    /* time between two sensor reads (default 60) */
    unsigned report_seconds;    /* time between two reports (default 600) */
    unsigned max_run_seconds;   /* the task ends successfully after this long, a new one follows (default 3 days) */
} ts_config_t;

int glue_init(int argc, char **argv, ts_config_t *cfg);                 /* 0 = ok */
size_t glue_checkpoint_read(char *buf, size_t cap);
void glue_checkpoint_write(const char *text, size_t n);
int glue_send_report(const char *json);                                  /* 0 = sent */
int glue_quit_requested(void);
void glue_log(const char *line);
void glue_done(int status);                                              /* never returns */

/* The monitor itself (thermopsis_core.c). Returns only if glue_done does not exit (tests). */
int thermopsis_run(const char *sysroot, int max_cycles_for_tests);

#ifdef __cplusplus
}
#endif

#endif
