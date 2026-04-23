import type { LayoutServerLoad } from './$types';

// With adapter-static, server load runs only at build time.
// Auth guard is client-side in +layout.ts.
// This file kept for type consistency; always returns null session.
export const load: LayoutServerLoad = async () => {
  return { session: null };
};

export const prerender = true;
