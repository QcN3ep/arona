// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

// Native ET_EXEC images are identified by every byte of their executable load segments.
#include <elf.h>

struct executable_segment {
    uint64_t address;
    uint64_t size;
    const unsigned char *bytes;
};

struct executable_image {
    char *bytes;
    GArray *segments;
};

static GPtrArray *images;
static GHashTable *code_pages;
static GByteArray *system_register_bytes;
static struct qemu_plugin_register *general_registers[32];
static bool riscv;
static bool root_dirty = true;
static bool root_known;
static uint64_t current_root;
static uint64_t mapping_epoch = 1;
static uint64_t pending_pc;
static uint64_t pending_root;
static bool pending_valid;
static struct context *current_context;
static GPtrArray *mapping_observations;

static void advance_epoch(void)
{
    if (mapping_epoch == UINT64_MAX) { fail("mapping epoch overflow"); }
    else { ++mapping_epoch; }
    current_context = NULL;
}

static void system_discontinuity(unsigned int cpu, enum qemu_plugin_discon_type type, uint64_t from_pc, uint64_t to_pc, void *userdata)
{
    (void)cpu; (void)type; (void)from_pc; (void)to_pc; (void)userdata;
    if (pending_pc) { root_known = false; pending_pc = 0; }
    root_dirty = true;
    advance_epoch();
}

static bool read_register_u64(struct qemu_plugin_register *reg, uint64_t *value)
{
    g_byte_array_set_size(system_register_bytes, 0);
    if (!reg || !qemu_plugin_read_register(reg, system_register_bytes) || system_register_bytes->len != 8) {
        return false;
    }
    memcpy(value, system_register_bytes->data, 8);
    *value = GUINT64_FROM_LE(*value);
    return true;
}

static void initialize_system(unsigned int cpu, void *userdata)
{
    (void)cpu; (void)userdata;
    GArray *registers = qemu_plugin_get_registers();
    for (guint index = 0; index < registers->len; ++index) {
        qemu_plugin_reg_descriptor *reg = &g_array_index(registers, qemu_plugin_reg_descriptor, index);
        if (riscv && !strcmp(reg->name, "satp")) { root_register = reg->handle; }
        unsigned int number;
        char tail;
        if (!riscv && sscanf(reg->name, "r%u%c", &number, &tail) == 1 && number < 32) {
            general_registers[number] = reg->handle;
        }
    }
    g_array_free(registers, true);
}

static bool load_image(const char *path)
{
    struct executable_image *image = g_new0(struct executable_image, 1);
    gsize size;
    if (!g_file_get_contents(path, &image->bytes, &size, NULL)) { g_free(image); return false; }
    Elf64_Ehdr header;
    if (size < sizeof(header)) { g_free(image->bytes); g_free(image); return false; }
    memcpy(&header, image->bytes, sizeof(header));
    if (memcmp(header.e_ident, ELFMAG, SELFMAG) || header.e_ident[EI_CLASS] != ELFCLASS64 ||
        header.e_ident[EI_DATA] != ELFDATA2LSB || GUINT16_FROM_LE(header.e_type) != ET_EXEC ||
        GUINT16_FROM_LE(header.e_machine) != (riscv ? EM_RISCV : EM_LOONGARCH) ||
        GUINT16_FROM_LE(header.e_phentsize) != sizeof(Elf64_Phdr)) {
        g_free(image->bytes); g_free(image); return false;
    }
    image->segments = g_array_new(false, false, sizeof(struct executable_segment));
    uint64_t offset = GUINT64_FROM_LE(header.e_phoff);
    for (unsigned int index = 0; index < GUINT16_FROM_LE(header.e_phnum); ++index) {
        Elf64_Phdr segment;
        if (offset > size || size - offset < sizeof(segment)) { goto invalid; }
        memcpy(&segment, image->bytes + offset, sizeof(segment));
        offset += sizeof(segment);
        if (GUINT32_FROM_LE(segment.p_type) != PT_LOAD || !(GUINT32_FROM_LE(segment.p_flags) & PF_X)) { continue; }
        uint64_t start = GUINT64_FROM_LE(segment.p_offset), length = GUINT64_FROM_LE(segment.p_filesz);
        uint64_t address = GUINT64_FROM_LE(segment.p_vaddr);
        if (GUINT32_FROM_LE(segment.p_flags) & PF_W || !length || start > size || length > size - start ||
            address < UINT64_C(0x10000) || address >= UINT64_C(0x4000000000) || length > UINT64_C(0x4000000000) - address) { goto invalid; }
        struct executable_segment value = {address, length, (unsigned char *)image->bytes + start};
        g_array_append_val(image->segments, value);
    }
    if (!image->segments->len) { goto invalid; }
    g_ptr_array_add(images, image);
    return true;
invalid:
    g_array_free(image->segments, true); g_free(image->bytes); g_free(image);
    return false;
}

static bool image_contains(const struct executable_image *image, uint64_t pc, size_t size)
{
    for (guint index = 0; index < image->segments->len; ++index) {
        const struct executable_segment *segment = &g_array_index(image->segments, struct executable_segment, index);
        if (pc >= segment->address && pc - segment->address < segment->size && size <= segment->size - (pc - segment->address)) { return true; }
    }
    return false;
}

static GByteArray *physical_page(uint64_t address)
{
    uint64_t page = address & ~UINT64_C(4095);
    GByteArray *bytes = g_hash_table_lookup(code_pages, &page);
    if (!bytes) {
        bytes = g_byte_array_new();
        if (qemu_plugin_read_memory_hwaddr(page, bytes, 4096) != QEMU_PLUGIN_HWADDR_OPERATION_OK || bytes->len != 4096) {
            g_byte_array_unref(bytes); return NULL;
        }
        uint64_t *key = g_new(uint64_t, 1); *key = page;
        g_hash_table_insert(code_pages, key, bytes);
    }
    return bytes;
}

static bool image_matches(const struct executable_image *image)
{
    for (guint index = 0; index < image->segments->len; ++index) {
        const struct executable_segment *segment = &g_array_index(image->segments, struct executable_segment, index);
        for (uint64_t done = 0; done < segment->size;) {
            uint64_t physical;
            if (!qemu_plugin_translate_vaddr(segment->address + done, &physical)) { return false; }
            size_t offset = physical & 4095;
            size_t length = MIN(UINT64_C(4096) - offset, segment->size - done);
            GByteArray *page = physical_page(physical);
            if (!page || memcmp(page->data + offset, segment->bytes + done, length)) { return false; }
            done += length;
        }
    }
    return true;
}

static struct context *system_context(uint64_t pc, size_t size)
{
    if (pc >= UINT64_C(0xffff000000000000)) { return &static_context; }
    if (pc < UINT64_C(0x10000) || pc >= UINT64_C(0x4000000000)) { return &unknown_context; }
    if (riscv && root_dirty) {
        root_known = read_register_u64(root_register, &current_root) && (current_root >> 60) == 8;
        root_dirty = false;
    }
    if (!root_known || !current_root) { return &unknown_context; }
    bool candidate = false;
    for (guint index = 0; index < images->len; ++index) {
        if (image_contains(g_ptr_array_index(images, index), pc, size)) { candidate = true; break; }
    }
    if (!candidate) { return &unknown_context; }
    if (!current_context) {
        if (mapping_observations->len >= MAX_CONTEXTS) { fail("mapping observation capacity exhausted"); return &unknown_context; }
        struct context *context = g_new0(struct context, 1);
        context->root = current_root;
        context->epoch = mapping_epoch; context->image = -1;
        context->reason = "no matching executable image";
        for (guint index = 0; index < images->len; ++index) {
            struct executable_image *image = g_ptr_array_index(images, index);
            if (!image_contains(image, pc, size) || !image_matches(image)) { continue; }
            if (context->image >= 0) {
                context->image = -1; context->reason = "ambiguous executable image"; break;
            }
            context->image = index; context->reason = NULL;
        }
        g_ptr_array_add(mapping_observations, context); current_context = context;
    }
    if (current_context->image >= 0 && !image_contains(g_ptr_array_index(images, current_context->image), pc, size)) { return &unknown_context; }
    return current_context;
}

static void observe_system(unsigned int cpu, void *userdata)
{
    (void)cpu;
    struct instruction *instruction = userdata;
    if (pending_pc) {
        root_known = pending_valid && instruction->pc == pending_pc + 4;
        if (root_known) { current_root = pending_root; }
        pending_pc = 0;
    }
    if (instruction->root_access) {
        root_dirty = true; advance_epoch();
        if (!riscv) {
            root_known = false; pending_pc = instruction->pc;
            pending_valid = instruction->root_operand >= 0 && read_register_u64(general_registers[instruction->root_operand], &pending_root);
        }
    }
    if (instruction->mapping_barrier) { advance_epoch(); }
}

static void code_written(unsigned int cpu, qemu_plugin_meminfo_t info, uint64_t address, void *userdata)
{
    (void)cpu; (void)userdata;
    struct qemu_plugin_hwaddr *hardware = qemu_plugin_get_hwaddr(info, address);
    if (!hardware || qemu_plugin_hwaddr_is_io(hardware)) { return; }
    uint64_t physical = qemu_plugin_hwaddr_phys_addr(hardware);
    uint64_t page = physical & ~UINT64_C(4095);
    if (g_hash_table_remove(code_pages, &page)) { advance_epoch(); }
    unsigned int shift = qemu_plugin_mem_size_shift(info);
    if (shift < 12 && (physical & 4095) + (UINT64_C(1) << shift) > 4096) {
        page += 4096;
        if (g_hash_table_remove(code_pages, &page)) { advance_epoch(); }
    }
}

static void decode_system(struct instruction *instruction)
{
    uint32_t word = 0;
    memcpy(&word, instruction->bytes, MIN(sizeof(word), instruction->size));
    word = GUINT32_FROM_LE(word);
    instruction->root_operand = -1;
    if (riscv) {
        // REFERENCE: https://docs.riscv.org/reference/isa/priv/supervisor.html
        instruction->root_access = instruction->size == 4 && (word & 0x7f) == 0x73 && (word >> 20) == 0x180 && (word & 0x7000);
    // REFERENCE: https://loongson.github.io/LoongArch-Documentation/LoongArch-Vol1-EN.html
    } else if (instruction->size == 4 && (word >> 24) == 4 && ((word >> 10) & 0x3fff) == 0x19 && ((word >> 5) & 31)) {
        instruction->root_access = true;
        if (((word >> 5) & 31) == 1) { instruction->root_operand = word & 31; }
    }
    const char *text = instruction->disassembly ? instruction->disassembly : "";
    while (g_ascii_isspace(*text)) { ++text; }
    instruction->mapping_barrier = g_str_has_prefix(text, "sfence.vma") || g_str_has_prefix(text, "fence.i") ||
        g_str_has_prefix(text, "invtlb") || g_str_has_prefix(text, "ibar") ||
        g_str_has_prefix(text, "ertn") || g_str_has_prefix(text, "sret") || g_str_has_prefix(text, "mret");
}

static void write_system_contexts(void)
{
    fprintf(output, ",\"context_register\":\"%s\",\"contexts\":[{\"id\":0,\"kind\":\"shared-kernel\"},{\"id\":1,\"kind\":\"unknown\"}", riscv ? "satp" : "csrwr-pgdl");
    for (guint index = 2; index < contexts->len; ++index) {
        struct context *context = g_ptr_array_index(contexts, index);
        fprintf(output, ",{\"id\":%u,\"kind\":\"native-image\",\"image\":%d}", context->id, context->image);
    }
    fputs("],\"mappings\":[", output);
    for (guint index = 0; index < mapping_observations->len; ++index) {
        struct context *context = g_ptr_array_index(mapping_observations, index);
        fprintf(output, "%s{\"root\":\"0x%" PRIx64 "\",\"epoch\":%" PRIu64 ",\"image\":", index ? "," : "", context->root, context->epoch);
        if (context->image < 0) { fputs("null", output); } else { fprintf(output, "%d", context->image); }
        fputs(",\"reason\":", output);
        if (context->reason) { json_string(context->reason); } else { fputs("null", output); }
        fputc('}', output);
    }
    fputc(']', output);
}

static void destroy_system(void)
{
    for (guint index = 0; index < images->len; ++index) {
        struct executable_image *image = g_ptr_array_index(images, index);
        g_array_free(image->segments, true); g_free(image->bytes); g_free(image);
    }
    g_ptr_array_free(images, true);
    for (guint index = 0; index < mapping_observations->len; ++index) { g_free(g_ptr_array_index(mapping_observations, index)); }
    g_ptr_array_free(mapping_observations, true);
    for (guint index = 2; index < contexts->len; ++index) { g_free(g_ptr_array_index(contexts, index)); }
    g_ptr_array_free(contexts, true);
    g_hash_table_destroy(code_pages);
    g_byte_array_unref(system_register_bytes);
}
