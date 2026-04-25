<script lang="ts">
  let { ticks = 12, height = 6, color = 'var(--cyan-dim)', orientation = 'horizontal' }: { ticks?: number; height?: number; color?: string; orientation?: 'horizontal' | 'vertical' } = $props();
</script>

<div
  class="tick-ruler"
  class:vertical={orientation === 'vertical'}
  style:--tr-color={color}
  style:--tr-height="{height}px"
>
  {#each Array(ticks) as _, i (i)}
    <span class="tick"></span>
  {/each}
</div>

<style>
  .tick-ruler {
    display: flex;
    flex-direction: row;
    justify-content: space-between;
    align-items: flex-start;
    border-top: 1px solid var(--tr-color);
    padding-top: 0;
  }
  .tick-ruler.vertical {
    flex-direction: column;
    justify-content: space-between;
    border-top: 0;
    border-left: 1px solid var(--tr-color);
    padding-left: 0;
  }
  .tick {
    width: 1px;
    height: var(--tr-height);
    background: var(--tr-color);
  }
  .tick-ruler.vertical .tick {
    width: var(--tr-height);
    height: 1px;
  }
</style>
