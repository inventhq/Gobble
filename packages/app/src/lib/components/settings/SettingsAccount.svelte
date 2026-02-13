<script lang="ts">
  import { Shield, Server, CheckCircle, XCircle, Copy, Check } from 'lucide-svelte';
  import { healthCheck, type HealthResponse } from '$lib/api/health';
  import { getApiKey } from '$lib/api/client';
  import { API_URL } from '$lib/utils/constants';
  import { auth } from '$lib/stores/auth.svelte';

  let health: HealthResponse | null = $state(null);
  let healthError = $state('');
  let checking = $state(false);

  async function checkHealth() {
    checking = true;
    healthError = '';
    try {
      health = await healthCheck();
    } catch (e) {
      healthError = e instanceof Error ? e.message : 'Health check failed';
    } finally {
      checking = false;
    }
  }

  $effect(() => {
    checkHealth();
  });

  const apiKey = getApiKey();
  let copiedKey = $state(false);
  async function copyApiKey() {
    if (!apiKey) return;
    try {
      await navigator.clipboard.writeText(apiKey);
      copiedKey = true;
      setTimeout(() => { copiedKey = false; }, 2000);
    } catch {}
  }
</script>

<div class="space-y-6">
  <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
    <!-- Connection Status -->
    <div class="bg-background border border-border rounded-xl p-5 space-y-4">
      <div class="flex items-center gap-2">
        <Server class="w-4 h-4 text-muted-foreground" />
        <h3 class="text-sm font-semibold">Platform API</h3>
      </div>

      <div class="space-y-3">
        <div class="flex items-center justify-between">
          <span class="text-sm text-muted-foreground">URL</span>
          <code class="text-xs font-mono bg-muted px-2 py-1 rounded">{API_URL}</code>
        </div>
        <div class="flex items-center justify-between">
          <span class="text-sm text-muted-foreground">Status</span>
          {#if checking}
            <span class="text-xs text-muted-foreground">Checking...</span>
          {:else if health}
            <span class="flex items-center gap-1.5 text-xs text-success">
              <CheckCircle class="w-3.5 h-3.5" />
              {health.status}
            </span>
          {:else}
            <span class="flex items-center gap-1.5 text-xs text-destructive">
              <XCircle class="w-3.5 h-3.5" />
              {healthError || 'Unreachable'}
            </span>
          {/if}
        </div>
      </div>

      <button
        onclick={checkHealth}
        disabled={checking}
        class="w-full px-3 py-2 rounded-lg border border-border text-sm hover:bg-muted transition-colors disabled:opacity-50"
      >
        {checking ? 'Checking...' : 'Check Connection'}
      </button>
    </div>

    <!-- Auth Info -->
    <div class="bg-background border border-border rounded-xl p-5 space-y-4">
      <div class="flex items-center gap-2">
        <Shield class="w-4 h-4 text-muted-foreground" />
        <h3 class="text-sm font-semibold">Authentication</h3>
      </div>

      <div class="space-y-3">
        <div class="flex items-center justify-between">
          <span class="text-sm text-muted-foreground">Email</span>
          <span class="text-xs text-foreground">{auth.email || '—'}</span>
        </div>
        <div class="flex items-center justify-between">
          <span class="text-sm text-muted-foreground">Role</span>
          <span class="text-xs text-foreground capitalize">{auth.permissions.role}</span>
        </div>
        <div class="flex items-center justify-between">
          <span class="text-sm text-muted-foreground">API Key</span>
          <div class="flex items-center gap-1.5">
            <code class="text-xs font-mono bg-muted px-2 py-1 rounded max-w-[200px] truncate" title={apiKey || 'Not set'}>
              {apiKey || 'Not set'}
            </code>
            {#if apiKey}
              <button
                onclick={copyApiKey}
                class="p-1 rounded hover:bg-muted transition-colors shrink-0"
                title="Copy API key"
              >
                {#if copiedKey}
                  <Check class="w-3 h-3 text-green-400" />
                {:else}
                  <Copy class="w-3 h-3 text-muted-foreground" />
                {/if}
              </button>
            {/if}
          </div>
        </div>
      </div>
    </div>
  </div>

  <!-- Environment Info -->
  <div class="bg-background border border-border rounded-xl p-5 space-y-4">
    <h3 class="text-sm font-semibold">Environment</h3>
    <div class="grid grid-cols-1 md:grid-cols-3 gap-4 text-sm">
      <div class="flex items-center justify-between p-3 bg-muted/50 rounded-lg">
        <span class="text-muted-foreground">Mode</span>
        <span class="font-mono text-xs">{import.meta.env.MODE}</span>
      </div>
      <div class="flex items-center justify-between p-3 bg-muted/50 rounded-lg">
        <span class="text-muted-foreground">API URL</span>
        <span class="font-mono text-xs">{API_URL}</span>
      </div>
      <div class="flex items-center justify-between p-3 bg-muted/50 rounded-lg">
        <span class="text-muted-foreground">Framework</span>
        <span class="font-mono text-xs">SvelteKit</span>
      </div>
    </div>
  </div>
</div>
