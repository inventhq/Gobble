<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { Radio } from 'lucide-svelte';
	import { truncate } from '$lib/utils/format';
	import { registerSparkDraw, getGlowPhase } from '$lib/utils/sparkAnimation';

	interface Props {
		tuId: string;
		destination?: string;
		liveBuffer?: number[];
		totalClicks?: number;
		totalPostbacks?: number;
		sseConnected?: boolean;
		onSelect?: (tuId: string) => void;
	}

	let {
		tuId,
		destination = '',
		liveBuffer = [],
		totalClicks: propClicks = 0,
		totalPostbacks: propPostbacks = 0,
		sseConnected = false,
		onSelect
	}: Props = $props();

	// --- Display state (reactive, for template) ---
	let clicks = $state(0);
	let postbacks = $state(0);

	// --- Draw state (plain vars, read by rAF draw callback) ---
	let drawBuf: number[] = [];
	let drawPeak = 1;

	// --- Canvas ---
	let canvasEl: HTMLCanvasElement | undefined = $state();
	let unregister: (() => void) | null = null;

	// Sync draw buffer from parent's rolling SSE buffer
	$effect(() => {
		if (liveBuffer.length > 0) {
			drawBuf = [...liveBuffer];
			drawPeak = Math.max(1, ...liveBuffer);
		}
	});

	// Sync totals from parent props
	$effect(() => {
		clicks = propClicks;
		postbacks = propPostbacks;
	});

	// --- Canvas drawing (called by shared animation loop) ---
	function draw() {
		if (!canvasEl) return;
		const ctx = canvasEl.getContext('2d');
		if (!ctx) return;

		const W = canvasEl.width;
		const H = canvasEl.height;
		const dpr = window.devicePixelRatio || 1;
		const numPoints = drawBuf.length;

		ctx.clearRect(0, 0, W, H);

		// Subtle grid lines
		ctx.strokeStyle = 'rgba(255,255,255,0.04)';
		ctx.lineWidth = dpr;
		for (let i = 1; i < 4; i++) {
			const y = (H / 4) * i;
			ctx.beginPath();
			ctx.moveTo(0, y);
			ctx.lineTo(W, y);
			ctx.stroke();
		}

		const glowPhase = getGlowPhase();

		// When buffer is empty or all zeros, draw a flat baseline with the traveling dot
		if (numPoints < 2) {
			// Draw traveling dot along the bottom
			const baseY = H - 4 * dpr;
			const t = (glowPhase * 0.15) % 1;
			const dotX = t * W;
			const dotGrad = ctx.createRadialGradient(dotX, baseY, 0, dotX, baseY, 8 * dpr);
			dotGrad.addColorStop(0, 'rgba(96,165,250,0.5)');
			dotGrad.addColorStop(1, 'rgba(96,165,250,0.0)');
			ctx.fillStyle = dotGrad;
			ctx.beginPath();
			ctx.arc(dotX, baseY, 8 * dpr, 0, Math.PI * 2);
			ctx.fill();
			ctx.fillStyle = 'rgba(180,220,255,0.7)';
			ctx.beginPath();
			ctx.arc(dotX, baseY, 2 * dpr, 0, Math.PI * 2);
			ctx.fill();
			return;
		}

		// Precompute point positions
		const stepX = W / (numPoints - 1);
		const padding = 4 * dpr;
		const drawH = H - padding * 2;
		const points: { x: number; y: number }[] = [];
		for (let i = 0; i < numPoints; i++) {
			points.push({
				x: i * stepX,
				y: padding + drawH - (drawBuf[i] / drawPeak) * drawH
			});
		}

		// Build path
		ctx.beginPath();
		for (let i = 0; i < points.length; i++) {
			if (i === 0) ctx.moveTo(points[i].x, points[i].y);
			else ctx.lineTo(points[i].x, points[i].y);
		}

		// Ambient glow pulse — visible oscillation
		const glowAlpha = 0.15 + 0.25 * Math.sin(glowPhase);
		ctx.strokeStyle = `rgba(96,165,250,${glowAlpha})`;
		ctx.lineWidth = 5 * dpr;
		ctx.lineJoin = 'round';
		ctx.lineCap = 'round';
		ctx.stroke();

		// Main line
		ctx.strokeStyle = '#60a5fa';
		ctx.lineWidth = 1.5 * dpr;
		ctx.stroke();

		// Fill under the line
		const lastPt = points[points.length - 1];
		ctx.lineTo(lastPt.x, H);
		ctx.lineTo(0, H);
		ctx.closePath();
		const grad = ctx.createLinearGradient(0, 0, 0, H);
		const fillAlpha = 0.06 + 0.08 * Math.sin(glowPhase);
		grad.addColorStop(0, `rgba(96,165,250,${fillAlpha})`);
		grad.addColorStop(1, 'rgba(96,165,250,0.0)');
		ctx.fillStyle = grad;
		ctx.fill();

		// --- Traveling light dot ---
		const totalLen = points.length - 1;
		const t = (glowPhase * 0.15) % 1;
		const segFloat = t * totalLen;
		const seg = Math.floor(segFloat);
		const frac = segFloat - seg;
		const i0 = Math.min(seg, totalLen - 1);
		const i1 = Math.min(seg + 1, totalLen);
		const dotX = points[i0].x + (points[i1].x - points[i0].x) * frac;
		const dotY = points[i0].y + (points[i1].y - points[i0].y) * frac;

		// Outer glow
		const dotGrad = ctx.createRadialGradient(dotX, dotY, 0, dotX, dotY, 8 * dpr);
		dotGrad.addColorStop(0, 'rgba(96,165,250,0.6)');
		dotGrad.addColorStop(1, 'rgba(96,165,250,0.0)');
		ctx.fillStyle = dotGrad;
		ctx.beginPath();
		ctx.arc(dotX, dotY, 8 * dpr, 0, Math.PI * 2);
		ctx.fill();

		// Inner bright dot
		ctx.fillStyle = 'rgba(180,220,255,0.9)';
		ctx.beginPath();
		ctx.arc(dotX, dotY, 2 * dpr, 0, Math.PI * 2);
		ctx.fill();
	}

	function setupCanvas() {
		if (!canvasEl) return;
		const dpr = window.devicePixelRatio || 1;
		const rect = canvasEl.getBoundingClientRect();
		canvasEl.width = rect.width * dpr;
		canvasEl.height = rect.height * dpr;
	}

	onMount(() => {
		setupCanvas();
		unregister = registerSparkDraw(draw);
	});

	onDestroy(() => {
		unregister?.();
	});

	function handleClick() {
		onSelect?.(tuId);
	}

	function formatCompact(n: number): string {
		if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + 'M';
		if (n >= 1_000) return (n / 1_000).toFixed(1) + 'K';
		return n.toString();
	}
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
	class="relative bg-card border border-border rounded-lg overflow-hidden cursor-pointer hover:border-primary/50 hover:bg-card/80 transition-all group"
	style="width: 250px; height: 100px;"
	onclick={handleClick}
>
	<!-- Header overlay -->
	<div class="absolute inset-x-0 top-0 z-10 px-3 py-1.5 flex items-center justify-between">
		<div class="min-w-0 flex-1">
			<p class="text-[10px] font-mono text-foreground/80 truncate group-hover:text-foreground transition-colors">
				{truncate(tuId, 20)}
			</p>
			{#if destination}
				<p class="text-[9px] text-muted-foreground truncate">{truncate(destination, 30)}</p>
			{/if}
		</div>
		{#if sseConnected}
			<span class="flex items-center gap-1 shrink-0 ml-1">
				<Radio class="w-2.5 h-2.5 text-green-400" />
				<span class="w-1 h-1 rounded-full bg-green-400 animate-pulse"></span>
			</span>
		{/if}
	</div>

	<!-- Stats overlay (bottom) -->
	<div class="absolute inset-x-0 bottom-0 z-10 px-3 py-1 flex items-center gap-3">
		<span class="text-[9px] text-blue-400 font-mono">{formatCompact(clicks)} clicks</span>
		<span class="text-[9px] text-green-400 font-mono">{formatCompact(postbacks)} pb</span>
	</div>

	<!-- Canvas sparkline (fills entire card) -->
	<canvas
		bind:this={canvasEl}
		class="absolute inset-0 w-full h-full"
	></canvas>
</div>
