/*
 * tests/cffi_smoke.c
 *
 * Minimum-viable C consumer of zimru's C ABI. The Rust harness in
 * tests/cffi_smoke.rs builds a small ZIM via the writer API, then
 * invokes this program as a subprocess with the path to the ZIM. The C
 * program exercises the read path and exits 0 on success.
 *
 * Argv:
 *   argv[1] = path to a ZIM file (must contain entry "home" of type
 *             text/html with body containing "ZIMRU-OK").
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "zimru.h"

#define DIE(...) do { fprintf(stderr, "cffi_smoke: " __VA_ARGS__); fputc('\n', stderr); return 1; } while (0)

int main(int argc, char **argv) {
    if (argc != 2) {
        DIE("usage: %s <zim>", argv[0]);
    }

    zimru_error_t *err = NULL;
    zimru_archive_t *a = zimru_archive_open(argv[1], &err);
    if (!a) {
        DIE("open failed: %s", err ? zimru_error_message(err) : "no error");
    }

    /* basic shape */
    uint32_t entry_count = zimru_archive_entry_count(a);
    if (entry_count == 0) {
        DIE("entry_count == 0");
    }
    uint32_t cluster_count = zimru_archive_cluster_count(a);
    if (cluster_count == 0) {
        DIE("cluster_count == 0");
    }

    /* uuid is non-zero */
    uint8_t uuid[16] = {0};
    zimru_archive_uuid(a, uuid);
    int any = 0;
    for (int i = 0; i < 16; i++) any |= uuid[i];
    if (!any) {
        DIE("uuid is all zero");
    }

    /* checksum verifies */
    if (!zimru_archive_has_checksum(a)) {
        DIE("archive has no checksum");
    }
    if (!zimru_archive_check(a, &err)) {
        DIE("checksum mismatch: %s", err ? zimru_error_message(err) : "?");
    }

    /* main entry exists, has a path */
    if (!zimru_archive_has_main_entry(a)) {
        DIE("no main entry");
    }
    zimru_entry_t *main_entry = zimru_archive_main_entry(a, &err);
    if (!main_entry) {
        DIE("main_entry: %s", err ? zimru_error_message(err) : "?");
    }
    const char *main_path = zimru_entry_path(main_entry);
    if (!main_path || !*main_path) {
        DIE("main path empty");
    }
    fprintf(stderr, "main path: %s\n", main_path);
    zimru_entry_free(main_entry);

    /* lookup "home" by path, follow redirects, read bytes */
    zimru_entry_t *e = zimru_archive_get_entry_by_path(a, "home", &err);
    if (!e) {
        DIE("get_entry_by_path(home): %s", err ? zimru_error_message(err) : "?");
    }
    fprintf(stderr, "home title: %s\n", zimru_entry_title(e));

    zimru_item_t *it = zimru_entry_get_item(e, true, &err);
    if (!it) {
        DIE("get_item: %s", err ? zimru_error_message(err) : "?");
    }
    const char *mime = zimru_item_mimetype(it);
    fprintf(stderr, "home mime: %s\n", mime);
    if (strcmp(mime, "text/html") != 0) {
        DIE("expected mime text/html, got %s", mime);
    }

    zimru_blob_t *b = zimru_item_get_data(it, &err);
    if (!b) {
        DIE("get_data: %s", err ? zimru_error_message(err) : "?");
    }
    const uint8_t *bytes = zimru_blob_data(b);
    size_t len = zimru_blob_size(b);
    if (len == 0) {
        DIE("blob is empty");
    }
    if (memmem(bytes, len, "ZIMRU-OK", 8) == NULL) {
        DIE("blob does not contain expected marker; first 32 bytes:");
    }
    fprintf(stderr, "blob len: %zu\n", len);

    /* metadata round-trip */
    size_t mlen = 0;
    const uint8_t *title = zimru_archive_metadata(a, "Title", &mlen, &err);
    if (!title) {
        DIE("metadata(Title): %s", err ? zimru_error_message(err) : "?");
    }
    fprintf(stderr, "Title: %.*s\n", (int)mlen, (const char *)title);

    /* cleanup in reverse order */
    zimru_blob_free(b);
    zimru_item_free(it);
    zimru_entry_free(e);
    zimru_archive_close(a);

    /* error path: open a nonexistent file */
    zimru_error_t *err2 = NULL;
    zimru_archive_t *bad = zimru_archive_open("/this/does/not/exist.zim", &err2);
    if (bad) {
        zimru_archive_close(bad);
        DIE("expected open of missing file to fail");
    }
    if (!err2) {
        DIE("expected error pointer to be set on failure");
    }
    fprintf(stderr, "expected-failure msg: %s\n", zimru_error_message(err2));
    zimru_error_free(err2);

    fprintf(stderr, "cffi_smoke: OK\n");
    return 0;
}
