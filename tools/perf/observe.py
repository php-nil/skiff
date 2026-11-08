#!/usr/bin/env python3
"""1 Hz 资源采样：宿主机 / skiff 进程与线程 / 插件 / 容器 cgroup / nginx / php-fpm。

用法：observe.py <输出目录>
停止：在输出目录创建 stop 文件；阶段名从输出目录的 phase 文件读取。
环境变量：SKIFF_ROOT（默认 /www/skiff）、SITES_ROOT（默认 /www/sites）。
"""
import csv
import glob
import os
import subprocess
import sys
import time

OUT = sys.argv[1]
STOP = os.path.join(OUT, "stop")
PHASE = os.path.join(OUT, "phase")
SKIFF_ROOT = os.environ.get("SKIFF_ROOT", "/www/skiff")
SITES_ROOT = os.environ.get("SITES_ROOT", "/www/sites")
CLK = os.sysconf("SC_CLK_TCK")
PAGE_KB = os.sysconf("SC_PAGE_SIZE") // 1024


def rd(path):
    try:
        with open(path, "rb") as f:
            return f.read().decode("utf-8", "replace")
    except Exception:
        return ""


def proc_fields(pid):
    s = rd("/proc/%d/stat" % pid)
    if not s:
        return None
    try:
        i = s.rfind(")")
        f = s[i + 2:].split()
        return int(f[11]) + int(f[12]), int(f[17]), int(f[21]) * PAGE_KB
    except Exception:
        return None


def all_pids():
    for d in os.listdir("/proc"):
        if d.isdigit():
            yield int(d)


def cmdline(pid):
    return rd("/proc/%d/cmdline" % pid).replace("\0", " ").strip()


def scan(pred):
    out = []
    for pid in all_pids():
        try:
            c = cmdline(pid)
        except Exception:
            continue
        if pred(c):
            out.append((pid, c))
    return out


def host_cpu():
    v = [int(x) for x in rd("/proc/stat").splitlines()[0].split()[1:9]]
    while len(v) < 8:
        v.append(0)
    return v  # user nice system idle iowait irq softirq steal


def meminfo():
    m = {}
    for line in rd("/proc/meminfo").splitlines():
        k, _, rest = line.partition(":")
        m[k] = int(rest.strip().split()[0])
    return m.get("MemAvailable", 0), m.get("MemFree", 0)


def disk_sectors():
    r = w = 0
    for line in rd("/proc/diskstats").splitlines():
        f = line.split()
        if len(f) > 13 and f[2] == "sda":
            r, w = int(f[5]), int(f[9])
    return r, w


def net_bytes():
    rx = tx = 0
    for line in rd("/proc/net/dev").splitlines()[2:]:
        name, _, rest = line.partition(":")
        if name.strip() == "lo":
            continue
        f = rest.split()
        rx += int(f[0])
        tx += int(f[8])
    return rx, tx


def conns():
    est80 = tw80 = est9100 = 0
    for path in ("/proc/net/tcp", "/proc/net/tcp6"):
        for line in rd(path).splitlines()[1:]:
            f = line.split()
            if len(f) < 4:
                continue
            try:
                port = int(f[1].split(":")[1], 16)
            except Exception:
                continue
            if port == 80:
                if f[3] == "01":
                    est80 += 1
                elif f[3] == "06":
                    tw80 += 1
            elif port == 9100 and f[3] == "01":
                est9100 += 1
    return est80, tw80, est9100


def cgroup_paths():
    paths = {}
    for p in glob.glob("/sys/fs/cgroup/system.slice/docker-*.scope"):
        cid = p.rsplit("docker-", 1)[1].rsplit(".scope", 1)[0]
        paths[cid] = p
    names = {}
    try:
        out = subprocess.run(["docker", "ps", "--format", "{{.ID}} {{.Names}}"],
                             capture_output=True, text=True, timeout=5).stdout
        for line in out.splitlines():
            short, _, name = line.partition(" ")
            for cid in paths:
                if cid.startswith(short):
                    names[cid] = name
    except Exception:
        pass
    return {names.get(cid, cid[:12]): p for cid, p in paths.items()}


def cg_stat(path):
    cpu = mem = oom = pids = 0
    try:
        for line in rd(path + "/cpu.stat").splitlines():
            if line.startswith("usage_usec"):
                cpu = int(line.split()[1])
        mem = int(rd(path + "/memory.current").strip() or 0)
        for line in rd(path + "/memory.events").splitlines():
            if line.startswith("oom_kill"):
                oom = int(line.split()[1])
        pids = int(rd(path + "/pids.current").strip() or 0)
    except Exception:
        pass
    return cpu, mem, oom, pids


def size(path):
    try:
        return os.path.getsize(path)
    except Exception:
        return 0


def monitor_samples():
    """监控采样文件：新实现为 JSONL，旧实现为 samples.json，取实际存在的那份。"""
    d = os.path.join(SKIFF_ROOT, "plugins/com.skiff.monitor/data")
    jsonl = os.path.join(d, "samples.jsonl")
    return jsonl if os.path.exists(jsonl) else os.path.join(d, "samples.json")


def main():
    cg = cgroup_paths()
    f = open(os.path.join(OUT, "samples.csv"), "w", newline="", encoding="utf-8")
    w = csv.writer(f)
    w.writerow([
        "ts", "phase", "host_user", "host_system", "host_idle", "host_iowait", "host_steal",
        "mem_avail_kb", "mem_free_kb", "load1", "disk_read_sectors", "disk_write_sectors",
        "net_rx_bytes", "net_tx_bytes", "conn80_est", "conn80_tw", "conn9100_est",
        "nginx_workers", "nginx_cpu", "nginx_rss_kb", "php_children", "php_cpu", "php_rss_kb",
        "rust_cpu", "rust_rss_kb", "skiff_cpu", "skiff_rss_kb", "skiff_threads",
        "skiff_vcsw", "skiff_ivcsw", "skiff_io_read", "skiff_io_write",
        "dockerd_cpu", "dockerd_rss_kb", "loadgen_cpu", "loadgen_rss_kb",
        "plugin_count", "plugin_cpu", "plugin_rss_kb",
        "monitor_samples_bytes", "db_bytes", "site_log_bytes",
    ])
    fp = open(os.path.join(OUT, "procs.csv"), "w", newline="", encoding="utf-8")
    wp = csv.writer(fp)
    wp.writerow(["ts", "phase", "kind", "name", "pid", "cpu_ticks", "rss_kb"])
    ft = open(os.path.join(OUT, "threads.csv"), "w", newline="", encoding="utf-8")
    wt = csv.writer(ft)
    wt.writerow(["ts", "phase", "tid", "name", "ticks", "vcsw", "ivcsw"])
    fc = open(os.path.join(OUT, "conts.csv"), "w", newline="", encoding="utf-8")
    wc = csv.writer(fc)
    wc.writerow(["ts", "phase", "name", "cpu_usec", "mem_current", "oom_kill", "pids"])
    skiff_pid = None
    while not os.path.exists(STOP):
        ts = round(time.time(), 3)
        phase = rd(PHASE).strip() or "idle"
        hc = host_cpu()
        avail, free = meminfo()
        load1 = float(rd("/proc/loadavg").split()[0] or 0)
        dr, dw = disk_sectors()
        nrx, ntx = net_bytes()
        e80, t80, e9100 = conns()

        nginx_all = [(p, c) for p, c in scan(lambda c: "nginx" in c or "openresty" in c)]
        nginx = [(p, s) for p, s in [(p, proc_fields(p)) for p, _ in nginx_all] if s]
        nworkers = sum(1 for _, c in nginx_all if "worker" in c)
        php = [(p, s) for p, s in [(p, proc_fields(p)) for p, c in scan(
            lambda c: "php-fpm" in c and "master" not in c)] if s]
        rust = [(p, s) for p, s in [(p, proc_fields(p)) for p, c in scan(
            lambda c: "skiff-runtime-app" in c)] if s]
        plugins = [(p, c, proc_fields(p)) for p, c in scan(lambda c: SKIFF_ROOT + "/plugins/" in c)]
        skiff = [(p, s) for p, s in [(p, proc_fields(p)) for p, c in scan(
            lambda c: "/usr/local/bin/skiff serve" in c)] if s]
        dockerd = [(p, s) for p, s in [(p, proc_fields(p)) for p, c in scan(
            lambda c: c.startswith("/usr/bin/dockerd") or c.startswith("/usr/bin/containerd"))] if s]
        loadgen = [(p, s) for p, s in [(p, proc_fields(p)) for p, c in scan(
            lambda c: "perf/load.py" in c)] if s]
        if skiff:
            skiff_pid = skiff[0][0]

        vcsw = ivcsw = 0
        if skiff_pid:
            for line in rd("/proc/%d/status" % skiff_pid).splitlines():
                if line.startswith("voluntary_ctxt_switches"):
                    vcsw = int(line.split()[1])
                elif line.startswith("nonvoluntary_ctxt_switches"):
                    ivcsw = int(line.split()[1])
            for tid in os.listdir("/proc/%d/task" % skiff_pid):
                s = rd("/proc/%d/task/%s/stat" % (skiff_pid, tid))
                if not s:
                    continue
                try:
                    i = s.rfind(")")
                    tf = s[i + 2:].split()
                    comm = rd("/proc/%d/task/%s/comm" % (skiff_pid, tid)).strip()
                    tst = rd("/proc/%d/task/%s/status" % (skiff_pid, tid))
                    tvc = tiv = 0
                    for line in tst.splitlines():
                        if line.startswith("voluntary_ctxt_switches"):
                            tvc = int(line.split()[1])
                        elif line.startswith("nonvoluntary_ctxt_switches"):
                            tiv = int(line.split()[1])
                    wt.writerow([ts, phase, tid, comm, int(tf[11]) + int(tf[12]), tvc, tiv])
                except Exception:
                    pass
        ior = iow = 0
        if skiff_pid:
            for line in rd("/proc/%d/io" % skiff_pid).splitlines():
                if line.startswith("read_bytes"):
                    ior = int(line.split()[1])
                elif line.startswith("write_bytes"):
                    iow = int(line.split()[1])

        w.writerow([
            ts, phase, hc[0], hc[2], hc[3], hc[4], hc[7], avail, free, load1, dr, dw, nrx, ntx,
            e80, t80, e9100,
            nworkers, sum(s[0] for _, s in nginx), sum(s[2] for _, s in nginx),
            len(php), sum(s[0] for _, s in php), sum(s[2] for _, s in php),
            sum(s[0] for _, s in rust), sum(s[2] for _, s in rust),
            skiff[0][1][0] if skiff else 0, skiff[0][1][2] if skiff else 0,
            skiff[0][1][1] if skiff else 0, vcsw, ivcsw, ior, iow,
            sum(s[0] for _, s in dockerd), sum(s[2] for _, s in dockerd),
            sum(s[0] for _, s in loadgen), sum(s[2] for _, s in loadgen),
            len(plugins), sum(s[0] for _, _, s in plugins), sum(s[2] for _, _, s in plugins),
            size(monitor_samples()),
            size(os.path.join(SKIFF_ROOT, "skiff.db")),
            sum(size(p) for p in glob.glob(os.path.join(SITES_ROOT, "*", "log", "*.log"))),
        ])
        for kind, items in (("nginx", nginx), ("php", php), ("rust", rust),
                            ("dockerd", dockerd), ("loadgen", loadgen)):
            for p, s in items:
                wp.writerow([ts, phase, kind, cmdline(p)[:60], p, s[0], s[2]])
        for p, c, s in plugins:
            name = c.split("/plugins/")[1].split("/")[0] if "/plugins/" in c else c[:30]
            wp.writerow([ts, phase, "plugin", name, p, s[0], s[2]])
        for name, path in cg.items():
            cpu, mem, oom, pids = cg_stat(path)
            wc.writerow([ts, phase, name, cpu, mem, oom, pids])
        for fh in (f, fp, ft, fc):
            fh.flush()
        time.sleep(max(0.0, 1.0 - (time.time() - ts)))


if __name__ == "__main__":
    main()
