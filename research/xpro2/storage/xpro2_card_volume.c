/* Owned X-Pro2 volume-label transaction. No flash or firmware-container writes.
 * Call only after successful native formatting, while its filesystem task owns
 * the card. work must point to 3088 writable, 4-byte-aligned bytes.
 * Native FAT implementation/cache execute; read/write services are SDK calls.
 */
typedef unsigned char u8;
typedef unsigned int u32;
typedef int (*io_fn)(u32, u32, u32, void *);
typedef void (*cache_fn)(u32);

struct transaction {
    u8 original[3][512]; /* boot, first root sector, optional backup boot */
    u8 changed[3][512];
    u32 attempted, restored, rollback_error, completed;
};

static __attribute__((always_inline)) inline u32 le16(const u8 *p) {
    return p[0] | ((u32)p[1] << 8);
}
static __attribute__((always_inline)) inline u32 le32(const u8 *p) {
    return le16(p) | (le16(p + 2) << 16);
}
static __attribute__((always_inline)) inline void copy(u8 *dst, const u8 *src, u32 n) {
    for (u32 i = 0; i < n; ++i) dst[i] = src[i];
}

int xpro2_volume_label(u32 drive, const u8 *label, struct transaction *work, u32 delta) {
    work->attempted = work->restored = work->rollback_error = work->completed = 0;
    if (drive > 1 || (delta != 0 && delta != 0x80000)) return 0x13;
    for (u32 i = 0; i < 11; ++i) {
        if (i == 4 || i == 7) { if (label[i] != '-') return 0x13; }
        else if (i == 10) { if (label[i] != ' ') return 0x13; }
        else if (label[i] < '0' || label[i] > '9') return 0x13;
    }
    const u8 *ctx = (const u8 *)(*(volatile u32 *)(0x9b4bfc + delta) + drive * 0xa4);
    u32 capacity = le32(ctx + 4), boot = le32(ctx + 0x24);
    if (le16(ctx + 0x1c) != 512 || boot >= capacity) return 0x13;
    io_fn read = (io_fn)(0x974e4c + delta), write = (io_fn)(0x974e9c + delta);
    cache_fn invalidate = (cache_fn)(0x979da0 + delta);
    u8 *b = work->original[0];
    if (read(drive, boot, 1, b)) return 0x19;
    if (b[510] != 0x55 || b[511] != 0xaa || le16(b + 11) != 512) return 0x13;
    u32 spc = b[13], reserved = le16(b + 14), fats = b[16];
    u32 total = le16(b + 19);
    if (!total) total = le32(b + 32);
    if (!spc || (spc & (spc - 1)) || spc > 128 || !reserved ||
        !fats || fats > 2 || !total || total > capacity - boot) return 0x13;
    u32 fat_size = le16(b + 22), fat32 = !fat_size;
    if (fat32) fat_size = le32(b + 36);
    if (!fat_size || reserved >= total || fat_size > (total - reserved) / fats) return 0x13;
    u32 data = reserved + fats * fat_size;
    u32 root, backup = 0, root_entries = le16(b + 17);
    if (fat32) {
        u32 cluster = le32(b + 44), shift = 0;
        while ((1u << shift) != spc) ++shift;
        if (root_entries || cluster < 2 || data >= total ||
            cluster - 2 > (total - data - 1) >> shift) return 0x13;
        root = boot + data + ((cluster - 2) << shift);
        u32 backup_offset = le16(b + 50);
        if (backup_offset && backup_offset != 0xffff) {
            if (backup_offset >= reserved) return 0x13;
            backup = boot + backup_offset;
            if (read(drive, backup, 1, work->original[2])) return 0x19;
            for (u32 i = 0; i < 512; ++i)
                if (b[i] != work->original[2][i]) return 0x13;
        }
    } else {
        u32 root_sectors = (root_entries * 32 + 511) >> 9;
        if (!root_entries || data >= total || root_sectors >= total - data) return 0x13;
        root = boot + data;
    }
    if (root == boot || (backup && root == backup)) return 0x13;
    if (read(drive, root, 1, work->original[1])) return 0x19;
    u32 free_entry = 512, entry = 512;
    for (u32 i = 0; i < 512; i += 32) {
        const u8 *p = work->original[1] + i;
        if (!p[0]) { if (free_entry == 512) free_entry = i; break; }
        if (p[0] == 0xe5) { if (free_entry == 512) free_entry = i; }
        else if (p[11] == 8) { entry = i; break; }
    }
    if (entry == 512) entry = free_entry;
    if (entry == 512) return 0x13;
    copy(work->changed[0], b, 512);
    copy(work->changed[1], work->original[1], 512);
    if (backup) copy(work->changed[2], work->original[2], 512);
    u32 label_offset = fat32 ? 71 : 43;
    work->changed[0][fat32 ? 66 : 38] = 0x29;
    copy(work->changed[0] + label_offset, label, 11);
    if (backup) {
        work->changed[2][66] = 0x29;
        copy(work->changed[2] + label_offset, label, 11);
    }
    for (u32 i = 0; i < 32; ++i) work->changed[1][entry + i] = 0;
    copy(work->changed[1] + entry, label, 11);
    work->changed[1][entry + 11] = 8;
    /* Preflight is complete. Publish root, backup, then primary BPB. */
    u32 order[3], count = 0, sectors[3];
    sectors[0] = boot; sectors[1] = root; sectors[2] = backup;
    order[count++] = 1;
    if (backup) order[count++] = 2;
    order[count++] = 0;
    for (u32 n = 0; n < count; ++n) {
        u32 index = order[n];
        work->attempted = n + 1;
        if (write(drive, sectors[index], 1, work->changed[index])) {
            /* Include the failed write: a device may write before reporting error. */
            for (u32 r = n + 1; r; --r) {
                u32 restore = order[r - 1];
                if (write(drive, sectors[restore], 1, work->original[restore]))
                    work->rollback_error = 1;
                else ++work->restored;
            }
            invalidate(0); invalidate(1);
            return 0x19;
        }
    }
    invalidate(0); invalidate(1);
    work->completed = 1;
    return 0;
}
