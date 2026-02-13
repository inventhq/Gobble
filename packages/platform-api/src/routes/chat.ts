/**
 * AI Chat endpoint — thin re-export from @tracker/vivgrid-tools.
 *
 * All tool definitions, LLM loop logic, and system prompt live in
 * packages/vivgrid-tools/. This file just re-exports for mounting.
 */

export { chatRoute as default, setAppRef } from "@tracker/vivgrid-tools";
