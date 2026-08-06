# DS4 sweep review — zimru-misc

Exhaustive per-file pass: 3 code files across 1 batches.

## Findings

# Batch 1 findings — bench/ benchmark harness

- [low] bench/bench.sh:64 — `$SRC` interpolated directly into a `python -c` source string without escaping — a source archive path containing a single quote, newline, or crafted shell metacharacters breaks the Python one-liner or injects arbitrary Python (`Archive('$SRC')` becomes `Archive('foo'));<code>`), and a filename with an apostrophe corrupts the verification step — fix by passing the path as an argv argument and reading `sys.argv[1]` (`$PY -c "..." "$SRC"`), never interpolating into source.

- [medium] bench/build-baseline.sh:46 — libzim cloned from unpinned git master (`git clone -q --depth 1 https://github.com/openzim/libzim.git`) despite the comment claiming "libzim 9.7.0" — the baseline is non-reproducible and silently drifts to arbitrary future commits, invalidating the apples-to-apples benchmark comparison against a fixed 9.7.0 baseline — fix by pinning the tag, e.g. `git clone -q --depth 1 --branch v9.7.0 .../libzim.git`.

- [low] bench/build-baseline.sh:35 — xz 5.4.6 tarball fetched with `curl -fsSL` but never verified against a pinned SHA256 checksum — a tampered/compromised release artifact is built and installed unverified into the benchmark baseline — fix by pinning and checking the checksum (e.g. `echo "<sha256>  $SRC/xz.tar.gz" | sha256sum -c -`) before `tar -xzf`.

## Coverage
- bench/bench.sh — findings: 1
- bench/build-baseline.sh — findings: 2
- bench/zimrecreate_libzim.cpp — clean
