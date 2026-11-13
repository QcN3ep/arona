// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#include <qemu-plugin.h>
#include <stdio.h>
#include <string.h>

#if QEMU_PLUGIN_VERSION != 7
#error "Arona perf requires QEMU plugin API 7 headers"
#endif

QEMU_PLUGIN_EXPORT int qemu_plugin_version = QEMU_PLUGIN_VERSION;

struct window;

struct boundary {
    struct window *window;
    bool start;
};

struct window {
    char *name;
    uint64_t start_pc;
    uint64_t end_pc;
    uint64_t first;
    uint64_t instructions;
    unsigned int starts;
    unsigned int ends;
    struct boundary start;
    struct boundary end;
};

static struct window *windows;
static size_t window_count;
static struct window *active;
static struct qemu_plugin_scoreboard *score;
static FILE *output;
static char *output_path;
static char *pending_path;
static const char *error;

static qemu_plugin_u64 counter(void)
{
    return qemu_plugin_scoreboard_u64(score);
}

static void fail(const char *message)
{
    if (!error) {
        error = message;
    }
}

static void overflow(unsigned int cpu, void *userdata)
{
    (void)cpu;
    (void)userdata;
    fail("instruction counter overflow");
}

static void boundary(unsigned int cpu, void *userdata)
{
    struct boundary *edge = userdata;
    struct window *window = edge->window;
    uint64_t instructions = qemu_plugin_u64_get(counter(), cpu);

    if (edge->start) {
        ++window->starts;
        if (active || window->starts != 1 || window->ends) {
            fail("overlapping or repeated start boundary");
            return;
        }
        active = window;
        // The inline increment for this marker follows this callback.
        window->first = instructions + 1;
    } else {
        ++window->ends;
        if (active != window || window->ends != 1) {
            fail("unmatched or repeated end boundary");
            return;
        }
        window->instructions = instructions - window->first;
        active = NULL;
    }
}

// REFERENCE: https://www.qemu.org/docs/master/devel/tcg-plugins.html
static void translate(struct qemu_plugin_tb *tb, void *userdata)
{
    (void)userdata;
    size_t count = qemu_plugin_tb_n_insns(tb);
    for (size_t index = 0; index < count; ++index) {
        struct qemu_plugin_insn *insn = qemu_plugin_tb_get_insn(tb, index);
        uint64_t pc = qemu_plugin_insn_vaddr(insn);
        for (size_t region = 0; region < window_count; ++region) {
            struct window *window = &windows[region];
            struct boundary *edge = pc == window->start_pc ? &window->start :
                                    pc == window->end_pc ? &window->end : NULL;
            if (edge) {
                qemu_plugin_register_vcpu_insn_exec_cb(
                    insn, boundary, QEMU_PLUGIN_CB_NO_REGS, edge);
            }
        }
        qemu_plugin_register_vcpu_insn_exec_inline_per_vcpu(
            insn, QEMU_PLUGIN_INLINE_ADD_U64, counter(), 1);
        qemu_plugin_register_vcpu_insn_exec_cond_cb(
            insn, overflow, QEMU_PLUGIN_CB_NO_REGS, QEMU_PLUGIN_COND_EQ,
            counter(), 0, NULL);
    }
}

static void json_string(const char *value)
{
    fputc('"', output);
    for (const unsigned char *byte = (const unsigned char *)value; *byte; ++byte) {
        if (*byte == '"' || *byte == '\\') {
            fputc('\\', output);
            fputc(*byte, output);
        } else if (*byte < 32) {
            fprintf(output, "\\u%04x", *byte);
        } else {
            fputc(*byte, output);
        }
    }
    fputc('"', output);
}

static void finish(void *userdata)
{
    (void)userdata;
    for (size_t index = 0; index < window_count; ++index) {
        if (windows[index].starts != 1 || windows[index].ends != 1) {
            fail("missing measurement boundary");
        }
    }
    fprintf(output, "{\"schema\":1,\"metric\":\"guest_instruction_dispatches\","
            "\"plugin_api\":%d,\"valid\":%s,\"error\":",
            QEMU_PLUGIN_VERSION, error ? "false" : "true");
    if (error) {
        json_string(error);
    } else {
        fputs("null", output);
    }
    fputs(",\"windows\":[", output);
    for (size_t index = 0; index < window_count; ++index) {
        struct window *window = &windows[index];
        if (index) {
            fputc(',', output);
        }
        fputs("{\"name\":", output);
        json_string(window->name);
        fprintf(output, ",\"start\":\"0x%" PRIx64 "\",\"end\":\"0x%" PRIx64
                "\",\"starts\":%u,\"ends\":%u,\"instructions\":%" PRIu64 "}",
                window->start_pc, window->end_pc, window->starts, window->ends,
                window->instructions);
        g_free(window->name);
    }
    fputs("]}\n", output);
    bool failed = ferror(output);
    if (fclose(output) || failed || rename(pending_path, output_path)) {
        fputs("Arona perf: failed to write instruction counts\n", stderr);
        remove(pending_path);
    }
    g_free(output_path);
    g_free(pending_path);
    g_free(windows);
    qemu_plugin_scoreboard_free(score);
}

static bool add_window(const char *spec)
{
    gchar **fields = g_strsplit(spec, ":", -1);
    struct window *window = &windows[window_count];
    bool valid = g_strv_length(fields) == 3 && *fields[0] &&
        g_ascii_string_to_unsigned(fields[1], 16, 1, UINT64_MAX,
                                   &window->start_pc, NULL) &&
        g_ascii_string_to_unsigned(fields[2], 16, 1, UINT64_MAX,
                                   &window->end_pc, NULL) &&
        window->start_pc != window->end_pc;
    for (size_t index = 0; valid && index < window_count; ++index) {
        const struct window *other = &windows[index];
        valid = strcmp(fields[0], other->name) &&
            window->start_pc != other->start_pc && window->start_pc != other->end_pc &&
            window->end_pc != other->start_pc && window->end_pc != other->end_pc;
    }
    if (valid) {
        window->name = g_strdup(fields[0]);
        window->start = (struct boundary){window, true};
        window->end = (struct boundary){window, false};
        ++window_count;
    }
    g_strfreev(fields);
    return valid;
}

QEMU_PLUGIN_EXPORT int qemu_plugin_install(qemu_plugin_id_t id,
                                           const qemu_info_t *info,
                                           int argc, char **argv)
{
    if (!info->system_emulation || info->system.max_vcpus != 1 ||
        (strcmp(info->target_name, "riscv64") &&
         strcmp(info->target_name, "loongarch64"))) {
        fputs("Arona perf requires one RISC-V or LoongArch system vCPU\n", stderr);
        return -1;
    }
    const char *path = NULL;
    windows = g_new0(struct window, argc);
    for (int index = 0; index < argc; ++index) {
        if (g_str_has_prefix(argv[index], "output=") && !path) {
            path = argv[index] + strlen("output=");
        } else if (g_str_has_prefix(argv[index], "window=") &&
                   add_window(argv[index] + strlen("window="))) {
            continue;
        } else {
            fprintf(stderr, "Arona perf: invalid plugin option: %s\n", argv[index]);
            return -1;
        }
    }
    if (!path || !*path || !window_count) {
        fputs("Arona perf requires an output file and distinct measurement windows\n", stderr);
        return -1;
    }
    output_path = g_strdup(path);
    pending_path = g_strconcat(path, ".tmp", NULL);
    output = fopen(pending_path, "w");
    if (!output) {
        fputs("Arona perf: cannot open instruction count output\n", stderr);
        return -1;
    }
    score = qemu_plugin_scoreboard_new(sizeof(uint64_t));
    qemu_plugin_register_vcpu_tb_trans_cb(id, translate, NULL);
    qemu_plugin_register_atexit_cb(id, finish, NULL);
    return 0;
}
