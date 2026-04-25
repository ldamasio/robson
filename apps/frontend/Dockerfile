# Stage 1: build
FROM node:20-alpine AS build

WORKDIR /app

COPY package.json pnpm-lock.yaml ./
RUN corepack enable && corepack prepare pnpm@9 --activate
RUN pnpm install --frozen-lockfile

COPY . .

ARG PUBLIC_ROBSON_API_BASE
ENV PUBLIC_ROBSON_API_BASE=$PUBLIC_ROBSON_API_BASE

RUN pnpm run build

# Stage 2: runtime
FROM nginx:1.27-alpine

COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /app/build /usr/share/nginx/html

EXPOSE 8080

HEALTHCHECK CMD wget -q -O /dev/null http://127.0.0.1:8080/healthz || exit 1

USER nginx
