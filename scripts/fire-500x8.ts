import { buildTrackedClickUrl } from '../packages/sdk-typescript/src/links.js';

const BASE_URL = 'http://localhost:3030';
const SECRET = 'f21ec8cd1a2a34341cf5736b86a64e90386990d0c1e330e0684a9c5c4d8617fa';
const KEY_PREFIX = '6vct';
const COUNT = 5000;

const LINKS = [
  'tu_019c4493-4800-77f7-b5f6-39a079e136bf',
  'tu_019c4343-1eb1-7b09-b126-9c7c21215de0',
  'tu_019c4178-c243-7a41-b411-0dc5e1708aeb',
  'tu_019c4178-9bd4-752f-8285-8e4860b1ad41',
  'tu_019c4120-b32a-77b3-a813-71287e4ba0c8',
  'tu_019c4048-b121-7dc7-840d-3180ff425aab',
  'tu_019c3f8d-aa19-7261-a5d6-9aa75cea309d',
  'tu_019c3c79-c5f7-7ed5-970f-409d3b42155b',
];

const sources = ['google_search', 'facebook_ads', 'tiktok', 'email_blast', 'organic'];
const geos = ['US', 'UK', 'DE', 'FR', 'JP', 'BR', 'AU'];
const pick = (arr: string[]) => arr[Math.floor(Math.random() * arr.length)];

async function fireLink(tu_id: string, idx: number) {
  let ok = 0, fail = 0;
  for (let i = 0; i < COUNT; i++) {
    const url = buildTrackedClickUrl(BASE_URL, SECRET, tu_id, {
      key_prefix: KEY_PREFIX,
      click_id: `clk_${Date.now()}_${idx}_${i}`,
      sub1: pick(sources),
      geo: pick(geos),
    });
    try {
      const r = await fetch(url, { redirect: 'manual' });
      if (r.status === 307) ok++; else fail++;
    } catch { fail++; }
  }
  return { tu_id, ok, fail };
}

async function main() {
  const total = COUNT * LINKS.length;
  console.log(`Firing ${COUNT} clicks × ${LINKS.length} links = ${total} total clicks...`);
  const start = Date.now();

  const results = await Promise.all(LINKS.map((tu, i) => fireLink(tu, i)));

  const elapsed = ((Date.now() - start) / 1000).toFixed(1);
  let totalOk = 0, totalFail = 0;
  for (const r of results) {
    totalOk += r.ok;
    totalFail += r.fail;
    console.log(`  ${r.tu_id}: ${r.ok}/${COUNT} OK${r.fail > 0 ? `, ${r.fail} fail` : ''}`);
  }
  console.log(`\nDone: ${totalOk}/${total} OK, ${totalFail} failed — ${elapsed}s`);
}

main();
