/**
 * Reusable polling composable using Svelte 5 runes.
 *
 * Auto-fetches data at a configurable interval, pauses when the tab
 * is hidden, and exposes reactive state for loading, errors, and
 * last-updated time.
 *
 * @example
 * ```svelte
 * <script lang="ts">
 *   import { createPoller } from '$lib/utils/polling.svelte';
 *   const poller = createPoller(() => getStats({ hours: 24 }), { intervalMs: 10_000 });
 * </script>
 * {#if poller.data}
 *   <pre>{JSON.stringify(poller.data)}</pre>
 * {/if}
 * ```
 */

import { onMount, onDestroy } from 'svelte';

export interface PollerOptions {
	/** Polling interval in milliseconds (default: 10000). */
	intervalMs?: number;
	/** Whether polling starts enabled (default: true). */
	enabled?: boolean;
}

export interface Poller<T> {
	readonly data: T | null;
	readonly loading: boolean;
	readonly error: string;
	readonly lastUpdated: number | null;
	refresh: () => Promise<void>;
	pause: () => void;
	resume: () => void;
}

/**
 * Create a polling instance that calls `fetchFn` on mount and every
 * `intervalMs` thereafter. Automatically pauses when the browser tab
 * is hidden and resumes when it becomes visible again.
 *
 * Must be called during component initialization (like onMount).
 */
export function createPoller<T>(
	fetchFn: () => Promise<T>,
	options: PollerOptions = {}
): Poller<T> {
	const intervalMs = options.intervalMs ?? 10_000;

	let data = $state<T | null>(null);
	let loading = $state(true);
	let error = $state('');
	let lastUpdated = $state<number | null>(null);

	let enabled = $state(options.enabled ?? true);
	let timer: ReturnType<typeof setInterval> | null = null;

	async function refresh() {
		loading = true;
		error = '';
		try {
			data = await fetchFn();
			lastUpdated = Date.now();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to fetch';
		} finally {
			loading = false;
		}
	}

	function startTimer() {
		stopTimer();
		if (enabled && intervalMs > 0) {
			timer = setInterval(() => {
				refresh();
			}, intervalMs);
		}
	}

	function stopTimer() {
		if (timer) {
			clearInterval(timer);
			timer = null;
		}
	}

	function pause() {
		enabled = false;
		stopTimer();
	}

	function resume() {
		enabled = true;
		refresh();
		startTimer();
	}

	function handleVisibility() {
		if (document.visibilityState === 'hidden') {
			stopTimer();
		} else if (enabled) {
			refresh();
			startTimer();
		}
	}

	onMount(() => {
		if (enabled) {
			refresh();
			startTimer();
		}
		document.addEventListener('visibilitychange', handleVisibility);
	});

	onDestroy(() => {
		stopTimer();
		document.removeEventListener('visibilitychange', handleVisibility);
	});

	return {
		get data() { return data; },
		get loading() { return loading; },
		get error() { return error; },
		get lastUpdated() { return lastUpdated; },
		refresh,
		pause,
		resume
	};
}
