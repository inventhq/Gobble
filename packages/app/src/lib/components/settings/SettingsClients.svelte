<script lang="ts">
  import { onMount } from 'svelte';
  import { Plus, RotateCcw, Copy, Check, RefreshCw } from 'lucide-svelte';
  import {
    getTenants,
    createTenant,
    updateTenant,
    rotateSecrets,
    type Tenant
  } from '$lib/api/tenants';
  import { formatDate } from '$lib/utils/format';
  import { PLANS } from '$lib/utils/constants';

  let tenants: Tenant[] = $state([]);
  let loading = $state(true);
  let error = $state('');

  // Create form
  let showCreate = $state(false);
  let newName = $state('');
  let newEmail = $state('');
  let newPlan = $state('free');
  let creating = $state(false);
  let createResult = $state('');

  // Rotate result
  let rotateResult = $state<Record<string, string>>({});
  let copied = $state('');

  async function load() {
    loading = true;
    error = '';
    try {
      tenants = await getTenants();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load clients';
    } finally {
      loading = false;
    }
  }

  onMount(() => load());

  async function handleCreate() {
    if (!newName) return;
    creating = true;
    createResult = '';
    try {
      const result = await createTenant(newName, newPlan, newEmail || undefined);
      let msg = `Client created!\nHMAC Secret: ${result.hmac_secret}\nEncryption Key: ${result.encryption_key}\n\nSave these — they will not be shown again.`;
      if (newEmail) msg += `\n\nPermit.io: User auto-provisioned with "tenant" role.`;
      createResult = msg;
      newName = '';
      newEmail = '';
      newPlan = 'free';
      await load();
    } catch (e) {
      createResult = e instanceof Error ? e.message : 'Failed to create client';
    } finally {
      creating = false;
    }
  }

  async function handleRotate(id: string) {
    if (!confirm('Rotate secrets for this client? This will invalidate all existing signed URLs.')) return;
    try {
      const result = await rotateSecrets(id);
      rotateResult[id] = `New HMAC: ${result.hmac_secret}\nNew Encryption: ${result.encryption_key}`;
    } catch (e) {
      rotateResult[id] = e instanceof Error ? e.message : 'Failed to rotate';
    }
  }

  async function handlePlanChange(id: string, plan: string) {
    try {
      await updateTenant(id, { plan });
      await load();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to update';
    }
  }

  async function copyText(text: string, key: string) {
    await navigator.clipboard.writeText(text);
    copied = key;
    setTimeout(() => (copied = ''), 2000);
  }
</script>

<div class="space-y-6">
  <div class="flex items-center justify-between">
    <p class="text-sm text-muted-foreground">Manage platform clients and their secrets.</p>
    <div class="flex items-center gap-2">
      <button
        onclick={load}
        class="p-2 rounded-lg hover:bg-muted transition-colors"
        title="Refresh"
      >
        <RefreshCw class="w-4 h-4 {loading ? 'animate-spin' : ''}" />
      </button>
      <button
        onclick={() => (showCreate = !showCreate)}
        class="flex items-center gap-2 px-3 py-1.5 rounded-lg bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 transition-colors"
      >
        <Plus class="w-4 h-4" />
        Add Client
      </button>
    </div>
  </div>

  <!-- Create form -->
  {#if showCreate}
    <div class="bg-muted/30 border border-border rounded-xl p-5 space-y-4">
      <h3 class="text-sm font-semibold">New Client</h3>
      <div class="grid grid-cols-1 md:grid-cols-3 gap-4">
        <div>
          <label for="client-name" class="block text-xs text-muted-foreground mb-1.5">Name</label>
          <input
            id="client-name"
            bind:value={newName}
            placeholder="e.g. Acme Corp"
            class="w-full bg-background border border-border rounded-lg px-3 py-2 text-sm text-foreground placeholder:text-muted-foreground"
          />
        </div>
        <div>
          <label for="client-email" class="block text-xs text-muted-foreground mb-1.5">Email (for RBAC)</label>
          <input
            id="client-email"
            type="email"
            bind:value={newEmail}
            placeholder="owner@example.com"
            class="w-full bg-background border border-border rounded-lg px-3 py-2 text-sm text-foreground placeholder:text-muted-foreground"
          />
        </div>
        <div>
          <label for="client-plan" class="block text-xs text-muted-foreground mb-1.5">Plan</label>
          <select
            id="client-plan"
            bind:value={newPlan}
            class="w-full bg-background border border-border rounded-lg px-3 py-2 text-sm text-foreground"
          >
            {#each PLANS as plan}
              <option value={plan}>{plan}</option>
            {/each}
          </select>
        </div>
      </div>
      <div class="flex items-center gap-3">
        <button
          onclick={handleCreate}
          disabled={creating || !newName}
          class="px-4 py-2 rounded-lg bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 transition-colors disabled:opacity-50"
        >
          {creating ? 'Creating...' : 'Create'}
        </button>
        <button
          onclick={() => { showCreate = false; createResult = ''; }}
          class="px-4 py-2 rounded-lg border border-border text-sm hover:bg-muted transition-colors"
        >
          Cancel
        </button>
      </div>
      {#if createResult}
        <div class="p-3 rounded-lg bg-warning/10 border border-warning/20 space-y-2">
          <pre class="text-xs font-mono whitespace-pre-wrap break-all text-foreground">{createResult}</pre>
          <button
            onclick={() => copyText(createResult, 'create')}
            class="flex items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground transition-colors"
          >
            {#if copied === 'create'}
              <Check class="w-3 h-3 text-success" /> Copied
            {:else}
              <Copy class="w-3 h-3" /> Copy to clipboard
            {/if}
          </button>
        </div>
      {/if}
    </div>
  {/if}

  <!-- Clients table -->
  <div class="border border-border rounded-xl overflow-hidden">
    {#if loading && tenants.length === 0}
      <div class="p-8 text-center text-muted-foreground text-sm">Loading clients...</div>
    {:else if error}
      <div class="p-4 text-destructive text-sm">{error}</div>
    {:else if tenants.length === 0}
      <div class="p-8 text-center text-muted-foreground text-sm">No clients yet</div>
    {:else}
      <div class="overflow-x-auto">
        <table class="w-full text-sm">
          <thead>
            <tr class="border-b border-border text-muted-foreground">
              <th class="text-left px-5 py-3 font-medium">Name</th>
              <th class="text-left px-5 py-3 font-medium">Email</th>
              <th class="text-left px-5 py-3 font-medium">Key Prefix</th>
              <th class="text-left px-5 py-3 font-medium">Plan</th>
              <th class="text-left px-5 py-3 font-medium">Created</th>
              <th class="text-right px-5 py-3 font-medium">Actions</th>
            </tr>
          </thead>
          <tbody>
            {#each tenants as tenant}
              <tr class="border-b border-border/50 hover:bg-muted/30 transition-colors">
                <td class="px-5 py-3 font-medium">{tenant.name}</td>
                <td class="px-5 py-3 text-xs text-muted-foreground">{tenant.email || '—'}</td>
                <td class="px-5 py-3 font-mono text-xs text-muted-foreground">{tenant.key_prefix}</td>
                <td class="px-5 py-3">
                  <select
                    value={tenant.plan}
                    onchange={(e) => handlePlanChange(tenant.id, (e.target as HTMLSelectElement).value)}
                    class="bg-muted border-none rounded px-2 py-1 text-xs font-medium
                      {tenant.plan === 'enterprise' ? 'text-purple-400' :
                       tenant.plan === 'pro' ? 'text-blue-400' :
                       'text-muted-foreground'}"
                  >
                    {#each PLANS as plan}
                      <option value={plan}>{plan}</option>
                    {/each}
                  </select>
                </td>
                <td class="px-5 py-3 text-muted-foreground text-xs">{formatDate(Number(tenant.created_at))}</td>
                <td class="px-5 py-3 text-right">
                  <button
                    onclick={() => handleRotate(tenant.id)}
                    class="p-1.5 rounded hover:bg-muted transition-colors"
                    title="Rotate secrets"
                  >
                    <RotateCcw class="w-4 h-4 text-warning" />
                  </button>
                </td>
              </tr>
              {#if rotateResult[tenant.id]}
                <tr>
                  <td colspan="6" class="px-5 py-3">
                    <div class="p-3 rounded-lg bg-warning/10 border border-warning/20">
                      <pre class="text-xs font-mono whitespace-pre-wrap break-all">{rotateResult[tenant.id]}</pre>
                      <button
                        onclick={() => copyText(rotateResult[tenant.id], tenant.id)}
                        class="flex items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground mt-2 transition-colors"
                      >
                        {#if copied === tenant.id}
                          <Check class="w-3 h-3 text-success" /> Copied
                        {:else}
                          <Copy class="w-3 h-3" /> Copy
                        {/if}
                      </button>
                    </div>
                  </td>
                </tr>
              {/if}
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </div>
</div>
