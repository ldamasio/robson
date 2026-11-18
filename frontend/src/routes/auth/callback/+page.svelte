<script lang="ts">
  import { browser } from "$app/env";
  import * as env from "$app/env/public";
  import { goto } from "$app/navigation";
  import { _ } from "svelte-i18n";
  import Card from "$design/components/Card.svelte";
  import Stack from "$design/components/Stack.svelte";
  import { handleWebOidcCallback, resolveOidcConfig } from "#lib/auth/oidc.ts";
  import { clearAuth, setWebOidcToken } from "$stores/auth";
  import { robsonApi } from "$api/robson";

  let error = $state("");

  $effect(() => {
    if (!browser) return;
    let cancelled = false;
    void (async () => {
      try {
        const config = resolveOidcConfig(env);
        if (!config) throw new Error($_("login.notConfigured"));
        const result = await handleWebOidcCallback(
          window.location.href,
          config,
        );
        if (cancelled) return;
        setWebOidcToken(result.accessToken, result.expiresIn);
        await robsonApi.authSession();
        if (!cancelled) void goto(result.returnPath, { replaceState: true });
      } catch (reason) {
        if (cancelled) return;
        clearAuth();
        error =
          reason instanceof Error
            ? reason.message
            : $_("login.connectionFailed");
      }
    })();
    return () => {
      cancelled = true;
    };
  });
</script>

<svelte:head>
  <title>{$_("login.pageTitle")}</title>
</svelte:head>

<div class="callback-page">
  <Card padding={7}>
    <Stack gap={4}>
      <img src="/brand/rbx-mark.svg" alt="RBX" width="48" height="48" />
      {#if error}
        <p class="error">{error}</p>
        <a href="/login">{$_("login.tryAgain")}</a>
      {:else}
        <p>{$_("login.signingIn")}</p>
      {/if}
    </Stack>
  </Card>
</div>

<style>
  .callback-page {
    min-height: 100vh;
    display: grid;
    place-items: center;
    padding: var(--s-5);
  }
  .error {
    color: var(--fg-error, #ff4444);
  }
</style>
