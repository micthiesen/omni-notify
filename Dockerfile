FROM node:24.14.0-slim

RUN corepack enable

WORKDIR /app
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY patches/ patches/
COPY frontend/package.json frontend/
RUN corepack prepare pnpm@10.28.0 --activate && pnpm install

COPY . .
RUN pnpm run build && pnpm --filter frontend run build

ENV DOCKERIZED=true
CMD ["node", "dist/index.js"]
