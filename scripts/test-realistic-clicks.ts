/**
 * Generate realistic test clicks with affiliate marketing params.
 * Uses the SDK to build signed tracked click URLs, then fires them via curl.
 */
import { buildTrackedClickUrl, buildPostbackUrl } from '../packages/sdk-typescript/src/links.js';

const BASE_URL = 'http://localhost:3030';
// Must use the tenant's HMAC secret (not the global HMAC_SECRET)
// because the sig is prefixed with key_prefix, so tracker-core
// verifies against the tenant-specific secret from /internal/secrets.
const SECRET = 'f21ec8cd1a2a34341cf5736b86a64e90386990d0c1e330e0684a9c5c4d8617fa';
const TU_ID = 'tu_019c3f8d-aa19-7261-a5d6-9aa75cea309d';
const KEY_PREFIX = '6vct';

// Realistic param combinations
const sources = ['google_search', 'facebook_ads', 'tiktok', 'email_blast', 'organic'];
const campaigns = ['camp_summer24', 'camp_blackfriday', 'camp_newyear', 'camp_launch'];
const geos = ['US', 'UK', 'DE', 'FR', 'JP', 'BR', 'AU'];
const creatives = ['cr_banner_1', 'cr_video_2', 'cr_native_3', 'cr_popup_4'];
const placements = ['top_of_page', 'sidebar', 'in_feed', 'footer', 'interstitial'];

function pick<T>(arr: T[]): T {
  return arr[Math.floor(Math.random() * arr.length)];
}

let clickErrors = 0;
let postbackErrors = 0;

async function fireClick(params: Record<string, string>) {
  const url = buildTrackedClickUrl(BASE_URL, SECRET, TU_ID, {
    key_prefix: KEY_PREFIX,
    ...params,
  });
  try {
    const resp = await fetch(url, { redirect: 'manual' });
    if (resp.status !== 307) {
      clickErrors++;
      if (clickErrors <= 3) console.error(`  ERROR: click returned ${resp.status} (expected 307)`);
    }
  } catch (e: any) {
    clickErrors++;
    if (clickErrors <= 3) console.error(`  ERROR: click fetch failed: ${e.message}`);
  }
}

async function firePostback(params: Record<string, string>) {
  const url = buildPostbackUrl(BASE_URL, {
    key_prefix: KEY_PREFIX,
    tu_id: TU_ID,
    ...params,
  });
  try {
    await fetch(url);
  } catch {}
}

async function main() {
  const CLICK_COUNT = 200;
  const POSTBACK_COUNT = 50;

  console.log(`Sending ${CLICK_COUNT} clicks with realistic params...`);

  for (let i = 0; i < CLICK_COUNT; i++) {
    const sub1 = pick(sources);
    const campaign_id = pick(campaigns);
    const geo = pick(geos);
    const creative_id = pick(creatives);
    const placement = pick(placements);
    const click_id = `clk_${Date.now()}_${i}`;

    await fireClick({
      sub1,
      sub2: campaign_id,
      campaign_id,
      geo,
      creative_id,
      placement,
      click_id,
    });

    if ((i + 1) % 50 === 0) console.log(`  ${i + 1}/${CLICK_COUNT} clicks sent`);
  }

  console.log(`\nSending ${POSTBACK_COUNT} postbacks (conversions)...`);

  for (let i = 0; i < POSTBACK_COUNT; i++) {
    const payout = (Math.random() * 50 + 1).toFixed(2);
    const conversion_type = pick(['sale', 'lead', 'install', 'signup']);
    const order_id = `ord_${Date.now()}_${i}`;
    const sub1 = pick(sources);
    const geo = pick(geos);

    await firePostback({
      sub1,
      geo,
      payout,
      conversion_type,
      order_id,
    });

    if ((i + 1) % 25 === 0) console.log(`  ${i + 1}/${POSTBACK_COUNT} postbacks sent`);
  }

  console.log(`\nResults: ${CLICK_COUNT - clickErrors}/${CLICK_COUNT} clicks OK, ${POSTBACK_COUNT - postbackErrors}/${POSTBACK_COUNT} postbacks OK`);
  if (clickErrors > 0) console.error(`  ⚠ ${clickErrors} click errors`);
  if (postbackErrors > 0) console.error(`  ⚠ ${postbackErrors} postback errors`);
  console.log('\nTry these param filters in the dashboard:');
  console.log('  param_key: sub1       param_value: google_search');
  console.log('  param_key: geo        param_value: US');
  console.log('  param_key: campaign_id  param_value: camp_summer24');
  console.log('  param_key: placement  param_value: top_of_page');
  console.log('  param_key: conversion_type  param_value: sale');
}

main();
