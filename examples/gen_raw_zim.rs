//! Generate a bench archive containing one large UNCOMPRESSED item —
//! a stand-in for the conventionally-uncompressed Xapian index / media
//! clusters that `blob_direct_access` / `warmup` are used on.
//!
//! Usage: gen_raw_zim OUT.ZIM [SIZE_MB]

use zimru::writer::{Creator, Item};

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().expect("usage: gen_raw_zim OUT.ZIM [SIZE_MB]");
    let mb: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(256);

    // Incompressible deterministic bytes (xorshift64*), so the raw
    // cluster's on-disk size matches its payload size.
    let mut x = 0x243F6A8885A308D3u64;
    let mut body = Vec::with_capacity(mb << 20);
    while body.len() < (mb << 20) {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        body.extend_from_slice(&x.wrapping_mul(0x2545F4914F6CDD1D).to_le_bytes());
    }

    let mut c = Creator::new();
    c.set_main_path("home");
    c.add_item(Item::html("home", "Home", "<html><body>home</body></html>"));
    c.add_item(
        Item::new("bigraw", "bigraw", "application/octet-stream", body).with_compress(false),
    );
    c.add_metadata("Title", "raw bench");
    c.add_metadata("Language", "eng");
    c.write_to(&out).expect("write");
    println!("wrote {out} ({mb} MiB raw item)");
}
