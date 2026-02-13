<script lang="ts">
  import { formatNumber } from '$lib/utils/format';

  let { title, value, icon: Icon, color = 'text-primary' }: {
    title: string;
    value: number;
    icon: any;
    color?: string;
  } = $props();
</script>

<div class="bg-card border border-border rounded-xl p-5 flex items-start justify-between">
  <div>
    <p class="text-sm text-muted-foreground">{title}</p>
    <p class="text-3xl font-bold mt-1">{formatNumber(value)}</p>
  </div>
  <div class="p-2.5 rounded-lg bg-muted {color}">
    <Icon class="w-5 h-5" />
  </div>
</div>
