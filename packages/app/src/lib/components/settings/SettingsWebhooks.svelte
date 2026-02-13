<script lang="ts">
  import { Plus, Trash2, Play, ToggleLeft, ToggleRight, RefreshCw } from 'lucide-svelte';
  import {
    getWebhooks,
    registerWebhook,
    deleteWebhook,
    updateWebhook,
    testWebhook,
    type Webhook
  } from '$lib/api/webhooks';
  import { formatDate } from '$lib/utils/format';
  import { createPoller } from '$lib/utils/polling.svelte';

  // Create dialog
  let showCreate = $state(false);
  let newUrl = $state('');
  let newEventTypes = $state('*');
  let creating = $state(false);
  let createResult = $state('');

  // Test result
  let testResult = $state<Record<string, string>>({});

  const poller = createPoller<Webhook[]>(() => getWebhooks(), { intervalMs: 30_000 });

  async function handleCreate() {
    if (!newUrl) return;
    creating = true;
    createResult = '';
    try {
      const types = newEventTypes === '*' ? ['*'] : newEventTypes.split(',').map((t) => t.trim());
      const result = await registerWebhook(newUrl, types);
      createResult = `Webhook created! Secret: ${result.secret} — Save this, it won't be shown again.`;
      newUrl = '';
      newEventTypes = '*';
      await poller.refresh();
    } catch (e) {
      createResult = e instanceof Error ? e.message : 'Failed to create webhook';
    } finally {
      creating = false;
    }
  }

  async function handleDelete(id: string) {
    if (!confirm('Delete this webhook? This cannot be undone.')) return;
    try {
      await deleteWebhook(id);
      await poller.refresh();
    } catch (e) {
      poller.refresh();
    }
  }

  async function handleToggle(wh: Webhook) {
    try {
      await updateWebhook(wh.id, { active: !wh.active });
      await poller.refresh();
    } catch (e) {
      poller.refresh();
    }
  }

  async function handleTest(id: string) {
    testResult[id] = 'Testing...';
    try {
      const result = await testWebhook(id);
      testResult[id] = result.delivered
        ? `✓ Delivered (HTTP ${result.status_code})`
        : `✗ Failed: ${result.error}`;
    } catch (e) {
      testResult[id] = e instanceof Error ? e.message : 'Test failed';
    }
  }
</script>

<div class="space-y-6">
  <div class="flex items-center justify-between">
    <p class="text-sm text-muted-foreground">Manage webhook endpoints</p>
    <div class="flex items-center gap-3">
      <button
        onclick={() => poller.refresh()}
        class="p-2 rounded-lg bg-background border border-border hover:bg-muted transition-colors"
      >
        <RefreshCw class="w-4 h-4 {poller.loading ? 'animate-spin' : ''}" />
      </button>
      <button
        onclick={() => (showCreate = !showCreate)}
        class="flex items-center gap-2 px-4 py-2 rounded-lg bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 transition-colors"
      >
        <Plus class="w-4 h-4" />
        Register Webhook
      </button>
    </div>
  </div>

  <!-- Create form -->
  {#if showCreate}
    <div class="bg-background border border-border rounded-xl p-5 space-y-4">
      <h3 class="text-sm font-semibold">Register New Webhook</h3>
      <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
        <div>
          <label for="wh-url" class="block text-xs text-muted-foreground mb-1.5">URL</label>
          <input
            id="wh-url"
            bind:value={newUrl}
            type="url"
            placeholder="https://your-app.com/webhooks"
            class="w-full bg-muted border border-border rounded-lg px-3 py-2 text-sm text-foreground placeholder:text-muted-foreground"
          />
        </div>
        <div>
          <label for="wh-event-types" class="block text-xs text-muted-foreground mb-1.5">Event Types</label>
          <input
            id="wh-event-types"
            bind:value={newEventTypes}
            placeholder="* or click,postback,impression"
            class="w-full bg-muted border border-border rounded-lg px-3 py-2 text-sm text-foreground placeholder:text-muted-foreground"
          />
        </div>
      </div>
      <div class="flex items-center gap-3">
        <button
          onclick={handleCreate}
          disabled={creating || !newUrl}
          class="px-4 py-2 rounded-lg bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 transition-colors disabled:opacity-50"
        >
          {creating ? 'Creating...' : 'Create'}
        </button>
        <button
          onclick={() => (showCreate = false)}
          class="px-4 py-2 rounded-lg border border-border text-sm hover:bg-muted transition-colors"
        >
          Cancel
        </button>
      </div>
      {#if createResult}
        <div class="text-sm p-3 rounded-lg bg-muted text-foreground break-all">{createResult}</div>
      {/if}
    </div>
  {/if}

  <!-- Webhooks table -->
  <div class="bg-background border border-border rounded-xl overflow-hidden">
    {#if poller.loading && !poller.data}
      <div class="p-8 text-center text-muted-foreground text-sm">Loading webhooks...</div>
    {:else if poller.error}
      <div class="p-4 text-destructive text-sm">{poller.error}</div>
    {:else if !poller.data || poller.data.length === 0}
      <div class="p-8 text-center text-muted-foreground text-sm">No webhooks registered</div>
    {:else}
      <div class="overflow-x-auto">
        <table class="w-full text-sm">
          <thead>
            <tr class="border-b border-border text-muted-foreground">
              <th class="text-left px-5 py-3 font-medium">URL</th>
              <th class="text-left px-5 py-3 font-medium">Event Types</th>
              <th class="text-left px-5 py-3 font-medium">Status</th>
              <th class="text-left px-5 py-3 font-medium">Created</th>
              <th class="text-right px-5 py-3 font-medium">Actions</th>
            </tr>
          </thead>
          <tbody>
            {#each poller.data as wh}
              <tr class="border-b border-border/50 hover:bg-muted/30 transition-colors">
                <td class="px-5 py-3 font-mono text-xs max-w-xs truncate">{wh.url}</td>
                <td class="px-5 py-3">
                  <div class="flex flex-wrap gap-1">
                    {#each wh.event_types as type}
                      <span class="px-1.5 py-0.5 rounded text-xs bg-muted text-muted-foreground">{type}</span>
                    {/each}
                  </div>
                </td>
                <td class="px-5 py-3">
                  <span class="inline-flex items-center gap-1.5 text-xs font-medium
                    {wh.active ? 'text-success' : 'text-muted-foreground'}">
                    <span class="w-1.5 h-1.5 rounded-full {wh.active ? 'bg-success' : 'bg-muted-foreground'}"></span>
                    {wh.active ? 'Active' : 'Inactive'}
                  </span>
                </td>
                <td class="px-5 py-3 text-muted-foreground text-xs">{formatDate(Number(wh.created_at))}</td>
                <td class="px-5 py-3">
                  <div class="flex items-center justify-end gap-1">
                    <button
                      onclick={() => handleToggle(wh)}
                      class="p-1.5 rounded hover:bg-muted transition-colors"
                      title={wh.active ? 'Disable' : 'Enable'}
                    >
                      {#if wh.active}
                        <ToggleRight class="w-4 h-4 text-success" />
                      {:else}
                        <ToggleLeft class="w-4 h-4 text-muted-foreground" />
                      {/if}
                    </button>
                    <button
                      onclick={() => handleTest(wh.id)}
                      class="p-1.5 rounded hover:bg-muted transition-colors"
                      title="Send test event"
                    >
                      <Play class="w-4 h-4 text-blue-400" />
                    </button>
                    <button
                      onclick={() => handleDelete(wh.id)}
                      class="p-1.5 rounded hover:bg-muted transition-colors"
                      title="Delete"
                    >
                      <Trash2 class="w-4 h-4 text-destructive" />
                    </button>
                  </div>
                  {#if testResult[wh.id]}
                    <div class="text-xs mt-1 text-right text-muted-foreground">{testResult[wh.id]}</div>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </div>
</div>
