# Analytics

Milkrice's apps collect nothing: no telemetry, no analytics, no crash reporting. Everything
below is measured from the outside, in aggregate, without cookies. If you change any of
it, update the "This website" section of `site/privacy.html` in the same commit.

## What is measured where

| Question | Where to look |
| --- | --- |
| Website visits, top pages, referring sites, countries | Cloudflare dashboard > Web analytics > yasithfernando.com, filtered to host `milkrice.yasithfernando.com` (the beacon is injected automatically for the zone) |
| Which pages and outside links lead to a Download click | `functions/download.js` writes to the Analytics Engine dataset `milkrice_downloads`; archived daily to `site-downloads.csv` on the `stats` branch |
| GitHub repo views, clones, referrers, popular files, stars | `traffic-*.csv`, `referrers.csv`, `paths.csv`, `repo.csv` on the `stats` branch |
| DMG downloads per release (Homebrew installs included) | `release-downloads.csv`, kind `dmg` |
| Roughly how many Macs still run Milkrice | `release-downloads.csv`, kind `update-check` (downloads of `latest.json` by the in-app update check) |
| iPhone App Store impressions, page views, downloads, sources, retention | App Store Connect > Analytics (Apple only reports usage from people who opted in) |

The `stats` branch is written by `.github/workflows/repo-stats.yml` every day at 03:17 UTC.
GitHub keeps traffic for only 14 days and Analytics Engine for about 3 months, so the
branch is the long-term record. Secrets it uses: `STATS_TOKEN` (fine-grained GitHub token,
Administration and Contents read on this repo), `CF_ACCOUNT_ID` and `CF_ANALYTICS_TOKEN`
(Cloudflare API token with Account Analytics: Read).

Weekly summary:

```
git fetch origin stats && git worktree add /tmp/milkrice-stats origin/stats
python3 scripts/stats/report.py /tmp/milkrice-stats        # last 7 days; pass 30 for a month
```

Every run also prints the summary on its page in the Actions tab.

## Tagging links

Point outside links at the site with a `ref` tag, so visits and downloads are credited to
the right place. Use short lowercase names.

| Where the link is | Use |
| --- | --- |
| Blog posts | `https://milkrice.yasithfernando.com/?ref=blog` |
| Show HN, Reddit, X, Mastodon | `?ref=hn`, `?ref=reddit`, `?ref=x`, `?ref=mastodon` |
| Newsletters, podcasts | `?ref=<name>` |
| A specific launch push | add `&utm_campaign=launch-2026-12` |
| Straight to the download | `https://milkrice.yasithfernando.com/download?ref=blog` |

Links inside the site already go through `/download?from=<button>` (`nav`, `hero`, `faq`,
`cta`), and `site/js/site.js` carries a landing page's `ref`/`utm_*` tags on to those links.

For the iPhone app, once the App Store listing exists, use Apple campaign links
(`https://apps.apple.com/app/id<APP_ID>?pt=<PROVIDER_ID>&ct=<campaign>`); the `ct` value
shows up under Sources in App Store Connect.
