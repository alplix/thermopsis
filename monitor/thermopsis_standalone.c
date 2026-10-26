/* Thermopsis monitor without BOINC: prints what it would report. For tests and for a look at real hardware.
 *   thermopsis_standalone [--sysroot DIR] [--cycles N] [--sample SEC] [--report SEC] [--inventory]
 */
#define _GNU_SOURCE
#include "thermopsis.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static unsigned g_sample = 60, g_report = 600;

int glue_init(int argc, char **argv, ts_config_t *cfg)
{
    (void)argc; (void)argv;
    cfg->sample_seconds = g_sample;
    cfg->report_seconds = g_report;
    return 0;
}
size_t glue_checkpoint_read(char *buf, size_t cap) { (void)buf; (void)cap; return 0; }
void glue_checkpoint_write(const char *text, size_t n) { (void)text; (void)n; }
int glue_send_report(const char *json) { printf("REPORT %s\n", json); fflush(stdout); return 0; }
int glue_quit_requested(void) { return 0; }
void glue_log(const char *line) { fprintf(stderr, "%s\n", line); }
void glue_done(int status) { exit(status); }

int main(int argc, char **argv)
{
    const char *root = NULL;
    int cycles = 1;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--sysroot") && i + 1 < argc) root = argv[++i];
        else if (!strcmp(argv[i], "--cycles") && i + 1 < argc) cycles = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--sample") && i + 1 < argc) g_sample = (unsigned)atoi(argv[++i]);
        else if (!strcmp(argv[i], "--report") && i + 1 < argc) g_report = (unsigned)atoi(argv[++i]);
        else if (!strcmp(argv[i], "--inventory")) {
            static char b[24576];
            int l = ts_inventory_json(root, b, sizeof b);
            printf("%s\n(%d bytes)\n", l >= 0 ? b : "(did not fit)", l);
            return 0;
        }
    }
    thermopsis_run(root, cycles);
    return 0;
}
