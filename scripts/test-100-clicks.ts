import { buildTrackedClickUrl } from '../packages/sdk-typescript/src/links.js';

const BASE_URL = 'http://localhost:3030';
const SECRET = 'f21ec8cd1a2a34341cf5736b86a64e90386990d0c1e330e0684a9c5c4d8617fa';
const TU_ID = 'tu_019c3c79-c5f7-7ed5-970f-409d3b42155b';
const KEY_PREFIX = '6vct';

const sources = ['google_search', 'facebook_ads', 'tiktok', 'email_blast', 'organic'];
const geos = ['US', 'UK', 'DE', 'FR', 'JP'];
const pick = (arr: string[]) => arr[Math.floor(Math.random() * arr.length)];

async function main() {
  let ok = 0, fail = 0;
  console.log('Sending 100 clicks to ' + TU_ID + '...');
  for (let i = 0; i < 100; i++) {
    const url = buildTrackedClickUrl(BASE_URL, SECRET, TU_ID, {
      key_prefix: KEY_PREFIX,
      click_id: 'clk_' + Date.now() + '_' + i,
      sub1: pick(sources),
      geo: pick(geos),
    });
    try {
      const r = await fetch(url, { redirect: 'manual' });
      if (r.status === 307) ok++; else fail++;
    } catch { fail++; }
    if ((i + 1) % 25 === 0) console.log('  ' + (i + 1) + '/100 (' + ok + ' ok, ' + fail + ' fail)');
  }
  console.log('Done: ' + ok + '/100 OK, ' + fail + ' failed');
}

main();
