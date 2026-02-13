<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { Activity } from 'lucide-svelte';
	import { isStytchEnabled, getStytchClient } from '$lib/auth/stytch';

	let mounted = $state(false);
	let stytchReady = $state(false);

	onMount(() => {
		mounted = true;

		if (!isStytchEnabled()) {
			// Mock mode — skip login, go straight to dashboard
			goto('/dashboard');
			return;
		}

		// Mount Stytch UI
		const client = getStytchClient();
		if (client) {
			const container = document.getElementById('stytch-login');
			if (container) {
				client.mountLogin({
					elementId: '#stytch-login',
					config: {
						products: ['emailMagicLinks', 'oauth'],
						emailMagicLinksOptions: {
							loginRedirectURL: window.location.origin + '/authenticate',
							signupRedirectURL: window.location.origin + '/authenticate'
						},
						oauthOptions: {
							providers: [{ type: 'google' }],
							loginRedirectURL: window.location.origin + '/authenticate',
							signupRedirectURL: window.location.origin + '/authenticate'
						}
					},
					styles: {
						container: {
							backgroundColor: '#111118',
							borderColor: '#1e293b',
							borderRadius: '12px'
						},
						inputs: {
							backgroundColor: '#0a0a0f',
							borderColor: '#1e293b',
							textColor: '#f8fafc'
						},
						buttons: {
							primary: {
								backgroundColor: '#6366f1',
								textColor: '#ffffff',
								borderRadius: '8px'
							}
						},
						colors: {
							primary: '#6366f1',
							secondary: '#94a3b8',
							success: '#22c55e',
							error: '#ef4444'
						}
					}
				});
				stytchReady = true;
			}
		}
	});
</script>

<div class="min-h-screen bg-background flex items-center justify-center">
	<div class="w-full max-w-md space-y-8 px-4">
		<div class="text-center">
			<div class="flex items-center justify-center gap-2 mb-4">
				<Activity class="w-8 h-8 text-primary" />
				<span class="text-2xl font-bold">Tracker</span>
			</div>
			<p class="text-sm text-muted-foreground">Sign in to your dashboard</p>
		</div>

		{#if !isStytchEnabled()}
			<div class="bg-card border border-border rounded-xl p-6 text-center space-y-3">
				<p class="text-sm text-muted-foreground">Auth is not configured. Redirecting to dashboard...</p>
			</div>
		{:else}
			<div id="stytch-login" class="min-h-[300px]">
				{#if !stytchReady}
					<div class="bg-card border border-border rounded-xl p-6 text-center">
						<p class="text-sm text-muted-foreground">Loading login...</p>
					</div>
				{/if}
			</div>
		{/if}
	</div>
</div>
