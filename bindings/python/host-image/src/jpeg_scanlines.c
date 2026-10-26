/* Bounded host pixel conversion; no guest memory or hardware effects.
 * IJG libjpeg's public scanline API keeps only MCU rows of source pixels.
 * All setjmp/longjmp scopes are contained in this C translation unit. */
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <setjmp.h>
#include <jpeglib.h>

struct jpeg_error {
    struct jpeg_error_mgr api;
    jmp_buf jump;
    char *message;
};
static void fail(j_common_ptr info) {
    struct jpeg_error *error = (struct jpeg_error *)info->err;
    info->err->format_message(info, error->message);
    longjmp(error->jump, 1);
}
/* A heap context remains defined across longjmp, including partial creation. */
struct encoder {
    struct jpeg_compress_struct info;
    struct jpeg_error error;
    unsigned char *row;
    unsigned char *output;
    unsigned long length;
};
int camera_jpeg_encode(const unsigned char *source, unsigned width,
        unsigned height, size_t stride, const unsigned int *tables,
        unsigned restart, unsigned char **output, size_t *length, char *message) {
    struct encoder *context = calloc(1, sizeof(*context));
    if (!context) { strcpy(message, "JPEG context allocation failed"); return 0; }
    context->info.err = jpeg_std_error(&context->error.api);
    context->error.api.error_exit = fail;
    context->error.message = message;
    if (setjmp(context->error.jump)) {
        jpeg_destroy_compress(&context->info);
        free(context->row); free(context->output); free(context); return 0;
    }
    jpeg_create_compress(&context->info);
    context->row = malloc((size_t)width * 3);
    if (!context->row) {
        jpeg_destroy_compress(&context->info); free(context);
        strcpy(message, "JPEG scanline allocation failed"); return 0;
    }
    jpeg_mem_dest(&context->info, &context->output, &context->length);
    context->info.image_width = width; context->info.image_height = height;
    context->info.input_components = 3; context->info.in_color_space = JCS_YCbCr;
    jpeg_set_defaults(&context->info);
    jpeg_add_quant_table(&context->info, 0, tables, 100, FALSE);
    jpeg_add_quant_table(&context->info, 1, tables + 64, 100, FALSE);
    context->info.comp_info[0].h_samp_factor = 2;
    context->info.comp_info[0].v_samp_factor = 1;
    for (int i = 1; i < 3; i++) {
        context->info.comp_info[i].h_samp_factor = 1;
        context->info.comp_info[i].v_samp_factor = 1;
    }
    context->info.restart_interval = restart;
    context->info.optimize_coding = FALSE;
    jpeg_start_compress(&context->info, TRUE);
    while (context->info.next_scanline < height) {
        const unsigned char *input = source + stride * context->info.next_scanline;
        for (unsigned x = 0; x < width; x += 2) {
            unsigned char *pixel = context->row + x * 3;
            pixel[0] = input[0]; pixel[3] = input[1];
            pixel[1] = pixel[4] = input[2]; pixel[2] = pixel[5] = input[3];
            input += 4;
        }
        JSAMPROW row = context->row;
        jpeg_write_scanlines(&context->info, &row, 1);
    }
    jpeg_finish_compress(&context->info);
    jpeg_destroy_compress(&context->info);
    *output = context->output; *length = context->length;
    free(context->row); free(context); return 1;
}
void camera_jpeg_free(unsigned char *output) { free(output); }

struct decoder {
    struct jpeg_decompress_struct info;
    struct jpeg_error error;
    unsigned char *row;
};
/* Pillow's RGB -> YCbCr law rounds each coefficient to a six-bit fixed-point
 * lookup separately (negative products truncate after adding 0.5). Generate
 * those numerical tables, rather than change the established pixel law to
 * libjpeg's direct YCbCr output. Reference: Pillow ConvertYCbCr.c, SCALE=6.
 * https://github.com/python-pillow/Pillow/blob/12.2.0/src/libImaging/ConvertYCbCr.c */
static void rgb_ycc(const unsigned char *rgb, unsigned char *ycc, int table[9][256]) {
    int r = rgb[0], g = rgb[1], b = rgb[2];
    ycc[0] = (table[0][r] + table[1][g] + table[2][b]) >> 6;
    ycc[1] = ((table[3][r] + table[4][g] + table[5][b]) >> 6) + 128;
    ycc[2] = ((table[6][r] + table[7][g] + table[8][b]) >> 6) + 128;
}
int camera_jpeg_decode(const unsigned char *encoded, size_t count,
        unsigned width, unsigned height, size_t pitch, unsigned char *output,
        char *message) {
    struct decoder *context = calloc(1, sizeof(*context));
    if (!context) { strcpy(message, "JPEG context allocation failed"); return 0; }
    context->info.err = jpeg_std_error(&context->error.api);
    context->error.api.error_exit = fail;
    context->error.message = message;
    if (setjmp(context->error.jump)) {
        jpeg_destroy_decompress(&context->info); free(context->row); free(context); return 0;
    }
    jpeg_create_decompress(&context->info);
    jpeg_mem_src(&context->info, encoded, count);
    jpeg_read_header(&context->info, TRUE);
    if (context->info.image_width != width || context->info.image_height != height
            || context->info.data_precision != 8 || context->info.jpeg_color_space == JCS_CMYK
            || context->info.jpeg_color_space == JCS_YCCK) {
        jpeg_destroy_decompress(&context->info); free(context);
        strcpy(message, "JPEG dimensions/color space differ from bounded RGB law"); return 0;
    }
    int grayscale = context->info.jpeg_color_space == JCS_GRAYSCALE;
    context->info.out_color_space = JCS_RGB;
    context->info.dct_method = JDCT_ISLOW;
    jpeg_start_decompress(&context->info);
    context->row = malloc((size_t)width * 3);
    if (!context->row) {
        jpeg_destroy_decompress(&context->info); free(context);
        strcpy(message, "JPEG scanline allocation failed"); return 0;
    }
    const double coefficients[9] = {.299, .587, .114, -.16874, -.33126, .5, .5, -.41869, -.08131};
    int table[9][256];
    for (unsigned c = 0; c < 9; c++)
        for (unsigned i = 0; i < 256; i++) table[c][i] = (int)(coefficients[c] * 64 * i + .5);
    while (context->info.output_scanline < height) {
        unsigned char *target = output + pitch * 2 * context->info.output_scanline;
        JSAMPROW row = context->row;
        jpeg_read_scanlines(&context->info, &row, 1);
        for (unsigned x = 0; x < width; x += 2) {
            unsigned char a[3], b[3];
            if (grayscale) {
                a[0] = row[x*3]; b[0] = row[(x+1)*3];
                a[1] = a[2] = b[1] = b[2] = 128;
            } else {
                rgb_ycc(row + x * 3, a, table); rgb_ycc(row + (x+1) * 3, b, table);
            }
            target[0] = a[0]; target[1] = b[0];
            target[2] = ((unsigned)a[1] + b[1] + 1) / 2;
            target[3] = ((unsigned)a[2] + b[2] + 1) / 2;
            target += 4;
        }
    }
    jpeg_finish_decompress(&context->info);
    int warnings = context->error.api.num_warnings;
    jpeg_destroy_decompress(&context->info); free(context->row); free(context);
    if (warnings) { strcpy(message, "JPEG decode reported corrupt/truncated input"); return 0; }
    return 1;
}
