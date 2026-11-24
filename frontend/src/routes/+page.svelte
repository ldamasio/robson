<script lang="ts">
  import { browser } from "$app/env";
  import { goto } from "$app/navigation";
  import { session } from "$stores/auth";

  if (browser) {
    const unsubscribe = session.subscribe((current) => {
      if (!current.initialized) return;
      unsubscribe();
      void goto(current.authenticated ? "/dashboard" : "/login", {
        replaceState: true,
      });
    });
  }
</script>

<svelte:head>
  <title>RBX Robson</title>
  <meta name="robots" content="noindex" />
</svelte:head>

<noscript>
  <p style="padding:2rem;font-family:system-ui;color:#fff;background:#07080A;">
    JavaScript is required to use this application. Please go to <a
      href="/login">/login</a
    >.
  </p>
</noscript>
