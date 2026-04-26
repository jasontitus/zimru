/*
 * tests/cffi_writer_smoke.c
 *
 * End-to-end smoke test for the writer C ABI. Built and driven by
 * tests/cffi_writer_smoke.rs, which compiles this against
 * include/zimru.h, links libzimru, and invokes it with one argv: the
 * path of a temp file the test should create.
 *
 * The program:
 *   1. Builds a tiny ZIM via the writer C ABI alone (no Rust Creator).
 *      Exercises every primitive the libzim-shim wrapper expects:
 *      set_compression / set_compression_level / set_cluster_size_target
 *      / set_uuid / set_main_path / add_item / add_metadata /
 *      add_illustration / add_redirection / add_alias / write_to.
 *   2. Confirms that an unsupported compression_id is rejected.
 *   3. Confirms that calls after write_to fail cleanly rather than
 *      silently producing a second (corrupt) archive.
 *   4. Reopens the just-written ZIM via the reader C ABI and asserts:
 *      uuid round-trips, the main entry resolves through the redirect
 *      to the C/home item carrying the WRITER-OK marker, metadata is
 *      readable, the illustration and redirect round-trip, and the
 *      MD5 trailer verifies.
 *
 * Argv:
 *   argv[1] = path to write the new ZIM to (will be created/overwritten).
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "zimru.h"

#define DIE(...) do { fprintf(stderr, "writer-smoke: " __VA_ARGS__); fputc('\n', stderr); return 1; } while (0)

/* Same minimal-PNG bytes used by tests/writer_roundtrip.rs::minimal_png:
 * the 8-byte PNG signature is what the readers' regex actually checks,
 * the rest is enough chunk skeleton to pass `zimcheck -A` if anyone
 * wires that in. */
static const uint8_t MINIMAL_PNG[] = {
    0x89,0x50,0x4e,0x47,0x0d,0x0a,0x1a,0x0a,
    /* IHDR */
    0x00,0x00,0x00,0x0d, 'I','H','D','R',
    0x00,0x00,0x00,0x01, 0x00,0x00,0x00,0x01,
    0x08,0x00,0x00,0x00,0x00,
    0x00,0x00,0x00,0x00, /* stub CRC */
    /* empty IDAT */
    0x00,0x00,0x00,0x00, 'I','D','A','T',
    0x00,0x00,0x00,0x00,
    /* IEND */
    0x00,0x00,0x00,0x00, 'I','E','N','D',
    0xae,0x42,0x60,0x82,
};

int main(int argc, char **argv) {
    if (argc != 2) DIE("usage: %s <out.zim>", argv[0]);
    const char *out_path = argv[1];

    zimru_error_t *err = NULL;
    zimru_creator_t *c = zimru_creator_new();
    if (!c) DIE("creator_new returned NULL");

    /* --- configuration --- */

    if (!zimru_creator_set_compression(c, 5 /* zstd */, &err))
        DIE("set_compression: %s", err ? zimru_error_message(err) : "?");
    if (!zimru_creator_set_compression_level(c, 3, &err))
        DIE("set_compression_level: %s", err ? zimru_error_message(err) : "?");
    if (!zimru_creator_set_cluster_size_target(c, 64 * 1024, &err))
        DIE("set_cluster_size_target: %s", err ? zimru_error_message(err) : "?");

    /* Bogus compression_id must error. */
    zimru_error_t *bad_err = NULL;
    if (zimru_creator_set_compression(c, 99, &bad_err)) {
        DIE("expected set_compression(99) to fail");
    }
    if (!bad_err) DIE("set_compression(99) failed silently");
    fprintf(stderr, "expected-bad-compression: %s\n", zimru_error_message(bad_err));
    zimru_error_free(bad_err);

    /* Round-trippable UUID. Last byte chosen so we can spot it back. */
    uint8_t uuid[16] = {
        0x52,0x9b,0x7e,0x6e, 0x3e,0x90, 0x9b,0x9b,
        0x3d,0x24, 0xea,0xc1,0x4f,0x2f,0x1f,0xa5,
    };
    if (!zimru_creator_set_uuid(c, uuid, &err))
        DIE("set_uuid: %s", err ? zimru_error_message(err) : "?");

    if (!zimru_creator_set_main_path(c, "home", &err))
        DIE("set_main_path: %s", err ? zimru_error_message(err) : "?");

    /* --- content --- */

    const char *home_body =
        "<!doctype html><html><body>WRITER-OK marker for C smoke.</body></html>";
    if (!zimru_creator_add_item(c, "home", "Home Page", "text/html",
                                (const uint8_t *)home_body, strlen(home_body), &err))
        DIE("add_item home: %s", err ? zimru_error_message(err) : "?");

    const char *about_body =
        "<!doctype html><html><body>About page (test fixture).</body></html>";
    if (!zimru_creator_add_item(c, "about", "About", "text/html",
                                (const uint8_t *)about_body, strlen(about_body), &err))
        DIE("add_item about: %s", err ? zimru_error_message(err) : "?");

    /* Empty payload must be permitted (zim-tools writers occasionally
     * emit zero-byte items, e.g. placeholders). */
    if (!zimru_creator_add_item(c, "empty", "Empty", "text/plain",
                                NULL, 0, &err))
        DIE("add_item empty: %s", err ? zimru_error_message(err) : "?");

    /* Metadata with the default text mime. */
    if (!zimru_creator_add_metadata(c, "Title", "text/plain;charset=utf-8",
                                    (const uint8_t *)"Writer Smoke", 12, &err))
        DIE("add_metadata Title: %s", err ? zimru_error_message(err) : "?");
    if (!zimru_creator_add_metadata(c, "Language", "text/plain;charset=utf-8",
                                    (const uint8_t *)"eng", 3, &err))
        DIE("add_metadata Language: %s", err ? zimru_error_message(err) : "?");

    /* Metadata with a non-text mime — proves the per-entry mimetype
     * round-trips, not just the historical default. */
    if (!zimru_creator_add_metadata(c, "Counter", "application/octet-stream",
                                    (const uint8_t *)"\x01\x02\x03\x04", 4, &err))
        DIE("add_metadata Counter: %s", err ? zimru_error_message(err) : "?");

    /* Illustration (square, 48px). */
    if (!zimru_creator_add_illustration(c, 48, MINIMAL_PNG, sizeof(MINIMAL_PNG), &err))
        DIE("add_illustration: %s", err ? zimru_error_message(err) : "?");

    /* Redirect: `C/index` → `C/home`. */
    if (!zimru_creator_add_redirection(c, "index", "Index Page", "home", &err))
        DIE("add_redirection: %s", err ? zimru_error_message(err) : "?");

    /* Alias: today wired as redirection — proves the symbol is callable
     * and accepts the spec'd argument shape. */
    if (!zimru_creator_add_alias(c, "start", "Start", "home", &err))
        DIE("add_alias: %s", err ? zimru_error_message(err) : "?");

    /* --- finalize --- */

    if (!zimru_creator_write_to(c, out_path, &err))
        DIE("write_to: %s", err ? zimru_error_message(err) : "?");

    /* Calls after finalize must fail cleanly. */
    zimru_error_t *post_err = NULL;
    if (zimru_creator_add_item(c, "x", "X", "text/html",
                               (const uint8_t *)"y", 1, &post_err)) {
        DIE("expected add_item after write_to to fail");
    }
    if (!post_err) DIE("post-finalize add_item failed silently");
    fprintf(stderr, "expected-post-finalize: %s\n",
            zimru_error_message(post_err));
    zimru_error_free(post_err);

    post_err = NULL;
    if (zimru_creator_write_to(c, out_path, &post_err)) {
        DIE("expected double write_to to fail");
    }
    if (post_err) zimru_error_free(post_err);

    zimru_creator_free(c);

    /* --- read back via the reader C ABI --- */

    zimru_archive_t *a = zimru_archive_open(out_path, &err);
    if (!a) DIE("reopen: %s", err ? zimru_error_message(err) : "?");

    /* UUID round-trips byte-for-byte. */
    uint8_t uuid_back[16] = {0};
    zimru_archive_uuid(a, uuid_back);
    if (memcmp(uuid_back, uuid, 16) != 0) DIE("uuid did not round-trip");

    /* MD5 trailer verifies. */
    if (!zimru_archive_check(a, &err))
        DIE("checksum mismatch: %s", err ? zimru_error_message(err) : "?");

    /* The W/mainPage redirect → C/home article carrying the marker. */
    if (!zimru_archive_has_main_entry(a)) DIE("no main entry");
    zimru_entry_t *me = zimru_archive_main_entry(a, &err);
    if (!me) DIE("main_entry: %s", err ? zimru_error_message(err) : "?");
    zimru_item_t *mi = zimru_entry_get_item(me, true /* follow */, &err);
    if (!mi) DIE("get_item(main, follow): %s", err ? zimru_error_message(err) : "?");
    zimru_blob_t *mb = zimru_item_get_data(mi, &err);
    if (!mb) DIE("get_data(main): %s", err ? zimru_error_message(err) : "?");
    if (memmem(zimru_blob_data(mb), zimru_blob_size(mb), "WRITER-OK", 9) == NULL)
        DIE("home body missing WRITER-OK marker");
    zimru_blob_free(mb);
    zimru_item_free(mi);
    zimru_entry_free(me);

    /* Metadata round-trips (both the text and the binary entry). */
    size_t mlen = 0;
    const uint8_t *title = zimru_archive_metadata(a, "Title", &mlen, &err);
    if (!title) DIE("metadata Title: %s", err ? zimru_error_message(err) : "?");
    if (mlen != 12 || memcmp(title, "Writer Smoke", 12) != 0)
        DIE("metadata Title wrong (mlen=%zu)", mlen);

    size_t clen = 0;
    const uint8_t *counter = zimru_archive_metadata(a, "Counter", &clen, &err);
    if (!counter) DIE("metadata Counter: %s", err ? zimru_error_message(err) : "?");
    if (clen != 4 || memcmp(counter, "\x01\x02\x03\x04", 4) != 0)
        DIE("metadata Counter wrong (clen=%zu)", clen);

    /* Confirm the per-entry mimetype made it onto the dirent: look up
     * M/Counter directly and check its item's mimetype. */
    zimru_entry_t *cm = zimru_archive_get_entry_by_ns_path(a, 'M', "Counter", &err);
    if (!cm) DIE("M/Counter lookup: %s", err ? zimru_error_message(err) : "?");
    zimru_item_t *ci = zimru_entry_get_item(cm, false, &err);
    if (!ci) DIE("M/Counter get_item: %s", err ? zimru_error_message(err) : "?");
    const char *cmime = zimru_item_mimetype(ci);
    if (!cmime || strcmp(cmime, "application/octet-stream") != 0)
        DIE("M/Counter mimetype wrong: %s", cmime ? cmime : "(null)");
    zimru_item_free(ci);
    zimru_entry_free(cm);

    /* Redirect entry exists and is a redirect. */
    zimru_entry_t *idx = zimru_archive_get_entry_by_path(a, "index", &err);
    if (!idx) DIE("lookup index: %s", err ? zimru_error_message(err) : "?");
    if (!zimru_entry_is_redirect(idx)) DIE("index should be a redirect");
    zimru_entry_free(idx);

    /* Alias also present (today as redirect). */
    zimru_entry_t *alias = zimru_archive_get_entry_by_path(a, "start", &err);
    if (!alias) DIE("lookup alias start: %s", err ? zimru_error_message(err) : "?");
    if (!zimru_entry_is_redirect(alias))
        DIE("alias 'start' should be a redirect today");
    zimru_entry_free(alias);

    /* The empty-payload item round-trips with zero bytes. */
    zimru_entry_t *ee = zimru_archive_get_entry_by_path(a, "empty", &err);
    if (!ee) DIE("lookup empty: %s", err ? zimru_error_message(err) : "?");
    zimru_item_t *ei = zimru_entry_get_item(ee, true, &err);
    if (!ei) DIE("get_item(empty): %s", err ? zimru_error_message(err) : "?");
    zimru_blob_t *eb = zimru_item_get_data(ei, &err);
    if (!eb) DIE("get_data(empty): %s", err ? zimru_error_message(err) : "?");
    if (zimru_blob_size(eb) != 0)
        DIE("empty item should have 0 bytes, got %zu", zimru_blob_size(eb));
    zimru_blob_free(eb);
    zimru_item_free(ei);
    zimru_entry_free(ee);

    /* Illustration enumerated. */
    size_t ill_count = 0;
    zimru_illustration_t *ills = zimru_archive_illustrations(a, &ill_count);
    if (ill_count != 1) DIE("expected 1 illustration, got %zu", ill_count);
    if (ills[0].width != 48 || ills[0].height != 48)
        DIE("illustration dims wrong: %ux%u", ills[0].width, ills[0].height);
    zimru_illustrations_free(ills, ill_count);

    /* Structural integrity primitives all pass. */
    if (!zimru_archive_check_dirent_ptrs(a, &err))
        DIE("check_dirent_ptrs: %s", err ? zimru_error_message(err) : "false");
    if (!zimru_archive_check_dirent_order(a, &err))
        DIE("check_dirent_order: %s", err ? zimru_error_message(err) : "false");
    if (!zimru_archive_check_title_index(a, &err))
        DIE("check_title_index: %s", err ? zimru_error_message(err) : "false");
    if (!zimru_archive_check_cluster_ptrs(a, &err))
        DIE("check_cluster_ptrs: %s", err ? zimru_error_message(err) : "false");
    if (!zimru_archive_check_mimetypes(a, &err))
        DIE("check_mimetypes: %s", err ? zimru_error_message(err) : "false");

    zimru_archive_close(a);

    fprintf(stderr, "writer-smoke: OK (%s)\n", out_path);
    return 0;
}
