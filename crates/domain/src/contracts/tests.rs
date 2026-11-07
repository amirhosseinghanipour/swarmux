use std::collections::BTreeSet;

use super::*;

fn facts(path: &str, text: &str) -> Vec<Fact> {
    extract(path, text)
}

fn uses(f: &[Fact]) -> Vec<String> {
    f.iter()
        .filter_map(|f| match f {
            Fact::Use { sym, .. } => Some(sym.label()),
            _ => None,
        })
        .collect()
}

fn provides(f: &[Fact]) -> Vec<String> {
    f.iter()
        .filter_map(|f| match f {
            Fact::Provide { sym, .. } => Some(sym.label()),
            _ => None,
        })
        .collect()
}

const MIGRATION: &str = r#"
-- the orders of the shop; create table ghost (id int);
create table public.orders (
  id uuid primary key default gen_random_uuid(),
  "total" numeric(10,2) not null,
  status order_status not null default 'new',
  constraint orders_total_pos check (total >= 0)
);
/* alter table orders add column hidden int; */
create type order_status as enum ('new', 'paid', 'it''s');
alter table orders add column note text, add column if not exists paid_at timestamptz;
create policy "owner reads" on orders for select using (auth.uid() = user_id);
create or replace function public.get_orders(uid uuid) returns setof orders
language sql as $$ select * from orders where user_id = uid; drop table nothing; $$;
create view order_totals as select 1;
"#;

#[test]
fn sql_reads_tables_columns_enums_functions_policies_and_views() {
    let f = facts("supabase/migrations/001_init.sql", MIGRATION);
    let p = provides(&f);
    for want in [
        "table orders",
        "column orders.id",
        "column orders.total",
        "column orders.status",
        "enum order_status",
        "enum-value order_status.paid",
        "enum-value order_status.it's",
        "column orders.note",
        "column orders.paid_at",
        "policy orders.owner reads",
        "function get_orders",
        "table order_totals",
    ] {
        assert!(p.contains(&want.to_string()), "{want} in {p:?}");
    }
    assert!(
        !p.iter()
            .any(|s| s.contains("ghost") || s.contains("hidden"))
    );
    assert!(!p.iter().any(|s| s.contains("constraint")));
    assert!(
        !p.iter().any(|s| s.contains("nothing")),
        "bodies are skipped"
    );
    assert!(uses(&f).is_empty(), "queries in migrations are history");
}

#[test]
fn sql_function_keeps_its_signature() {
    let f = facts("m/001.sql", MIGRATION);
    let d = f
        .iter()
        .find_map(|f| match f {
            Fact::Provide { sym, detail, .. } if sym.kind == SymKind::Function => Some(detail),
            _ => None,
        })
        .unwrap();
    assert_eq!(d, "(uid uuid) returns setof orders");
}

#[test]
fn sql_drops_and_renames_are_ordered_facts() {
    let f = facts(
        "db/migrate/002.sql",
        "alter table orders rename to purchases;\n\
         alter table purchases rename column total to amount;\n\
         alter table purchases drop column note;\n\
         drop policy \"owner reads\" on purchases;\n\
         drop type if exists order_status cascade;\n\
         drop function if exists get_orders(uuid);\n\
         alter type order_status add value if not exists 'void';",
    );
    assert!(f.contains(&Fact::Rename {
        from: Symbol::table("orders"),
        to: Symbol::table("purchases")
    }));
    assert!(f.contains(&Fact::Rename {
        from: Symbol::column("purchases", "total"),
        to: Symbol::column("purchases", "amount")
    }));
    assert!(f.contains(&Fact::Remove {
        sym: Symbol::column("purchases", "note")
    }));
    assert!(f.contains(&Fact::Remove {
        sym: Symbol::function("get_orders")
    }));
    assert!(provides(&f).contains(&"enum-value order_status.void".to_string()));
}

#[test]
fn sql_query_files_use_tables_but_prose_does_not() {
    let f = facts(
        "queries/report.sql",
        "-- from the archive\nselect o.id from public.orders o join customers c on c.id = o.cid;\n\
         update orders set note = 'x' where id = 1;",
    );
    let u = uses(&f);
    assert!(u.contains(&"table orders".to_string()));
    assert!(u.contains(&"table customers".to_string()));
    let strings = facts(
        "lib/a.dart",
        "var a = 'select the option from the menu';\nvar b = 'select id, total from orders where id = 1';",
    );
    let u = uses(&strings);
    assert_eq!(u, vec!["table orders".to_string()], "{u:?}");
}

#[test]
fn dart_finds_from_rpc_select_and_ignores_comments_and_strings() {
    let src = r#"
// final a = supabase.from('commented');
/* supabase.rpc('also_commented') */
final msg = "call supabase.from('in_a_string')";
final rows = await supabase.from('orders').select('id, total:amount, customer:customers(name), status::text').eq('a', 1);
final n = await supabase.rpc('get_orders', params: {'uid': id});
final x = Array.from('abc');
"#;
    let f = facts("app/lib/orders.dart", src);
    let u = uses(&f);
    for want in [
        "table orders",
        "column orders.id",
        "column orders.amount",
        "table customers",
        "column customers.name",
        "column orders.status",
        "function get_orders",
    ] {
        assert!(u.contains(&want.to_string()), "{want} in {u:?}");
    }
    assert!(
        !u.iter()
            .any(|s| s.contains("commented") || s.contains("in_a_string"))
    );
    assert!(!u.iter().any(|s| s.contains("abc")));
    let lines: Vec<u32> = f
        .iter()
        .filter_map(|f| match f {
            Fact::Use { sym, line, .. } if sym.name == "orders" => Some(*line),
            _ => None,
        })
        .collect();
    assert_eq!(lines, vec![5]);
}

#[test]
fn dart_models_are_matched_by_class_name_and_marked_inferred() {
    let src = r#"
class OrderItem {
  factory OrderItem.fromJson(Map<String, dynamic> json) => OrderItem(json['unit_price'], json["qty"]);
  @JsonKey(name: 'sku_code')
  final String sku;
}
"#;
    let f = facts("lib/models/order_item.dart", src);
    assert!(
        f.iter()
            .all(|f| matches!(f, Fact::Use { inferred: true, .. }))
    );
    let u = uses(&f);
    assert!(
        u.contains(&"field order_item.unit_price".to_string()),
        "{u:?}"
    );
    assert!(u.contains(&"field order_item.sku_code".to_string()));
    // Without a fromJson the keys are just some map.
    assert!(facts("lib/x.dart", "class A { void f(){ json['k']; } }").is_empty());
}

#[test]
fn dart_http_calls_use_endpoints() {
    let f = facts(
        "lib/api.dart",
        "final r = await http.post(Uri.parse('$baseUrl/api/orders/$id?x=1'));\n\
         final d = dio.get('/api/customers');",
    );
    let u = uses(&f);
    assert!(
        u.contains(&"endpoint POST /api/orders/{}".to_string()),
        "{u:?}"
    );
    assert!(u.contains(&"endpoint GET /api/customers".to_string()));
}

#[test]
fn typescript_finds_calls_fetch_axios_env_and_generated_types() {
    let src = r#"
const { data } = await supabase.from('orders').select('*, customers(name)');
const r = await supabase.rpc('get_orders');
const a = await fetch(`${API}/api/orders/${id}`, { method: 'DELETE' });
const b = await axios.get('https://x.test/api/customers?limit=3');
router.get('/api/served', handler);
const url = process.env.SUPABASE_URL + import.meta.env.VITE_KEY + process.env['TOKEN'];
type Row = Database['public']['Tables']['invoices']['Row'];
// supabase.from('nope')
"#;
    let f = facts("admin/src/api.ts", src);
    let u = uses(&f);
    for want in [
        "table orders",
        "table customers",
        "column customers.name",
        "function get_orders",
        "endpoint DELETE /api/orders/{}",
        "endpoint GET /api/customers",
        "env SUPABASE_URL",
        "env VITE_KEY",
        "env TOKEN",
        "table invoices",
    ] {
        assert!(u.contains(&want.to_string()), "{want} in {u:?}");
    }
    assert!(
        !u.iter().any(|s| s.contains("served") || s.contains("nope")),
        "{u:?}"
    );
}

#[test]
fn typescript_generated_database_types_name_tables_columns_functions() {
    let src = r#"
export type Database = {
  public: {
    Tables: {
      orders: {
        Row: { id: string; total: number }
        Insert: { id?: string; extra: number }
      }
    }
    Functions: { get_orders: { Args: { uid: string }; Returns: unknown } }
    Enums: { order_status: 'new' | 'paid' }
  }
}
"#;
    let u = uses(&facts("admin/src/database.types.ts", src));
    for want in [
        "table orders",
        "column orders.id",
        "column orders.total",
        "function get_orders",
        "enum order_status",
    ] {
        assert!(u.contains(&want.to_string()), "{want} in {u:?}");
    }
    assert!(!u.contains(&"column orders.extra".to_string()), "only Row");
}

#[test]
fn python_finds_table_rpc_env_and_sql_strings() {
    let src = r#"
# supabase.table('commented')
rows = supabase.table("orders").select("id, total").execute()
supabase.rpc("get_orders", {"uid": 1})
key = os.environ["API_KEY"] or os.getenv("OTHER_KEY") or os.environ.get("THIRD_KEY")
cur.execute("""
    select id from invoices where paid
""")
r = requests.get(f"{BASE}/api/orders")
"#;
    let u = uses(&facts("svc/main.py", src));
    for want in [
        "table orders",
        "column orders.total",
        "function get_orders",
        "env API_KEY",
        "env OTHER_KEY",
        "env THIRD_KEY",
        "table invoices",
        "endpoint GET /api/orders",
    ] {
        assert!(u.contains(&want.to_string()), "{want} in {u:?}");
    }
    assert!(!u.contains(&"table commented".to_string()));
}

#[test]
fn openapi_json_and_yaml_give_endpoints_and_schemas() {
    let json = r#"{"openapi":"3.0.0","paths":{"/orders/{id}":{"get":{"operationId":"getOrder","parameters":[{"name":"id"}]},"delete":{}},"/orders":{"post":{"requestBody":{}}}},
        "components":{"schemas":{"Order":{"properties":{"id":{},"total":{}}}}}}"#;
    let p = provides(&facts("backend/openapi.json", json));
    for want in [
        "endpoint GET /orders/{}",
        "endpoint DELETE /orders/{}",
        "endpoint POST /orders",
        "schema Order",
    ] {
        assert!(p.contains(&want.to_string()), "{want} in {p:?}");
    }
    let yaml = "openapi: 3.0.0\npaths:\n  /orders/{id}:\n    parameters:\n      - name: id\n    get:\n      operationId: getOrder\n      parameters:\n        - name: id\n    delete:\n      summary: x\n  '/customers':\n    post:\n      operationId: mk\ncomponents:\n  schemas:\n    Order:\n      type: object\n      properties:\n        id:\n          type: string\n        total:\n          type: number\n    Other:\n      type: string\n";
    let f = facts("api/openapi.yaml", yaml);
    let p = provides(&f);
    for want in [
        "endpoint GET /orders/{}",
        "endpoint DELETE /orders/{}",
        "endpoint POST /customers",
        "schema Order",
        "schema Other",
    ] {
        assert!(p.contains(&want.to_string()), "{want} in {p:?}");
    }
    let order = f
        .iter()
        .find_map(|f| match f {
            Fact::Provide { sym, detail, .. } if sym.name == "Order" => Some(detail.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(order, "properties id,total");
    assert!(!wanted("package.json"));
}

#[test]
fn env_examples_provide_keys_and_never_secrets() {
    let f = facts(
        ".env.example",
        "# c\nSUPABASE_URL=\nexport API_KEY=abc\nlower=1\n",
    );
    assert_eq!(provides(&f), vec!["env SUPABASE_URL", "env API_KEY"]);
    assert!(!wanted(".env"), "a real .env is never read");
}

fn index(files: &[(&str, &str, &str)]) -> Index {
    let owned: Vec<(String, String, Vec<Fact>)> = files
        .iter()
        .map(|(repo, path, text)| (repo.to_string(), path.to_string(), extract(path, text)))
        .collect();
    let refs: Vec<FileFacts<'_>> = owned
        .iter()
        .map(|(r, p, f)| FileFacts {
            repo: r,
            path: p,
            hash: "h",
            facts: f,
        })
        .collect();
    Index::build(&refs)
}

const APP: &str = "final r = supabase.from('orders').select('id, total');\n\
                   final s = supabase.rpc('get_orders');";
const ADMIN: &str = "const r = await supabase.from('orders').select('*');\n\
                     const f = await fetch('/api/orders');";

#[test]
fn the_index_folds_migrations_in_order() {
    let ix = index(&[
        ("backend", "supabase/migrations/001.sql", MIGRATION),
        (
            "backend",
            "supabase/migrations/002.sql",
            "alter table orders rename to purchases; alter table purchases rename column total to amount;",
        ),
    ]);
    let names: Vec<String> = ix.provided().map(|p| p.sym.label()).collect();
    assert!(names.contains(&"table purchases".to_string()), "{names:?}");
    assert!(names.contains(&"column purchases.amount".to_string()));
    assert!(
        !names
            .iter()
            .any(|n| n == "table orders" || n == "column orders.total")
    );
    let t = ix.find("purchases");
    assert_eq!(t.len(), 1);
    assert_eq!(
        t[0].file, "supabase/migrations/002.sql",
        "the rename is its latest anchor"
    );
}

#[test]
fn consumers_are_found_across_repos_with_places() {
    let ix = index(&[
        ("backend", "supabase/migrations/001.sql", MIGRATION),
        ("app", "lib/orders.dart", APP),
        ("admin", "src/api.ts", ADMIN),
    ]);
    let orders = ix.find("table orders");
    assert_eq!(orders.len(), 1);
    let places: Vec<String> = ix
        .consumers_of(orders[0])
        .iter()
        .map(|s| s.place())
        .collect();
    assert_eq!(
        places,
        vec!["admin/src/api.ts:1", "app/lib/orders.dart:1"],
        "a table is consumed by its users and its columns' users"
    );
    let f = ix.find("get_orders");
    let places: Vec<String> = ix.consumers_of(f[0]).iter().map(|s| s.place()).collect();
    assert_eq!(places, vec!["app/lib/orders.dart:2"]);
    let col = ix.find("orders.total");
    assert_eq!(ix.consumers_of(col[0]).len(), 1);
}

#[test]
fn renaming_a_table_without_the_consumers_is_a_dangling_report() {
    let before = index(&[
        ("backend", "supabase/migrations/001.sql", MIGRATION),
        ("app", "lib/orders.dart", APP),
        ("admin", "src/api.ts", ADMIN),
    ]);
    let renamed = MIGRATION.replace("public.orders", "public.purchases");
    let after = index(&[
        ("backend", "supabase/migrations/001.sql", &renamed),
        ("app", "lib/orders.dart", APP),
        ("admin", "src/api.ts", ADMIN),
    ]);
    let r = Report::of(&before, &after);
    let problems = r.problems();
    assert!(
        problems.iter().any(|p| p.contains("table orders")
            && p.contains("app/lib/orders.dart:1")
            && p.contains("admin/src/api.ts:1")),
        "{problems:?}"
    );
    assert_eq!(
        r.repos(),
        BTreeSet::from(["admin".to_string(), "app".to_string()])
    );
    // Updating the consumers clears it.
    let fixed = index(&[
        ("backend", "supabase/migrations/001.sql", &renamed),
        (
            "app",
            "lib/orders.dart",
            &APP.replace("orders", "purchases"),
        ),
        (
            "admin",
            "src/api.ts",
            &ADMIN.replace("'orders'", "'purchases'"),
        ),
    ]);
    assert!(Report::of(&before, &fixed).dangling().is_empty());
}

#[test]
fn a_changed_signature_is_reshaped_not_dangling() {
    let before = index(&[
        ("backend", "m/001.sql", MIGRATION),
        ("app", "lib/orders.dart", APP),
    ]);
    let after = index(&[
        (
            "backend",
            "m/001.sql",
            &MIGRATION.replace("uid uuid", "uid uuid, since date"),
        ),
        ("app", "lib/orders.dart", APP),
    ]);
    let r = Report::of(&before, &after);
    assert!(r.dangling().is_empty());
    assert_eq!(r.reshaped().len(), 1);
    assert_eq!(r.consumers()[0].place(), "app/lib/orders.dart:2");
}

#[test]
fn notes_list_consumers_of_a_provider_file_and_the_shape_for_a_consumer() {
    let ix = index(&[
        ("backend", "supabase/migrations/001.sql", MIGRATION),
        ("app", "lib/orders.dart", APP),
        ("admin", "src/api.ts", ADMIN),
    ]);
    let none = BTreeSet::new();
    let n = note_for(
        &ix,
        "backend",
        "supabase/migrations/001.sql",
        &none,
        PULL_TOKENS,
    )
    .unwrap();
    assert!(n.text.contains("table orders"), "{}", n.text);
    assert!(n.text.contains("app/lib/orders.dart:1"));
    assert!(n.text.contains("admin/src/api.ts:1"));
    assert!(n.tokens <= PULL_TOKENS);
    // Delivered keys are not repeated.
    let skip: BTreeSet<String> = n.keys.iter().cloned().collect();
    assert!(
        note_for(
            &ix,
            "backend",
            "supabase/migrations/001.sql",
            &skip,
            PULL_TOKENS
        )
        .is_none()
    );
    let c = note_for(&ix, "app", "lib/orders.dart", &none, PULL_TOKENS).unwrap();
    assert!(c.text.contains("provided by"), "{}", c.text);
    assert!(c.text.contains("columns id uuid"), "{}", c.text);
    assert!(note_for(&ix, "app", "lib/unrelated.dart", &none, PULL_TOKENS).is_none());
    let tiny = note_for(&ix, "backend", "supabase/migrations/001.sql", &none, 20);
    assert!(tiny.is_none(), "nothing fits in a few tokens");
}

#[test]
fn facts_survive_storing() {
    let f = facts("supabase/migrations/001.sql", MIGRATION);
    assert_eq!(decode(&encode(&f)), f);
    let g = facts("lib/a.dart", APP);
    assert_eq!(decode(&encode(&g)), g);
    let h = facts(
        "m/002.sql",
        "alter table a rename to b; alter table b drop column c; alter table b alter column d type int;",
    );
    assert_eq!(decode(&encode(&h)), h);
}

#[test]
fn url_paths_are_normalised() {
    assert_eq!(
        code::url_path("https://h.test/api/x/{}?a=1#f").as_deref(),
        Some("/api/x/{}")
    );
    assert_eq!(code::url_path("{}/api/x/").as_deref(), Some("/api/x"));
    assert_eq!(code::url_path("api/x"), None);
    assert_eq!(code::url_path("/has space"), None);
}
