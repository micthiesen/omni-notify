# Pet Weight Tracker

Track pet weight data from a Litter-Robot 4 via the Whisker API, store it in SQLite, and display it in a lightweight web frontend.

## Two Pieces

### 1. Backend: Whisker Pet Data Task

A new `ScheduledTask` that authenticates with the Whisker cloud API, fetches pet weight history, and stores it in SQLite.

### 2. Frontend: Vite/React Dashboard

A minimal Vite + React app served alongside the main process. Single page showing pet weight charts over time. Named generically (e.g. `frontend/`) so it can grow into a general omni-notify dashboard later, but no nav/routing/layout work now.

---

## Backend Design

### Auth Flow

1. **Cognito SRP** against AWS `us-east-1`
   - User Pool ID: `us-east-1_rjhNnZVAm`
   - Client ID: `4552ujeu3aic90nf8qn53levmn`
2. Credentials from `WHISKER_CREDENTIALS=email:password` env var
3. Use `amazon-cognito-identity-js` (or `@aws-sdk/client-cognito-identity-provider` with `USER_SRP_AUTH`) to get `id_token`
4. Decode `mid` claim from the JWT for the user ID
5. Re-authenticate on every task run (tokens last 1 hour, task runs every 2 hours, simpler than caching/refresh logic)

### API Calls

All requests hit `https://pet-profile.iothings.site/graphql/` with `Authorization: Bearer <id_token>`.

**Initial sync (first run or backfill):**
```graphql
query GetPetsByUser($userId: String!) {
  getPetsByUser(userId: $userId) {
    petId name weight lastWeightReading
    weightHistory { weight timestamp }
  }
}
```

**Incremental sync (subsequent runs):**
```graphql
query GetWeightHistoryByPetId($petId: String!, $limit: Int) {
  getWeightHistoryByPetId(petId: $petId, limit: $limit) {
    weight timestamp
  }
}
```

Use a reasonable limit (e.g. 100) for incremental fetches. Dedup on insert via `ON CONFLICT DO NOTHING`.

### Schedule

Run every 2 hours: `0 0 */2 * * *` (6-field cron with seconds). Weight readings only happen on litter box visits so this cadence is more than sufficient. `runOnStartup: true` to backfill on deploy.

### File Structure

```
src/
└── pet-tracker/
    ├── task.ts          # PetTrackerTask extends ScheduledTask
    ├── auth.ts          # Cognito SRP auth, token caching/refresh
    ├── api.ts           # GraphQL queries to pet-profile endpoint
    └── persistence.ts   # SQLite table init + read/write helpers
```

---

## Persistence: Typed SQLite Tables in mitools

The Entity document store (CBOR blobs, PK-only lookups) is wrong for time-series weight data. We need real SQL tables with timestamps, indexes, and range queries.

### New mitools Primitive: `Table<T>`

Add a lightweight typed-table helper to mitools alongside the existing Entity system. It should:

1. Accept a table name, column definitions (with TS types), and optional indexes
2. Auto-create the table on first use (`CREATE TABLE IF NOT EXISTS`) so no migration files needed
3. Provide typed `insert`, `query`, `upsert` methods that return/accept `T`
4. Use the same shared `getDb()` SQLite instance (same `docstore.db`)

#### Sketch

```typescript
// In mitools/src/persistence/table.ts
import { getDb } from "./docstore.js";

interface ColumnDef {
  type: "TEXT" | "INTEGER" | "REAL" | "BLOB";
  primaryKey?: boolean;
  notNull?: boolean;
}

interface TableOptions<T> {
  name: string;
  columns: Record<keyof T, ColumnDef>;
  indexes?: Array<{ columns: (keyof T)[]; unique?: boolean }>;
}

export class Table<T extends Record<string, unknown>> {
  constructor(private options: TableOptions<T>) {
    this.ensureTable();
  }

  private ensureTable(): void {
    // CREATE TABLE IF NOT EXISTS with column defs
    // CREATE INDEX IF NOT EXISTS for each index
  }

  insert(row: T): void { /* INSERT OR IGNORE */ }
  upsert(row: T): void { /* INSERT OR REPLACE */ }
  query(where: string, params?: unknown[]): T[] { /* SELECT * WHERE */ }
  all(): T[] { /* SELECT * */ }
}
```

This is intentionally minimal. No migrations, no ALTER TABLE. If a column changes during development, drop and recreate (acceptable for this data since we can re-backfill). If we need migrations later, we add them.

### Tables

**`pets` table:**
| Column | Type | Notes |
|--------|------|-------|
| pet_id | TEXT PK | Whisker pet ID |
| name | TEXT | Pet name |
| current_weight | REAL | Latest weight in lbs |
| updated_at | TEXT | ISO timestamp of last update |

**`pet_weight_history` table:**
| Column | Type | Notes |
|--------|------|-------|
| pet_id | TEXT | FK to pets |
| timestamp | TEXT | ISO timestamp from API |
| weight | REAL | Weight in lbs |
| PK | | (pet_id, timestamp) composite |

Index on `pet_weight_history(pet_id, timestamp)` for range queries.

No retention limit, no aggregation. Store every reading the API returns.

---

## Frontend

### Stack

- **Vite + React + TypeScript** in a `frontend/` directory at the repo root
- Chart library: `recharts` (lightweight, React-native, good for line charts)
- Styling: keep it dead simple. Plain CSS or a minimal utility (no Tailwind setup overhead for one page).

### Serving

The main omni-notify Node process serves the built frontend as static files via **Hono** (lightweight, modern, excellent TS types). Hono on `@hono/node-server` is ~14kb and handles static files + JSON API routes trivially.

1. Serves `frontend/dist/` as static files on a configurable port (e.g. `FRONTEND_PORT=3000`)
2. Exposes a `/api/pets` endpoint returning all pets with their weight history from SQLite
3. Starts alongside the scheduler in `src/index.ts`

No auth on the frontend. This runs on a private network.

### API Shape

```
GET /api/pets
```
```json
[
  {
    "petId": "abc123",
    "name": "Luna",
    "currentWeight": 9.2,
    "weightHistory": [
      { "timestamp": "2026-03-25T10:00:00Z", "weight": 9.2 },
      { "timestamp": "2026-03-24T14:30:00Z", "weight": 9.1 }
    ]
  }
]
```

### UI

Single page. For each pet:
- Name and current weight displayed
- Line chart of weight over time (all history, zoomable/scrollable via recharts brush)
- Time range selector (7d / 30d / 90d / all)

No routing, no navigation, no layout scaffolding beyond what's needed for this one page.

### Project Structure

`frontend/` is a pnpm workspace member alongside the root backend package. This keeps deps isolated without the churn of moving existing code into `packages/backend/`. Add `frontend` to `pnpm-workspace.yaml`:

```yaml
packages:
  - frontend
onlyBuiltDependencies:
  - better-sqlite3
  - esbuild
```

### Build & Dev

- `frontend/package.json` with its own deps (react, recharts, vite)
- Root `pnpm build` should also build the frontend (add a workspace build script or prebuild step)

**Dev workflow:**

1. `pnpm dev` — backend with tsx watch (includes Hono server on `FRONTEND_PORT`)
2. `pnpm --filter frontend dev` — Vite dev server with HMR, proxies `/api/*` to Hono

Vite config:
```typescript
server: {
  proxy: {
    "/api": "http://localhost:3000",  // proxy to Hono
  },
}
```

The frontend always hits the real API (no hardcoded/fake data in React code). The API reads from SQLite, populated by either the seed script or the real Whisker task.

### Seed Script

`src/pet-tracker/seed.ts` — inserts realistic fake data directly into SQLite for local dev/testing. Run with:

```bash
npx dotenvx run -- npx tsx src/pet-tracker/seed.ts
```

Generates:
- 2-3 pets with names
- ~90 days of weight readings per pet (a few readings per day, matching real litter box visit patterns)
- Realistic weight variance (small daily fluctuations around a baseline)

Idempotent: clears existing seed data before inserting. Uses the same persistence layer as the real task.

---

## Config Changes

Add to `src/utils/config.ts`:

```typescript
WHISKER_CREDENTIALS: z.string().transform((s) => {
  const [email, password] = s.split(":");
  return { email, password };
}),
FRONTEND_PORT: z.coerce.number().default(3000),
```

Add to `.env.example`:
```
WHISKER_CREDENTIALS=email:password
FRONTEND_PORT=3000
```

---

## Dependencies to Add

**omni-notify (root):**
- `amazon-cognito-identity-js` (Cognito SRP auth)
- `hono` + `@hono/node-server` (HTTP server)
- `@hono/node-ws` (if we want websockets later, but not needed now)

**frontend/ (separate package.json):**
- `react`, `react-dom`
- `recharts`
- `vite`, `@vitejs/plugin-react`
- `typescript`

**mitools:**
- No new deps. The `Table` primitive uses `better-sqlite3` which is already there.

---

## Implementation Order

1. **mitools `Table` primitive** - new file, tests, export from package
2. **Pet tracker backend** - auth, API client, persistence, scheduled task
3. **HTTP server** - API endpoint + static serving, wire into index.ts
4. **Frontend** - Vite scaffold, pet weight chart page
5. **Integration** - build pipeline, env config, CLAUDE.md updates

---

## Decisions Made

- **Token refresh**: Re-auth on every task run. Cognito tokens last 1 hour, task runs every 2 hours. Simpler than caching/refresh.
- **Project structure**: `frontend/` as a pnpm workspace member. Root stays as the backend. Avoids churn of moving everything to `packages/`.
- **HTTP server**: Hono + `@hono/node-server`. Tiny, modern, great TS types. Not overkill for this use case.
