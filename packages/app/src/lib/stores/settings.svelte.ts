/**
 * Settings modal state — controls visibility and active section.
 *
 * Uses Svelte 5 runes ($state) for reactivity.
 */

export type SettingsSection = 'account' | 'keys' | 'webhooks' | 'clients';

interface SettingsState {
  open: boolean;
  section: SettingsSection;
}

export const settings: SettingsState = $state({
  open: false,
  section: 'account',
});

export function openSettings(section: SettingsSection = 'account') {
  settings.section = section;
  settings.open = true;
}

export function closeSettings() {
  settings.open = false;
}
