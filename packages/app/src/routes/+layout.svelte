<script lang="ts">
	import './layout.css';
	import { setApiKey } from '$lib/api/client';

	setApiKey(import.meta.env.VITE_API_KEY || '');

	let { children } = $props();
</script>

<svelte:head>
	<title>Tracker Dashboard</title>
	<link rel="preconnect" href="https://fonts.googleapis.com" />
	<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin="anonymous" />
	<link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap" rel="stylesheet" />
</svelte:head>
{@render children()}
