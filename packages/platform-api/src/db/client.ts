/**
 * Turso/libSQL database client factory.
 *
 * Creates a libSQL client from Cloudflare Worker environment bindings.
 * The client uses Turso's HTTP driver, compatible with CF Workers.
 */

import { createClient, type Client } from "@libsql/client/web";
import { type Env } from "../types.js";

/** Create a libSQL client from Worker env bindings. */
export function createDb(env: Env): Client {
  return createClient({
    url: env.TURSO_URL,
    authToken: env.TURSO_AUTH_TOKEN,
  });
}
