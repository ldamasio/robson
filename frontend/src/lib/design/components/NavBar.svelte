<script lang="ts">
  import { page } from '$app/stores';
  import { status, startStatusPolling, stopStatusPolling } from '$stores/status';
  import Row from './Row.svelte';

  $effect(() => {
    startStatusPolling();
    return () => stopStatusPolling();
  });

  let currentPath = $derived($page.url.pathname);

  function isActive(href: string): boolean {
    if (href === '/') return currentPath === '/';
    return currentPath.startsWith(href);
  }
</script>

<nav class="navbar">
  <Row justify="between" align="center">
    <Row gap={3} align="center">
      <a href="/" class="brand" aria-label="Dashboard">
        <img src="/brand/rbx-mark.svg" alt="" width={24} height={24} />
        <img src="/brand/wordmark-robson.svg" alt="Robson" height={16} />
      </a>
      <div class="nav-links">
        <a href="/" class="nav-link" class:active={isActive('/')}>Dashboard</a>
        <a href="/funding" class="nav-link" class:active={isActive('/funding')}
          >Tesouraria</a
        >
      </div>
    </Row>
    <Row gap={4} align="center">
      {#if $status !== null}
        <span class="capital" title="USDT-M futures wallet">
          <span class="capital-label">FUTURES</span>
          <span class="capital-value">{($status.wallet_balance).toFixed(2)}</span>
          <span class="capital-unit">USDT</span>
        </span>
      {/if}
    </Row>
  </Row>
</nav>

<style>
  .navbar {
    position: sticky;
    top: 0;
    z-index: 100;
    background: var(--bg-1);
    border-bottom: 1px solid var(--border);
    padding: var(--s-2) var(--s-5);
    height: var(--header-h);
    display: flex;
    align-items: center;
    box-sizing: border-box;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: var(--s-2);
    text-decoration: none;
    border-bottom: none;
  }

  .brand:hover {
    border-bottom: none;
  }

  .nav-links {
    display: flex;
    gap: var(--s-1);
    margin-left: var(--s-4);
  }

  .nav-link {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--fg-2);
    text-decoration: none;
    padding: var(--s-1) var(--s-2);
    border-bottom: 1px solid transparent;
    transition: color var(--dur) var(--ease),
      border-color var(--dur) var(--ease);
  }

  .nav-link:hover {
    color: var(--cyan-brand);
    border-bottom-color: var(--cyan-dim);
  }

  .nav-link.active {
    color: var(--cyan-brand);
  }

  .capital {
    display: flex;
    align-items: baseline;
    gap: var(--s-1);
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
  }

  .capital-label {
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--fg-3);
  }

  .capital-value {
    font-size: var(--text-sm);
    font-weight: 500;
    color: var(--fg-0);
    letter-spacing: var(--track-wide);
  }

  .capital-unit {
    font-size: var(--text-xs);
    color: var(--fg-2);
  }
</style>
