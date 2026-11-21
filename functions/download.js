// GET /download: count a download click, then send the visitor to the latest release.
//
// Every Download button on the site points here. We record one anonymous data point
// in Cloudflare Workers Analytics Engine (binding DOWNLOADS, dataset milkrice_downloads)
// so we can see which pages and which outside links lead to downloads. No cookie is
// set and nothing that identifies a person is kept: no IP address, no user agent, no
// full referring URL from other sites (only its domain). See site/privacy.html.
//
// Query params (all optional): ref or utm_source, utm_medium, utm_campaign, and from
// (which button on our own site was clicked, e.g. "nav", "hero", "faq", "footer").

const TARGET = 'https://github.com/thekindofme/milkrice/releases/latest';
const BOTS = /bot|crawl|spider|slurp|preview|fetch|curl|wget|python|headless/i;

const clip = (s, n = 96) => (s || '').slice(0, n);

function platform(ua) {
  if (/iPhone|iPad|iPod/.test(ua)) return 'ios';
  if (/Macintosh|Mac OS X/.test(ua)) return 'mac';
  if (/Android/.test(ua)) return 'android';
  if (/Windows/.test(ua)) return 'windows';
  if (/Linux|CrOS/.test(ua)) return 'linux';
  return 'other';
}

export async function onRequestGet({ request, env }) {
  const url = new URL(request.url);
  const q = url.searchParams;
  const ua = request.headers.get('User-Agent') || '';

  let refHost = '';
  let refPath = '';
  try {
    const r = new URL(request.headers.get('Referer') || '');
    refHost = r.hostname;
    refPath = r.pathname;
  } catch {}
  const internal = refHost === url.hostname;
  const source = q.get('ref') || q.get('utm_source') || (internal ? 'site' : refHost || 'direct');

  if (env.DOWNLOADS && !BOTS.test(ua)) {
    try {
      env.DOWNLOADS.writeDataPoint({
        indexes: [clip(source)],
        blobs: [
          clip(source),                 // blob1 source: ref/utm_source, else referrer domain, "site" or "direct"
          clip(q.get('utm_medium')),    // blob2
          clip(q.get('utm_campaign')),  // blob3
          clip(q.get('from')),          // blob4 which button on our site
          internal ? clip(refPath) : '',// blob5 which page of our site
          internal ? '' : clip(refHost),// blob6 referring domain for outside links
          clip(request.cf?.country, 2), // blob7 country code
          platform(ua),                 // blob8 coarse platform; the user agent itself is not stored
        ],
        doubles: [1],
      });
    } catch {}
  }

  return new Response(null, {
    status: 302,
    headers: { Location: TARGET, 'Cache-Control': 'no-store', 'Referrer-Policy': 'no-referrer' },
  });
}
