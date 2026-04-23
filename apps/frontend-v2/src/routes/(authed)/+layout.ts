import { browser } from '$app/environment';
import { goto } from '$app/navigation';
import { authToken } from '$stores/auth';
import { get } from 'svelte/store';
import type { LayoutLoad } from './$types';

const STORAGE_KEY = 'robson_api_token';

export const load: LayoutLoad = async ({ url }) => {
  if (!browser) return;

  const storeToken = get(authToken);
  if (storeToken) return;

  const stored = sessionStorage.getItem(STORAGE_KEY);
  if (!stored) {
    void goto(`/login?redirect=${encodeURIComponent(url.pathname)}`);
  }
};
