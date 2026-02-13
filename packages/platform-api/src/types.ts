/**
 * Shared Hono type definitions for the Platform API.
 *
 * Defines the environment bindings and context variables used across
 * all middleware and route handlers.
 */

import { type Client } from "@libsql/client/web";

/** Cloudflare Worker environment bindings. */
export type Env = {
  TURSO_URL: string;
  TURSO_AUTH_TOKEN: string;
  ADMIN_BOOTSTRAP_KEY: string;
  ENVIRONMENT: string;
  PERMIT_API_KEY?: string;
  RISINGWAVE_URL?: string;
  POLARS_QUERY_URL?: string;
  POLARS_LITE_URL?: string;
  VIVGRID_API_KEY?: string;
};

/** Variables set by middleware and available in route handlers. */
export type Variables = {
  db: Client;
  tenantId: string;
  isAdmin: boolean;
};

/** Combined Hono app type with bindings and variables. */
export type AppType = {
  Bindings: Env;
  Variables: Variables;
};
