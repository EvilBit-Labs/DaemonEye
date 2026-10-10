# SQL Dialect Quick Reference

A detection rule is one `SELECT` over one table. It is parsed with `sqlparser`, checked against the gates below when it is loaded, lowered into a pushed-down half (predicates and a projection the collector evaluates) plus a residual the agent evaluates, and run by the executor every collection cycle. Anything a gate refuses never loads, never runs and never alerts; the rejection names the construct.

## Shape

```sql
SELECT <columns or *>
FROM processes
WHERE <predicate>;
```

- **One table.** A rule reads exactly one table. `JOIN` of any kind, including a self-join, is refused by the planner.
- **`WHERE` predicates:** comparisons (`=`, `<>`, `<`, `<=`, `>`, `>=`), `AND`, `OR`, `NOT`, `LIKE`, `IN (<literals>)`, `BETWEEN`, `IS NULL` / `IS NOT NULL`, and the seven functions below.
- **Literals** are plain strings and numbers. There are no parameters: a rule is a complete statement.

## Allowed Functions

A rule may call exactly seven functions: `hex`, `instr`, `length`, `like`, `match`, `regexp` and `unhex`. Any other function call is refused at rule load, naming the function. Names are not case-sensitive.

| Function              | Description                                                                          | Example                            |
| --------------------- | ------------------------------------------------------------------------------------ | ---------------------------------- |
| `LENGTH(str)`         | String length                                                                        | `LENGTH(command_line) > 1000`      |
| `INSTR(str, substr)`  | Position of a substring, 0 if absent                                                 | `INSTR(command_line, 'nc -l') > 0` |
| `str LIKE pattern`    | SQL wildcard match (`%`, `_`)                                                        | `name LIKE '%suspicious%'`         |
| `str REGEXP pattern`  | Regular-expression match; `REGEXP(str, pattern)` is the same test as a function call | `name REGEXP '^svc[0-9]+$'`        |
| `MATCH(str, pattern)` | Pattern match, function form                                                         | `MATCH(command_line, 'nc -l')`     |
| `HEX(data)`           | Bytes to hexadecimal                                                                 | `HEX(executable_hash)`             |
| `UNHEX(hex)`          | Hexadecimal to bytes                                                                 | `UNHEX('deadbeef')`                |

Infix `MATCH` does not parse in this dialect; write it as a function call.

Syntax that looks like a function call but is its own grammar rule is refused at load by the same gate: `SUBSTR`/`SUBSTRING`, `TRIM`, `POSITION`, `EXTRACT`, `CEIL`, `FLOOR`, `OVERLAY`, `CONVERT`. `CAST`, `TRY_CAST`, `SAFE_CAST` and `::` are refused as casts.

## Refused Constructs

| Construct                                       | Why                                                                                                                                                                                                                                                 |
| ----------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `COUNT`, `SUM`, `AVG`, `MAX`, `MIN`             | The planner lowers a rule to predicates and a projection; it cannot compute an aggregate. A projected `COUNT(pid)` would lower to the bare column `pid` and return rows where a count was asked for, so it is refused rather than answered wrongly. |
| `GROUP BY`, `HAVING`                            | Same reason.                                                                                                                                                                                                                                        |
| `ORDER BY`, `LIMIT`, `DISTINCT`, `TOP`, `FETCH` | The plan has no representation for them: `LIMIT 1` would compile to "every match". Narrow with predicates instead.                                                                                                                                  |
| `JOIN`                                          | One table per rule.                                                                                                                                                                                                                                 |
| `WITH` (CTE), set operations, `VALUES`          | A rule is one plain `SELECT`.                                                                                                                                                                                                                       |
| Table-valued functions in `FROM`                | No collector serves one; `SELECT * FROM readfile('/etc/passwd')` is refused at the `FROM`.                                                                                                                                                          |
| Any statement other than `SELECT`               | `INSERT`, `UPDATE`, `DELETE`, `DROP`, `PRAGMA`, ... are refused by leading keyword.                                                                                                                                                                 |

Functions that are not on the allowlist are not "banned" by name; they are absent, which is the same refusal. `load_extension`, `readfile`, `system`, `random`, `printf` and every other function fall out the same way.

## Process Data Schema

The `processes` columns a rule may name (the illustrative DDL in [query-pipeline.md](query-pipeline.md#process-data-schema) matches this list):

| Column            | Type    | Notes                        |
| ----------------- | ------- | ---------------------------- |
| `pid`             | integer | unsigned                     |
| `ppid`            | integer | unsigned, nullable           |
| `name`            | text    |                              |
| `executable_path` | text    | nullable                     |
| `command_line`    | text    | nullable                     |
| `start_time`      | integer | nullable, epoch milliseconds |
| `cpu_usage`       | real    | nullable                     |
| `memory_usage`    | integer | unsigned, nullable           |
| `executable_hash` | text    | SHA-256 hex, nullable        |
| `user_id`         | text    | nullable                     |
| `accessible`      | boolean |                              |
| `file_exists`     | boolean |                              |
| `collection_time` | integer | epoch milliseconds           |

## Common Patterns

```sql
-- Find processes by name
SELECT * FROM processes WHERE name = 'suspicious-process';

-- Pattern match
SELECT * FROM processes WHERE name LIKE '%malware%';

-- Resource thresholds
SELECT * FROM processes WHERE cpu_usage > 80.0 OR memory_usage > 2147483648;

-- Known hashes
SELECT * FROM processes
WHERE executable_hash IN (
    'a1b2c3d4e5f6789012345678901234567890abcdef1234567890abcdef',
    'f1e2d3c4b5a6978012345678901234567890abcdef1234567890abcdef'
);

-- Command-line analysis
SELECT * FROM processes
WHERE command_line LIKE '%nc -l%'
   OR command_line LIKE '%wget%'
   OR LENGTH(command_line) > 1000;

-- Suspicious executable locations
SELECT * FROM processes
WHERE executable_path LIKE '/tmp/%'
   OR executable_path LIKE '/var/tmp/%'
   OR executable_path IS NULL;
```

## Performance

- A predicate of the shape `column op literal` (comparisons, `LIKE`, `IN`, infix `REGEXP`) on a column whose collector advertises and conformance-passed that operation is pushed down to the collector. Everything else, including every function call and any negated `LIKE`/`IN`/`REGEXP`, is a residual the agent evaluates over the rows the pushed half admitted, so lead with a pushable predicate and keep the pattern narrow.
- Equality and range predicates on `pid` and `name` are served by an index; a `LIKE` on `command_line` reads every row in the cycle's window.
- A rule whose pattern exceeds `detection.pattern_latency_threshold_ms` on a measured execution is disabled until it is reloaded; see [query-pipeline.md](query-pipeline.md#sizing-the-executor) for the executor's knobs.
