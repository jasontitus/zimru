//! Generate a bench archive whose URL-sorted order is decorrelated
//! from cluster order (random hex path names), mimicking real
//! scraper-packed archives where iterating by path visits clusters
//! near-randomly. Usage: gen_shuffled_zim OUT.ZIM [ITEMS] [ITEM_KB]

use zimru::writer::{Creator, Item};

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .expect("usage: gen_shuffled_zim OUT.ZIM [ITEMS] [ITEM_KB]");
    let items: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(100_000);
    let kb: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(3);

    let mut x = 0x9E3779B97F4A7C15u64;
    let mut next = move || {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    };

    const WORDS: &[&str] = &[
        "the", "quick", "brown", "fox", "wiki", "article", "section", "content", "<p>", "</p>",
    ];
    let mut c = Creator::new();
    c.set_main_path("home");
    c.add_item(Item::html("home", "Home", "<html><body>home</body></html>"));
    for _ in 0..items {
        let path = format!("{:016x}", next());
        let mut body = String::with_capacity(kb * 1024 + 16);
        while body.len() < kb * 1024 {
            body.push_str(WORDS[(next() % WORDS.len() as u64) as usize]);
            body.push(' ');
        }
        c.add_item(Item::html(path, "", body));
    }
    c.add_metadata("Title", "shuffled bench");
    c.add_metadata("Language", "eng");
    c.write_to(&out).expect("write");
    println!("wrote {out} ({items} items x ~{kb} KiB, random path order)");
}
