<script lang="ts">
	import { MessageCircle, X, Send, Loader2, Sparkles } from 'lucide-svelte';
	import { sendChatMessage, type ChatMessage } from '$lib/api/chat';
	import { tick } from 'svelte';

	let open = $state(false);
	let input = $state('');
	let loading = $state(false);
	let messages = $state<ChatMessage[]>([]);
	let messagesEl: HTMLDivElement | undefined = $state();

	async function scrollToBottom() {
		await tick();
		if (messagesEl) {
			messagesEl.scrollTop = messagesEl.scrollHeight;
		}
	}

	async function handleSend() {
		const text = input.trim();
		if (!text || loading) return;

		input = '';
		messages.push({ role: 'user', content: text });
		await scrollToBottom();

		loading = true;
		try {
			const history = messages.slice(0, -1);
			const resp = await sendChatMessage(text, history);
			if (resp.error) {
				messages.push({ role: 'assistant', content: `Error: ${resp.error}` });
			} else {
				messages.push({ role: 'assistant', content: resp.response });
			}
		} catch (e) {
			messages.push({
				role: 'assistant',
				content: `Error: ${e instanceof Error ? e.message : 'Request failed'}`
			});
		} finally {
			loading = false;
			await scrollToBottom();
		}
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey) {
			e.preventDefault();
			handleSend();
		}
	}

	function clearChat() {
		messages = [];
	}

	const SUGGESTIONS = [
		'How many clicks in the last 24 hours?',
		'What are my top traffic sources?',
		'Show me my conversion rate',
		'Break down clicks by geo'
	];
</script>

<!-- Floating toggle button -->
{#if !open}
	<button
		onclick={() => (open = true)}
		class="fixed bottom-6 right-6 z-50 flex items-center justify-center w-14 h-14 rounded-full bg-primary text-primary-foreground shadow-lg shadow-primary/25 hover:bg-primary/90 hover:scale-105 transition-all duration-200"
		title="Ask AI"
	>
		<Sparkles class="w-6 h-6" />
	</button>
{/if}

<!-- Chat panel -->
{#if open}
	<div class="fixed bottom-6 right-6 z-50 w-[420px] max-h-[600px] flex flex-col bg-card border border-border rounded-2xl shadow-2xl shadow-black/40 overflow-hidden">
		<!-- Header -->
		<div class="flex items-center justify-between px-4 py-3 border-b border-border bg-card">
			<div class="flex items-center gap-2">
				<Sparkles class="w-4 h-4 text-primary" />
				<span class="text-sm font-semibold">AI Assistant</span>
				<span class="text-[10px] px-1.5 py-0.5 rounded-full bg-primary/10 text-primary font-medium">Vivgrid</span>
			</div>
			<div class="flex items-center gap-1">
				{#if messages.length > 0}
					<button
						onclick={clearChat}
						class="p-1.5 rounded-lg hover:bg-muted transition-colors text-muted-foreground hover:text-foreground text-xs"
						title="Clear chat"
					>
						Clear
					</button>
				{/if}
				<button
					onclick={() => (open = false)}
					class="p-1.5 rounded-lg hover:bg-muted transition-colors text-muted-foreground hover:text-foreground"
					title="Close"
				>
					<X class="w-4 h-4" />
				</button>
			</div>
		</div>

		<!-- Messages -->
		<div bind:this={messagesEl} class="flex-1 overflow-y-auto p-4 space-y-3 min-h-[300px] max-h-[440px]">
			{#if messages.length === 0}
				<!-- Empty state with suggestions -->
				<div class="flex flex-col items-center justify-center h-full gap-4 py-6">
					<div class="flex items-center justify-center w-10 h-10 rounded-full bg-primary/10">
						<Sparkles class="w-5 h-5 text-primary" />
					</div>
					<div class="text-center">
						<p class="text-sm font-medium text-foreground">Ask anything about your data</p>
						<p class="text-xs text-muted-foreground mt-1">Powered by Vivgrid AI with live platform data</p>
					</div>
					<div class="w-full space-y-2 mt-2">
						{#each SUGGESTIONS as suggestion}
							<button
								onclick={() => { input = suggestion; handleSend(); }}
								class="w-full text-left px-3 py-2 rounded-lg border border-border/50 text-xs text-muted-foreground hover:text-foreground hover:bg-muted/50 hover:border-border transition-colors"
							>
								{suggestion}
							</button>
						{/each}
					</div>
				</div>
			{:else}
				{#each messages as msg, i}
					<div class="flex {msg.role === 'user' ? 'justify-end' : 'justify-start'}">
						<div
							class="max-w-[85%] px-3 py-2 rounded-xl text-sm leading-relaxed
								{msg.role === 'user'
									? 'bg-primary text-primary-foreground rounded-br-sm'
									: 'bg-muted text-foreground rounded-bl-sm'}"
						>
							{#if msg.role === 'assistant'}
								<div class="chat-response whitespace-pre-wrap">{msg.content}</div>
							{:else}
								{msg.content}
							{/if}
						</div>
					</div>
				{/each}
				{#if loading}
					<div class="flex justify-start">
						<div class="bg-muted rounded-xl rounded-bl-sm px-3 py-2 flex items-center gap-2">
							<Loader2 class="w-3.5 h-3.5 text-primary animate-spin" />
							<span class="text-xs text-muted-foreground">Thinking...</span>
						</div>
					</div>
				{/if}
			{/if}
		</div>

		<!-- Input -->
		<div class="border-t border-border p-3">
			<div class="flex items-end gap-2">
				<textarea
					bind:value={input}
					onkeydown={handleKeydown}
					placeholder="Ask about your data..."
					rows={1}
					disabled={loading}
					class="flex-1 resize-none bg-muted/50 border border-border rounded-xl px-3 py-2.5 text-sm text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-primary/50 focus:border-primary/50 disabled:opacity-50 max-h-[80px]"
				></textarea>
				<button
					onclick={handleSend}
					disabled={loading || !input.trim()}
					class="flex items-center justify-center w-9 h-9 rounded-xl bg-primary text-primary-foreground hover:bg-primary/90 transition-colors disabled:opacity-40 disabled:hover:bg-primary shrink-0"
					title="Send"
				>
					{#if loading}
						<Loader2 class="w-4 h-4 animate-spin" />
					{:else}
						<Send class="w-4 h-4" />
					{/if}
				</button>
			</div>
		</div>
	</div>
{/if}

<style>
	.chat-response :global(strong) {
		font-weight: 600;
	}
	.chat-response :global(code) {
		font-family: ui-monospace, monospace;
		font-size: 0.8em;
		background: rgba(99, 102, 241, 0.1);
		padding: 0.1em 0.3em;
		border-radius: 0.25em;
	}
</style>
