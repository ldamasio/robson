# Stage 1: build
# Pinned by digest; Vite 7 needs Node >= 20.19 and engine-strict in .npmrc
# makes pnpm fail loudly if the runtime ever drifts below it.
FROM docker.io/library/node:20.20-alpine@sha256:fb4cd12c85ee03686f6af5362a0b0d56d50c58a04632e6c0fb8363f609372293 AS build

WORKDIR /app

COPY package.json pnpm-lock.yaml .npmrc ./
RUN corepack enable && corepack prepare pnpm@9 --activate
RUN pnpm install --frozen-lockfile

COPY . .

ARG PUBLIC_ROBSON_API_BASE
ENV PUBLIC_ROBSON_API_BASE=$PUBLIC_ROBSON_API_BASE
# ADR-0054: Google OAuth Client ID (Web application type) for Google
# Identity Services. Not secret — GIS client IDs are meant to be visible
# in the browser — but still build-time-baked like PUBLIC_ROBSON_API_BASE
# above, since this is a static site with no runtime server.
ARG PUBLIC_GOOGLE_WEB_CLIENT_ID
ENV PUBLIC_GOOGLE_WEB_CLIENT_ID=$PUBLIC_GOOGLE_WEB_CLIENT_ID

RUN pnpm run build

# Stage 2: runtime
FROM docker.io/library/nginx:1.27-alpine@sha256:65645c7bb6a0661892a8b03b89d0743208a18dd2f3f17a54ef4b76fb8e2f2a10

COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /app/build /usr/share/nginx/html

EXPOSE 8080

HEALTHCHECK CMD wget -q -O /dev/null http://127.0.0.1:8080/healthz || exit 1

USER nginx
