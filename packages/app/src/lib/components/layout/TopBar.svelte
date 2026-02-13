<script lang="ts">
  import { page } from '$app/state';

  function getBreadcrumb(): string {
    const path = page.url.pathname;
    if (path === '/dashboard') return 'Overview';
    if (path.startsWith('/dashboard/events')) return 'Events';
    if (path.startsWith('/dashboard/webhooks')) return 'Webhooks';
    if (path.startsWith('/dashboard/keys')) return 'API Keys';
    if (path.startsWith('/dashboard/tenants')) return 'Tenants';
    if (path.startsWith('/dashboard/settings')) return 'Settings';
    return 'Dashboard';
  }
</script>

<header class="h-14 border-b border-border bg-card/50 backdrop-blur-sm flex items-center justify-between px-6">
  <div class="flex items-center gap-2 text-sm">
    <span class="text-muted-foreground">Dashboard</span>
    <span class="text-muted-foreground">/</span>
    <span class="text-foreground font-medium">{getBreadcrumb()}</span>
  </div>

  <div class="flex items-center gap-4">
    <div class="text-xs text-muted-foreground">
      <span class="inline-block w-2 h-2 rounded-full bg-success mr-1"></span>
      Connected
    </div>
  </div>
</header>
