#!/usr/bin/env python3
"""Save the website's download-click counts (from functions/download.js) to a CSV.

Cloudflare Workers Analytics Engine keeps data points for about three months, so the
daily Repo stats workflow runs this and commits the result to the `stats` branch.

Usage: CF_ACCOUNT_ID=... CF_ANALYTICS_TOKEN=... python3 downloads.py OUT_DIR [DAYS]
The token needs the "Account Analytics: Read" permission.
"""
import csv
import json
import os
import sys
import urllib.request

FIELDS = ["date", "source", "medium", "campaign", "button", "page", "referrer", "country", "platform", "clicks"]
SQL = """
SELECT toDate(timestamp) AS date,
       blob1 AS source, blob2 AS medium, blob3 AS campaign, blob4 AS button,
       blob5 AS page, blob6 AS referrer, blob7 AS country, blob8 AS platform,
       SUM(_sample_interval) AS clicks
FROM milkrice_downloads
WHERE timestamp > NOW() - INTERVAL '{days}' DAY
GROUP BY date, source, medium, campaign, button, page, referrer, country, platform
FORMAT JSON
"""


def query(days):
    req = urllib.request.Request(
        f"https://api.cloudflare.com/client/v4/accounts/{os.environ['CF_ACCOUNT_ID']}/analytics_engine/sql",
        data=SQL.format(days=int(days)).encode(),
        headers={"Authorization": "Bearer " + os.environ["CF_ANALYTICS_TOKEN"], "User-Agent": "milkrice-stats"},
        method="POST",
    )
    with urllib.request.urlopen(req) as resp:
        return json.load(resp)["data"]


def main():
    out = sys.argv[1]
    days = int(sys.argv[2]) if len(sys.argv) > 2 else 14
    path = os.path.join(out, "site-downloads.csv")
    rows = query(days)
    fresh_dates = {r["date"] for r in rows}
    kept = []
    if os.path.exists(path):
        with open(path, newline="") as f:
            # Days inside the queried window are replaced by the fresh numbers.
            kept = [r for r in csv.DictReader(f) if r["date"] not in fresh_dates]
    merged = kept + [{k: str(r[k]) for k in FIELDS} for r in rows]
    merged.sort(key=lambda r: tuple(r[k] for k in FIELDS[:-1]))
    with open(path, "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=FIELDS)
        w.writeheader()
        w.writerows(merged)
    print(f"Saved {len(rows)} download-click rows")


if __name__ == "__main__":
    main()
