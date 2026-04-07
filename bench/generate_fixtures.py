#!/usr/bin/env python3
"""Generate benchmark data fixtures for gnata.

Creates logs/, strings/, and datetime/ fixture files.
Account fixtures are symlinked from existing bench/data*.json files.

Usage:
    python3 bench/generate_fixtures.py           # generate all fixtures
    python3 bench/generate_fixtures.py --only logs,strings,datetime
    python3 bench/generate_fixtures.py --sizes 1k,10k
"""

import argparse
import base64
import json
import os
import random
import sys
from pathlib import Path
from urllib.parse import quote as url_quote

BENCH_DIR = Path(__file__).resolve().parent
FIXTURES_DIR = BENCH_DIR / "fixtures"

# Deterministic output for reproducibility
random.seed(42)

# ── Vocabulary for realistic data ────────────────────────────────────────────

LOG_LEVELS = ["debug", "info", "info", "info", "warn", "warn", "error", "error"]
SERVICES = ["auth-svc", "api-gateway", "order-svc", "inventory-svc", "payment-svc", "notification-svc"]
LOG_MESSAGES = [
    "Connection timeout after {ms}ms to {host}",
    "Request completed successfully in {ms}ms",
    "Failed to authenticate user {user}: invalid credentials",
    "Cache miss for key {key}, fetching from database",
    "Rate limit exceeded for client {client}, throttling",
    "Health check passed: {service} (latency={ms}ms)",
    "Database query took {ms}ms: SELECT * FROM orders WHERE status = 'pending'",
    "Incoming request: {method} {path} from {ip}",
    "Response sent: {status} in {ms}ms",
    "Circuit breaker tripped for {service} after {count} failures",
    "Deploying version {version} to {env} environment",
    "Garbage collection completed in {ms}ms, freed {mb}MB",
    "SSL certificate for {domain} expires in {days} days",
    "Outbound webhook delivery failed: {url} returned {status}",
    "Message published to queue {queue}: {bytes} bytes",
]
HOSTS = ["db-primary.internal", "redis-01.cache", "kafka-broker-1", "postgres-replica.internal"]
METHODS = ["GET", "POST", "PUT", "DELETE", "PATCH"]
PATHS = ["/api/v2/users", "/api/v2/orders", "/api/v2/inventory", "/api/v2/auth/token", "/health"]
TAGS_POOL = ["timeout", "database", "auth", "cache", "rate-limit", "deploy", "gc", "ssl", "webhook", "queue"]
REGIONS = ["us-east-1", "us-west-2", "eu-west-1", "ap-southeast-1"]

STRING_TEMPLATES = [
    "ERROR: Connection refused to {host} (host={fqdn}, port={port})",
    "WARNING: Slow query detected ({ms}ms): SELECT orders.* FROM orders JOIN products ON ...",
    "INFO: User {user} logged in from {ip} via {method}",
    "DEBUG: Cache hit ratio: {pct}% ({hits}/{total} requests)",
    "CRITICAL: Disk usage at {pct}% on volume /dev/sda1 ({gb}GB free)",
    "NOTICE: Scheduled maintenance window starting at {time} UTC",
    "ERROR: Invalid JSON payload in request body: unexpected token at position {pos}",
    "WARNING: Memory usage exceeds threshold: {mb}MB / {limit}MB ({pct}%)",
    "INFO: Batch job completed: processed {count} records in {ms}ms",
    "ERROR: TLS handshake failed with {host}: certificate verification error",
]


def size_to_count(size: str) -> int:
    return {"1k": 1000, "10k": 10000, "100k": 100000}.get(size, 1000)


def rand_iso_timestamp() -> str:
    year = 2026
    month = random.randint(1, 12)
    day = random.randint(1, 28)
    hour = random.randint(0, 23)
    minute = random.randint(0, 59)
    second = random.randint(0, 59)
    ms = random.randint(0, 999)
    return f"{year}-{month:02d}-{day:02d}T{hour:02d}:{minute:02d}:{second:02d}.{ms:03d}Z"


def rand_epoch_ms() -> int:
    # Random timestamp in 2026
    base = 1735689600000  # 2025-01-01 00:00:00 UTC
    return base + random.randint(0, 365 * 24 * 3600 * 1000)


# ── Generators ───────────────────────────────────────────────────────────────

def generate_log_entry(i: int) -> dict:
    level = random.choice(LOG_LEVELS)
    service = random.choice(SERVICES)
    template = random.choice(LOG_MESSAGES)
    ms = random.randint(1, 30000)
    msg = template.format(
        ms=ms, host=random.choice(HOSTS), user=f"usr-{random.randint(1000,9999)}",
        key=f"cache:{random.randint(1,1000)}", client=f"client-{random.randint(1,50)}",
        service=service, method=random.choice(METHODS), path=random.choice(PATHS),
        ip=f"{random.randint(10,255)}.{random.randint(0,255)}.{random.randint(0,255)}.{random.randint(1,254)}",
        status=random.choice([200, 201, 400, 401, 403, 404, 500, 502, 503]),
        count=random.randint(1, 100), version=f"v{random.randint(1,5)}.{random.randint(0,20)}.{random.randint(0,99)}",
        env=random.choice(["production", "staging", "canary"]),
        mb=random.randint(50, 500), domain=f"api-{random.randint(1,5)}.example.com",
        days=random.randint(1, 365), url=f"https://webhook.example.com/endpoint-{random.randint(1,20)}",
        bytes=random.randint(100, 10000), queue=random.choice(["orders", "notifications", "analytics"]),
    )
    tags = random.sample(TAGS_POOL, k=random.randint(1, 3))
    return {
        "timestamp": rand_iso_timestamp(),
        "level": level,
        "service": service,
        "message": msg,
        "requestId": f"req-{random.randint(100000, 999999):06x}",
        "duration_ms": ms,
        "tags": tags,
        "context": {
            "host": random.choice(HOSTS),
            "port": random.choice([5432, 6379, 8080, 8443, 9092]),
            "region": random.choice(REGIONS),
        },
    }


def generate_string_entry(i: int) -> dict:
    template = random.choice(STRING_TEMPLATES)
    text = template.format(
        host=random.choice(HOSTS), fqdn=f"prod-{random.randint(1,20):02d}.internal",
        port=random.choice([5432, 6379, 8080, 8443, 9092]),
        ms=random.randint(1, 30000), user=f"user-{random.randint(1000,9999)}",
        ip=f"{random.randint(10,255)}.{random.randint(0,255)}.{random.randint(0,255)}.{random.randint(1,254)}",
        method=random.choice(METHODS), pct=random.randint(50, 99),
        hits=random.randint(500, 9000), total=random.randint(9000, 10000),
        gb=random.randint(1, 500), time=f"{random.randint(0,23):02d}:{random.randint(0,59):02d}",
        pos=random.randint(1, 1000), mb=random.randint(256, 4096),
        limit=random.randint(4096, 8192), count=random.randint(100, 100000),
    )
    code = f"ERR-{random.randint(1000, 9999)}"
    url = f"https://api.example.com/v{random.randint(1,3)}/{random.choice(['users', 'orders', 'products'])}?id={random.randint(1,10000)}&status={random.choice(['active', 'pending'])}"
    b64 = base64.b64encode(text[:50].encode()).decode()
    urlencoded = url_quote(text[:40])
    urlencoded_full = url_quote(url, safe="")

    return {
        "text": text,
        "code": code,
        "url": url,
        "b64": b64,
        "urlencoded": urlencoded,
        "urlencoded_full": urlencoded_full,
    }


def generate_datetime_entry(i: int) -> dict:
    ts = rand_iso_timestamp()
    epoch = rand_epoch_ms()
    months = ["January", "February", "March", "April", "May", "June",
              "July", "August", "September", "October", "November", "December"]
    m = random.randint(1, 12)
    d = random.randint(1, 28)
    return {
        "timestamp": ts,
        "epoch_ms": epoch,
        "formatted": f"{months[m-1]} {d}, 2026",
    }


# ── Setup account symlinks ──────────────────────────────────────────────────

def setup_account_symlinks():
    account_dir = FIXTURES_DIR / "account"
    account_dir.mkdir(parents=True, exist_ok=True)

    links = {
        "tiny.json": "../../data.json",
        "1k.json": "../../data_1k.json",
        "10k.json": "../../data_10k.json",
        "100k.json": "../../data_100k.json",
        "10k_long.json": "../../data_10k_long.json",
        "100k_long.json": "../../data_100k_long.json",
    }

    for name, target in links.items():
        link = account_dir / name
        if link.exists() or link.is_symlink():
            link.unlink()
        link.symlink_to(target)
        print(f"  {link} -> {target}")


def setup_inventory_symlinks():
    inv_dir = FIXTURES_DIR / "inventory"
    inv_dir.mkdir(parents=True, exist_ok=True)

    link = inv_dir / "full.json"
    target = "../../../data/inv.json"
    if link.exists() or link.is_symlink():
        link.unlink()
    link.symlink_to(target)
    print(f"  {link} -> {target}")


# ── Write fixtures ───────────────────────────────────────────────────────────

def write_fixture(fixture_type: str, size: str, generator):
    out_dir = FIXTURES_DIR / fixture_type
    out_dir.mkdir(parents=True, exist_ok=True)
    out_file = out_dir / f"{size}.json"

    count = size_to_count(size)
    print(f"  Generating {fixture_type}/{size}.json ({count} entries)...", end=" ", flush=True)

    # Use the appropriate wrapper key
    wrapper_key = {"logs": "entries", "strings": "items", "datetime": "events"}[fixture_type]
    data = {wrapper_key: [generator(i) for i in range(count)]}

    with open(out_file, "w") as f:
        json.dump(data, f, separators=(",", ":"))

    size_kb = os.path.getsize(out_file) / 1024
    if size_kb > 1024:
        print(f"{size_kb / 1024:.1f}MB")
    else:
        print(f"{size_kb:.0f}KB")


def main():
    p = argparse.ArgumentParser(description="Generate benchmark fixtures")
    p.add_argument("--only", help="Fixture types to generate (comma-separated: logs,strings,datetime)")
    p.add_argument("--sizes", default="1k", help="Sizes to generate (comma-separated, default: 1k)")
    args = p.parse_args()

    types = args.only.split(",") if args.only else ["logs", "strings", "datetime"]
    sizes = args.sizes.split(",")

    generators = {
        "logs": generate_log_entry,
        "strings": generate_string_entry,
        "datetime": generate_datetime_entry,
    }

    print("Setting up symlinks...")
    setup_account_symlinks()
    setup_inventory_symlinks()

    print("\nGenerating fixtures...")
    for fixture_type in types:
        gen = generators.get(fixture_type)
        if not gen:
            print(f"  Unknown fixture type: {fixture_type}", file=sys.stderr)
            continue
        for size in sizes:
            write_fixture(fixture_type, size, gen)

    print("\nDone.")


if __name__ == "__main__":
    main()
