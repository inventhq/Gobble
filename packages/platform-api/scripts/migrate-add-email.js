/**
 * Migration: Add email column to tenants table.
 *
 * This is a one-time migration for existing databases.
 * New databases get the column via schema.sql.
 *
 * Usage: TURSO_URL=... TURSO_AUTH_TOKEN=... node scripts/migrate-add-email.js
 */

import { createClient } from "@libsql/client";

const url = process.env.TURSO_URL;
const authToken = process.env.TURSO_AUTH_TOKEN || "";

if (!url) {
  console.error("TURSO_URL environment variable is required");
  process.exit(1);
}

const client = createClient({ url, authToken });

console.log("Adding email column to tenants table...");

try {
  await client.execute("ALTER TABLE tenants ADD COLUMN email TEXT");
  console.log("  ✓ Column added successfully.");
} catch (err) {
  if (err.message?.includes("duplicate column")) {
    console.log("  ✓ Column already exists, skipping.");
  } else {
    console.error("  ✗ Migration failed:", err.message);
    process.exit(1);
  }
}

console.log("Done.");
client.close();
