<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { Activity } from 'lucide-svelte';
	import { getStytchClient, isStytchEnabled } from '$lib/auth/stytch';

	let status = $state('Authenticating...');

	onMount(async () => {
		if (!isStytchEnabled()) {
			goto('/dashboard');
			return;
		}

		const client = getStytchClient();
		if (!client) {
			status = 'Auth client not available';
			return;
		}

		// Extract token from URL params (Stytch magic link callback)
		const token = page.url.searchParams.get('token');
		const tokenType = page.url.searchParams.get('stytch_token_type');

		if (!token || !tokenType) {
			// No token — check if already authenticated
			const session = client.session.getSync();
			if (session) {
				goto('/dashboard');
			} else {
				goto('/login');
			}
			return;
		}

		try {
			if (tokenType === 'magic_links') {
				await client.magicLinks.authenticate(token, { session_duration_minutes: 60 });
			} else if (tokenType === 'oauth') {
				await client.oauth.authenticate(token, { session_duration_minutes: 60 });
			}
			status = 'Success! Redirecting...';
			goto('/dashboard');
		} catch (err) {
			console.error('Stytch authenticate error:', err);
			status = 'Authentication failed. Please try again.';
			setTimeout(() => goto('/login'), 2000);
		}
	});
</script>

<div class="min-h-screen bg-background flex items-center justify-center">
	<div class="w-full max-w-md space-y-8 px-4 text-center">
		<div class="flex items-center justify-center gap-2 mb-4">
			<Activity class="w-8 h-8 text-primary" />
			<span class="text-2xl font-bold">Tracker</span>
		</div>
		<p class="text-sm text-muted-foreground">{status}</p>
	</div>
</div>
