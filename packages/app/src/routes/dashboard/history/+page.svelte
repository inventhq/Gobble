<script lang="ts">
  import { getHistory, getMergedStats, type HistoryResponse, type MergedStatsResponse } from '$lib/api/events';
  import { Search, Calendar, Database, Clock, Loader2 } from 'lucide-svelte';

  let dateFrom = $state('');
  let dateTo = $state('');
  let mode = $state<'stats' | 'events'>('stats');
  let eventType = $state('');
  let groupBy = $state('event_type');
  let loading = $state(false);
  let error = $state('');

  let historyResult = $state<HistoryResponse | null>(null);
  let mergedResult = $state<MergedStatsResponse | null>(null);
  let queryType = $state<'history' | 'merged'>('history');

  // Default dates: last 30 days
  const now = new Date();
  const thirtyDaysAgo = new Date(now.getTime() - 30 * 24 * 60 * 60 * 1000);
  dateFrom = thirtyDaysAgo.toISOString().slice(0, 10);
  dateTo = now.toISOString().slice(0, 10);

  async function runQuery() {
    loading = true;
    error = '';
    historyResult = null;
    mergedResult = null;

    try {
      if (queryType === 'merged') {
        mergedResult = await getMergedStats({
          date_from: dateFrom,
          date_to: dateTo,
          event_type: eventType || undefined,
          group_by: groupBy || undefined,
        });
      } else {
        historyResult = await getHistory({
          date_from: dateFrom,
          date_to: dateTo,
          mode,
          event_type: eventType || undefined,
          group_by: mode === 'stats' ? groupBy || undefined : undefined,
          limit: mode === 'events' ? 100 : 1000,
        });
      }
    } catch (e: any) {
      error = e.message || 'Query failed';
    } finally {
      loading = false;
    }
  }

  function formatNumber(n: number): string {
    return n.toLocaleString();
  }

  function formatTimestamp(ms: number): string {
    return new Date(ms).toLocaleString();
  }
</script>

<div class="p-6 max-w-6xl">
  <div class="mb-6">
    <h1 class="text-2xl font-bold text-foreground">Historical Analytics</h1>
    <p class="text-sm text-muted-foreground mt-1">
      Query archived events from cold storage (R2 Parquet via Polars)
    </p>
  </div>

  <!-- Query Form -->
  <div class="bg-card border border-border rounded-lg p-4 mb-6">
    <div class="flex flex-wrap gap-3 items-end">
      <!-- Date Range -->
      <div class="flex gap-2 items-end">
        <div>
          <label class="text-xs text-muted-foreground block mb-1">From</label>
          <input
            type="date"
            bind:value={dateFrom}
            class="bg-background border border-border rounded px-2 py-1.5 text-sm text-foreground"
          />
        </div>
        <div>
          <label class="text-xs text-muted-foreground block mb-1">To</label>
          <input
            type="date"
            bind:value={dateTo}
            class="bg-background border border-border rounded px-2 py-1.5 text-sm text-foreground"
          />
        </div>
      </div>

      <!-- Query Type -->
      <div>
        <label class="text-xs text-muted-foreground block mb-1">Source</label>
        <select
          bind:value={queryType}
          class="bg-background border border-border rounded px-2 py-1.5 text-sm text-foreground"
        >
          <option value="history">Cold Only (Parquet)</option>
          <option value="merged">Hot + Cold (Merged)</option>
        </select>
      </div>

      <!-- Mode -->
      <div>
        <label class="text-xs text-muted-foreground block mb-1">Mode</label>
        <select
          bind:value={mode}
          class="bg-background border border-border rounded px-2 py-1.5 text-sm text-foreground"
        >
          <option value="stats">Stats (Aggregated)</option>
          <option value="events">Events (Raw Rows)</option>
        </select>
      </div>

      <!-- Event Type Filter -->
      <div>
        <label class="text-xs text-muted-foreground block mb-1">Event Type</label>
        <select
          bind:value={eventType}
          class="bg-background border border-border rounded px-2 py-1.5 text-sm text-foreground"
        >
          <option value="">All</option>
          <option value="click">Click</option>
          <option value="postback">Postback</option>
          <option value="impression">Impression</option>
        </select>
      </div>

      <!-- Group By (stats mode only) -->
      {#if mode === 'stats'}
        <div>
          <label class="text-xs text-muted-foreground block mb-1">Group By</label>
          <select
            bind:value={groupBy}
            class="bg-background border border-border rounded px-2 py-1.5 text-sm text-foreground"
          >
            <option value="event_type">Event Type</option>
            <option value="tu_id">Tracking URL</option>
            <option value="date">Date</option>
          </select>
        </div>
      {/if}

      <!-- Run Button -->
      <button
        onclick={runQuery}
        disabled={loading || !dateFrom || !dateTo}
        class="flex items-center gap-1.5 bg-primary text-primary-foreground px-4 py-1.5 rounded text-sm font-medium hover:bg-primary/90 disabled:opacity-50 disabled:cursor-not-allowed"
      >
        {#if loading}
          <Loader2 class="w-4 h-4 animate-spin" />
        {:else}
          <Search class="w-4 h-4" />
        {/if}
        Query
      </button>
    </div>
  </div>

  <!-- Error -->
  {#if error}
    <div class="bg-destructive/10 border border-destructive/20 rounded-lg p-3 mb-6 text-sm text-destructive">
      {error}
    </div>
  {/if}

  <!-- Merged Stats Result -->
  {#if mergedResult}
    <div class="space-y-4">
      <!-- Sources Badge -->
      <div class="flex gap-2 items-center text-xs text-muted-foreground">
        <div class="flex items-center gap-1">
          <Database class="w-3.5 h-3.5" />
          Sources:
        </div>
        {#if mergedResult.sources.hot}
          <span class="bg-emerald-500/10 text-emerald-400 px-2 py-0.5 rounded-full">
            RisingWave ({mergedResult.hot_hours}h)
          </span>
        {/if}
        {#if mergedResult.sources.cold}
          <span class="bg-blue-500/10 text-blue-400 px-2 py-0.5 rounded-full">
            Polars ({mergedResult.sources.cold_partitions} files, {mergedResult.sources.cold_query_ms}ms)
          </span>
        {/if}
        {#if mergedResult.cold_range}
          <span class="text-muted-foreground">
            Cold: {mergedResult.cold_range.from} → {mergedResult.cold_range.to}
          </span>
        {/if}
      </div>

      <!-- Summary Cards -->
      <div class="grid grid-cols-3 gap-3">
        {#each mergedResult.summary as stat}
          <div class="bg-card border border-border rounded-lg p-4">
            <div class="text-xs text-muted-foreground uppercase tracking-wide">{stat.event_type}</div>
            <div class="text-2xl font-bold text-foreground mt-1">{formatNumber(stat.total)}</div>
          </div>
        {/each}
      </div>

      <!-- Hourly Table (hot data) -->
      {#if mergedResult.hourly.length > 0}
        <div class="bg-card border border-border rounded-lg overflow-hidden">
          <div class="px-4 py-2 border-b border-border text-xs text-muted-foreground font-medium">
            Hourly Breakdown (Hot Window)
          </div>
          <div class="max-h-80 overflow-y-auto">
            <table class="w-full text-sm">
              <thead class="bg-muted/50 sticky top-0">
                <tr>
                  <th class="text-left px-4 py-2 text-xs text-muted-foreground font-medium">Hour</th>
                  <th class="text-left px-4 py-2 text-xs text-muted-foreground font-medium">Type</th>
                  <th class="text-right px-4 py-2 text-xs text-muted-foreground font-medium">Count</th>
                </tr>
              </thead>
              <tbody>
                {#each mergedResult.hourly as row}
                  <tr class="border-t border-border/50">
                    <td class="px-4 py-1.5 text-muted-foreground font-mono text-xs">
                      {new Date(row.hour * 1000).toLocaleString()}
                    </td>
                    <td class="px-4 py-1.5">
                      <span class="px-1.5 py-0.5 rounded text-xs font-medium
                        {row.event_type === 'click' ? 'bg-blue-500/10 text-blue-400' :
                         row.event_type === 'postback' ? 'bg-emerald-500/10 text-emerald-400' :
                         'bg-amber-500/10 text-amber-400'}">
                        {row.event_type}
                      </span>
                    </td>
                    <td class="px-4 py-1.5 text-right font-mono">{formatNumber(row.count)}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        </div>
      {/if}
    </div>
  {/if}

  <!-- History (Cold Only) Result -->
  {#if historyResult}
    <div class="space-y-4">
      <!-- Meta -->
      <div class="flex gap-3 items-center text-xs text-muted-foreground">
        <div class="flex items-center gap-1">
          <Database class="w-3.5 h-3.5" />
          {historyResult.partitions_scanned} partitions scanned
        </div>
        <div class="flex items-center gap-1">
          <Clock class="w-3.5 h-3.5" />
          {historyResult.query_ms}ms
        </div>
        <div>{historyResult.count} rows</div>
        <span class="bg-blue-500/10 text-blue-400 px-2 py-0.5 rounded-full">
          {historyResult.source}
        </span>
      </div>

      <!-- Stats Mode -->
      {#if mode === 'stats' && historyResult.rows.length > 0}
        <div class="bg-card border border-border rounded-lg overflow-hidden">
          <div class="max-h-96 overflow-y-auto">
            <table class="w-full text-sm">
              <thead class="bg-muted/50 sticky top-0">
                <tr>
                  {#each Object.keys(historyResult.rows[0]) as col}
                    <th class="text-left px-4 py-2 text-xs text-muted-foreground font-medium">{col}</th>
                  {/each}
                </tr>
              </thead>
              <tbody>
                {#each historyResult.rows as row}
                  <tr class="border-t border-border/50">
                    {#each Object.values(row) as val}
                      <td class="px-4 py-1.5 font-mono text-xs">
                        {typeof val === 'number' ? formatNumber(val) : val ?? '—'}
                      </td>
                    {/each}
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        </div>
      {/if}

      <!-- Events Mode -->
      {#if mode === 'events' && historyResult.rows.length > 0}
        <div class="bg-card border border-border rounded-lg overflow-hidden">
          <div class="max-h-[500px] overflow-y-auto">
            <table class="w-full text-sm">
              <thead class="bg-muted/50 sticky top-0">
                <tr>
                  <th class="text-left px-4 py-2 text-xs text-muted-foreground font-medium">Time</th>
                  <th class="text-left px-4 py-2 text-xs text-muted-foreground font-medium">Type</th>
                  <th class="text-left px-4 py-2 text-xs text-muted-foreground font-medium">Tenant</th>
                  <th class="text-left px-4 py-2 text-xs text-muted-foreground font-medium">TU ID</th>
                  <th class="text-left px-4 py-2 text-xs text-muted-foreground font-medium">IP</th>
                  <th class="text-left px-4 py-2 text-xs text-muted-foreground font-medium">Event ID</th>
                </tr>
              </thead>
              <tbody>
                {#each historyResult.rows as row}
                  <tr class="border-t border-border/50">
                    <td class="px-4 py-1.5 text-muted-foreground font-mono text-xs">
                      {row.timestamp_ms ? formatTimestamp(row.timestamp_ms as number) : '—'}
                    </td>
                    <td class="px-4 py-1.5">
                      <span class="px-1.5 py-0.5 rounded text-xs font-medium
                        {row.event_type === 'click' ? 'bg-blue-500/10 text-blue-400' :
                         row.event_type === 'postback' ? 'bg-emerald-500/10 text-emerald-400' :
                         'bg-amber-500/10 text-amber-400'}">
                        {row.event_type}
                      </span>
                    </td>
                    <td class="px-4 py-1.5 font-mono text-xs">{row.tenant_id ?? '—'}</td>
                    <td class="px-4 py-1.5 font-mono text-xs">{row.tu_id ?? '—'}</td>
                    <td class="px-4 py-1.5 font-mono text-xs">{row.ip ?? '—'}</td>
                    <td class="px-4 py-1.5 font-mono text-xs text-muted-foreground">
                      {(row.event_id as string)?.slice(0, 18) ?? '—'}…
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        </div>
      {/if}

      <!-- Empty State -->
      {#if historyResult.rows.length === 0}
        <div class="bg-card border border-border rounded-lg p-8 text-center text-muted-foreground">
          <Calendar class="w-8 h-8 mx-auto mb-2 opacity-50" />
          <p>No data found for this date range</p>
        </div>
      {/if}
    </div>
  {/if}

  <!-- Initial State -->
  {#if !historyResult && !mergedResult && !loading && !error}
    <div class="bg-card border border-border rounded-lg p-12 text-center text-muted-foreground">
      <Database class="w-10 h-10 mx-auto mb-3 opacity-40" />
      <p class="text-lg font-medium">Query Historical Data</p>
      <p class="text-sm mt-1">Select a date range and click Query to search archived events</p>
    </div>
  {/if}
</div>
