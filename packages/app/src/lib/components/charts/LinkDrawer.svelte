<script lang="ts">
	import { X } from 'lucide-svelte';
	import LinkChart from './LinkChart.svelte';
	import type { TrackingEvent } from '$lib/api/events';

	interface Props {
		tuId: string;
		destination?: string;
		hours?: number;
		liveEvents?: TrackingEvent[];
		sseConnected?: boolean;
		onClose: () => void;
	}

	let { tuId, destination = '', hours = 24, liveEvents = [], sseConnected = false, onClose }: Props = $props();

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') onClose();
	}

	function handleBackdropClick(e: MouseEvent) {
		if (e.target === e.currentTarget) onClose();
	}
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
	class="fixed inset-0 z-50 flex justify-end"
	onclick={handleBackdropClick}
>
	<!-- Backdrop -->
	<div class="absolute inset-0 bg-black/40 backdrop-blur-sm transition-opacity"></div>

	<!-- Drawer panel -->
	<div
		class="relative w-full max-w-2xl bg-background border-l border-border shadow-2xl overflow-y-auto animate-slide-in"
	>
		<!-- Close button -->
		<div class="sticky top-0 z-10 flex items-center justify-between px-5 py-3 bg-background/80 backdrop-blur border-b border-border">
			<div class="min-w-0">
				<p class="text-sm font-semibold font-mono truncate">{tuId}</p>
				{#if destination}
					<p class="text-xs text-muted-foreground truncate">{destination}</p>
				{/if}
			</div>
			<button
				onclick={onClose}
				class="p-1.5 rounded-lg hover:bg-muted transition-colors shrink-0 ml-3"
				title="Close"
			>
				<X class="w-4 h-4 text-muted-foreground" />
			</button>
		</div>

		<!-- Full LinkChart -->
		<div class="p-4">
			<LinkChart
				{tuId}
				{destination}
				{hours}
				{liveEvents}
				{sseConnected}
			/>
		</div>
	</div>
</div>

<style>
	@keyframes slide-in {
		from {
			transform: translateX(100%);
		}
		to {
			transform: translateX(0);
		}
	}

	.animate-slide-in {
		animation: slide-in 200ms ease-out;
	}
</style>
