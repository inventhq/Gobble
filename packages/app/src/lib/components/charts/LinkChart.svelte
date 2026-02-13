<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import uPlot from 'uplot';
	import { getStats, getEvents, type StatsResponse, type TrackingEvent } from '$lib/api/events';
	import { createPoller } from '$lib/utils/polling.svelte';
	import { truncate, formatTimestamp } from '$lib/utils/format';
	import { RefreshCw, ChevronDown, ChevronUp, Radio, Copy, Check, Filter, X } from 'lucide-svelte';

	interface Props {
		tuId: string;
		destination?: string;
		hours?: number;
		liveEvents?: TrackingEvent[];
		sseConnected?: boolean;
	}

	let { tuId, destination = '', hours = 24, liveEvents = [], sseConnected = false }: Props = $props();

	// --- Stats poller (chart data) ---
	const poller = createPoller<StatsResponse>(
		() => getStats({ hours, tu_id: tuId }),
		{ intervalMs: 10_000 }
	);

	// --- Events table (manual fetch, no poller) ---
	let showTable = $state(false);
	let tableEvents = $state<TrackingEvent[]>([]);
	let tableLoading = $state(false);
	let tableError = $state('');
	let tableFetched = $state(false);
	let selectedEvent = $state<TrackingEvent | null>(null);
	let copiedParams = $state<Record<string, boolean>>({});
	async function copyText(text: string, key: string) {
		try {
			await navigator.clipboard.writeText(text);
			copiedParams[key] = true;
			setTimeout(() => { copiedParams[key] = false; }, 2000);
		} catch {}
	}

	// --- Param filter ---
	let showFilter = $state(false);
	let filterParamKey = $state('');
	let filterParamValue = $state('');
	let activeParamKey = $state('');
	let activeParamValue = $state('');

	// Set of event IDs known at the time of the last fetch — anything not in this set is "new"
	let snapshotIds = $state<Set<string>>(new Set());

	async function fetchEvents() {
		tableLoading = true;
		tableError = '';
		try {
			const res = await getEvents({
				limit: 20,
				tu_id: tuId,
				param_key: activeParamKey || undefined,
				param_value: activeParamValue || undefined,
			});
			tableEvents = res.events;
			// Snapshot: all IDs from the API response + all live events known right now
			const ids = new Set(res.events.map(e => e.event_id));
			for (const e of liveEvents) ids.add(e.event_id);
			snapshotIds = ids;
			tableFetched = true;
		} catch (e) {
			tableError = e instanceof Error ? e.message : 'Failed to fetch';
		} finally {
			tableLoading = false;
		}
	}

	// Derived: SSE events that arrived AFTER the fetch, prepended to the stable fetched rows.
	// Before fetch: show any live events directly (so the table isn't empty while SSE is active).
	// After fetch: show new SSE events (not in snapshot) prepended to API results.
	let displayEvents = $derived.by(() => {
		if (!tableFetched) {
			// No API fetch yet — show live events if any, otherwise empty
			return liveEvents.length > 0 ? liveEvents : tableEvents;
		}
		// Only show live events whose IDs were NOT in the snapshot (truly new)
		const existingIds = new Set(tableEvents.map(e => e.event_id));
		const newEvents = liveEvents.filter(e => !snapshotIds.has(e.event_id) && !existingIds.has(e.event_id));
		if (newEvents.length === 0) return tableEvents;
		return [...newEvents, ...tableEvents];
	});

	function applyFilter() {
		activeParamKey = filterParamKey.trim();
		activeParamValue = filterParamValue.trim();
		fetchEvents();
	}

	function clearFilter() {
		filterParamKey = '';
		filterParamValue = '';
		activeParamKey = '';
		activeParamValue = '';
		fetchEvents();
	}

	function toggleTable() {
		showTable = !showTable;
		if (showTable && !tableFetched) {
			fetchEvents();
		}
	}

	// --- Live counters (from SSE) ---
	// Track IDs we've already counted to avoid double-counting on array churn
	let liveCounts = $state<Record<string, number>>({ click: 0, postback: 0, impression: 0 });
	let countedIds = new Set<string>();
	let refreshTimer: ReturnType<typeof setTimeout> | null = null;

	$effect(() => {
		const events = liveEvents;
		let hasNew = false;
		for (const e of events) {
			if (countedIds.has(e.event_id)) continue;
			countedIds.add(e.event_id);
			const t = e.event_type;
			if (t in liveCounts) liveCounts[t] += 1;
			hasNew = true;
		}

		if (hasNew) {
			// Debounced chart refresh — re-fetch stats from API every 5s
			if (refreshTimer) clearTimeout(refreshTimer);
			refreshTimer = setTimeout(() => {
				poller.refresh();
			}, 5_000);
		}
	});

	let chartEl: HTMLDivElement | undefined = $state();
	let chart: uPlot | null = null;

	function buildData(stats: StatsResponse): uPlot.AlignedData {
		// Build a map: hour -> { click, postback, impression }
		const buckets = new Map<number, { click: number; postback: number; impression: number }>();

		for (const row of stats.hourly) {
			const h = row.hour;
			if (!buckets.has(h)) buckets.set(h, { click: 0, postback: 0, impression: 0 });
			const b = buckets.get(h)!;
			if (row.event_type === 'click') b.click = row.count;
			else if (row.event_type === 'postback') b.postback = row.count;
			else if (row.event_type === 'impression') b.impression = row.count;
		}

		// Sort hours ascending
		const sortedHours = [...buckets.keys()].sort((a, b) => a - b);

		// If no data, create empty range
		if (sortedHours.length === 0) {
			const now = Math.floor(Date.now() / 1000);
			const nowHour = now - (now % 3600);
			return [[nowHour], [0], [0], [0]];
		}

		const timestamps = new Float64Array(sortedHours.length);
		const clicks = new Float64Array(sortedHours.length);
		const postbacks = new Float64Array(sortedHours.length);
		const impressions = new Float64Array(sortedHours.length);

		for (let i = 0; i < sortedHours.length; i++) {
			const h = sortedHours[i];
			const b = buckets.get(h)!;
			timestamps[i] = h;
			clicks[i] = b.click;
			postbacks[i] = b.postback;
			impressions[i] = b.impression;
		}

		return [timestamps, clicks, postbacks, impressions];
	}

	function getOpts(width: number, data: uPlot.AlignedData): uPlot.Options {
		// Auto-zoom x-axis to actual data range with padding
		const ts = data[0] as number[];
		const dataMin = ts[0];
		const dataMax = ts[ts.length - 1];
		const span = dataMax - dataMin;
		// Minimum 1-hour span so single-point data doesn't look weird
		const minSpan = 3600;
		const effectiveSpan = Math.max(span, minSpan);
		const pad = effectiveSpan * 0.1;
		const xMin = dataMin - pad;
		const xMax = dataMax + pad;

		return {
			width,
			height: 200,
			cursor: { show: true, drag: { x: false, y: false } },
			legend: { show: true, live: true },
			scales: {
				x: { min: xMin, max: xMax },
			},
			axes: [
				{
					stroke: 'rgba(255,255,255,0.3)',
					grid: { stroke: 'rgba(255,255,255,0.06)', width: 1 },
					ticks: { stroke: 'rgba(255,255,255,0.1)', width: 1 },
					values: (_u: uPlot, vals: number[]) =>
						vals.map((v) => {
							const d = new Date(v * 1000);
							return d.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit' });
						}),
					font: '11px system-ui',
				},
				{
					stroke: 'rgba(255,255,255,0.3)',
					grid: { stroke: 'rgba(255,255,255,0.06)', width: 1 },
					ticks: { stroke: 'rgba(255,255,255,0.1)', width: 1 },
					font: '11px system-ui',
				},
			],
			series: [
				{},
				{
					label: 'Clicks',
					stroke: '#60a5fa',
					width: 2,
					fill: 'rgba(96,165,250,0.08)',
				},
				{
					label: 'Postbacks',
					stroke: '#4ade80',
					width: 2,
					fill: 'rgba(74,222,128,0.08)',
				},
				{
					label: 'Impressions',
					stroke: '#fbbf24',
					width: 2,
					fill: 'rgba(251,191,36,0.08)',
				},
			],
		};
	}

	function renderChart() {
		if (!chartEl || !poller.data) return;

		const data = buildData(poller.data);
		const width = chartEl.clientWidth;

		if (chart) {
			// Re-apply auto-zoom scales when data changes
			const opts = getOpts(width, data);
			chart.setScale('x', { min: opts.scales!.x!.min!, max: opts.scales!.x!.max! });
			chart.setData(data);
			chart.setSize({ width, height: 200 });
		} else {
			chart = new uPlot(getOpts(width, data), data, chartEl);
		}
	}

	// Watch for data changes
	$effect(() => {
		if (poller.data && chartEl) {
			renderChart();
		}
	});

	// Handle resize
	let resizeObserver: ResizeObserver | null = null;

	onMount(() => {
		if (chartEl) {
			resizeObserver = new ResizeObserver(() => {
				if (chart && chartEl) {
					chart.setSize({ width: chartEl.clientWidth, height: 200 });
				}
			});
			resizeObserver.observe(chartEl);
		}
	});

	onDestroy(() => {
		chart?.destroy();
		chart = null;
		resizeObserver?.disconnect();
		if (refreshTimer) clearTimeout(refreshTimer);
	});

	function totalFor(type: string): number {
		const base = poller.data?.summary.find((s) => s.event_type === type)?.total ?? 0;
		return base + (liveCounts[type] ?? 0);
	}

	// Reset live counters when poller refreshes (API now includes those events)
	$effect(() => {
		if (poller.data) {
			liveCounts = { click: 0, postback: 0, impression: 0 };
			countedIds = new Set();
		}
	});
</script>

<svelte:head>
	<link rel="stylesheet" href="https://unpkg.com/uplot/dist/uPlot.min.css" />
</svelte:head>

<svelte:window onkeydown={(e) => { if (e.key === 'Escape' && selectedEvent) { e.stopPropagation(); selectedEvent = null; } }} />

<div class="bg-card border border-border rounded-xl overflow-hidden">
	<!-- Header -->
	<div class="px-5 py-3 border-b border-border flex items-center justify-between">
		<div class="min-w-0 flex-1">
			<div class="flex items-center gap-2">
				<p class="text-sm font-semibold font-mono truncate">{tuId}</p>
				{#if sseConnected}
					<span class="flex items-center gap-1" title="SSE connected — live events for this link: {liveEvents.length}">
						<Radio class="w-3 h-3 text-green-400" />
						{#if liveEvents.length > 0}
							<span class="text-[10px] text-green-400 font-mono">{liveEvents.length}</span>
						{/if}
					</span>
				{/if}
			</div>
			{#if destination}
				<p class="text-xs text-muted-foreground truncate" title={destination}>
					{truncate(destination, 60)}
				</p>
			{/if}
		</div>
		<div class="flex items-center gap-1 ml-3 shrink-0">
			<button
				onclick={toggleTable}
				class="p-1.5 rounded-lg hover:bg-muted transition-colors"
				title="{showTable ? 'Hide' : 'Show'} events table"
			>
				{#if showTable}
					<ChevronUp class="w-3.5 h-3.5 text-muted-foreground" />
				{:else}
					<ChevronDown class="w-3.5 h-3.5 text-muted-foreground" />
				{/if}
			</button>
			<button
				onclick={() => poller.refresh()}
				class="p-1.5 rounded-lg hover:bg-muted transition-colors"
				title="Refresh"
			>
				<RefreshCw class="w-3.5 h-3.5 text-muted-foreground {poller.loading ? 'animate-spin' : ''}" />
			</button>
		</div>
	</div>

	<!-- Summary badges -->
	{#if poller.data}
		<div class="px-5 py-2 flex items-center gap-4 border-b border-border/50">
			<span class="text-xs">
				<span class="inline-block w-2 h-2 rounded-full bg-blue-400 mr-1.5"></span>
				<span class="text-muted-foreground">Clicks</span>
				<span class="font-mono font-medium ml-1">{totalFor('click').toLocaleString()}</span>
			</span>
			<span class="text-xs">
				<span class="inline-block w-2 h-2 rounded-full bg-green-400 mr-1.5"></span>
				<span class="text-muted-foreground">Postbacks</span>
				<span class="font-mono font-medium ml-1">{totalFor('postback').toLocaleString()}</span>
			</span>
			<span class="text-xs">
				<span class="inline-block w-2 h-2 rounded-full bg-amber-400 mr-1.5"></span>
				<span class="text-muted-foreground">Impressions</span>
				<span class="font-mono font-medium ml-1">{totalFor('impression').toLocaleString()}</span>
			</span>
		</div>
	{/if}

	<!-- Chart -->
	<div class="px-3 py-3">
		{#if poller.loading && !poller.data}
			<div class="h-[200px] flex items-center justify-center text-muted-foreground text-sm">
				Loading chart...
			</div>
		{:else if poller.error}
			<div class="h-[200px] flex items-center justify-center text-destructive text-sm">
				{poller.error}
			</div>
		{:else}
			<div bind:this={chartEl} class="w-full h-[200px]"></div>
		{/if}
	</div>


	<!-- Collapsible Data Table -->
	{#if showTable}
		<div class="border-t border-border">
			<div class="px-5 py-2 border-b border-border/50 flex items-center justify-between bg-muted/20">
				<div class="flex items-center gap-2">
					<span class="text-xs font-medium text-muted-foreground">Recent Events</span>
					{#if activeParamKey}
						<span class="inline-flex items-center gap-1 px-1.5 py-0.5 rounded bg-primary/10 text-primary text-[10px] font-mono">
							{activeParamKey}{#if activeParamValue}={activeParamValue}{/if}
							<button onclick={clearFilter} class="hover:text-destructive"><X class="w-2.5 h-2.5" /></button>
						</span>
					{/if}
				</div>
				<div class="flex items-center gap-1">
					<button
						onclick={() => (showFilter = !showFilter)}
						class="p-1 rounded hover:bg-muted transition-colors {showFilter ? 'text-primary' : 'text-muted-foreground'}"
						title="Filter by param"
					>
						<Filter class="w-3 h-3" />
					</button>
						<button
						onclick={fetchEvents}
						class="p-1 rounded hover:bg-muted transition-colors text-muted-foreground"
						title="Refresh events"
					>
						<RefreshCw class="w-3 h-3 {tableLoading ? 'animate-spin' : ''}" />
					</button>
				</div>
			</div>
			{#if showFilter}
				<div class="px-5 py-2 border-b border-border/50 bg-muted/10 flex items-center gap-2">
					<input
						bind:value={filterParamKey}
						placeholder="param key (e.g. sub1)"
						class="flex-1 bg-background border border-border rounded px-2 py-1 text-xs text-foreground placeholder:text-muted-foreground font-mono"
					/>
					<input
						bind:value={filterParamValue}
						placeholder="value (optional)"
						class="flex-1 bg-background border border-border rounded px-2 py-1 text-xs text-foreground placeholder:text-muted-foreground font-mono"
					/>
					<button
						onclick={applyFilter}
						disabled={!filterParamKey.trim()}
						class="px-2 py-1 rounded bg-primary text-primary-foreground text-xs font-medium hover:bg-primary/90 disabled:opacity-50"
					>
						Apply
					</button>
					{#if activeParamKey}
						<button
							onclick={clearFilter}
							class="px-2 py-1 rounded border border-border text-xs hover:bg-muted"
						>
							Clear
						</button>
					{/if}
				</div>
			{/if}
			{#if tableLoading && displayEvents.length === 0}
				<div class="px-5 py-6 text-center text-muted-foreground text-xs">Loading events...</div>
			{:else if tableError}
				<div class="px-5 py-4 text-destructive text-xs">{tableError}</div>
			{:else if tableFetched && displayEvents.length === 0}
				<div class="px-5 py-6 text-center text-muted-foreground text-xs">No events for this link</div>
			{:else if displayEvents.length > 0}
				<div class="max-h-64 overflow-y-auto">
					<table class="w-full text-xs">
						<thead>
							<tr class="border-b border-border text-muted-foreground">
								<th class="text-left px-5 py-2 font-medium">Type</th>
								<th class="text-left px-3 py-2 font-medium">Timestamp</th>
								<th class="text-left px-3 py-2 font-medium">IP</th>
								<th class="text-left px-3 py-2 font-medium">Path</th>
								<th class="text-left px-3 py-2 font-medium">Params</th>
							</tr>
						</thead>
						<tbody>
							{#each displayEvents as event (event.event_id)}
							<!-- svelte-ignore a11y_click_events_have_key_events -->
							<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
							<tr class="border-b border-border/30 hover:bg-muted/20 transition-colors cursor-pointer" onclick={() => (selectedEvent = event)}>
								<td class="px-5 py-2">
									<span class="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium
										{event.event_type === 'click' ? 'bg-blue-500/10 text-blue-400' :
										 event.event_type === 'postback' ? 'bg-green-500/10 text-green-400' :
										 'bg-amber-500/10 text-amber-400'}">
										{event.event_type}
									</span>
								</td>
								<td class="px-3 py-2 text-muted-foreground font-mono">{formatTimestamp(event.timestamp)}</td>
								<td class="px-3 py-2 font-mono">{event.ip}</td>
								<td class="px-3 py-2 text-muted-foreground">{event.request_path}</td>
								<td class="px-3 py-2">
									<div class="flex items-center gap-1">
										<code class="text-[10px] bg-muted px-1.5 py-0.5 rounded text-muted-foreground max-w-[200px] truncate block" title={JSON.stringify(event.params)}>
											{JSON.stringify(event.params)}
										</code>
										<button
											onclick={() => copyText(JSON.stringify(event.params, null, 2), event.event_id)}
											class="p-0.5 rounded hover:bg-muted transition-colors shrink-0"
											title="Copy params"
										>
											{#if copiedParams[event.event_id]}
												<Check class="w-3 h-3 text-green-400" />
											{:else}
												<Copy class="w-3 h-3 text-muted-foreground" />
											{/if}
										</button>
									</div>
								</td>
							</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</div>
	{/if}
</div>

<!-- Event Detail Modal -->
{#if selectedEvent}
	<!-- svelte-ignore a11y_click_events_have_key_events -->
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div
		class="fixed inset-0 z-[60] flex items-center justify-center"
		onclick={(e) => { if (e.target === e.currentTarget) selectedEvent = null; }}
	>
		<div class="absolute inset-0 bg-black/50 backdrop-blur-sm"></div>
		<div class="relative bg-card border border-border rounded-xl shadow-2xl w-full max-w-lg mx-4 max-h-[80vh] overflow-y-auto">
			<!-- Modal header -->
			<div class="sticky top-0 z-10 flex items-center justify-between px-5 py-3 bg-card/95 backdrop-blur border-b border-border rounded-t-xl">
				<div class="flex items-center gap-2">
					<span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium
						{selectedEvent.event_type === 'click' ? 'bg-blue-500/10 text-blue-400' :
						 selectedEvent.event_type === 'postback' ? 'bg-green-500/10 text-green-400' :
						 'bg-amber-500/10 text-amber-400'}">
						{selectedEvent.event_type}
					</span>
					<span class="text-xs text-muted-foreground font-mono">{selectedEvent.event_id.slice(0, 12)}...</span>
				</div>
				<button
					onclick={() => (selectedEvent = null)}
					class="p-1.5 rounded-lg hover:bg-muted transition-colors"
					title="Close"
				>
					<X class="w-4 h-4 text-muted-foreground" />
				</button>
			</div>

			<!-- Modal body -->
			<div class="px-5 py-4 space-y-3">
				<!-- Core fields -->
				<div class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-xs">
					<span class="text-muted-foreground font-medium">Event ID</span>
					<span class="font-mono break-all">{selectedEvent.event_id}</span>

					<span class="text-muted-foreground font-medium">Type</span>
					<span class="font-mono">{selectedEvent.event_type}</span>

					<span class="text-muted-foreground font-medium">Timestamp</span>
					<span class="font-mono">{new Date(selectedEvent.timestamp).toLocaleString()} <span class="text-muted-foreground">({selectedEvent.timestamp})</span></span>

					<span class="text-muted-foreground font-medium">IP</span>
					<span class="font-mono">{selectedEvent.ip}</span>

					<span class="text-muted-foreground font-medium">User Agent</span>
					<span class="font-mono break-all text-muted-foreground">{selectedEvent.user_agent || '—'}</span>

					<span class="text-muted-foreground font-medium">Referer</span>
					<span class="font-mono break-all text-muted-foreground">{selectedEvent.referer || '—'}</span>

					<span class="text-muted-foreground font-medium">Path</span>
					<span class="font-mono">{selectedEvent.request_path}</span>

					<span class="text-muted-foreground font-medium">Host</span>
					<span class="font-mono">{selectedEvent.request_host}</span>
				</div>

				<!-- Params -->
				{#if Object.keys(selectedEvent.params).length > 0}
					<div class="border-t border-border pt-3">
						<div class="flex items-center justify-between mb-2">
							<span class="text-xs font-medium text-muted-foreground">Parameters</span>
							<button
								onclick={() => copyText(JSON.stringify(selectedEvent!.params, null, 2), 'modal-params')}
								class="flex items-center gap-1 text-[10px] text-muted-foreground hover:text-foreground transition-colors"
							>
								{#if copiedParams['modal-params']}
									<Check class="w-3 h-3 text-green-400" /> Copied
								{:else}
									<Copy class="w-3 h-3" /> Copy all
								{/if}
							</button>
						</div>
						<div class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-xs">
							{#each Object.entries(selectedEvent.params) as [key, value]}
								<span class="font-mono text-primary/80">{key}</span>
								<span class="font-mono break-all">{value}</span>
							{/each}
						</div>
					</div>
				{:else}
					<div class="border-t border-border pt-3">
						<span class="text-xs text-muted-foreground">No parameters</span>
					</div>
				{/if}

				{#if selectedEvent.raw_payload}
					<div class="border-t border-border pt-3">
						<div class="flex items-center justify-between mb-2">
							<span class="text-xs font-medium text-muted-foreground">Raw Payload</span>
							<button
								onclick={() => copyText(JSON.stringify(selectedEvent!.raw_payload, null, 2), 'modal-raw-payload')}
								class="flex items-center gap-1 text-[10px] text-muted-foreground hover:text-foreground transition-colors"
							>
								{#if copiedParams['modal-raw-payload']}
									<Check class="w-3 h-3 text-green-400" /> Copied
								{:else}
									<Copy class="w-3 h-3" /> Copy all
								{/if}
							</button>
						</div>
						<pre class="text-[11px] font-mono bg-muted/30 rounded-lg p-3 overflow-x-auto max-h-60 overflow-y-auto whitespace-pre-wrap break-all">{JSON.stringify(selectedEvent.raw_payload, null, 2)}</pre>
					</div>
				{/if}
			</div>
		</div>
	</div>
{/if}
