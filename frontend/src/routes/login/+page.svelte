<script lang="ts">
  import { browser } from "$app/env";
  import * as env from "$app/env/public";
  import { goto } from "$app/navigation";
  import { _ } from "svelte-i18n";
  import Card from "$design/components/Card.svelte";
  import Stack from "$design/components/Stack.svelte";
  import {
    beginNativeOidcLogin,
    isNativeRuntime,
    onNativeAuthError,
    onNativeAuthSession,
  } from "#lib/auth/native.ts";
  import {
    beginWebOidcLogin,
    resolveOidcConfig,
    sanitizeReturnPath,
  } from "#lib/auth/oidc.ts";
  import { webAuthMode } from "$stores/auth";

  let error = $state("");
  let loading = $state(false);

  const identityConfiguration = (() => {
    try {
      return { config: resolveOidcConfig(env), invalid: false };
    } catch {
      return { config: null, invalid: true };
    }
  })();
  const native = browser && isNativeRuntime();
  const usesBff = !native && webAuthMode() === "bff";
  const configured = usesBff || Boolean(identityConfiguration.config);

  function returnPath(): string {
    if (!browser) return "/dashboard";
    return sanitizeReturnPath(
      new URLSearchParams(window.location.search).get("redirect") ??
        "/dashboard",
    );
  }

  function bffLoginUrl(): string {
    const configuredUrl = env.PUBLIC_RBX_BFF_LOGIN_URL?.trim();
    const base = env.PUBLIC_ROBSON_API_BASE?.trim() || window.location.origin;
    const url = new URL(configuredUrl || "/oidc/login", base);
    url.searchParams.set("return_to", returnPath());
    return url.toString();
  }

  async function handleLogin(): Promise<void> {
    error = "";
    loading = true;
    try {
      if (usesBff) {
        window.location.assign(bffLoginUrl());
        return;
      }
      const config = identityConfiguration.config;
      if (!config) throw new Error($_("login.notConfigured"));
      if (native) {
        await beginNativeOidcLogin(
          { ...config, apiBase: env.PUBLIC_ROBSON_API_BASE ?? "" },
          returnPath(),
        );
        return;
      }
      await beginWebOidcLogin(config, returnPath());
    } catch (reason) {
      loading = false;
      error =
        reason instanceof Error ? reason.message : $_("login.connectionFailed");
    }
  }

  $effect(() => {
    if (!native) return;
    let cancelled = false;
    let cleanupSession: (() => void) | undefined;
    let cleanupError: (() => void) | undefined;
    void onNativeAuthSession((nativeSession) => {
      if (cancelled || !nativeSession.authenticated) return;
      loading = false;
      void goto(nativeSession.returnPath ?? "/dashboard", {
        replaceState: true,
      });
    }).then((cleanup) => {
      if (cancelled) cleanup();
      else cleanupSession = cleanup;
    });
    void onNativeAuthError((message) => {
      if (cancelled) return;
      loading = false;
      error = message || $_("login.connectionFailed");
    }).then((cleanup) => {
      if (cancelled) cleanup();
      else cleanupError = cleanup;
    });
    return () => {
      cancelled = true;
      cleanupSession?.();
      cleanupError?.();
    };
  });
</script>

<svelte:head>
  <title>{$_("login.pageTitle")}</title>
</svelte:head>

<div class="login-page">
  <Card padding={7}>
    <Stack gap={5}>
      <img src="/brand/rbx-mark.svg" alt="RBX" width="48" height="48" />
      <h1>{$_("login.title")}</h1>
      <p>{$_("login.description")}</p>
      <button
        class="google-button"
        type="button"
        disabled={loading || !configured}
        onclick={handleLogin}
      >
        {loading ? $_("login.signingIn") : $_("login.signIn")}
      </button>
      <p class="provider-note">{$_("login.identityProvider")}</p>
      {#if identityConfiguration.invalid || !configured}
        <p class="error">{$_("login.notConfigured")}</p>
      {/if}
      {#if error}
        <p class="error">{error}</p>
      {/if}
    </Stack>
  </Card>
</div>

<style>
  .login-page {
    min-height: 100vh;
    display: grid;
    place-items: center;
    padding: var(--s-5);
  }
  h1 {
    font-size: var(--text-3xl);
    font-weight: 300;
  }
  p {
    color: var(--fg-1);
  }
  .google-button {
    min-height: 44px;
    width: 100%;
    border: 1px solid var(--cyan-signal);
    border-radius: var(--radius-sm);
    background: var(--cyan-signal);
    color: var(--bg-0);
    cursor: pointer;
    font: 500 var(--text-base) var(--font-sans);
    padding: var(--s-3) var(--s-5);
  }
  .google-button:hover:not(:disabled) {
    background: var(--cyan-brand);
    border-color: var(--cyan-brand);
  }
  .google-button:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }
  .provider-note {
    font-size: var(--text-xs);
    text-align: center;
  }
  .error {
    color: var(--fg-error, #ff4444);
    font-size: var(--text-sm);
  }
</style>
