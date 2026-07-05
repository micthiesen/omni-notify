FROM node:24.18.0-slim

RUN npm install -g pnpm@11.10.0

WORKDIR /app
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY patches/ patches/
COPY frontend/package.json frontend/
RUN pnpm install

COPY . .
RUN pnpm run build && pnpm --filter frontend run build

ENV DOCKERIZED=true
CMD ["node", "dist/index.js"]
