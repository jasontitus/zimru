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
    if (strcmp(zimru_entry_path(e), "home") != 0)
        DIE("modern entry path must be home without namespace prefix");
    fprintf(stderr, "home title: %s\n", zimru_entry_title(e));

    zimru_item_t *it = zimru_entry_get_item(e, true, &err);
    if (!it) {
        DIE("get_item: %s", err ? zimru_error_message(err) : "?");
    }
    if (strcmp(zimru_item_path(it), "home") != 0)
        DIE("modern item path must be home without namespace prefix");
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

    /* free the per-entry handles; archive stays open for the next
     * exercises below. */
    zimru_blob_free(b);
    zimru_item_free(it);
    zimru_entry_free(e);

    /* New-scheme archives must let us reach the M/ namespace via the
     * new ns-path lookup. (zimru's writer always emits new-scheme.) */
    if (zimru_archive_uses_new_namespaces(a)) {
        zimru_error_t *ns_err = NULL;
        zimru_entry_t *m = zimru_archive_get_entry_by_ns_path(a, 'M', "Title", &ns_err);
        if (!m) {
            DIE("ns-path lookup M/Title: %s", ns_err ? zimru_error_message(ns_err) : "?");
        }
        zimru_entry_free(m);
    }

    /* By-index lookup. */
    zimru_error_t *idx_err = NULL;
    zimru_entry_t *first = zimru_archive_entry_by_url_index(a, 0, &idx_err);
    if (!first) {
        DIE("entry_by_url_index(0): %s", idx_err ? zimru_error_message(idx_err) : "?");
    }
    fprintf(stderr, "url[0] path: %s\n", zimru_entry_path(first));
    zimru_entry_free(first);

    /* Checksum as hex. */
    char hex[33] = {0};
    zimru_error_t *cks_err = NULL;
    if (!zimru_archive_checksum_hex(a, hex, &cks_err)) {
        DIE("checksum_hex: %s", cks_err ? zimru_error_message(cks_err) : "?");
    }
    fprintf(stderr, "md5: %.32s\n", hex);

    /* Phase 2: filesize, article/media counts, random entry. */
    uint64_t fsz = zimru_archive_filesize(a);
    if (fsz == 0) DIE("filesize == 0");
    fprintf(stderr, "filesize: %llu\n", (unsigned long long)fsz);

    zimru_error_t *cnt_err = NULL;
    uint64_t articles = zimru_archive_article_count(a, &cnt_err);
    uint64_t media    = zimru_archive_media_count(a, &cnt_err);
    fprintf(stderr, "articles: %llu, media: %llu\n",
            (unsigned long long)articles, (unsigned long long)media);
    if (articles == 0) {
        DIE("expected at least one article (the test ZIM has 'home')");
    }

    zimru_error_t *r_err = NULL;
    zimru_entry_t *rnd = zimru_archive_random_entry(a, &r_err);
    if (!rnd) {
        DIE("random_entry: %s", r_err ? zimru_error_message(r_err) : "?");
    }
    fprintf(stderr, "random entry path: %s\n", zimru_entry_path(rnd));
    zimru_entry_free(rnd);

    /* Newly added shim primitives: main_entry_index, cluster_offset,
     * item_cluster_index / item_blob_index, and the structural
     * integrity checks. Spot-check that they all return sensible
     * values on this clean fixture. */
    uint32_t mei = zimru_archive_main_entry_index(a);
    if (mei == 0xFFFFFFFFu) {
        DIE("main_entry_index returned NO_MAIN_PAGE on archive with main entry");
    }
    fprintf(stderr, "main_entry_index: %u\n", mei);

    zimru_error_t *co_err = NULL;
    uint64_t coff = zimru_archive_cluster_offset(a, 0, &co_err);
    if (co_err) {
        DIE("cluster_offset(0): %s", zimru_error_message(co_err));
    }
    if (coff == 0 || coff >= fsz) {
        DIE("cluster_offset(0) out of range: %llu (file size %llu)",
            (unsigned long long)coff, (unsigned long long)fsz);
    }

    /* Re-open "home" briefly to exercise item_cluster_index /
     * item_blob_index. */
    zimru_error_t *ie_err = NULL;
    zimru_entry_t *he = zimru_archive_get_entry_by_path(a, "home", &ie_err);
    if (!he) {
        DIE("re-lookup home: %s", ie_err ? zimru_error_message(ie_err) : "?");
    }
    zimru_item_t *hi = zimru_entry_get_item(he, true, &ie_err);
    if (!hi) {
        DIE("re-get_item home: %s", ie_err ? zimru_error_message(ie_err) : "?");
    }
    uint32_t ci = zimru_item_cluster_index(hi);
    uint32_t bi = zimru_item_blob_index(hi);
    if (ci >= cluster_count) {
        DIE("item_cluster_index %u out of range (count %u)", ci, cluster_count);
    }
    fprintf(stderr, "home cluster=%u blob=%u\n", ci, bi);
    zimru_item_free(hi);
    zimru_entry_free(he);

    /* Each integrity primitive should pass on a clean fixture. */
    zimru_error_t *chk_err = NULL;
    if (!zimru_archive_check_dirent_ptrs(a, &chk_err)) {
        DIE("check_dirent_ptrs failed on clean fixture: %s",
            chk_err ? zimru_error_message(chk_err) : "false");
    }
    if (!zimru_archive_check_dirent_order(a, &chk_err)) {
        DIE("check_dirent_order failed on clean fixture: %s",
            chk_err ? zimru_error_message(chk_err) : "false");
    }
    if (!zimru_archive_check_title_index(a, &chk_err)) {
        DIE("check_title_index failed on clean fixture: %s",
            chk_err ? zimru_error_message(chk_err) : "false");
    }
    if (!zimru_archive_check_cluster_ptrs(a, &chk_err)) {
        DIE("check_cluster_ptrs failed on clean fixture: %s",
            chk_err ? zimru_error_message(chk_err) : "false");
    }
    if (!zimru_archive_check_mimetypes(a, &chk_err)) {
        DIE("check_mimetypes failed on clean fixture: %s",
            chk_err ? zimru_error_message(chk_err) : "false");
    }

    /* Stable seed → UUID. Same seed twice == identical bytes. */
    uint8_t u1[16] = {0}, u2[16] = {0};
    zimru_uuid_generate((const uint8_t *)"seed", 4, u1);
    zimru_uuid_generate((const uint8_t *)"seed", 4, u2);
    if (memcmp(u1, u2, 16) != 0) DIE("uuid_generate is not deterministic");
    if ((u1[6] & 0xf0) != 0x40) DIE("uuid version nibble != 4");
    if ((u1[8] & 0xc0) != 0x80) DIE("uuid variant nibbles != 10xx");

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
