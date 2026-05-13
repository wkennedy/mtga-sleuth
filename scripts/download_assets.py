#!/usr/bin/env python3
"""Download Scryfall assets used by mtga-sleuth for offline use.

Reads the existing card cache at ~/.cache/mtga-sleuth/scryfall-arena.json
(populated by running the tracker at least once) and downloads:

  * Mana symbol SVGs (Scryfall /symbology endpoint)
  * Card images at the requested sizes (small / normal / large)

Files are stored under ~/.cache/mtga-sleuth/assets/ and skipped if already
present, so re-running just fills in what's missing.

Approximate disk usage for the full ~16k Arena card pool:
    small:    ~640 MB
    normal:   ~2.1 GB
    large:    ~4.0 GB

Usage:
    scripts/download_assets.py                  # symbols + small + normal
    scripts/download_assets.py --sizes small    # just thumbnails
    scripts/download_assets.py --sizes small,normal,large
    scripts/download_assets.py --symbols-only
    scripts/download_assets.py --workers 24
"""

import argparse
import json
import sys
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

CACHE_ROOT = Path.home() / ".cache" / "mtga-sleuth"
CARD_CACHE = CACHE_ROOT / "scryfall-arena.json"
ASSET_ROOT = CACHE_ROOT / "assets"
SYMBOLOGY_URL = "https://api.scryfall.com/symbology"
USER_AGENT = "mtga-sleuth-asset-downloader/0.1 (+https://github.com/local)"
VALID_SIZES = ("small", "normal", "large")


def fetch(url: str, timeout: int = 30) -> bytes:
    # Scryfall returns 400 without an Accept header; urllib doesn't send one by
    # default, so set it explicitly.
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT, "Accept": "*/*"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.read()


def download_to(url: str, dest: Path, timeout: int = 30) -> str:
    """Fetch `url` to `dest`, atomically. Returns 'ok' / 'skip' / error string."""
    if dest.exists() and dest.stat().st_size > 0:
        return "skip"
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(dest.suffix + ".tmp")
    try:
        data = fetch(url, timeout=timeout)
    except urllib.error.HTTPError as e:
        return f"http {e.code}"
    except (urllib.error.URLError, TimeoutError, ConnectionError) as e:
        return f"net {e}"
    tmp.write_bytes(data)
    tmp.rename(dest)
    return "ok"


def filename_for_image(url: str) -> str:
    """Strip query string and return the final path segment (e.g. UUID.jpg)."""
    return url.split("?", 1)[0].rsplit("/", 1)[-1]


def slug_for_symbol(symbol: str) -> str:
    """{W/B} → WB, {2/W} → 2W, {T} → T — matches the URL slug Scryfall uses."""
    return symbol.strip("{}").replace("/", "")


def gather_symbol_tasks() -> list[tuple[str, Path]]:
    print("→ fetching Scryfall symbology catalog…")
    catalog = json.loads(fetch(SYMBOLOGY_URL))["data"]
    out_dir = ASSET_ROOT / "symbols"
    tasks = []
    for sym in catalog:
        url = sym.get("svg_uri")
        symbol = sym.get("symbol")
        if not url or not symbol:
            continue
        tasks.append((url, out_dir / f"{slug_for_symbol(symbol)}.svg"))
    return tasks


def gather_card_tasks(sizes: list[str]) -> list[tuple[str, Path]]:
    if not CARD_CACHE.exists():
        sys.exit(
            f"missing {CARD_CACHE}\n"
            "→ run the tracker at least once (without --no-card-db) so the "
            "Scryfall card cache gets populated, then retry."
        )
    cards = json.loads(CARD_CACHE.read_text())
    tasks = []
    for c in cards:
        for size in sizes:
            url = c.get(f"image_{size}")
            if not url:
                continue
            tasks.append((url, ASSET_ROOT / "cards" / size / filename_for_image(url)))
    return tasks


def run_pool(label: str, tasks: list[tuple[str, Path]], workers: int) -> dict[str, int]:
    total = len(tasks)
    if total == 0:
        print(f"  {label}: nothing to do")
        return {"ok": 0, "skip": 0, "err": 0}
    print(f"→ downloading {total} {label} ({workers} workers)…")
    counts = {"ok": 0, "skip": 0, "err": 0}
    errors: list[str] = []
    started = time.monotonic()
    with ThreadPoolExecutor(max_workers=workers) as pool:
        futures = {pool.submit(download_to, url, dest): (url, dest) for url, dest in tasks}
        for i, fut in enumerate(as_completed(futures), 1):
            res = fut.result()
            if res == "ok":
                counts["ok"] += 1
            elif res == "skip":
                counts["skip"] += 1
            else:
                counts["err"] += 1
                if len(errors) < 5:
                    url, _ = futures[fut]
                    errors.append(f"  {url}: {res}")
            if i % 100 == 0 or i == total:
                elapsed = time.monotonic() - started
                rate = i / elapsed if elapsed > 0 else 0
                print(
                    f"  {i}/{total} ({rate:.0f}/s) "
                    f"ok={counts['ok']} skip={counts['skip']} err={counts['err']}",
                    end="\r",
                    flush=True,
                )
    print()
    if errors:
        print("first errors:")
        print("\n".join(errors))
    return counts


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    ap.add_argument(
        "--sizes",
        default="small,normal",
        help=f"card image sizes to fetch from {VALID_SIZES} (default: small,normal)",
    )
    ap.add_argument("--workers", type=int, default=12, help="concurrent downloads (default: 12)")
    ap.add_argument("--symbols-only", action="store_true", help="skip card images")
    ap.add_argument("--cards-only", action="store_true", help="skip mana symbols")
    args = ap.parse_args()

    if args.symbols_only and args.cards_only:
        sys.exit("--symbols-only and --cards-only are mutually exclusive")

    sizes = [s.strip() for s in args.sizes.split(",") if s.strip()]
    bad = set(sizes) - set(VALID_SIZES)
    if bad:
        sys.exit(f"unknown sizes: {sorted(bad)} — valid: {VALID_SIZES}")

    ASSET_ROOT.mkdir(parents=True, exist_ok=True)
    print(f"asset root: {ASSET_ROOT}")

    if not args.cards_only:
        run_pool("symbol SVGs", gather_symbol_tasks(), args.workers)
    if not args.symbols_only:
        run_pool(f"card images ({','.join(sizes)})", gather_card_tasks(sizes), args.workers)

    print(f"done. assets at {ASSET_ROOT}")


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\ninterrupted — partial downloads cleaned up via .tmp rename pattern")
        sys.exit(130)
