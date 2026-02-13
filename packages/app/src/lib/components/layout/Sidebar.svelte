<script lang="ts">
  import { page } from '$app/state';
  import { Activity, Home, Archive, Settings } from 'lucide-svelte';
  import { openSettings } from '$lib/stores/settings.svelte';

  function isActive(href: string): boolean {
    if (href === '/dashboard') return page.url.pathname === '/dashboard';
    return page.url.pathname.startsWith(href);
  }
</script>

<aside class="fixed left-0 top-0 h-screen w-14 bg-sidebar border-r border-border flex flex-col items-center">
  <!-- Logo -->
  <div class="py-3 border-b border-border w-full flex justify-center">
    <a href="/dashboard" class="flex items-center justify-center w-9 h-9 rounded-lg hover:bg-muted transition-colors" title="Tracker">
      <Activity class="w-5 h-5 text-primary" />
    </a>
  </div>

  <!-- Nav -->
  <nav class="flex-1 py-3 flex flex-col items-center gap-1">
    <a
      href="/dashboard"
      class="flex items-center justify-center w-9 h-9 rounded-lg transition-colors
        {isActive('/dashboard')
          ? 'bg-primary/10 text-primary'
          : 'text-sidebar-foreground hover:text-foreground hover:bg-muted'}"
      title="Home"
    >
      <Home class="w-5 h-5" />
    </a>
    <a
      href="/dashboard/history"
      class="flex items-center justify-center w-9 h-9 rounded-lg transition-colors
        {isActive('/dashboard/history')
          ? 'bg-primary/10 text-primary'
          : 'text-sidebar-foreground hover:text-foreground hover:bg-muted'}"
      title="History"
    >
      <Archive class="w-5 h-5" />
    </a>
  </nav>

  <!-- Settings -->
  <div class="py-3 border-t border-border w-full flex justify-center">
    <button
      onclick={() => openSettings()}
      class="flex items-center justify-center w-9 h-9 rounded-lg hover:bg-muted transition-colors text-muted-foreground hover:text-foreground"
      title="Settings"
    >
      <Settings class="w-5 h-5" />
    </button>
  </div>
</aside>
