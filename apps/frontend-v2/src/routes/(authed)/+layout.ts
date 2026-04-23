import { browser } from '$app/environment';
import { goto } from '$app/navigation';
import { authToken } from '$stores/auth';
import { get } from 'svelte/store';
import type { LayoutLoad } from './$types';

export const load: LayoutLoad = async ({ url }) => {
  if (!browser) return;

  const token = get(authToken);
  if (!token) {
    void goto(`/login?redirect=${encodeURIComponent(url.pathname)}`);
  }
};
