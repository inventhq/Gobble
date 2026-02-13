<script lang="ts">
  import { Plus, Trash2, RefreshCw, Copy, Check } from 'lucide-svelte';
  import { getKeys, createKey, revokeKey, type ApiKey } from '$lib/api/keys';
  import { formatDate, timeAgo } from '$lib/utils/format';
  import { createPoller } from '$lib/utils/polling.svelte';

  // Create dialog
  let showCreate = $state(false);
  let newName = $state('');
  let newTenantId = $state('');
  let creating = $state(false);
  let createdKey = $state('');

  // Copy feedback
  let copied = $state(false);
  let copiedCell = $state<Record<string, boolean>>({});
  async function copyCell(text: string, key: string) {
    try {
      await navigator.clipboard.writeText(text);
      copiedCell[key] = true;
      setTimeout(() => { copiedCell[key] = false; }, 2000);
    } catch {}
  }

  const poller = createPoller<ApiKey[]>(() => getKeys(), { intervalMs: 30_000 });

  async function handleCreate() {
    if (!newTenantId) return;
    creating = true;
    createdKey = '';
    try {
      const result = await createKey(newTenantId, newName || undefined);
      createdKey = result.key;
      newName = '';
      newTenantId = '';
      await poller.refresh();
    } catch (e) {
      poller.refresh();
    } finally {
      creating = false;
    }
  }

  async function handleRevoke(id: string) {
    if (!confirm('Revoke this API key? This cannot be undone.')) return;
    try {
      await revokeKey(id);
      await poller.refresh();
    } catch (e) {
      poller.refresh();
    }
  }

  async function copyKey() {
    await navigator.clipboard.writeText(createdKey);
    copied = true;
    setTimeout(() => (copied = false), 2000);
  }
</script>

<div class="space-y-6">
  <div class="flex items-center justify-between">
    <p class="text-sm text-muted-foreground">Manage API keys for authentication</p>
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
        Create Key
      </button>
    </div>
  </div>

  <!-- Create form -->
  {#if showCreate}
    <div class="bg-background border border-border rounded-xl p-5 space-y-4">
      <h3 class="text-sm font-semibold">Create New API Key</h3>
      <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
        <div>
          <label for="key-tenant-id" class="block text-xs text-muted-foreground mb-1.5">Tenant ID</label>
          <input
            id="key-tenant-id"
            bind:value={newTenantId}
            placeholder="Tenant ID"
            class="w-full bg-muted border border-border rounded-lg px-3 py-2 text-sm text-foreground placeholder:text-muted-foreground"
          />
        </div>
        <div>
          <label for="key-name" class="block text-xs text-muted-foreground mb-1.5">Name (optional)</label>
          <input
            id="key-name"
            bind:value={newName}
            placeholder="e.g. Production, Staging"
            class="w-full bg-muted border border-border rounded-lg px-3 py-2 text-sm text-foreground placeholder:text-muted-foreground"
          />
        </div>
      </div>
      <div class="flex items-center gap-3">
        <button
          onclick={handleCreate}
          disabled={creating || !newTenantId}
          class="px-4 py-2 rounded-lg bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 transition-colors disabled:opacity-50"
        >
          {creating ? 'Creating...' : 'Create'}
        </button>
        <button
          onclick={() => { showCreate = false; createdKey = ''; }}
          class="px-4 py-2 rounded-lg border border-border text-sm hover:bg-muted transition-colors"
        >
          Cancel
        </button>
      </div>
      {#if createdKey}
        <div class="p-3 rounded-lg bg-warning/10 border border-warning/20 space-y-2">
          <p class="text-xs font-medium text-warning">Save this key — it will not be shown again.</p>
          <div class="flex items-center gap-2">
            <code class="flex-1 text-xs bg-muted px-3 py-2 rounded border border-border font-mono break-all">
              {createdKey}
            </code>
            <button
              onclick={copyKey}
              class="p-2 rounded-lg border border-border hover:bg-muted transition-colors"
              title="Copy"
            >
              {#if copied}
                <Check class="w-4 h-4 text-success" />
              {:else}
                <Copy class="w-4 h-4" />
              {/if}
            </button>
          </div>
        </div>
      {/if}
    </div>
  {/if}

  <!-- Keys table -->
  <div class="bg-background border border-border rounded-xl overflow-hidden">
    {#if poller.loading && !poller.data}
      <div class="p-8 text-center text-muted-foreground text-sm">Loading keys...</div>
    {:else if poller.error}
      <div class="p-4 text-destructive text-sm">{poller.error}</div>
    {:else if !poller.data || poller.data.length === 0}
      <div class="p-8 text-center text-muted-foreground text-sm">No API keys</div>
    {:else}
      <div class="overflow-x-auto">
        <table class="w-full text-sm">
          <thead>
            <tr class="border-b border-border text-muted-foreground">
              <th class="text-left px-5 py-3 font-medium">Name</th>
              <th class="text-left px-5 py-3 font-medium">Prefix</th>
              {#if poller.data[0]?.tenant_id}
                <th class="text-left px-5 py-3 font-medium">Tenant</th>
              {/if}
              <th class="text-left px-5 py-3 font-medium">Last Used</th>
              <th class="text-left px-5 py-3 font-medium">Created</th>
              <th class="text-right px-5 py-3 font-medium">Actions</th>
            </tr>
          </thead>
          <tbody>
            {#each poller.data as key}
              <tr class="border-b border-border/50 hover:bg-muted/30 transition-colors">
                <td class="px-5 py-3 font-medium">{key.name}</td>
                <td class="px-5 py-3">
                  <div class="flex items-center gap-1">
                    <code class="font-mono text-xs text-muted-foreground">{key.key_prefix}</code>
                    <button
                      onclick={() => copyCell(key.key_prefix, 'prefix-' + key.id)}
                      class="p-0.5 rounded hover:bg-muted transition-colors shrink-0"
                      title="Copy prefix"
                    >
                      {#if copiedCell['prefix-' + key.id]}
                        <Check class="w-3 h-3 text-green-400" />
                      {:else}
                        <Copy class="w-3 h-3 text-muted-foreground" />
                      {/if}
                    </button>
                  </div>
                </td>
                {#if key.tenant_id}
                  <td class="px-5 py-3">
                    <div class="flex items-center gap-1">
                      <code class="font-mono text-xs text-muted-foreground max-w-[120px] truncate" title={key.tenant_id}>{key.tenant_id}</code>
                      <button
                        onclick={() => copyCell(key.tenant_id!, 'tenant-' + key.id)}
                        class="p-0.5 rounded hover:bg-muted transition-colors shrink-0"
                        title="Copy tenant ID"
                      >
                        {#if copiedCell['tenant-' + key.id]}
                          <Check class="w-3 h-3 text-green-400" />
                        {:else}
                          <Copy class="w-3 h-3 text-muted-foreground" />
                        {/if}
                      </button>
                    </div>
                  </td>
                {/if}
                <td class="px-5 py-3 text-muted-foreground text-xs">
                  {key.last_used_at ? timeAgo(Number(key.last_used_at)) : 'Never'}
                </td>
                <td class="px-5 py-3 text-muted-foreground text-xs">{formatDate(Number(key.created_at))}</td>
                <td class="px-5 py-3 text-right">
                  <button
                    onclick={() => handleRevoke(key.id)}
                    class="p-1.5 rounded hover:bg-muted transition-colors"
                    title="Revoke"
                  >
                    <Trash2 class="w-4 h-4 text-destructive" />
                  </button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </div>
</div>
