FROM node:24.18.0-slim AS build

ENV CI=true

RUN npm install -g pnpm@11.10.0

WORKDIR /app
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY patches/ patches/
COPY frontend/package.json frontend/
RUN pnpm install --frozen-lockfile

COPY . .
RUN pnpm run build && pnpm --filter frontend run build
RUN pnpm prune --prod

FROM node:24.18.0-slim AS runtime

WORKDIR /app

COPY --from=build --chown=node:node /app/node_modules ./node_modules
COPY --from=build --chown=node:node /app/dist ./dist
COPY --from=build --chown=node:node /app/frontend/dist ./frontend/dist
COPY --from=build --chown=node:node /app/package.json ./package.json

RUN mkdir -p /data && chown node:node /data

ENV DOCKERIZED=true NODE_ENV=production DB_NAME=/data/docstore.db
USER node

EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD ["node", "-e", "fetch('http://127.0.0.1:'+(process.env.FRONTEND_PORT||3000)+'/api/health').then(r=>{if(!r.ok)process.exit(1)}).catch(()=>process.exit(1))"]

CMD ["node", "dist/index.js"]
