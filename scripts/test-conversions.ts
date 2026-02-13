/**
 * Test event matching by sending trigger events (clicks with click_id),
 * then result events (postbacks referencing the same click_id).
 * Some triggers intentionally have no matching result (to test unmatched).
 *
 * Uses GET /api/events/match?trigger=click&result=postback&on=click_id
 */
import { buildTrackedClickUrl, buildPostbackUrl } from '../packages/sdk-typescript/src/links.js';

const BASE_URL = 'http://localhost:3030';
// Tenant-specific HMAC secret (not the global one)
const SECRET = 'f21ec8cd1a2a34341cf5736b86a64e90386990d0c1e330e0684a9c5c4d8617fa';
const TU_ID = 'tu_019c3f8d-aa19-7261-a5d6-9aa75cea309d';
const KEY_PREFIX = '6vct';

const TRIGGER_COUNT = 20;
const RESULT_COUNT = 12; // 12 of 20 triggers will get a result (60% match rate)

const sources = ['google_search', 'facebook_ads', 'tiktok', 'organic'];
const geos = ['US', 'UK', 'DE', 'JP'];

function pick<T>(arr: T[]): T {
  return arr[Math.floor(Math.random() * arr.length)];
}

let triggerErrors = 0;
let resultErrors = 0;

async function main() {
  const clickIds: string[] = [];

  console.log(`Sending ${TRIGGER_COUNT} trigger events (clicks with click_id)...`);
  for (let i = 0; i < TRIGGER_COUNT; i++) {
    const clickId = `clk_${Date.now()}_${i}`;
    clickIds.push(clickId);

    const url = buildTrackedClickUrl(BASE_URL, SECRET, TU_ID, {
      key_prefix: KEY_PREFIX,
      click_id: clickId,
      sub1: pick(sources),
      geo: pick(geos),
    });

    try {
      const resp = await fetch(url, { redirect: 'manual' });
      if (resp.status !== 307) {
        triggerErrors++;
        if (triggerErrors <= 3) console.error(`  ERROR: trigger ${i} returned ${resp.status}`);
      }
    } catch (e: any) {
      triggerErrors++;
    }
  }
  console.log(`  ${TRIGGER_COUNT - triggerErrors}/${TRIGGER_COUNT} triggers OK`);

  // Small delay to ensure triggers are ingested
  await new Promise(r => setTimeout(r, 500));

  console.log(`\nSending ${RESULT_COUNT} result events (postbacks matching first ${RESULT_COUNT} click_ids)...`);
  for (let i = 0; i < RESULT_COUNT; i++) {
    const payout = (Math.random() * 50 + 5).toFixed(2);
    const resultType = pick(['sale', 'lead', 'install']);

    const url = buildPostbackUrl(BASE_URL, {
      key_prefix: KEY_PREFIX,
      tu_id: TU_ID,
      click_id: clickIds[i],
      payout,
      conversion_type: resultType,
      sub1: pick(sources),
      geo: pick(geos),
    });

    try {
      const resp = await fetch(url);
      if (resp.status !== 200) {
        resultErrors++;
        if (resultErrors <= 3) console.error(`  ERROR: result ${i} returned ${resp.status}`);
      }
    } catch (e: any) {
      resultErrors++;
    }
  }
  console.log(`  ${RESULT_COUNT - resultErrors}/${RESULT_COUNT} results OK`);

  console.log(`\nResults: ${TRIGGER_COUNT - triggerErrors}/${TRIGGER_COUNT} triggers, ${RESULT_COUNT - resultErrors}/${RESULT_COUNT} results`);
  console.log(`Expected: ${RESULT_COUNT} matched, ${TRIGGER_COUNT - RESULT_COUNT} unmatched, ${(RESULT_COUNT / TRIGGER_COUNT * 100).toFixed(0)}% match rate`);

  // Wait for RisingWave ingestion
  console.log('\nWaiting 5s for RisingWave ingestion...');
  await new Promise(r => setTimeout(r, 5000));

  // Test the match API
  console.log('\nQuerying /api/events/match...');
  try {
    const resp = await fetch('http://localhost:8787/api/events/match?trigger=click&result=postback&on=click_id&hours=1&limit=50', {
      headers: { Authorization: 'Bearer tk_admin_42eaed9633daeec9772463c8b769d0d2b0518131708a1011' },
    });
    const data = await resp.json();
    console.log(`  Status: ${resp.status}`);
    console.log(`  Summary:`, JSON.stringify(data.summary, null, 2));
    if (data.pairs?.length > 0) {
      const first = data.pairs.find((p: any) => p.matched) || data.pairs[0];
      console.log(`  First pair: on=${first.on}, on_value=${first.on_value}, matched=${first.matched}, time_delta=${first.time_delta_ms}ms`);
    }
  } catch (e: any) {
    console.error(`  API error: ${e.message}`);
  }
}

main();
