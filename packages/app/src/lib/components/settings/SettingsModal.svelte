<script lang="ts">
  import { X, Key, Webhook, User, Users, LogOut } from 'lucide-svelte';
  import { settings, closeSettings, type SettingsSection } from '$lib/stores/settings.svelte';
  import { auth } from '$lib/stores/auth.svelte';
  import { logout, isStytchEnabled } from '$lib/auth/stytch';
  import { goto } from '$app/navigation';
  import SettingsAccount from './SettingsAccount.svelte';
  import SettingsKeys from './SettingsKeys.svelte';
  import SettingsWebhooks from './SettingsWebhooks.svelte';
  import SettingsClients from './SettingsClients.svelte';

  const allSections: { id: SettingsSection; label: string; icon: any; adminOnly?: boolean }[] = [
    { id: 'account', label: 'Account', icon: User },
    { id: 'keys', label: 'API Keys', icon: Key },
    { id: 'webhooks', label: 'Webhooks', icon: Webhook },
    { id: 'clients', label: 'Clients', icon: Users, adminOnly: true },
  ];

  function visibleSections() {
    return allSections.filter((s) => !s.adminOnly || auth.permissions.canManageTenants);
  }

  function sectionTitle(): string {
    return allSections.find((s) => s.id === settings.section)?.label ?? 'Settings';
  }

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) {
      closeSettings();
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      closeSettings();
    }
  }

  async function handleLogout() {
    closeSettings();
    await logout();
    goto('/login');
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if settings.open}
  <!-- Backdrop -->
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div
    class="fixed inset-0 z-50 bg-background/80 backdrop-blur-sm flex items-center justify-center"
    onclick={handleBackdropClick}
  >
    <!-- Modal -->
    <div class="w-full h-full max-w-5xl max-h-[90vh] my-auto flex bg-card rounded-2xl border border-border shadow-2xl overflow-hidden">
      <!-- Side Nav -->
      <nav class="w-56 shrink-0 bg-sidebar border-r border-border flex flex-col py-4">
        <div class="px-5 pb-4 border-b border-border mb-2">
          <h2 class="text-sm font-bold text-foreground">Settings</h2>
        </div>

        <div class="flex-1 px-3 space-y-1">
          {#each visibleSections() as sec}
            <button
              onclick={() => (settings.section = sec.id)}
              class="flex items-center gap-3 w-full px-3 py-2.5 rounded-lg text-sm font-medium transition-colors
                {settings.section === sec.id
                  ? 'bg-primary/10 text-primary'
                  : 'text-sidebar-foreground hover:text-foreground hover:bg-muted'}"
            >
              <sec.icon class="w-4 h-4" />
              {sec.label}
            </button>
          {/each}
        </div>

        <!-- Sign Out -->
        <div class="px-3 pt-2 border-t border-border">
          {#if auth.authenticated}
            <div class="flex items-center gap-2 px-3 py-2 mb-2">
              <div class="flex-1 min-w-0">
                <p class="text-xs font-medium text-foreground truncate">{auth.email || 'User'}</p>
                <p class="text-[10px] text-muted-foreground capitalize">{auth.permissions.role}</p>
              </div>
            </div>
          {/if}
          {#if isStytchEnabled()}
            <button
              onclick={handleLogout}
              class="flex items-center gap-3 w-full px-3 py-2.5 rounded-lg text-sm font-medium text-destructive hover:bg-destructive/10 transition-colors"
            >
              <LogOut class="w-4 h-4" />
              Sign Out
            </button>
          {/if}
        </div>
      </nav>

      <!-- Content -->
      <div class="flex-1 flex flex-col min-w-0">
        <!-- Header -->
        <div class="flex items-center justify-between px-8 py-5 border-b border-border">
          <h2 class="text-lg font-bold">{sectionTitle()}</h2>
          <button
            onclick={closeSettings}
            class="p-2 rounded-lg hover:bg-muted transition-colors"
            title="Close settings"
          >
            <X class="w-5 h-5 text-muted-foreground" />
          </button>
        </div>

        <!-- Section content -->
        <div class="flex-1 overflow-y-auto px-8 py-6">
          {#if settings.section === 'account'}
            <SettingsAccount />
          {:else if settings.section === 'keys'}
            <SettingsKeys />
          {:else if settings.section === 'webhooks'}
            <SettingsWebhooks />
          {:else if settings.section === 'clients'}
            <SettingsClients />
          {/if}
        </div>
      </div>
    </div>
  </div>
{/if}
