#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static uint64_t fnv1a64(const uint8_t *data, size_t len) {
    uint64_t h = 14695981039346656037ULL;
    for (size_t i = 0; i < len; i++) {
        h ^= (uint64_t)data[i];
        h *= 1099511628211ULL;
    }
    return h;
}

static uint32_t hash4(const uint8_t *p) {
    uint32_t v;
    memcpy(&v, p, 4);
    return (v * 2654435761U) >> (32 - 16);
}

static size_t lz4_compress_block(const uint8_t *src, size_t src_len, uint8_t *dst) {
    int32_t table[65536];
    for (int i = 0; i < 65536; i++) table[i] = -1;

    size_t ip = 0;
    size_t anchor = 0;
    size_t op = 0;
    size_t limit = src_len > 12 ? src_len - 12 : 0;

    while (ip < limit) {
        uint32_t h = hash4(src + ip);
        int32_t ref = table[h];
        table[h] = (int32_t)ip;

        if (ref >= 0 && (ip - (size_t)ref) <= 65535 && memcmp(src + ref, src + ip, 4) == 0) {
            size_t lit_len = ip - anchor;
            size_t match_len = 4;
            while (ip + match_len < src_len - 5 && src[ref + match_len] == src[ip + match_len]) {
                match_len++;
            }

            size_t token_pos = op++;
            uint8_t lit_tok = lit_len >= 15 ? 15 : (uint8_t)lit_len;
            uint8_t mat_tok = (match_len - 4) >= 15 ? 15 : (uint8_t)(match_len - 4);
            dst[token_pos] = (uint8_t)((lit_tok << 4) | mat_tok);

            if (lit_len >= 15) {
                size_t rem = lit_len - 15;
                while (rem >= 255) {
                    dst[op++] = 255;
                    rem -= 255;
                }
                dst[op++] = (uint8_t)rem;
            }
            if (lit_len > 0) {
                memcpy(dst + op, src + anchor, lit_len);
                op += lit_len;
            }

            uint16_t offset = (uint16_t)(ip - (size_t)ref);
            dst[op++] = (uint8_t)(offset & 0xFF);
            dst[op++] = (uint8_t)((offset >> 8) & 0xFF);

            if (match_len - 4 >= 15) {
                size_t rem = (match_len - 4) - 15;
                while (rem >= 255) {
                    dst[op++] = 255;
                    rem -= 255;
                }
                dst[op++] = (uint8_t)rem;
            }

            ip += match_len;
            anchor = ip;
        } else {
            ip++;
        }
    }

    size_t lit_len = src_len - anchor;
    size_t token_pos = op++;
    uint8_t lit_tok = lit_len >= 15 ? 15 : (uint8_t)lit_len;
    dst[token_pos] = (uint8_t)(lit_tok << 4);
    if (lit_len >= 15) {
        size_t rem = lit_len - 15;
        while (rem >= 255) {
            dst[op++] = 255;
            rem -= 255;
        }
        dst[op++] = (uint8_t)rem;
    }
    if (lit_len > 0) {
        memcpy(dst + op, src + anchor, lit_len);
        op += lit_len;
    }
    return op;
}

static int lz4_decompress_block(const uint8_t *src, size_t src_len, uint8_t *dst, size_t dst_cap, size_t *out_len) {
    size_t ip = 0;
    size_t op = 0;
    while (ip < src_len) {
        uint8_t token = src[ip++];
        size_t lit_len = token >> 4;
        if (lit_len == 15) {
            uint8_t s;
            do {
                if (ip >= src_len) return -1;
                s = src[ip++];
                lit_len += s;
            } while (s == 255);
        }
        if (ip + lit_len > src_len || op + lit_len > dst_cap) return -2;
        memcpy(dst + op, src + ip, lit_len);
        ip += lit_len;
        op += lit_len;

        if (ip >= src_len) break;

        if (ip + 2 > src_len) return -3;
        uint16_t offset = (uint16_t)src[ip] | ((uint16_t)src[ip + 1] << 8);
        ip += 2;
        if (offset == 0 || offset > op) return -4;

        size_t match_len = (token & 0x0F) + 4;
        if ((token & 0x0F) == 15) {
            uint8_t s;
            do {
                if (ip >= src_len) return -5;
                s = src[ip++];
                match_len += s;
            } while (s == 255);
        }
        if (op + match_len > dst_cap) return -6;
        size_t ref = op - offset;
        for (size_t i = 0; i < match_len; i++) {
            dst[op + i] = dst[ref + i];
        }
        op += match_len;
    }
    *out_len = op;
    return 0;
}

static uint8_t *read_entire_file(const char *path, size_t *out_size) {
    FILE *f = fopen(path, "rb");
    if (!f) return NULL;
    fseek(f, 0, SEEK_END);
    long sz = ftell(f);
    fseek(f, 0, SEEK_SET);
    uint8_t *buf = (uint8_t *)malloc((size_t)sz + 1);
    if (!buf) {
        fclose(f);
        return NULL;
    }
    if (sz > 0 && fread(buf, 1, (size_t)sz, f) != (size_t)sz) {
        free(buf);
        fclose(f);
        return NULL;
    }
    fclose(f);
    *out_size = (size_t)sz;
    return buf;
}

int main(int argc, char **argv) {
    if (argc < 4) {
        fprintf(stderr, "Usage: %s <stub.exe> <out_setup.exe> <name=path>...\n", argv[0]);
        return 1;
    }

    size_t stub_size = 0;
    uint8_t *stub = read_entire_file(argv[1], &stub_size);
    if (!stub) {
        fprintf(stderr, "Failed to read stub: %s\n", argv[1]);
        return 1;
    }

    FILE *out = fopen(argv[2], "wb");
    if (!out) {
        fprintf(stderr, "Failed to open output: %s\n", argv[2]);
        free(stub);
        return 1;
    }

    fwrite(stub, 1, stub_size, out);
    free(stub);

    uint64_t payload_offset = (uint64_t)stub_size;
    uint64_t file_count = (uint64_t)(argc - 3);

    for (int i = 3; i < argc; i++) {
        char *eq = strchr(argv[i], '=');
        if (!eq) {
            fprintf(stderr, "Invalid arg (expected name=path): %s\n", argv[i]);
            fclose(out);
            return 1;
        }
        size_t name_len = (size_t)(eq - argv[i]);
        const char *path = eq + 1;

        size_t raw_size = 0;
        uint8_t *raw = read_entire_file(path, &raw_size);
        if (!raw) {
            fprintf(stderr, "Failed to read input file: %s\n", path);
            fclose(out);
            return 1;
        }

        size_t comp_cap = raw_size + (raw_size / 255) + 64;
        uint8_t *comp = (uint8_t *)malloc(comp_cap);
        size_t comp_size = lz4_compress_block(raw, raw_size, comp);

        uint8_t *verify = (uint8_t *)malloc(raw_size + 1);
        size_t dec_len = 0;
        int rc = lz4_decompress_block(comp, comp_size, verify, raw_size, &dec_len);
        if (rc != 0 || dec_len != raw_size || memcmp(raw, verify, raw_size) != 0) {
            fprintf(stderr, "Compression verification failed for %s (rc=%d)\n", path, rc);
            return 1;
        }
        free(verify);

        uint64_t hash = fnv1a64(raw, raw_size);
        uint32_t nlen32 = (uint32_t)name_len;
        uint64_t raw64 = (uint64_t)raw_size;
        uint64_t comp64 = (uint64_t)comp_size;

        fwrite(&nlen32, sizeof(uint32_t), 1, out);
        fwrite(argv[i], 1, name_len, out);
        fwrite(&raw64, sizeof(uint64_t), 1, out);
        fwrite(&comp64, sizeof(uint64_t), 1, out);
        fwrite(&hash, sizeof(uint64_t), 1, out);
        fwrite(comp, 1, comp_size, out);

        printf("  Packed %-30.*s : %8zu -> %8zu bytes (%.1f%%)\n",
               (int)name_len, argv[i], raw_size, comp_size,
               raw_size ? (100.0 * (double)comp_size / (double)raw_size) : 100.0);

        free(comp);
        free(raw);
    }

    const char magic[8] = {'V', 'G', 'S', 'E', 'T', 'U', 'P', '1'};
    fwrite(&payload_offset, sizeof(uint64_t), 1, out);
    fwrite(&file_count, sizeof(uint64_t), 1, out);
    fwrite(magic, 1, 8, out);
    fclose(out);
    return 0;
}
