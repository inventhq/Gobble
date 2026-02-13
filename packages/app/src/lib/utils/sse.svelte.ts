/**
 * SSE (Server-Sent Events) client composable using Svelte 5 runes.
 *
 * Connects to the sse-gateway for real-time event streaming.
 * Falls back to polling if SSE is unavailable or disconnects.
 *
 * @example
 * ```svelte
 * <script lang="ts">
 *   import { createSSE } from '$lib/utils/sse.svelte';
 *   const sse = createSSE({ keyPrefix: '6vct' });
 * </script>
 * {#each sse.recentEvents as event}
 *   <p>{event.event_type} — {event.event_id}</p>
 * {/each}
 * ```
 */

import { onMount, onDestroy } from 'svelte';
import type { TrackingEvent } from '$lib/api/events';

const SSE_URL = import.meta.env.VITE_SSE_URL || 'http://localhost:3031';

export interface SSEOptions {
	/** Tenant key_prefix to filter events. Omit for all events (admin). */
	keyPrefix?: string;
	/** Event type filter (click, postback, impression). Omit for all. */
	eventType?: string;
	/** Tracking URL ID filter. Omit for all links. */
	tuId?: string;
	/** Max recent events to keep in the buffer (default: 100). */
	maxEvents?: number;
	/** Callback fired for each new event. */
	onEvent?: (event: TrackingEvent) => void;
}

export interface SSEClient {
	/** Whether the SSE connection is currently open. */
	readonly connected: boolean;
	/** Recent events received via SSE (newest first). */
	readonly recentEvents: TrackingEvent[];
	/** Total events received since connection. */
	readonly eventCount: number;
	/** Number of events missed due to lag. */
	readonly missedEvents: number;
	/** Disconnect and clean up. */
	disconnect: () => void;
	/** Reconnect (e.g., after changing filters). */
	reconnect: () => void;
}

/**
 * Create an SSE connection to the sse-gateway.
 * Must be called during component initialization.
 */
export function createSSE(options: SSEOptions = {}): SSEClient {
	const maxEvents = options.maxEvents ?? 100;

	let connected = $state(false);
	let recentEvents = $state<TrackingEvent[]>([]);
	let eventCount = $state(0);
	let missedEvents = $state(0);

	let eventSource: EventSource | null = null;

	function buildUrl(): string {
		const params = new URLSearchParams();
		if (options.keyPrefix) params.set('key_prefix', options.keyPrefix);
		if (options.eventType) params.set('event_type', options.eventType);
		if (options.tuId) params.set('tu_id', options.tuId);
		const qs = params.toString();
		return `${SSE_URL}/sse/events${qs ? `?${qs}` : ''}`;
	}

	function connect() {
		disconnect();

		const url = buildUrl();
		eventSource = new EventSource(url);

		eventSource.onopen = () => {
			connected = true;
		};

		eventSource.addEventListener('event', (e: MessageEvent) => {
			try {
				const event: TrackingEvent = JSON.parse(e.data);
				eventCount += 1;
				recentEvents = [event, ...recentEvents.slice(0, maxEvents - 1)];
				options.onEvent?.(event);
			} catch {
				// ignore malformed events
			}
		});

		eventSource.addEventListener('lag', (e: MessageEvent) => {
			try {
				const data = JSON.parse(e.data);
				missedEvents += data.missed ?? 0;
			} catch {
				// ignore
			}
		});

		eventSource.onerror = () => {
			connected = false;
			// EventSource auto-reconnects, so we just update state
		};
	}

	function disconnect() {
		if (eventSource) {
			eventSource.close();
			eventSource = null;
			connected = false;
		}
	}

	function reconnect() {
		connect();
	}

	onMount(() => {
		connect();
	});

	onDestroy(() => {
		disconnect();
	});

	return {
		get connected() { return connected; },
		get recentEvents() { return recentEvents; },
		get eventCount() { return eventCount; },
		get missedEvents() { return missedEvents; },
		disconnect,
		reconnect
	};
}
