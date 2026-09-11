# Stage 1: build
FROM node:20-alpine AS build

WORKDIR /app

COPY package.json pnpm-lock.yaml ./
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
FROM nginx:1.27-alpine

COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /app/build /usr/share/nginx/html

EXPOSE 8080

HEALTHCHECK CMD wget -q -O /dev/null http://127.0.0.1:8080/healthz || exit 1

USER nginx
