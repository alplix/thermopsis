// Thermopsis monitor: BOINC glue. No boinc_temporary_exit anywhere: a task either runs, or finishes, so a client never sees a "deferred" job.
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <unistd.h>

#include "boinc_api.h"
#include "filesys.h"
#include "app_ipc.h"
#include "thermopsis.h"

static const char *CKPT = "thermopsis_state";

// project preferences (inside <project_specific>): optional <sample_seconds>, <report_seconds>
static unsigned tag_uint(const std::string &xml, const char *tag, unsigned dflt, unsigned lo, unsigned hi)
{
    std::string open = std::string("<") + tag + ">";
    size_t a = xml.find(open);
    if (a == std::string::npos) return dflt;
    unsigned long v = strtoul(xml.c_str() + a + open.size(), NULL, 10);
    if (v < lo || v > hi) return dflt;
    return (unsigned)v;
}

extern "C" int glue_init(int, char **, ts_config_t *cfg)
{
    BOINC_OPTIONS o;
    boinc_options_defaults(o);
    o.main_program = true;
    o.check_heartbeat = true;
    o.handle_process_control = true;
    if (boinc_init_options(&o)) return 1;
    APP_INIT_DATA aid;
    boinc_get_init_data(aid);
    std::string prefs = aid.project_preferences ? aid.project_preferences : "";
    cfg->sample_seconds = tag_uint(prefs, "sample_seconds", cfg->sample_seconds, 10, 600);
    cfg->report_seconds = tag_uint(prefs, "report_seconds", cfg->report_seconds, 60, 3600);
    return 0;
}

extern "C" size_t glue_checkpoint_read(char *buf, size_t cap)
{
    std::string p;
    if (boinc_resolve_filename_s(CKPT, p)) return 0;
    FILE *f = boinc_fopen(p.c_str(), "r");
    if (!f) return 0;
    size_t n = fread(buf, 1, cap - 1, f);
    fclose(f);
    buf[n] = 0;
    return n;
}

extern "C" void glue_checkpoint_write(const char *text, size_t n)
{
    std::string p;
    if (boinc_resolve_filename_s(CKPT, p)) return;
    std::string tmp = p + ".tmp";
    FILE *f = boinc_fopen(tmp.c_str(), "w");
    if (!f) return;
    fwrite(text, 1, n, f);
    fclose(f);
    boinc_rename(tmp.c_str(), p.c_str());
    boinc_checkpoint_completed();
}

extern "C" int glue_send_report(const char *json)
{
    return boinc_send_trickle_up(const_cast<char *>("thermopsis"), const_cast<char *>(json));
}

extern "C" int glue_quit_requested(void)
{
    BOINC_STATUS s;
    boinc_get_status(&s);
    return s.quit_request || s.abort_request || s.no_heartbeat;
}

extern "C" void glue_log(const char *line) { fprintf(stderr, "thermopsis: %s\n", line); }

extern "C" void glue_done(int status)
{
    std::string p;
    if (!boinc_resolve_filename_s("out", p)) {
        FILE *f = boinc_fopen(p.c_str(), "w");
        if (f) { fputs("thermopsis monitor finished\n", f); fclose(f); }
    }
    boinc_finish(status);
}

int main(int, char **)
{
    int r = thermopsis_run(NULL, 0);
    // thermopsis_run returns non-zero when the client asked us to quit: leave through the BOINC API without marking the task finished
    if (r != 0) _exit(0);
    boinc_finish(0);
    return 0;
}
