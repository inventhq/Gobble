/**
 * RisingWave client for event/stats queries.
 *
 * Uses the `pg` (node-postgres) library to connect to RisingWave Cloud
 * via the Postgres wire protocol. Cloudflare Workers support TCP sockets
 * natively, so `pg` works out of the box.
 *
 * This module handles connection pooling and provides typed query helpers
 * for the events routes.
 */

import { Client } from "pg";

let cachedClient: Client | null = null;

/**
 * Get a connected RisingWave client.
 *
 * Reuses the same connection across requests within a Worker invocation.
 * In production, consider using Hyperdrive for connection pooling.
 */
export async function getRisingWaveClient(connectionString: string): Promise<Client> {
  if (cachedClient) {
    return cachedClient;
  }

  const client = new Client({ connectionString });
  await client.connect();
  cachedClient = client;
  return client;
}
