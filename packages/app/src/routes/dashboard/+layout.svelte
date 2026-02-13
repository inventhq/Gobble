<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import Sidebar from '$lib/components/layout/Sidebar.svelte';
	import TopBar from '$lib/components/layout/TopBar.svelte';
	import SettingsModal from '$lib/components/settings/SettingsModal.svelte';
	import ChatBubble from '$lib/components/ChatBubble.svelte';
	import { auth, initAuth } from '$lib/stores/auth.svelte';
	import { isStytchEnabled } from '$lib/auth/stytch';

	let { children } = $props();

	onMount(async () => {
		await initAuth();
		if (isStytchEnabled() && !auth.authenticated) {
			goto('/login');
		}
	});
</script>

{#if !auth.initialized}
	<div class="min-h-screen bg-background flex items-center justify-center">
		<p class="text-sm text-muted-foreground">Loading...</p>
	</div>
{:else}
	<div class="flex min-h-screen">
		<Sidebar />
		<div class="flex-1 ml-14 flex flex-col">
			<TopBar />
			<main class="flex-1 p-6">
				{@render children()}
			</main>
		</div>
	</div>
	<SettingsModal />
	<ChatBubble />
{/if}
