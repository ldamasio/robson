<script lang="ts">
  import { goto } from "$app/navigation";
  import { onMount } from "svelte";
  import { env } from "$env/dynamic/public";
  import Card from "$design/components/Card.svelte";
  import Stack from "$design/components/Stack.svelte";
  import { clearAuth, setToken } from "$stores/auth";
  import { robsonApi } from "$api/robson";
  import { resolveClientMode } from "$lib/config/clientMode";
  import {
    beginOidcLogin,
    handleOidcCallback,
    isOidcCallback,
    resolveOidcConfig,
  } from "$lib/auth/oidc";
  import { _ } from "svelte-i18n";

  let tokenInput = $state("");
  let error = $state("");
  let loading = $state(false);
  let callbackHandled = false;
  const allowLegacyLogin = env.PUBLIC_ROBSON_ALLOW_LEGACY_LOGIN
    ? env.PUBLIC_ROBSON_ALLOW_LEGACY_LOGIN === "true"
    : resolveClientMode(env.PUBLIC_ROBSON_CLIENT_MODE) === "operator";
  const identityConfiguration = (() => {
    try {
      return { oidcConfig: resolveOidcConfig(env), configurationError: "" };
    } catch (e) {
      return {
        oidcConfig: null,
        configurationError:
          e instanceof Error
            ? e.message
            : "RBX Identity configuration is invalid",
      };
    }
  })();
  const { oidcConfig, configurationError } = identityConfiguration;

  function returnPath(): string {
    const path = new URLSearchParams(window.location.search).get("redirect");
    return path?.startsWith("/") && !path.startsWith("//")
      ? path
      : "/dashboard";
  }

  async function completeOidcLogin(url: string) {
    if (!oidcConfig || callbackHandled) return;
    callbackHandled = true;
    error = "";
    loading = true;
    try {
      const result = await handleOidcCallback(url, oidcConfig);
      setToken(result.accessToken, "oidc", result.expiresIn);
      await robsonApi.authSession();
      const { Capacitor } = await import("@capacitor/core");
      if (Capacitor.isNativePlatform()) {
        const { Browser } = await import("@capacitor/browser");
        await Browser.close().catch(() => {});
      }
      void goto(result.returnPath, { replaceState: true });
    } catch (e) {
      clearAuth();
      error =
        e instanceof Error && e.message
          ? e.message
          : $_("login.connectionFailed");
    } finally {
      loading = false;
    }
  }

  async function handleRbxLogin() {
    if (!oidcConfig) return;
    callbackHandled = false;
    error = "";
    loading = true;
    try {
      await beginOidcLogin(oidcConfig, returnPath());
      loading = false;
    } catch (e) {
      error =
        e instanceof Error && e.message
          ? e.message
          : $_("login.connectionFailed");
      loading = false;
    }
  }

  async function handleLogin() {
    error = "";
    loading = true;
    try {
      setToken(tokenInput.trim(), "legacy");
      await robsonApi.authSession();
      const redirect = returnPath();
      void goto(redirect);
    } catch (e) {
      clearAuth();
      error =
        e instanceof Error && e.message
          ? e.message
          : $_("login.connectionFailed");
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    let removeListener: (() => Promise<void>) | undefined;
    if (
      oidcConfig &&
      isOidcCallback(window.location.href, oidcConfig.redirectUri)
    ) {
      void completeOidcLogin(window.location.href);
    }
    if (oidcConfig) {
      void import("@capacitor/app").then(async ({ App }) => {
        const listener = await App.addListener("appUrlOpen", ({ url }) => {
          if (isOidcCallback(url, oidcConfig.redirectUri)) {
            void completeOidcLogin(url);
          }
        });
        removeListener = () => listener.remove();
        const launch = await App.getLaunchUrl();
        if (launch?.url && isOidcCallback(launch.url, oidcConfig.redirectUri)) {
          void completeOidcLogin(launch.url);
        }
      });
    }
    return () => {
      void removeListener?.();
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
      <p>{$_("login.ssoDescription")}</p>
      {#if oidcConfig}
        <button
          class="btn-primary"
          type="button"
          disabled={loading}
          onclick={handleRbxLogin}
        >
          {loading ? $_("login.connecting") : $_("login.continueWithRbx")}
        </button>
        <p class="provider-note">{$_("login.googleAvailable")}</p>
      {/if}
      {#if allowLegacyLogin}
        <form
          onsubmit={(e) => {
            e.preventDefault();
            handleLogin();
          }}
        >
          <Stack gap={3}>
            <input
              type="password"
              bind:value={tokenInput}
              placeholder={$_("login.tokenPlaceholder")}
              autocomplete="off"
              disabled={loading}
            />
            <button
              class="btn-secondary"
              type="submit"
              disabled={!tokenInput.trim() || loading}
            >
              {$_("login.legacyConnect")}
            </button>
          </Stack>
        </form>
      {/if}
      {#if configurationError || (!oidcConfig && !allowLegacyLogin)}
        <p class="error">{configurationError || $_("login.notConfigured")}</p>
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
  input {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    width: 100%;
    padding: var(--s-3);
    background: var(--bg-1);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    color: var(--fg-0);
  }
  input:focus {
    border-color: var(--cyan-brand);
    outline: none;
  }
  .error {
    color: var(--fg-error, #ff4444);
    font-size: var(--text-sm);
  }
  .provider-note {
    font-size: var(--text-xs);
    text-align: center;
  }
  .btn-primary {
    font-family: var(--font-sans);
    font-size: var(--text-base);
    font-weight: 500;
    padding: var(--s-3) var(--s-5);
    border: 1px solid var(--cyan-signal);
    background: var(--cyan-signal);
    color: var(--bg-0);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition:
      background var(--dur) var(--ease),
      border-color var(--dur) var(--ease);
  }
  .btn-primary:hover:not(:disabled) {
    background: var(--cyan-brand);
    border-color: var(--cyan-brand);
  }
  .btn-primary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .btn-secondary {
    font-family: var(--font-sans);
    padding: var(--s-3) var(--s-5);
    border: 1px solid var(--border-strong);
    background: transparent;
    color: var(--fg-1);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .btn-secondary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
