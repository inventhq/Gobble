/**
 * Shared animation manager for SparkCard canvases.
 *
 * One requestAnimationFrame loop drives all registered draw callbacks.
 * Throttled to ~30fps (skip every other frame) to halve CPU cost.
 * Exports a reactive glowPhase so all cards pulse in sync.
 */

let callbacks = new Set<() => void>();
let animId: number | null = null;
let frameCount = 0;

/** Glow phase in radians — incremented each rendered frame (~30fps). */
export let glowPhase = 0;

/** Read current glow phase (for use in draw callbacks). */
export function getGlowPhase(): number {
	return glowPhase;
}

function tick() {
	frameCount++;
	// Skip every other frame → ~30fps
	if (frameCount % 2 === 0) {
		glowPhase += 0.05; // ~1 full cycle per ~4s at 30fps
		for (const cb of callbacks) {
			cb();
		}
	}
	animId = requestAnimationFrame(tick);
}

function startLoop() {
	if (animId !== null) return;
	frameCount = 0;
	animId = requestAnimationFrame(tick);
}

function stopLoop() {
	if (animId !== null) {
		cancelAnimationFrame(animId);
		animId = null;
	}
}

/**
 * Register a draw callback. Returns an unregister function.
 * The loop auto-starts on first register and stops when empty.
 */
export function registerSparkDraw(drawFn: () => void): () => void {
	callbacks.add(drawFn);
	if (callbacks.size === 1) startLoop();

	return () => {
		callbacks.delete(drawFn);
		if (callbacks.size === 0) stopLoop();
	};
}
