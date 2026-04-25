<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { authToken } from '$stores/auth';
  import { get } from 'svelte/store';

  onMount(() => {
    const token = get(authToken);
    void goto(token ? '/dashboard' : '/login', { replaceState: true });
  });
</script>

<svelte:head>
  <title>RBX Robson</title>
  <meta name="robots" content="noindex" />
</svelte:head>

<noscript>
  <p style="padding:2rem;font-family:system-ui;color:#fff;background:#07080A;">
    JavaScript is required to use this application.
    Please go to <a href="/login">/login</a>.
  </p>
</noscript>
