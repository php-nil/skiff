#!/usr/bin/env python3
"""站点压测客户端：固定并发、时长与每连接速率，输出吞吐/延迟/状态码。

用法：load.py <Host 头> <路径> <并发> <秒> <输出 json> [每连接 rps]
在测试机本机运行（127.0.0.1:80），默认 keep-alive，自动处理 nginx
keepalive_requests 到期后的重连（计入 reconnects，不算失败）。
"""
import asyncio
import json
import sys
import time

HOST, PATH, CONC, DUR, OUT = sys.argv[1], sys.argv[2], int(sys.argv[3]), float(sys.argv[4]), sys.argv[5]
RATE = float(sys.argv[6]) if len(sys.argv) > 6 else 0.0
TARGET_HOST = "127.0.0.1"
TARGET_PORT = 80


async def read_resp(reader):
    head = await reader.readuntil(b"\r\n\r\n")
    lines = head.decode("latin1").split("\r\n")
    status = int(lines[0].split()[1])
    hdrs = {}
    for line in lines[1:]:
        if ":" in line:
            k, v = line.split(":", 1)
            hdrs[k.strip().lower()] = v.strip()
    n = 0
    if "chunked" in hdrs.get("transfer-encoding", "").lower():
        while True:
            line = await reader.readline()
            if not line:
                break
            size = int(line.strip().split(b";")[0], 16)
            if size == 0:
                await reader.readline()
                break
            await reader.readexactly(size)
            n += size
            await reader.readline()
    elif "content-length" in hdrs:
        n = int(hdrs["content-length"])
        if n:
            await reader.readexactly(n)
    return status, len(head) + n


async def worker(deadline, stats, req, period):
    reader = writer = None
    while True:
        if time.time() >= deadline:
            break
        if writer is None:
            try:
                reader, writer = await asyncio.open_connection(TARGET_HOST, TARGET_PORT)
            except Exception:
                stats["errors"] += 1
                await asyncio.sleep(0.05)
                continue
        t0 = time.perf_counter()
        try:
            writer.write(req)
            await writer.drain()
            status, total = await read_resp(reader)
            stats["lat"].append((time.perf_counter() - t0) * 1000.0)
            stats["count"] += 1
            stats["bytes"] += total
            stats["status"][str(status)] = stats["status"].get(str(status), 0) + 1
            b = int((time.time() - stats["start"]) // 5)
            while len(stats["buckets"]) <= b:
                stats["buckets"].append(0)
            stats["buckets"][b] += 1
        except (asyncio.IncompleteReadError, ConnectionResetError, BrokenPipeError):
            stats["reconnects"] += 1
            try:
                writer.close()
            except Exception:
                pass
            reader = writer = None
        except Exception:
            stats["errors"] += 1
            try:
                writer.close()
            except Exception:
                pass
            reader = writer = None
        if period:
            wait = period - (time.perf_counter() - t0)
            if wait > 0:
                await asyncio.sleep(wait)
    if writer is not None:
        try:
            writer.close()
        except Exception:
            pass


def pct(sorted_lat, p):
    if not sorted_lat:
        return 0.0
    return sorted_lat[min(len(sorted_lat) - 1, int(round((len(sorted_lat) - 1) * p)))]


async def main():
    stats = {"lat": [], "count": 0, "errors": 0, "reconnects": 0, "bytes": 0,
             "status": {}, "buckets": [], "start": time.time()}
    req = (
        "GET %s HTTP/1.1\r\nHost: %s\r\nUser-Agent: skiff-perf/1\r\n"
        "Accept: */*\r\nConnection: keep-alive\r\n\r\n" % (PATH, HOST)
    ).encode()
    period = 1.0 / RATE if RATE > 0 else 0.0
    await asyncio.gather(*[worker(time.time() + DUR, stats, req, period) for _ in range(CONC)])
    el = time.time() - stats["start"]
    lat = sorted(stats["lat"])
    out = {
        "host": HOST, "path": PATH, "concurrency": CONC, "rate_per_conn": RATE,
        "duration_s": round(el, 2), "requests": stats["count"],
        "errors": stats["errors"], "reconnects": stats["reconnects"],
        "rps": round(stats["count"] / el, 1) if el else 0,
        "mbps": round(stats["bytes"] * 8 / el / 1e6, 2) if el else 0,
        "status": stats["status"],
        "lat_ms": {"mean": round(sum(lat) / len(lat), 2) if lat else 0,
                   "p50": round(pct(lat, 0.5), 2), "p90": round(pct(lat, 0.9), 2),
                   "p99": round(pct(lat, 0.99), 2), "max": round(lat[-1], 2) if lat else 0},
        "rps_5s_buckets": stats["buckets"],
    }
    with open(OUT, "w", encoding="utf-8") as f:
        json.dump(out, f, ensure_ascii=False, indent=2)
    print(json.dumps(out, ensure_ascii=False))


if __name__ == "__main__":
    asyncio.run(main())
