# Server-manager image for Cloud Run. Build context: this folder.
#
#   docker build -t server-manager .
#   docker run --rm -p 3001:3001 -e PORT=3001 server-manager

FROM oven/bun:1-alpine AS deps
WORKDIR /app
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile --production

FROM oven/bun:1-alpine AS runner
WORKDIR /app
RUN addgroup -S app && adduser -S app -G app
COPY --from=deps --chown=app:app /app/node_modules ./node_modules
COPY --chown=app:app package.json ./
COPY --chown=app:app src ./src
ENV NODE_ENV=production
USER app

# Cloud Run injects PORT at runtime; do not hardcode.
CMD ["bun", "run", "start"]
