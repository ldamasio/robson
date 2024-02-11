FROM node:14-alpine AS builder
WORKDIR /opt/web
COPY . ./
RUN npm install
ENV PATH="./node_modules/.bin:$PATH"
RUN npm run build
EXPOSE 4173
CMD [ "npm", "run", "preview" ]