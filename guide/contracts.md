# Contracts: what your repositories promise each other

When a product is several repositories, the most common way an agent breaks
it is quiet: it renames a table, a column or an endpoint in one repository
and never touches the others that depend on it. Following imports does not
find these links; a Flutter app calling `.from('orders')` has no import of
the SQL that creates `orders`.

swarmux reads the links out of the sources themselves, with no model and no
tokens, and keeps them as **contracts**: a thing one repository *provides*
and the places in other repositories that *consume* it.

## An example product

```text
shop/
  backend/   Supabase: supabase/migrations/*.sql, openapi.yaml
  app/       Flutter: lib/orders.dart
  admin/     TypeScript: src/api.ts
```

```sql
-- backend/supabase/migrations/001_orders.sql
create table orders (id uuid primary key, total numeric not null);
create function get_orders(uid uuid) returns setof orders language sql as $$ ... $$;
```

```dart
// app/lib/orders.dart
final rows = await supabase.from('orders').select('id, total');
```

```ts
// admin/src/api.ts
const { data } = await supabase.rpc('get_orders', { uid })
```

```sh
swarmux workspace add shop      # reads the contracts as part of the scan
swarmux contracts --workspace shop
```

```text
table orders      backend/supabase/migrations/001_orders.sql:2   1 consumer
column orders.id  backend/supabase/migrations/001_orders.sql:2   1 consumer
function get_orders   backend/supabase/migrations/001_orders.sql:3   1 consumer
...
```

```sh
swarmux contracts --workspace shop show orders
```

```text
table orders — backend/supabase/migrations/001_orders.sql:2
  shape: columns id uuid, total numeric
  consumed by 1:
    app/lib/orders.dart:2
```

## What counts as a contract

A contract is a symbol with a provider (repository, file, line, and the
file's content hash as its anchor) and consumers (repository, file, line).
Contracts are read from the sources by text matching, not declared, and they are
never stale: they are read again from the files, and what changed is shown. A
link the scan does not recognise is not seen.

| Provides | Read from |
| --- | --- |
| tables, views, columns, enum types and values, functions and procedures, row-level-security policies | SQL files: migrations (`supabase/migrations`, `migrations/`, `db/`) and schema dumps. Migrations are applied in file-name order, so a later `ALTER TABLE … RENAME` or `DROP COLUMN` changes what an earlier file created |
| endpoints (`GET /orders/{}`) and schemas | OpenAPI or Swagger files named `openapi*` or `swagger*` (JSON, or block-style YAML) |
| environment keys | `.env.example`, `.env.sample`, `.env.template`. A real `.env` is never opened |

| Consumed by | Found in |
| --- | --- |
| tables and columns | `.from('t')`, `.table('t')` and the columns of a following `.select('a, b, rel(c)')`, in Dart, TypeScript/JavaScript and Python; Supabase's generated TypeScript types (`Tables`, `Functions`, `Enums`); SQL inside strings (`select … from t`); query files (`.sql` outside migrations) |
| functions | `.rpc('f')`; generated `Functions` types |
| endpoints | `fetch`, `axios`, `$fetch`, `dio`, `requests`, `Uri.parse` and similar with a URL; the base URL and `${…}` parts are dropped |
| environment keys | `process.env.X`, `import.meta.env.X`, `Platform.environment['X']`, `String.fromEnvironment('X')`, `os.environ['X']`, `os.getenv('X')` |
| columns, loosely | Dart models: `json['col']` and `@JsonKey(name: 'col')` in a class with `fromJson`, matched to a table by the class name (`OrderItem` is `order_items`). These are marked *inferred* |

Comments and strings that merely contain a call (`// supabase.from('x')`,
`"call .from('x')"`) are not counted.

### Known limits

This is a token scan, not a compiler. It does not see a table name held in a
variable or built at run time, a URL kept in a constant defined elsewhere,
an ORM's model mapping, or SQL a function builds and `EXECUTE`s. Queries
written inside migrations are not counted as uses (they describe the schema
of their day). Server route definitions (`app.get('/x')`) are not consumers.
Endpoints match on the exact path after the base URL, so a gateway that adds
a prefix hides the link. GraphQL and gRPC are not read yet.

## It stays current, cheaply

Every file has a content hash. A rescan lists the files an extractor reads,
skips those whose size and time are unchanged, reads the rest, and runs an
extractor again only where the hash changed (an editor saving the same text
costs one read). `swarmux workspace add` and `rescan`, every
`swarmux contracts` call, a mission's delivery and the start of every job
use the same incremental scan. Build output, dependency and VCS folders are
never entered, and files over a megabyte are skipped.

## What agents are told

When a job reads or edits a file, the same hook that brings
[knowledge](knowledge.md) adds a short, quoted note (the pull is at most 300
tokens in all, each line once per job):

- a **provider** file: `"table orders" is consumed by: app/lib/orders.dart:42, admin/src/api.ts:17`
- a **consumer** file: `this file uses "table orders" provided by "backend/supabase/migrations/001.sql:2": "columns id uuid, total numeric"`

so an agent editing a migration hears who depends on it, and an agent editing
the app sees what the backend offers today.

## The cross-repo gate

For a [workspace mission](missions.md), the [done-gate](done-gate.md) also
compares the contracts when the job started with the contracts in its
worktrees now. If the job changed what a repository provides:

1. The checks of the repositories that consume it join the run: the
   mission's own `"repo: command"` checks where it has them, else the
   repository's detected command (`flutter analyze`, `tsc --noEmit`, `npm run
   typecheck`, `lint`, `build`, `test` in that order).
2. A deterministic check named `contracts` fails when a removed or renamed
   symbol is still used, and says where:

   ```text
   BROKEN: table orders (backend/supabase/migrations/001.sql:2) is gone
   (removed or renamed) but still used by: app/lib/orders.dart:2, admin/src/api.ts:4.
   ```

   The agent is blocked with that text until the consumers are updated (the
   usual number of blocks, then the mission is escalated). A changed
   signature or column type is reported as `CHANGED` and left to the
   consumers' own checks.
3. If a consumer's repository is not in the mission's `repos`, the change
   cannot be finished there. The check fails with `OUTSIDE THE MISSION`, and a
   P2 item in the inbox says *this change breaks app/lib/orders.dart:2 which
   isn't part of the mission*. Add the repository to the mission or revert.

The verdict, the symbols, the places and whether it passed are a ledger entry
(`contracts_check`), and so is every note given to an agent
(`contracts_injected`).

## The Contracts tab

On the Knowledge screen (`8`), `t` switches between the items and the
contracts of a workspace. `↑↓` select one and its consumers are listed under
it; `r` reads the files again; `w` picks the next workspace.

## Commands

```sh
swarmux contracts [--workspace W] [--consumers]
swarmux contracts [--workspace W] show <symbol>
```

`<symbol>` is `orders`, `table orders`, `orders.total`, `get_orders`,
`GET /api/orders` or `env API_KEY`. Without `--workspace`, the workspace
whose root is the current folder is meant, or the only one.

## Adding a stack

An extractor reads one file and says what it provides and uses. It is a
pure function of the path and the text (`crates/domain/src/contracts`):

```rust
pub trait Extractor: Sync {
    fn id(&self) -> &'static str;
    fn applies(&self, path: &str) -> bool;
    fn extract(&self, path: &str, text: &str) -> Vec<Fact>;
}
```

`Fact::Provide`, `Remove`, `Rename` and `Alter` build up what a repository
provides (files are applied in path order); `Fact::Use` names a place that
depends on a symbol. Add the extractor to `EXTRACTORS` and write tests
that show it ignoring comments and strings. Nothing else changes: the scan,
the store, the notes, the gate and the screen work from the facts.
