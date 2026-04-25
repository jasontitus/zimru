// tests/cffi_smoke.cpp
//
// C++ consumer of zimru's C ABI. Mirrors tests/cffi_smoke.c but exercises
// C++17 idioms (RAII wrappers around the opaque handles, std::string,
// std::optional) to confirm:
//
//   1. The cbindgen-generated header has correct `extern "C"` guards
//      (no C++ name-mangling on the symbols).
//   2. C++ callers can wrap the opaque handles in scoped types without
//      friction.
//   3. The C ABI is usable from any consumer that links against C
//      (which is every real-world target — libkiwix, kiwix-apple .mm,
//      python-libzim's Cython, etc., are all C++ or wrap C++).
//
// Argv:
//   argv[1] = path to a ZIM file (must contain entry "home" of type
//             text/html with body containing "ZIMRU-OK").

#include <cstdint>
#include <cstring>
#include <iostream>
#include <memory>
#include <string>
#include <string_view>

extern "C" {
#include "zimru.h"
}

namespace {

// RAII wrappers — proves the opaque handles round-trip through unique_ptr
// with custom deleters cleanly. This is exactly the pattern a libzim-shim
// `zim::Archive` PIMPL would use.
struct ArchiveDeleter { void operator()(zimru_archive_t* p) const { zimru_archive_close(p); } };
struct EntryDeleter   { void operator()(zimru_entry_t*   p) const { zimru_entry_free(p);    } };
struct ItemDeleter    { void operator()(zimru_item_t*    p) const { zimru_item_free(p);     } };
struct BlobDeleter    { void operator()(zimru_blob_t*    p) const { zimru_blob_free(p);     } };
struct ErrorDeleter   { void operator()(zimru_error_t*   p) const { zimru_error_free(p);    } };

using Archive = std::unique_ptr<zimru_archive_t, ArchiveDeleter>;
using Entry   = std::unique_ptr<zimru_entry_t,   EntryDeleter>;
using Item    = std::unique_ptr<zimru_item_t,    ItemDeleter>;
using Blob    = std::unique_ptr<zimru_blob_t,    BlobDeleter>;
using Err     = std::unique_ptr<zimru_error_t,   ErrorDeleter>;

[[noreturn]] void die(const std::string& msg) {
    std::cerr << "cffi_smoke_cpp: " << msg << '\n';
    std::exit(1);
}

std::string err_msg(zimru_error_t* e) {
    return e ? std::string(zimru_error_message(e)) : std::string("(no error)");
}

} // namespace

int main(int argc, char** argv) {
    if (argc != 2) {
        die("usage: " + std::string(argv[0]) + " <zim>");
    }

    // 1. Open archive — opaque handle wrapped in unique_ptr.
    zimru_error_t* raw_err = nullptr;
    Archive a(zimru_archive_open(argv[1], &raw_err));
    Err err(raw_err);
    if (!a) {
        die("open failed: " + err_msg(err.get()));
    }

    // 2. Shape: non-zero entries / clusters.
    if (zimru_archive_entry_count(a.get()) == 0) die("entry_count == 0");
    if (zimru_archive_cluster_count(a.get()) == 0) die("cluster_count == 0");

    // 3. UUID is non-zero — also confirm we can copy it into a std::array.
    std::uint8_t uuid_bytes[16] = {0};
    zimru_archive_uuid(a.get(), uuid_bytes);
    bool any_uuid = false;
    for (auto b : uuid_bytes) any_uuid |= (b != 0);
    if (!any_uuid) die("uuid all zero");

    // 4. Checksum verifies.
    if (!zimru_archive_has_checksum(a.get())) die("no checksum");
    raw_err = nullptr;
    if (!zimru_archive_check(a.get(), &raw_err)) {
        Err e(raw_err);
        die("checksum mismatch: " + err_msg(e.get()));
    }

    // 5. Main entry exists.
    if (!zimru_archive_has_main_entry(a.get())) die("no main entry");
    raw_err = nullptr;
    Entry main_entry(zimru_archive_main_entry(a.get(), &raw_err));
    if (!main_entry) {
        Err e(raw_err);
        die("main_entry: " + err_msg(e.get()));
    }
    std::string_view main_path = zimru_entry_path(main_entry.get());
    if (main_path.empty()) die("main path empty");
    std::cerr << "main path: " << main_path << '\n';

    // 6. Lookup "home", follow redirects, get item.
    raw_err = nullptr;
    Entry e(zimru_archive_get_entry_by_path(a.get(), "home", &raw_err));
    if (!e) {
        Err er(raw_err);
        die("get_entry_by_path(home): " + err_msg(er.get()));
    }
    std::cerr << "home title: " << zimru_entry_title(e.get()) << '\n';

    raw_err = nullptr;
    Item it(zimru_entry_get_item(e.get(), /*follow=*/true, &raw_err));
    if (!it) {
        Err er(raw_err);
        die("get_item: " + err_msg(er.get()));
    }
    std::string_view mime = zimru_item_mimetype(it.get());
    std::cerr << "home mime: " << mime << '\n';
    if (mime != "text/html") die("expected text/html, got " + std::string(mime));

    // 7. Read bytes via Blob.
    raw_err = nullptr;
    Blob b(zimru_item_get_data(it.get(), &raw_err));
    if (!b) {
        Err er(raw_err);
        die("get_data: " + err_msg(er.get()));
    }
    std::string_view body(
        reinterpret_cast<const char*>(zimru_blob_data(b.get())),
        zimru_blob_size(b.get()));
    if (body.empty()) die("blob empty");
    if (body.find("ZIMRU-OK") == std::string_view::npos) {
        die("blob missing marker; first 64 bytes: " + std::string(body.substr(0, 64)));
    }
    std::cerr << "blob len: " << body.size() << '\n';

    // 8. Metadata as raw bytes.
    raw_err = nullptr;
    std::size_t mlen = 0;
    const std::uint8_t* title_ptr = zimru_archive_metadata(a.get(), "Title", &mlen, &raw_err);
    if (!title_ptr) {
        Err er(raw_err);
        die("metadata(Title): " + err_msg(er.get()));
    }
    std::string_view title(reinterpret_cast<const char*>(title_ptr), mlen);
    std::cerr << "Title: " << title << '\n';

    // 9. Namespace-aware lookup. The new C ABI exposes
    //    zimru_archive_get_entry_by_ns_path so consumers can reach the
    //    M/ namespace (and X/ on full ZIMs) on new-scheme archives,
    //    where get_entry_by_path is restricted to C/.
    if (zimru_archive_uses_new_namespaces(a.get())) {
        std::cerr << "new-scheme archive — exercising ns-path lookup\n";
        zimru_error_t* err_ptr = nullptr;
        Entry m_entry(zimru_archive_get_entry_by_ns_path(
            a.get(), 'M', "Title", &err_ptr));
        if (!m_entry) {
            Err er(err_ptr);
            die("ns-path lookup M/Title: " + err_msg(er.get()));
        }
        std::cerr << "M/Title entry path: " << zimru_entry_path(m_entry.get()) << '\n';
    }

    // 10. By-index entry lookup.
    {
        zimru_error_t* err_ptr = nullptr;
        Entry first(zimru_archive_entry_by_url_index(a.get(), 0, &err_ptr));
        if (!first) {
            Err er(err_ptr);
            die("entry_by_url_index(0): " + err_msg(er.get()));
        }
        std::cerr << "url[0] path: " << zimru_entry_path(first.get()) << '\n';
    }

    // 11. Title count + by-title-index lookup.
    {
        zimru_error_t* err_ptr = nullptr;
        std::uint32_t tcount = zimru_archive_title_count(a.get(), &err_ptr);
        if (tcount == 0) {
            Err er(err_ptr);
            die("title_count returned 0: " + err_msg(er.get()));
        }
        Entry t0(zimru_archive_entry_by_title_index(a.get(), 0, &err_ptr));
        if (!t0) {
            Err er(err_ptr);
            die("entry_by_title_index(0): " + err_msg(er.get()));
        }
        std::cerr << "title[0] path: " << zimru_entry_path(t0.get())
                  << " (of " << tcount << ")\n";
    }

    // 12. Checksum as hex string.
    {
        char hex[33] = {0};
        zimru_error_t* err_ptr = nullptr;
        if (!zimru_archive_checksum_hex(a.get(), hex, &err_ptr)) {
            Err er(err_ptr);
            die("checksum_hex: " + err_msg(er.get()));
        }
        std::cerr << "md5: " << std::string(hex, 32) << '\n';
        if (std::string(hex, 32).find_first_not_of("0123456789abcdef") != std::string::npos) {
            die("checksum_hex returned non-hex bytes");
        }
    }

    // 12a. Phase 2 additions: filesize, article/media counts, random entry,
    //      deterministic seed → UUID.
    {
        std::uint64_t fsz = zimru_archive_filesize(a.get());
        if (fsz == 0) die("filesize == 0");
        std::cerr << "filesize: " << fsz << '\n';

        zimru_error_t* err_ptr = nullptr;
        std::uint64_t arts = zimru_archive_article_count(a.get(), &err_ptr);
        std::uint64_t med = zimru_archive_media_count(a.get(), &err_ptr);
        std::cerr << "articles: " << arts << ", media: " << med << '\n';
        if (arts == 0) die("expected at least one article");

        Entry rnd(zimru_archive_random_entry(a.get(), &err_ptr));
        if (!rnd) {
            Err er(err_ptr);
            die("random_entry: " + err_msg(er.get()));
        }
        std::cerr << "random entry path: " << zimru_entry_path(rnd.get()) << '\n';

        std::uint8_t u1[16] = {0}, u2[16] = {0};
        zimru_uuid_generate(reinterpret_cast<const std::uint8_t*>("seed"), 4, u1);
        zimru_uuid_generate(reinterpret_cast<const std::uint8_t*>("seed"), 4, u2);
        if (std::memcmp(u1, u2, 16) != 0) die("uuid_generate non-deterministic");
        if ((u1[6] & 0xf0) != 0x40) die("uuid v-nibble wrong");
        if ((u1[8] & 0xc0) != 0x80) die("uuid variant wrong");
    }

    // 13. Error path: open a missing file. Confirm the error pointer is set
    //     and that wrapping it in unique_ptr cleans it up automatically.
    {
        zimru_error_t* err_ptr = nullptr;
        Archive bad(zimru_archive_open("/this/does/not/exist.zim", &err_ptr));
        if (bad) die("expected open of missing file to fail");
        Err e(err_ptr);
        if (!e) die("expected error pointer to be set on failure");
        std::cerr << "expected-failure msg: " << err_msg(e.get()) << '\n';
    }

    // RAII unwinds Archive / Entry / Item / Blob / Err in reverse. No
    // explicit cleanup needed — exactly the ergonomics a libzim-shim
    // ::Archive class would provide to its callers.

    std::cerr << "cffi_smoke_cpp: OK\n";
    return 0;
}
