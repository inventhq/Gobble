/**
 * Database migration script for the Platform API.
 *
 * Reads schema.sql and executes it against the Turso database.
 * Usage: node scripts/migrate.js
 *
 * Requires TURSO_URL and TURSO_AUTH_TOKEN environment variables.
 */

import { createClient } from "@libsql/client";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));

const url = process.env.TURSO_URL;
const authToken = process.env.TURSO_AUTH_TOKEN || "";

if (!url) {
  console.error("TURSO_URL environment variable is required");
  process.exit(1);
}

const client = createClient({ url, authToken });

const schemaPath = join(__dirname, "..", "src", "db", "schema.sql");
const schema = readFileSync(schemaPath, "utf-8");

// Split on semicolons that are followed by a newline (not inside CREATE TABLE)
// by using executeMultiple which handles multi-statement SQL natively
console.log("Running migration...");

try {
  await client.executeMultiple(schema);
  console.log("  ✓ Migration complete.");
} catch (err) {
  console.error("  ✗ Migration failed:", err.message);
  // Fall back to statement-by-statement execution
  console.log("  Retrying statement by statement...");
  const statements = schema
    .split(/;\s*\n/)
    .map((s) => s.trim().replace(/;$/, ""))
    .filter((s) => s.length > 0 && !s.startsWith("--"));

  for (const stmt of statements) {
    try {
      await client.execute(stmt);
      console.log("    ✓", stmt.slice(0, 60).replace(/\n/g, " ") + "...");
    } catch (e) {
      console.error("    ✗ Failed:", stmt.slice(0, 80).replace(/\n/g, " "));
      console.error("     ", e.message);
    }
  }
}

console.log("Done.");
client.close();
