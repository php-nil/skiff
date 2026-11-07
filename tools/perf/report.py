#!/usr/bin/env python3
"""汇总压测采样：每阶段 CPU/IO/延迟/连接数、容器内存峰值、DB 与插件基线。

用法：report.py [输出目录]（默认 /root/perf-20261107）
环境变量：SKIFF_DB（默认 /www/skiff/skiff.db）、SKIFF_ROOT。
"""
import csv
import json
import os
import sqlite3
import sys
from collections import defaultdict

D = sys.argv[1] if len(sys.argv) > 1 else "/root/perf-20261107"
SKIFF_DB = os.environ.get("SKIFF_DB", "/www/skiff/skiff.db")
SKIFF_ROOT = os.environ.get("SKIFF_ROOT", "/www/skiff")
CLK = os.sysconf("SC_CLK_TCK")

rows = list(csv.DictReader(open(os.path.join(D, "samples.csv"), encoding="utf-8")))
for r in rows:
    for k in r:
        if k != "phase":
            try:
                r[k] = float(r[k])
            except Exception:
                r[k] = 0.0
ph = defaultdict(list)
for r in rows:
    ph[r["phase"]].append(r)
for v in ph.values():
    v.sort(key=lambda r: r["ts"])


def d(a, b, key):
    return b[key] - a[key]


def main():
    print("phase\trps\thost%\tskiff%\tplugin%\tnginx%\tphp%\trust%\tdockerd%\tdiskW_KB/s\tskiffW_KB/s\tp99_ms\tconns\tmem_min_MB")
    for name, rs in ph.items():
        if len(rs) < 5:
            continue
        a, b = rs[1], rs[-2]
        dt = b["ts"] - a["ts"]
        if dt <= 0:
            continue
        rps = 0.0
        lp = os.path.join(D, name + ".json")
        p99 = "-"
        if os.path.exists(lp):
            load = json.load(open(lp, encoding="utf-8"))
            rps = load["rps"]
            p99 = load["lat_ms"]["p99"]
        tot = sum(d(a, b, k) for k in ("host_user", "host_system", "host_idle", "host_iowait", "host_steal"))
        used = (tot - d(a, b, "host_idle") - d(a, b, "host_iowait")) / tot * 100 if tot else 0
        cg = defaultdict(list)
        for rr in csv.DictReader(open(os.path.join(D, "conts.csv"), encoding="utf-8")):
            if rr["phase"] == name:
                cg[rr["name"]].append((float(rr["ts"]), int(rr["cpu_usec"])))
        ng = pv = rv = 0.0
        for k, v in cg.items():
            v.sort()
            if len(v) > 2 and v[-1][0] > v[0][0]:
                pctv = (v[-1][1] - v[0][1]) / 1e6 / (v[-1][0] - v[0][0]) * 100
                ng = pctv if "nginx" in k else ng
                pv = pctv if "php" in k else pv
                rv = pctv if "rust" in k else rv
        dock = 0.0
        dd = defaultdict(list)
        for rr in csv.DictReader(open(os.path.join(D, "procs.csv"), encoding="utf-8")):
            if rr["phase"] == name and rr["kind"] == "dockerd":
                dd[rr["pid"]].append((float(rr["ts"]), int(rr["cpu_ticks"])))
        for v in dd.values():
            v.sort()
            if v[-1][0] > v[0][0]:
                dock += (v[-1][1] - v[0][1]) / 100 / (v[-1][0] - v[0][0]) * 100
        print("%s\t%.0f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.2f\t%.0f\t%.0f\t%s\t%d\t%d" % (
            name, rps, used, d(a, b, "skiff_cpu") / CLK / dt * 100,
            d(a, b, "plugin_cpu") / CLK / dt * 100, ng, pv, rv, dock,
            d(a, b, "disk_write_sectors") * 512 / 1024 / dt,
            d(a, b, "skiff_io_write") / 1024 / dt, p99,
            int(max(r["conn80_est"] for r in rs)),
            int(min(r["mem_avail_kb"] for r in rs) / 1024)))

    peak = defaultdict(float)
    for rr in csv.DictReader(open(os.path.join(D, "conts.csv"), encoding="utf-8")):
        peak[rr["name"]] = max(peak[rr["name"]], int(rr["mem_current"]) / 1048576)
    print("\n容器内存峰值(MB):", {k: round(v, 1) for k, v in peak.items()})

    try:
        con = sqlite3.connect("file:%s?mode=ro" % SKIFF_DB, uri=True)
        print("integrity:", con.execute("pragma integrity_check").fetchone()[0],
              "user_version:", con.execute("pragma user_version").fetchone()[0])
        print("plugins:", con.execute("select count(*) from plugins").fetchone()[0],
              "->已启用:", con.execute("select count(*) from plugins where enabled=1").fetchone()[0])
        print("tasks:", con.execute("select count(*) from scheduled_tasks").fetchone()[0])
    except Exception as e:
        print("db check skipped:", e)
    mon = os.path.join(SKIFF_ROOT, "plugins/com.skiff.monitor/data/samples.json")
    if os.path.exists(mon):
        print("monitor samples.json:", os.path.getsize(mon), "bytes")

    before = os.path.join(D, "state-before.json")
    after = os.path.join(D, "state-after.json")
    if os.path.exists(before) and os.path.exists(after):
        a = json.load(open(before, encoding="utf-8"))
        b = json.load(open(after, encoding="utf-8"))
        for key in ("plugins", "grants", "tasks", "sites", "site_dirs"):
            if key in a and key in b:
                print("state %s 一致:" % key, a[key] == b[key])


main()
