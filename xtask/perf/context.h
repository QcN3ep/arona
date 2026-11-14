// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

// Context descriptors belong only to the controlled calibration program.
#define CONTEXT_MAGIC UINT64_C(0x41524f4e414d4150)
#define MAX_CONTEXTS 262144

struct context {
    uint64_t descriptor[6];
    uint64_t observed_root;
    unsigned int id;
    bool root_readable;
    const char *reason;
};

static uint64_t context_address;
static bool require_root;
static struct qemu_plugin_register *root_register;
static GByteArray *context_bytes;
static GByteArray *register_bytes;
static GPtrArray *contexts;
static GHashTable *known_contexts;
static struct context static_context;

static void initialize_context(unsigned int cpu, void *userdata)
{
    (void)cpu;
    (void)userdata;
    GArray *registers = qemu_plugin_get_registers();
    for (guint index = 0; index < registers->len; ++index) {
        qemu_plugin_reg_descriptor *reg =
            &g_array_index(registers, qemu_plugin_reg_descriptor, index);
        if (!strcmp(reg->name, "satp")) {
            root_register = reg->handle;
        }
    }
    g_array_free(registers, true);
}

static struct context *observe_context(void)
{
    struct context value = {0};
    g_byte_array_set_size(register_bytes, 0);
    value.root_readable = root_register &&
        qemu_plugin_read_register(root_register, register_bytes) && register_bytes->len == 8;
    if (value.root_readable) {
        memcpy(&value.observed_root, register_bytes->data, 8);
        value.observed_root = GUINT64_FROM_LE(value.observed_root);
    }
    // The driver initializes inactive descriptors and completes a memory barrier before switching.
    g_byte_array_set_size(context_bytes, 0);
    if (!qemu_plugin_read_memory_vaddr(context_address, context_bytes, 48) || context_bytes->len != 48) {
        value.reason = "unreadable descriptor";
    } else {
        for (size_t word = 0; word < 6; ++word) {
            uint64_t raw;
            memcpy(&raw, context_bytes->data + word * 8, 8);
            value.descriptor[word] = GUINT64_FROM_LE(raw);
        }
        if (value.descriptor[0] != CONTEXT_MAGIC || !value.descriptor[2] ||
            (!value.descriptor[1] && (value.descriptor[3] || value.descriptor[4] || value.descriptor[5])) ||
            (value.descriptor[1] && (value.descriptor[3] != 0x10000 || !value.descriptor[4] || value.descriptor[5] != 12))) {
            value.reason = "unsupported descriptor";
        } else if (require_root && (!value.root_readable || value.observed_root != value.descriptor[2])) {
            value.reason = "page-table root not verified";
        }
    }
    GBytes *key = NULL;
    if (!value.reason) {
        key = g_bytes_new(value.descriptor, sizeof(value.descriptor));
        struct context *known = g_hash_table_lookup(known_contexts, key);
        if (known) {
            g_bytes_unref(key);
            return known;
        }
        for (guint index = 0; index < contexts->len; ++index) {
            const struct context *other = g_ptr_array_index(contexts, index);
            if (other != &static_context && !other->reason &&
                other->descriptor[1] == value.descriptor[1]) {
                value.reason = "reused instance identifier";
                g_bytes_unref(key);
                key = NULL;
                break;
            }
        }
    }
    if (contexts->len >= MAX_CONTEXTS) {
        fail("profile context capacity exhausted");
        if (key) { g_bytes_unref(key); }
        return NULL;
    }
    struct context *context = g_new(struct context, 1);
    *context = value;
    context->id = contexts->len;
    g_ptr_array_add(contexts, context);
    if (key) { g_hash_table_insert(known_contexts, key, context); }
    return context;
}

static void write_contexts(void)
{
    fprintf(output, ",\"context_register\":%s,\"contexts\":[{\"id\":0,\"kind\":\"static\"}",
            root_register ? "\"satp\"" : "null");
    for (guint index = 1; index < contexts->len; ++index) {
        const struct context *context = g_ptr_array_index(contexts, index);
        fprintf(output, ",{\"id\":%u,\"kind\":\"descriptor\",\"instance\":%" PRIu64
                ",\"root\":\"0x%" PRIx64 "\",\"runtime\":\"0x%" PRIx64
                "\",\"link\":\"0x%" PRIx64 "\",\"size\":%" PRIu64 ",\"observed_root\":",
                context->id, context->descriptor[1], context->descriptor[2],
                context->descriptor[3], context->descriptor[4], context->descriptor[5]);
        if (context->root_readable) {
            fprintf(output, "\"0x%" PRIx64 "\"", context->observed_root);
        } else {
            fputs("null", output);
        }
        fputs(",\"reason\":", output);
        if (context->reason) { json_string(context->reason); } else { fputs("null", output); }
        fputc('}', output);
    }
    fputc(']', output);
}

static void destroy_contexts(void)
{
    for (guint index = 1; index < contexts->len; ++index) {
        g_free(g_ptr_array_index(contexts, index));
    }
    g_ptr_array_free(contexts, true);
    g_hash_table_destroy(known_contexts);
    g_byte_array_unref(context_bytes);
    g_byte_array_unref(register_bytes);
}
