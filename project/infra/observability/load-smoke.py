"""Measure latency and errors on a running local, seeded Nexa Gateway."""

import argparse
from concurrent.futures import ThreadPoolExecutor
from http.client import HTTPConnection, HTTPException
import math
from time import monotonic


PATHS = (
    "/api/v1/catalog/products",
    "/api/v1/catalog/products/019b0000-0000-7000-8000-000000000002",
    "/api/v1/search/products?q=mug",
)


def run_worker(end, port):
    samples = []
    errors = 0
    connection = HTTPConnection("127.0.0.1", port, timeout=2)
    try:
        while monotonic() < end:
            path = PATHS[len(samples) % len(PATHS)]
            start = monotonic()
            try:
                connection.request("GET", path)
                response = connection.getresponse()
                response.read()
                if response.status != 200:
                    errors += 1
            except (OSError, HTTPException):
                errors += 1
                connection.close()
            samples.append((monotonic() - start) * 1000)
    finally:
        connection.close()
    return samples, errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seconds", type=int, choices=range(1, 61), default=15,
                        metavar="1..60")
    parser.add_argument("--workers", type=int, choices=range(1, 33), default=8,
                        metavar="1..32")
    parser.add_argument("--port", type=int, default=8082)
    args = parser.parse_args()
    end = monotonic() + args.seconds
    with ThreadPoolExecutor(max_workers=args.workers) as executor:
        runs = [executor.submit(run_worker, end, args.port) for _ in range(args.workers)]
        results = [run.result() for run in runs]
    latencies = sorted(sample for samples, _ in results for sample in samples)
    errors = sum(count for _, count in results)
    if not latencies:
        raise SystemExit("No requests completed")
    p95 = latencies[math.ceil(0.95 * len(latencies)) - 1]
    rate = len(latencies) / args.seconds
    print(f"requests={len(latencies)} errors={errors} requests_per_second={rate:.1f} p95_ms={p95:.2f}")
    raise SystemExit(int(errors != 0 or p95 > 500))


if __name__ == "__main__":
    main()
