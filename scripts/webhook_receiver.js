/**
 * Simple webhook receiver for testing.
 * Logs all incoming POST requests with headers and body.
 * Usage: node scripts/webhook_receiver.js
 */
const http = require("http");

const PORT = 9999;
let received = [];

const server = http.createServer((req, res) => {
  if (req.method === "POST") {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => {
      const entry = {
        timestamp: new Date().toISOString(),
        headers: {
          "x-webhook-signature": req.headers["x-webhook-signature"],
          "x-webhook-id": req.headers["x-webhook-id"],
          "x-event-id": req.headers["x-event-id"],
          "x-event-type": req.headers["x-event-type"],
        },
        body: JSON.parse(body),
      };
      received.push(entry);
      console.log(`\n✅ Webhook received #${received.length}:`);
      console.log(`   Event: ${entry.headers["x-event-type"]} (${entry.headers["x-event-id"]})`);
      console.log(`   Signature: ${entry.headers["x-webhook-signature"]?.slice(0, 20)}...`);
      console.log(`   Body: ${JSON.stringify(entry.body).slice(0, 120)}...`);
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ ok: true }));
    });
  } else if (req.method === "GET" && req.url === "/received") {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ count: received.length, webhooks: received }));
  } else {
    res.writeHead(200);
    res.end("Webhook receiver running");
  }
});

server.listen(PORT, () => {
  console.log(`🎯 Webhook receiver listening on http://localhost:${PORT}`);
  console.log(`   POST / — receive webhooks`);
  console.log(`   GET /received — list all received webhooks`);
});
