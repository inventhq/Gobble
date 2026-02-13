<script lang="ts">
	import { RefreshCw, Radio, Plus, BarChart3, List, Trash2, Pencil, Copy, ExternalLink, X } from 'lucide-svelte';
	import SparkCard from '$lib/components/charts/SparkCard.svelte';
	import LinkDrawer from '$lib/components/charts/LinkDrawer.svelte';
	import {
		getTrackingUrls,
		createTrackingUrl,
		updateTrackingUrl,
		deleteTrackingUrl,
		type TrackingUrl,
		type TrackingUrlsResponse
	} from '$lib/api/tracking-urls';
	import { getTenants, type Tenant } from '$lib/api/tenants';
	import { getStats, type TrackingEvent, type StatsResponse } from '$lib/api/events';
	import { createPoller } from '$lib/utils/polling.svelte';
	import { createSSE } from '$lib/utils/sse.svelte';
	import { formatDate, formatNumber, truncate } from '$lib/utils/format';
	import { onMount, onDestroy } from 'svelte';

	// --- View toggle ---
	let view = $state<'chart' | 'list'>('chart');
	let hours = $state(24);

	// --- Data poller ---
	const poller = createPoller<TrackingUrlsResponse>(
		() => getTrackingUrls({ limit: 50 }),
		{ intervalMs: 60_000 }
	);

	// --- Batch stats for sparkline card counts ---
	interface TuStats {
		totalClicks: number;
		totalPostbacks: number;
	}
	let statsMap = $state<Record<string, TuStats>>({});
	let statsTimer: ReturnType<typeof setInterval> | null = null;

	async function fetchAllStats() {
		const urls = poller.data?.tracking_urls;
		if (!urls || urls.length === 0) return;
		const results = await Promise.allSettled(
			urls.map((u) => getStats({ hours, tu_id: u.id }))
		);
		const newMap: Record<string, TuStats> = {};
		results.forEach((r, i) => {
			if (r.status === 'fulfilled') {
				const data = r.value;
				newMap[urls[i].id] = {
					totalClicks: data.summary.find((s) => s.event_type === 'click')?.total ?? 0,
					totalPostbacks: data.summary.find((s) => s.event_type === 'postback')?.total ?? 0
				};
			}
		});
		statsMap = newMap;
	}

	// Fetch stats when tracking URLs load or hours changes
	$effect(() => {
		if (poller.data?.tracking_urls && poller.data.tracking_urls.length > 0) {
			void hours;
			fetchAllStats();
		}
	});

	// Init rolling buffers for all TUs (runs once when TU list loads)
	let buffersInitialized = false;
	$effect(() => {
		if (!buffersInitialized && poller.data?.tracking_urls && poller.data.tracking_urls.length > 0) {
			const init: Record<string, number[]> = {};
			for (const url of poller.data.tracking_urls) {
				init[url.id] = new Array(SPARK_SLOTS).fill(0);
			}
			liveBuffers = init;
			buffersInitialized = true;
		}
	});

	function startStatsTimer() {
		stopStatsTimer();
		statsTimer = setInterval(fetchAllStats, 30_000);
	}

	function stopStatsTimer() {
		if (statsTimer) { clearInterval(statsTimer); statsTimer = null; }
	}

	// --- Rolling SSE buffer for sparkline (60 slots = last 60 seconds) ---
	const SPARK_SLOTS = 60;
	let liveBuffers = $state<Record<string, number[]>>({});
	let bufferTimer: ReturnType<typeof setInterval> | null = null;

	function ensureBuffer(tuId: string): number[] {
		if (!liveBuffers[tuId]) {
			liveBuffers[tuId] = new Array(SPARK_SLOTS).fill(0);
		}
		return liveBuffers[tuId];
	}

	function shiftAllBuffers() {
		const updated: Record<string, number[]> = {};
		for (const [tuId, buf] of Object.entries(liveBuffers)) {
			const next = buf.slice(1);
			next.push(0);
			updated[tuId] = next;
		}
		liveBuffers = updated;
	}

	// --- SSE live events ---
	let liveByLink = $state<Record<string, TrackingEvent[]>>({});
	let liveClicksByLink = $state<Record<string, number>>({});
	let totalLive = $state(0);

	const sse = createSSE({
		maxEvents: 200,
		onEvent: (event) => {
			const tuId = event.params?.tu_id;
			if (!tuId) return;
			// Ignore replayed old events (only count events from the last 60s)
			const age = Date.now() - event.timestamp;
			if (age > 60_000) return;
			totalLive += 1;
			const existing = liveByLink[tuId] ?? [];
			liveByLink[tuId] = [event, ...existing.slice(0, 19)];
			if (event.event_type === 'click') {
				liveClicksByLink[tuId] = (liveClicksByLink[tuId] ?? 0) + 1;
				// Bump the rolling sparkline buffer (last slot = current second)
				const buf = ensureBuffer(tuId);
				buf[SPARK_SLOTS - 1] += 1;
			}
		}
	});

	function liveClicksFor(tuId: string): number {
		return liveClicksByLink[tuId] ?? 0;
	}

	// --- Drawer state ---
	let selectedTuId = $state<string | null>(null);
	let selectedDestination = $state('');
	const selectedLiveEvents = $derived(selectedTuId ? (liveByLink[selectedTuId] ?? []) : []);

	function openDrawer(tuId: string) {
		selectedTuId = tuId;
		const url = poller.data?.tracking_urls.find((u) => u.id === tuId);
		selectedDestination = url?.destination ?? '';
	}

	function closeDrawer() {
		selectedTuId = null;
		selectedDestination = '';
	}

	function handleHoursChange(e: Event) {
		hours = Number((e.target as HTMLSelectElement).value);
	}

	// --- Create modal ---
	let showCreateModal = $state(false);
	let newDestination = $state('');
	let creating = $state(false);
	let createResult = $state('');
	let tenants: Tenant[] = $state([]);
	let selectedTenantId = $state('');

	onMount(async () => {
		try {
			tenants = await getTenants();
			if (tenants.length === 1) selectedTenantId = tenants[0].id;
		} catch {}
		startStatsTimer();
		bufferTimer = setInterval(shiftAllBuffers, 1000);
	});

	onDestroy(() => {
		stopStatsTimer();
		if (bufferTimer) clearInterval(bufferTimer);
	});

	async function handleCreate() {
		if (!newDestination) return;
		creating = true;
		createResult = '';
		try {
			const result = await createTrackingUrl(newDestination, selectedTenantId || undefined);
			createResult = result.id;
			newDestination = '';
			await poller.refresh();
		} catch (e) {
			createResult = e instanceof Error ? e.message : 'Failed to create';
		} finally {
			creating = false;
		}
	}

	function closeCreateModal() {
		showCreateModal = false;
		createResult = '';
		newDestination = '';
	}

	// --- List view CRUD ---
	let editingId = $state('');
	let editDestination = $state('');
	let saving = $state(false);
	let copied = $state<Record<string, boolean>>({});

	async function handleDelete(id: string) {
		if (!confirm('Delete this tracking URL? Existing links will return 404.')) return;
		try {
			await deleteTrackingUrl(id);
			await poller.refresh();
		} catch {
			poller.refresh();
		}
	}

	function startEdit(tu: TrackingUrl) {
		editingId = tu.id;
		editDestination = tu.destination;
	}

	async function handleSaveEdit() {
		if (!editingId || !editDestination) return;
		saving = true;
		try {
			await updateTrackingUrl(editingId, editDestination);
			editingId = '';
			editDestination = '';
			await poller.refresh();
		} catch {
			// keep editing
		} finally {
			saving = false;
		}
	}

	function cancelEdit() {
		editingId = '';
		editDestination = '';
	}

	async function copyId(id: string) {
		try {
			await navigator.clipboard.writeText(id);
			copied[id] = true;
			setTimeout(() => { copied[id] = false; }, 2000);
		} catch {}
	}
</script>

<div class="space-y-6">
	<!-- Header -->
	<div class="flex items-center justify-between">
		<div>
			<h1 class="text-2xl font-bold">Home</h1>
			<div class="flex items-center gap-2 mt-1">
				<p class="text-sm text-muted-foreground">
					{view === 'chart' ? 'Per-link performance charts' : 'Tracking URLs — ID to destination mapping'}
				</p>
				{#if sse.connected}
					<span class="flex items-center gap-1.5 text-xs text-green-400">
						<Radio class="w-3 h-3" />
						<span class="w-1.5 h-1.5 rounded-full bg-green-400 animate-pulse"></span>
						Live{#if totalLive > 0} · {totalLive}{/if}
					</span>
				{/if}
			</div>
		</div>
		<div class="flex items-center gap-3">
			{#if view === 'chart'}
				<select
					value={hours}
					onchange={handleHoursChange}
					class="bg-card border border-border rounded-lg px-3 py-2 text-sm text-foreground"
				>
					<option value={1}>Last 1 hour</option>
					<option value={6}>Last 6 hours</option>
					<option value={24}>Last 24 hours</option>
					<option value={72}>Last 3 days</option>
					<option value={168}>Last 7 days</option>
				</select>
			{/if}
			<!-- View toggle -->
			<div class="flex rounded-lg border border-border overflow-hidden">
				<button
					onclick={() => (view = 'chart')}
					class="flex items-center gap-1.5 px-3 py-2 text-sm transition-colors {view === 'chart' ? 'bg-primary text-primary-foreground' : 'bg-card text-muted-foreground hover:text-foreground hover:bg-muted'}"
					title="Chart view"
				>
					<BarChart3 class="w-3.5 h-3.5" />
					<span class="hidden sm:inline">Charts</span>
				</button>
				<button
					onclick={() => (view = 'list')}
					class="flex items-center gap-1.5 px-3 py-2 text-sm transition-colors border-l border-border {view === 'list' ? 'bg-primary text-primary-foreground' : 'bg-card text-muted-foreground hover:text-foreground hover:bg-muted'}"
					title="List view"
				>
					<List class="w-3.5 h-3.5" />
					<span class="hidden sm:inline">List</span>
				</button>
			</div>
			<button
				onclick={() => poller.refresh()}
				class="p-2 rounded-lg bg-card border border-border hover:bg-muted transition-colors"
				title="Refresh"
			>
				<RefreshCw class="w-4 h-4 {poller.loading ? 'animate-spin' : ''}" />
			</button>
			<button
				onclick={() => (showCreateModal = true)}
				class="flex items-center justify-center w-9 h-9 rounded-lg bg-primary text-primary-foreground hover:bg-primary/90 transition-colors"
				title="New Tracking URL"
			>
				<Plus class="w-5 h-5" />
			</button>
		</div>
	</div>

	<!-- Content -->
	{#if poller.loading && !poller.data}
		{#if view === 'chart'}
			<div class="flex flex-wrap gap-3">
				{#each Array(6) as _}
					<div class="bg-card border border-border rounded-lg animate-pulse" style="width: 250px; height: 100px;"></div>
				{/each}
			</div>
		{:else}
			<div class="bg-card border border-border rounded-xl p-8 text-center text-muted-foreground text-sm">
				Loading tracking URLs...
			</div>
		{/if}
	{:else if poller.error}
		<div class="bg-destructive/10 border border-destructive/20 rounded-xl p-4 text-destructive text-sm">
			{poller.error}
		</div>
	{:else if poller.data && poller.data.tracking_urls.length === 0}
		<div class="bg-card border border-border rounded-xl p-8 text-center">
			<p class="text-muted-foreground text-sm">No tracking URLs found.</p>
			<p class="text-muted-foreground text-xs mt-1">Create a tracking URL to get started.</p>
		</div>
	{:else if poller.data}
		<!-- Chart View — Sparkline Grid -->
		{#if view === 'chart'}
			<div class="flex flex-wrap gap-3">
				{#each poller.data.tracking_urls as url (url.id)}
					<SparkCard
						tuId={url.id}
						destination={url.destination}
						liveBuffer={liveBuffers[url.id] ?? []}
						totalClicks={statsMap[url.id]?.totalClicks ?? 0}
						totalPostbacks={statsMap[url.id]?.totalPostbacks ?? 0}
						sseConnected={sse.connected}
						onSelect={openDrawer}
					/>
				{/each}
			</div>
		{:else}
			<!-- List View -->
			<div class="bg-card border border-border rounded-xl overflow-hidden">
				<div class="overflow-x-auto">
					<table class="w-full text-sm">
						<thead>
							<tr class="border-b border-border text-muted-foreground">
								<th class="text-left px-5 py-3 font-medium">ID</th>
								<th class="text-left px-5 py-3 font-medium">Destination</th>
								<th class="text-right px-5 py-3 font-medium">Clicks</th>
								<th class="text-right px-5 py-3 font-medium">Postbacks</th>
								<th class="text-right px-5 py-3 font-medium">Impressions</th>
								<th class="text-left px-5 py-3 font-medium">Created</th>
								<th class="text-right px-5 py-3 font-medium">Actions</th>
							</tr>
						</thead>
						<tbody>
							{#each poller.data.tracking_urls as tu (tu.id)}
								<tr class="border-b border-border/50 hover:bg-muted/30 transition-colors">
									<td class="px-5 py-3">
										<div class="flex items-center gap-1.5">
											<code class="font-mono text-xs text-primary">{truncate(tu.id, 24)}</code>
											<button
												onclick={() => copyId(tu.id)}
												class="p-0.5 rounded hover:bg-muted transition-colors"
												title="Copy ID"
											>
												{#if copied[tu.id]}
													<span class="text-xs text-green-400">Copied</span>
												{:else}
													<Copy class="w-3 h-3 text-muted-foreground" />
												{/if}
											</button>
										</div>
									</td>
									<td class="px-5 py-3">
										{#if editingId === tu.id}
											<div class="flex items-center gap-2">
												<input
													bind:value={editDestination}
													class="flex-1 bg-background border border-border rounded px-2 py-1 text-xs text-foreground font-mono"
												/>
												<button
													onclick={handleSaveEdit}
													disabled={saving}
													class="px-2 py-1 rounded bg-primary text-primary-foreground text-xs hover:bg-primary/90"
												>
													{saving ? '...' : 'Save'}
												</button>
												<button
													onclick={cancelEdit}
													class="px-2 py-1 rounded border border-border text-xs hover:bg-muted"
												>
													Cancel
												</button>
											</div>
										{:else}
											<div class="flex items-center gap-1.5">
												<span class="font-mono text-xs max-w-xs truncate" title={tu.destination}>
													{truncate(tu.destination, 50)}
												</span>
												<a
													href={tu.destination}
													target="_blank"
													rel="noopener noreferrer"
													class="p-0.5 rounded hover:bg-muted transition-colors"
													title="Open destination"
												>
													<ExternalLink class="w-3 h-3 text-muted-foreground" />
												</a>
											</div>
										{/if}
									</td>
									<td class="px-5 py-3 text-right">
										<span class="text-blue-400 font-medium">{formatNumber(tu.clicks)}</span>
									</td>
									<td class="px-5 py-3 text-right">
										<span class="text-green-400 font-medium">{formatNumber(tu.postbacks)}</span>
									</td>
									<td class="px-5 py-3 text-right">
										<span class="text-amber-400 font-medium">{formatNumber(tu.impressions)}</span>
									</td>
									<td class="px-5 py-3 text-muted-foreground text-xs">{formatDate(Number(tu.created_at))}</td>
									<td class="px-5 py-3">
										<div class="flex items-center justify-end gap-1">
											<button
												onclick={() => startEdit(tu)}
												class="p-1.5 rounded hover:bg-muted transition-colors"
												title="Edit destination"
											>
												<Pencil class="w-4 h-4 text-muted-foreground" />
											</button>
											<button
												onclick={() => handleDelete(tu.id)}
												class="p-1.5 rounded hover:bg-muted transition-colors"
												title="Delete"
											>
												<Trash2 class="w-4 h-4 text-destructive" />
											</button>
										</div>
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			</div>
		{/if}
	{/if}
</div>

<!-- Create Tracking URL Modal -->
{#if showCreateModal}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<!-- svelte-ignore a11y_click_events_have_key_events -->
	<div
		class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
		onclick={(e) => { if (e.target === e.currentTarget) closeCreateModal(); }}
	>
		<div class="bg-card border border-border rounded-xl w-full max-w-md mx-4 shadow-2xl">
			<div class="flex items-center justify-between px-5 py-4 border-b border-border">
				<h2 class="text-lg font-semibold">New Tracking URL</h2>
				<button
					onclick={closeCreateModal}
					class="p-1 rounded-lg hover:bg-muted transition-colors"
				>
					<X class="w-4 h-4 text-muted-foreground" />
				</button>
			</div>
			<div class="px-5 py-5 space-y-4">
				<div>
					<label for="new-destination" class="block text-xs text-muted-foreground mb-1.5">Destination URL</label>
					<input
						id="new-destination"
						bind:value={newDestination}
						type="url"
						placeholder="https://example.com/landing-page"
						class="w-full bg-background border border-border rounded-lg px-3 py-2 text-sm text-foreground placeholder:text-muted-foreground"
					/>
				</div>
				<div>
					<label for="tenant-select" class="block text-xs text-muted-foreground mb-1.5">Client</label>
					<select
						id="tenant-select"
						bind:value={selectedTenantId}
						class="w-full bg-background border border-border rounded-lg px-3 py-2 text-sm text-foreground"
					>
						{#if tenants.length === 0}
							<option value="">Loading...</option>
						{:else}
							<option value="" disabled>Select a client</option>
							{#each tenants as t}
								<option value={t.id}>{t.name} ({t.key_prefix})</option>
							{/each}
						{/if}
					</select>
				</div>
				{#if createResult}
					<div class="text-sm p-3 rounded-lg bg-muted text-foreground break-all">
						{#if createResult.startsWith('tu_')}
							Created: <code class="font-mono text-primary">{createResult}</code>
						{:else}
							{createResult}
						{/if}
					</div>
				{/if}
			</div>
			<div class="flex items-center justify-end gap-3 px-5 py-4 border-t border-border">
				<button
					onclick={closeCreateModal}
					class="px-4 py-2 rounded-lg border border-border text-sm hover:bg-muted transition-colors"
				>
					{createResult?.startsWith('tu_') ? 'Done' : 'Cancel'}
				</button>
				{#if !createResult?.startsWith('tu_')}
					<button
						onclick={handleCreate}
						disabled={creating || !newDestination || !selectedTenantId}
						class="px-4 py-2 rounded-lg bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 transition-colors disabled:opacity-50"
					>
						{creating ? 'Creating...' : 'Create'}
					</button>
				{/if}
			</div>
		</div>
	</div>
{/if}

<!-- Detail Drawer -->
{#if selectedTuId}
	<LinkDrawer
		tuId={selectedTuId}
		destination={selectedDestination}
		{hours}
		liveEvents={selectedLiveEvents}
		sseConnected={sse.connected}
		onClose={closeDrawer}
	/>
{/if}
