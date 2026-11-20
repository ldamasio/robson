import { defineEnvVars } from '@sveltejs/kit/env';

// ADR-0054: retain optional dynamic public values for the static deployment.
// Missing configuration keeps the existing empty-string defaults.
export const variables = defineEnvVars({
  PUBLIC_ROBSON_API_BASE: {
    public: true,
    static: false,
    schema: (value: string | undefined) => value ?? ''
  },
  PUBLIC_ROBSON_WEB_AUTH_MODE: {
    public: true,
    static: false,
    schema: (value: string | undefined) => value ?? ''
  },
  PUBLIC_RBX_BFF_LOGIN_URL: {
    public: true,
    static: false,
    schema: (value: string | undefined) => value ?? ''
  },
  PUBLIC_RBX_OIDC_ISSUER: {
    public: true,
    static: false,
    schema: (value: string | undefined) => value ?? ''
  },
  PUBLIC_RBX_OIDC_CLIENT_ID: {
    public: true,
    static: false,
    schema: (value: string | undefined) => value ?? ''
  },
  PUBLIC_RBX_OIDC_REDIRECT_URI: {
    public: true,
    static: false,
    schema: (value: string | undefined) => value ?? ''
  },
  PUBLIC_RBX_OIDC_SCOPES: {
    public: true,
    static: false,
    schema: (value: string | undefined) => value ?? ''
  },
  PUBLIC_RBX_OIDC_ORG_ID: {
    public: true,
    static: false,
    schema: (value: string | undefined) => value ?? ''
  },
  PUBLIC_RBX_OIDC_PROJECT_ID: {
    public: true,
    static: false,
    schema: (value: string | undefined) => value ?? ''
  },
  PUBLIC_RBX_OIDC_GOOGLE_IDP_ID: {
    public: true,
    static: false,
    schema: (value: string | undefined) => value ?? ''
  }
});
