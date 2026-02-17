/**
 * t.js — Cookieless auto-tracking beacon for tracker-core.
 *
 * Drop-in script tag that automatically tracks pageviews and outbound clicks
 * via navigator.sendBeacon(). Zero cookies, zero localStorage, zero fingerprinting.
 *
 * Usage:
 *   <script src="https://js.juicyapi.com/t.js" data-key="6vct" async defer></script>
 *
 * Optional attributes:
 *   data-key       (required) Tenant key_prefix
 *   data-host      (optional) Tracker URL, defaults to https://track.juicyapi.com
 *   data-track     (optional) "outbound" (default) | "all" | "none"
 *   data-no-spa    (optional) Disable SPA history tracking if present
 *
 * Events sent:
 *   - pageview        on page load (and SPA navigations)
 *   - outbound_click  on clicks to external domains
 *
 * Session ID: crypto.getRandomValues() — lives in JS memory only, dies on navigation.
 * Ad click stitching: reads ?ad_click_id= from the URL if present.
 *
 * ~900 bytes minified + gzipped.
 */
;(function() {
  'use strict';

  var d = document, w = window, n = navigator;
  var s = d.currentScript;
  if (!s) return;

  var key = s.getAttribute('data-key');
  if (!key) return;

  var host = s.getAttribute('data-host') || 'https://track.juicyapi.com';
  var mode = s.getAttribute('data-track') || 'outbound';
  var noSpa = s.hasAttribute('data-no-spa');
  var endpoint = host + '/t/auto';
  var origin = location.hostname;

  // --- Session ID: random 128-bit hex, memory-only, no persistence ---
  var sid;
  try {
    var buf = new Uint8Array(16);
    crypto.getRandomValues(buf);
    sid = '';
    for (var i = 0; i < 16; i++) sid += (buf[i] < 16 ? '0' : '') + buf[i].toString(16);
  } catch(e) {
    sid = Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);
  }

  // --- Ad click ID: read from URL query string ---
  var acid = null;
  try {
    var sp = new URLSearchParams(location.search);
    acid = sp.get('ad_click_id');
  } catch(e) {}

  // --- Screen width for device bucketing ---
  var sw = w.innerWidth || d.documentElement.clientWidth || 0;

  // --- Beacon sender ---
  function send(eventType, extra) {
    var payload = {
      event_type: eventType,
      key_prefix: key,
      page: location.pathname + location.search,
      session_id: sid,
      screen_width: sw
    };
    if (acid) payload.ad_click_id = acid;
    if (extra) {
      for (var k in extra) {
        if (extra.hasOwnProperty(k)) payload[k] = extra[k];
      }
    }
    // sendBeacon is fire-and-forget, survives page unload
    if (n.sendBeacon) {
      n.sendBeacon(endpoint, JSON.stringify(payload));
    } else {
      // Fallback: XHR with keepalive (IE11 won't reach here but just in case)
      try {
        var x = new XMLHttpRequest();
        x.open('POST', endpoint, true);
        x.setRequestHeader('Content-Type', 'text/plain');
        x.send(JSON.stringify(payload));
      } catch(e) {}
    }
  }

  // --- Pageview ---
  function pageview() {
    send('pageview');
  }

  // --- Outbound click tracking ---
  function setupClicks() {
    if (mode === 'none') return;
    d.addEventListener('click', function(e) {
      var el = e.target;
      // Walk up to find the nearest <a>
      while (el && el.tagName !== 'A') el = el.parentElement;
      if (!el || !el.href) return;

      try {
        var u = new URL(el.href, location.href);
        // Skip same-origin, javascript:, mailto:, tel:
        if (u.protocol !== 'http:' && u.protocol !== 'https:') return;
        if (mode === 'outbound' && u.hostname === origin) return;

        send('outbound_click', {
          href: el.href,
          text: (el.textContent || '').trim().slice(0, 150)
        });
      } catch(e) {}
    }, true); // capture phase — fires before default navigation
  }

  // --- SPA support: detect pushState/replaceState navigations ---
  function setupSpa() {
    if (noSpa) return;
    var origPush = history.pushState;
    var origReplace = history.replaceState;
    if (origPush) {
      history.pushState = function() {
        origPush.apply(this, arguments);
        pageview();
      };
    }
    if (origReplace) {
      history.replaceState = function() {
        origReplace.apply(this, arguments);
        pageview();
      };
    }
    w.addEventListener('popstate', pageview);
  }

  // --- Boot ---
  pageview();
  setupClicks();
  setupSpa();
})();
