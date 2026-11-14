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
    int image;
};

struct window {
    char *name;
    uint64_t start_pc;
    uint64_t end_pc;
    uint64_t first;
    uint64_t instructions;
    uint64_t profiled;
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
static GHashTable *instructions;
static bool system_mode;
static bool profiling;

#define MAX_INSTRUCTIONS 262144
#define MAX_SAMPLES 1048576

struct instruction {
    uint64_t pc;
    size_t size;
    unsigned char bytes[16];
    char *disassembly;
    bool root_access;
    bool mapping_barrier;
    int root_operand;
};

static void free_instruction(void *data)
{
    struct instruction *instruction = data;
    g_free(instruction->disassembly);
    g_free(instruction);
}

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

static void json_string(const char *value);
#include "context.h"
#include "system.h"

struct sample {
    struct instruction *instruction;
    struct context *context;
    uint64_t *counts;
};

static GHashTable *samples;

static void free_sample(void *data)
{
    struct sample *sample = data;
    g_free(sample->counts);
    g_free(sample);
}

static uint64_t *sample_count(struct instruction *instruction)
{
    struct context *context = system_mode
        ? system_context(instruction->pc, instruction->size)
        : (!strcmp(active->name, "mappings") ? observe_context() : &static_context);
    if (!context) { return NULL; }
    if (system_mode && context != &static_context) {
        context = context->image < 0 ? &unknown_context : g_ptr_array_index(contexts, context->image + 2);
    }
    struct { struct instruction *instruction; struct context *context; } key = {instruction, context};
    GBytes *bytes = g_bytes_new(&key, sizeof(key));
    struct sample *sample = g_hash_table_lookup(samples, bytes);
    if (!sample) {
        if (g_hash_table_size(samples) >= MAX_SAMPLES) {
            fail("profile sample capacity exhausted");
            g_bytes_unref(bytes);
            return NULL;
        }
        sample = g_new(struct sample, 1);
        *sample = (struct sample){instruction, context, g_new0(uint64_t, window_count)};
        g_hash_table_insert(samples, bytes, sample);
    } else {
        g_bytes_unref(bytes);
    }
    return &sample->counts[active - windows];
}

static void profile(unsigned int cpu, void *userdata)
{
    (void)cpu;
    struct instruction *instruction = userdata;
    if (!active || (instruction->pc == active->start_pc &&
        (active->start.image < 0 || system_context(instruction->pc, instruction->size)->image == active->start.image))) {
        return;
    }
    uint64_t *count = sample_count(instruction);
    if (!count) { return; }
    if (*count == UINT64_MAX || active->profiled == UINT64_MAX) {
        fail("profile counter overflow");
        return;
    }
    ++*count;
    ++active->profiled;
}

static struct instruction *instrument(struct qemu_plugin_insn *insn)
{
    struct {
        uint64_t pc;
        unsigned char bytes[16];
        uint64_t size;
    } key = { .pc = qemu_plugin_insn_vaddr(insn),
              .size = qemu_plugin_insn_size(insn) };
    if (!key.size || key.size > sizeof(key.bytes) ||
        qemu_plugin_insn_data(insn, key.bytes, sizeof(key.bytes)) != key.size) {
        fail("unsupported profile instruction size");
        return NULL;
    }
    GBytes *bytes = g_bytes_new(&key, sizeof(key));
    struct instruction *instruction = g_hash_table_lookup(instructions, bytes);
    if (!instruction) {
        if (g_hash_table_size(instructions) >= MAX_INSTRUCTIONS) {
            fail("profile instruction capacity exhausted");
            g_bytes_unref(bytes);
            return NULL;
        }
        instruction = g_new0(struct instruction, 1);
        instruction->pc = key.pc;
        instruction->size = key.size;
        memcpy(instruction->bytes, key.bytes, key.size);
        instruction->disassembly = qemu_plugin_insn_disas(insn);
        if (system_mode) { decode_system(instruction); }
        g_hash_table_insert(instructions, bytes, instruction);
    } else {
        g_bytes_unref(bytes);
    }
    if (system_mode && images->len) {
        qemu_plugin_register_vcpu_insn_exec_cb(insn, observe_system, QEMU_PLUGIN_CB_R_REGS, instruction);
        qemu_plugin_register_vcpu_mem_cb(insn, code_written, QEMU_PLUGIN_CB_NO_REGS, QEMU_PLUGIN_MEM_W, NULL);
    }
    return instruction;
}

static void boundary(unsigned int cpu, void *userdata)
{
    struct boundary *edge = userdata;
    struct window *window = edge->window;
    if (edge->image >= 0 && system_context(edge->start ? window->start_pc : window->end_pc, 1)->image != edge->image) { return; }
    uint64_t dispatched = qemu_plugin_u64_get(counter(), cpu);

    if (edge->start) {
        ++window->starts;
        if (active || window->starts != 1 || window->ends) {
            fail("overlapping or repeated start boundary");
            return;
        }
        active = window;
        // The inline increment for this marker follows this callback.
        window->first = dispatched + 1;
    } else {
        ++window->ends;
        if (active != window || window->ends != 1) {
            fail("unmatched or repeated end boundary");
            return;
        }
        window->instructions = dispatched - window->first;
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
        // Observe page-table writes before evaluating an image-qualified boundary.
        struct instruction *instruction = instructions ? instrument(insn) : NULL;
        for (size_t region = 0; region < window_count; ++region) {
            struct window *window = &windows[region];
            if (pc == window->start_pc) {
                qemu_plugin_register_vcpu_insn_exec_cb(
                    insn, boundary, QEMU_PLUGIN_CB_R_REGS, &window->start);
            }
            if (pc == window->end_pc) {
                qemu_plugin_register_vcpu_insn_exec_cb(
                    insn, boundary, QEMU_PLUGIN_CB_R_REGS, &window->end);
            }
        }
        if (profiling && instruction) {
            qemu_plugin_register_vcpu_insn_exec_cb(insn, profile, QEMU_PLUGIN_CB_R_REGS, instruction);
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

static void write_profile(size_t window)
{
    fputs(",\"addresses\":[", output);
    GHashTableIter iterator;
    void *value;
    bool first = true;
    g_hash_table_iter_init(&iterator, samples);
    while (g_hash_table_iter_next(&iterator, NULL, &value)) {
        const struct sample *sample = value;
        const struct instruction *instruction = sample->instruction;
        uint64_t count = sample->counts[window];
        if (!count) {
            continue;
        }
        fprintf(output, "%s{\"pc\":\"0x%" PRIx64 "\",\"bytes\":\"",
                first ? "" : ",", instruction->pc);
        first = false;
        for (size_t byte = 0; byte < instruction->size; ++byte) {
            fprintf(output, "%02x", instruction->bytes[byte]);
        }
        fprintf(output, "\",\"instructions\":%" PRIu64 ",\"disassembly\":",
                count);
        json_string(instruction->disassembly ? instruction->disassembly : "unknown");
        fprintf(output, ",\"context\":%u", sample->context->id);
        fputc('}', output);
    }
    fputc(']', output);
}

static void finish(void *userdata)
{
    (void)userdata;
    for (size_t index = 0; index < window_count; ++index) {
        if (windows[index].starts != 1 || windows[index].ends != 1) {
            fail("missing measurement boundary");
        }
        if (profiling && windows[index].instructions != windows[index].profiled) {
            fail("profile does not sum to the interval count");
        }
    }
    fprintf(output, "{\"metric\":\"guest_instruction_dispatches\","
            "\"plugin_api\":%d,\"valid\":%s,\"error\":",
            QEMU_PLUGIN_VERSION, error ? "false" : "true");
    if (error) {
        json_string(error);
    } else {
        fputs("null", output);
    }
    if (profiling) {
        fprintf(output, ",\"scope\":\"%s\"", system_mode ? "system" : "controlled-mappings");
        if (system_mode) { write_system_contexts(); } else { write_contexts(); }
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
                "\",\"starts\":%u,\"ends\":%u,\"instructions\":%" PRIu64,
                window->start_pc, window->end_pc, window->starts, window->ends,
                window->instructions);
        fputs(",\"start_image\":", output);
        if (window->start.image < 0) { fputs("null", output); } else { fprintf(output, "%d", window->start.image); }
        fputs(",\"end_image\":", output);
        if (window->end.image < 0) { fputs("null", output); } else { fprintf(output, "%d", window->end.image); }
        if (profiling) {
            write_profile(index);
        }
        fputc('}', output);
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
    if (profiling) {
        g_hash_table_destroy(samples);
    }
    if (system_mode) { destroy_system(); } else { g_ptr_array_free(images, true); }
    if (profiling && !system_mode) { destroy_contexts(); }
    if (instructions) { g_hash_table_destroy(instructions); }
    qemu_plugin_scoreboard_free(score);
}

static bool add_window(const char *spec)
{
    gchar **fields = g_strsplit(spec, ":", -1);
    struct window *window = &windows[window_count];
    gint64 start_image = -1, end_image = -1;
    bool valid = g_strv_length(fields) == 5 && *fields[0] &&
        g_ascii_string_to_unsigned(fields[1], 16, 1, UINT64_MAX,
                                   &window->start_pc, NULL) &&
        g_ascii_string_to_unsigned(fields[2], 16, 1, UINT64_MAX,
                                   &window->end_pc, NULL) &&
        g_ascii_string_to_signed(fields[3], 10, -1, G_MAXINT, &start_image, NULL) &&
        g_ascii_string_to_signed(fields[4], 10, -1, G_MAXINT, &end_image, NULL) &&
        (window->start_pc != window->end_pc || start_image != end_image);
    for (size_t index = 0; valid && index < window_count; ++index) {
        const struct window *other = &windows[index];
        valid = strcmp(fields[0], other->name) &&
            window->start_pc != other->start_pc && window->start_pc != other->end_pc &&
            window->end_pc != other->start_pc && window->end_pc != other->end_pc;
    }
    if (valid) {
        window->name = g_strdup(fields[0]);
        window->start = (struct boundary){window, true, start_image};
        window->end = (struct boundary){window, false, end_image};
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
    riscv = !strcmp(info->target_name, "riscv64");
    images = g_ptr_array_new();
    windows = g_new0(struct window, argc);
    for (int index = 0; index < argc; ++index) {
        if (g_str_has_prefix(argv[index], "output=") && !path) {
            path = argv[index] + strlen("output=");
        } else if (!strcmp(argv[index], "case=system") && !system_mode) {
            system_mode = true;
        } else if (!strcmp(argv[index], "profile=on") && !profiling) {
            profiling = true;
        } else if (g_str_has_prefix(argv[index], "kernel=") && !kernel_image) {
            if (!load_image(argv[index] + strlen("kernel="), true)) {
                fputs("Arona perf: invalid kernel executable image\n", stderr); return -1;
            }
        } else if (g_str_has_prefix(argv[index], "image=")) {
            if (!load_image(argv[index] + strlen("image="), false)) {
                fputs("Arona perf: invalid native executable image\n", stderr); return -1;
            }
        } else if (g_str_has_prefix(argv[index], "context=") && !context_address &&
                   g_ascii_string_to_unsigned(argv[index] + strlen("context="), 16, 1, UINT64_MAX,
                                              &context_address, NULL)) {
            continue;
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
    if (system_mode != (kernel_image != NULL)) {
        fputs("Arona perf requires a kernel image only for system collection\n", stderr);
        return -1;
    }
    if ((profiling && !system_mode && !context_address) ||
        (context_address && (!profiling || system_mode)) || (images->len && !system_mode)) {
        fputs("Arona perf requires a context address only for calibration profiling\n", stderr);
        return -1;
    }
    for (size_t index = 0; index < window_count; ++index) {
        if (windows[index].start.image >= (int)images->len || windows[index].end.image >= (int)images->len) {
            fputs("Arona perf: invalid boundary image\n", stderr); return -1;
        }
    }
    if (profiling || (system_mode && images->len)) {
        instructions = g_hash_table_new_full(g_bytes_hash, g_bytes_equal, (GDestroyNotify)g_bytes_unref, free_instruction);
    }
    if (profiling) {
        samples = g_hash_table_new_full(g_bytes_hash, g_bytes_equal,
                                        (GDestroyNotify)g_bytes_unref, free_sample);
    }
    if (system_mode) {
        contexts = g_ptr_array_new();
        static_context.image = -1; unknown_context.image = -1;
        g_ptr_array_add(contexts, &static_context); g_ptr_array_add(contexts, &unknown_context);
        for (guint index = 0; index < images->len; ++index) {
            struct context *context = g_new0(struct context, 1);
            context->id = index + 2; context->image = index;
            g_ptr_array_add(contexts, context);
        }
        mapping_observations = g_ptr_array_new();
        code_pages = g_hash_table_new_full(g_int64_hash, g_int64_equal, g_free, (GDestroyNotify)g_byte_array_unref);
        system_register_bytes = g_byte_array_new();
        kernel_bytes = g_byte_array_new();
        if (images->len) {
            qemu_plugin_register_vcpu_init_cb(id, initialize_system, NULL);
            qemu_plugin_register_vcpu_discon_cb(id, QEMU_PLUGIN_DISCON_ALL, system_discontinuity, NULL);
        }
    } else if (profiling) {
        require_root = !strcmp(info->target_name, "riscv64");
        contexts = g_ptr_array_new();
        g_ptr_array_add(contexts, &static_context);
        known_contexts = g_hash_table_new_full(g_bytes_hash, g_bytes_equal,
                                              (GDestroyNotify)g_bytes_unref, NULL);
        context_bytes = g_byte_array_new();
        register_bytes = g_byte_array_new();
        qemu_plugin_register_vcpu_init_cb(id, initialize_context, NULL);
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
