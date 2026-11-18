import { defineEnvVars } from '@sveltejs/kit/env';

// ADR-0054: retain optional dynamic public values for the static deployment.
// Missing configuration keeps the existing empty-string defaults.
export const variables = defineEnvVars({
  PUBLIC_ROBSON_API_BASE: {
    public: true,
    static: false,
    schema: (value: string | undefined) => value ?? ''
  },
  PUBLIC_GOOGLE_WEB_CLIENT_ID: {
    public: true,
    static: false,
    schema: (value: string | undefined) => value ?? ''
  }
});
