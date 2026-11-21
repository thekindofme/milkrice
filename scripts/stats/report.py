#!/usr/bin/env python3
"""Print a weekly summary from the CSVs that collect.py writes.

Usage: python3 report.py STATS_DIR [DAYS]   (DAYS defaults to 7)
Read the stats branch without checking it out:
  git fetch origin stats && git worktree add /tmp/milkrice-stats origin/stats
  python3 scripts/stats/report.py /tmp/milkrice-stats
"""
import csv
import datetime as dt
import os
import sys
from collections import defaultdict


def rows(path):
    if not os.path.exists(path):
        return []
    with open(path, newline="") as f:
        return list(csv.DictReader(f))


def window_sum(data, start, end):
    total = uniques = 0
    for r in data:
        if start <= r["date"] < end:
            total += int(r["count"])
            uniques += int(r["uniques"])
    return total, uniques


def downloads_by_kind(data, date):
    out = defaultdict(int)
    for r in data:
        if r["date"] == date:
            out[r["kind"]] += int(r["downloads"])
    return out


def main():
    d = sys.argv[1]
    days = int(sys.argv[2]) if len(sys.argv) > 2 else 7
    today = dt.date.today()
    end = today.isoformat()
    start = (today - dt.timedelta(days=days)).isoformat()
    prev = (today - dt.timedelta(days=2 * days)).isoformat()

    print(f"Milkrice GitHub stats, last {days} days ({start} to {end})\n")
    for kind in ("views", "clones"):
        data = rows(os.path.join(d, f"traffic-{kind}.csv"))
        now, now_u = window_sum(data, start, end)
        before, _ = window_sum(data, prev, start)
        print(f"Repo {kind}: {now} ({now_u} unique per day, summed); previous period {before}")

    repo = rows(os.path.join(d, "repo.csv"))
    if repo:
        last = repo[-1]
        old = next((r for r in repo if r["date"] >= start), repo[0])
        print(f"Stars: {last['stars']} (+{int(last['stars']) - int(old['stars'])}), "
              f"forks: {last['forks']}")

    rel = rows(os.path.join(d, "release-downloads.csv"))
    dates = sorted({r["date"] for r in rel})
    if dates:
        latest = dates[-1]
        earlier = next((x for x in dates if x >= start), dates[0])
        now, then = downloads_by_kind(rel, latest), downloads_by_kind(rel, earlier)
        print(f"\nRelease downloads since {earlier} (all releases):")
        for kind in ("dmg", "update-bundle", "update-check"):
            print(f"  {kind}: +{now[kind] - then[kind]} (total {now[kind]})")

    clicks = [r for r in rows(os.path.join(d, "site-downloads.csv")) if start <= r["date"] < end]
    if clicks:
        by_source, by_button = defaultdict(int), defaultdict(int)
        for r in clicks:
            by_source[r["source"]] += int(r["clicks"])
            by_button[r["button"] or "(outside link)"] += int(r["clicks"])
        print(f"\nWebsite Download clicks: {sum(by_source.values())}")
        for k, v in sorted(by_source.items(), key=lambda kv: -kv[1])[:10]:
            print(f"  from {k}: {v}")
        print("  by button: " + ", ".join(f"{k} {v}" for k, v in sorted(by_button.items(), key=lambda kv: -kv[1])))

    refs = rows(os.path.join(d, "referrers.csv"))
    if refs:
        snap = max(r["snapshot"] for r in refs)
        top = sorted((r for r in refs if r["snapshot"] == snap), key=lambda r: -int(r["count"]))
        print(f"\nTop referrers to the repo (14 days to {snap}):")
        for r in top[:10]:
            print(f"  {r['referrer']}: {r['count']} views, {r['uniques']} unique")


if __name__ == "__main__":
    main()
