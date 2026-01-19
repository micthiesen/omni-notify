FROM node:24.13.0-slim

RUN corepack enable

WORKDIR /app
COPY package.json pnpm-lock.yaml ./
RUN corepack prepare pnpm@10.28.0 --activate && pnpm install

COPY . .
RUN pnpm run build

ENV DOCKERIZED=true
EXPOSE 3000
CMD ["node", "dist/index.js"]
