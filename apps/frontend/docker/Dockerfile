FROM node:22 as build
WORKDIR /app
COPY ../package*.json ./
RUN npm install
COPY ../. .
ARG VITE_API_BASE_URL
ARG VITE_APP_VERSION
RUN VITE_API_BASE_URL=${VITE_API_BASE_URL} VITE_APP_VERSION=${VITE_APP_VERSION} npm run build

FROM nginx:stable-alpine as prod
COPY --from=build /app/build /usr/share/nginx/html
COPY --from=build /app/docker/nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 80
CMD ["nginx", "-g", "daemon off;"]
