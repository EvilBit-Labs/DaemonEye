# SQL Dialect Quick Reference

## Allowed Functions

A rule may call exactly seven functions: `hex`, `instr`, `length`, `like`, `match`, `regexp` and `unhex`. Any other function call is refused at rule load.

That check applies to function-call syntax only. `substr`/`substring`, `cast`, `trim`, `position`, `extract`, `ceil` and `floor` parse into their own syntax-tree nodes and never reach it, so **a rule using one is accepted at load but fails at execution**: the executor registers only the seven functions above, and no `substr` implementation exists to run. Do not use them.

### String Functions

| Function             | Description             | Example                            |
| -------------------- | ----------------------- | ---------------------------------- |
| `LENGTH(str)`        | String length           | `LENGTH(command_line)`             |
| `INSTR(str, substr)` | Find substring position | `INSTR(command_line, 'malicious')` |
| `LIKE pattern`       | Pattern matching        | `name LIKE '%suspicious%'`         |

### Encoding Functions

| Function     | Description              | Example                |
| ------------ | ------------------------ | ---------------------- |
| `HEX(data)`  | Convert to hexadecimal   | `HEX(executable_hash)` |
| `UNHEX(hex)` | Convert from hexadecimal | `UNHEX('deadbeef')`    |

### Pattern Matching Functions

| Function              | Description                           | Example                        |
| --------------------- | ------------------------------------- | ------------------------------ |
| `REGEXP pattern`      | Regular-expression matching (infix)   | `name REGEXP '^svc[0-9]+$'`    |
| `MATCH(str, pattern)` | Pattern matching (function-call form) | `MATCH(command_line, 'nc -l')` |

Infix `MATCH` does not parse in this dialect — write it as a function call, as shown.

## Refused: Aggregate Functions

`COUNT`, `SUM`, `AVG`, `MAX` and `MIN` are **not allowed**. A rule that calls one is refused when it is loaded, with the offending function named — it never runs and never alerts.

A rule is lowered into a pushed-down half (predicates and a projection the collector evaluates) plus a residual the agent evaluates. An aggregate is neither a predicate nor a column, so the planner has no way to compute one: a projected `COUNT(pid)` would lower to the bare column `pid`, and the rule would return rows where you asked for a count. Refusing at load is the point — a rule that cannot be lowered is rejected outright rather than silently answering the wrong question. `GROUP BY` and `HAVING` are refused for the same reason, as are `ORDER BY`, `LIMIT` and `DISTINCT`.

## Banned Functions

### Security-Critical (Always Banned)

- `load_extension()` - SQLite extension loading
- `eval()` - Code evaluation
- `exec()` - Command execution
- `system()` - System calls
- `shell()` - Shell execution

### File System Operations (Not Applicable)

- `readfile()` - File reading
- `writefile()` - File writing
- `edit()` - File editing

### Complex Pattern Matching (Performance Concerns)

- `glob()` - Glob patterns

### Mathematical Functions (Not Applicable)

- `abs()` - Absolute value
- `random()` - Random numbers
- `randomblob()` - Random binary data

### Formatting Functions (Not Applicable)

- `quote()` - SQL quoting
- `printf()` - String formatting
- `format()` - String formatting
- `char()` - Character conversion
- `unicode()` - Unicode functions
- `soundex()` - Soundex algorithm
- `difference()` - String difference

## Process Data Schema

```sql
-- Core process information
CREATE TABLE processes (
    id INTEGER PRIMARY KEY,
    scan_id INTEGER NOT NULL,
    collection_time INTEGER NOT NULL,
    pid INTEGER NOT NULL,
    ppid INTEGER,
    name TEXT NOT NULL,
    executable_path TEXT,
    command_line TEXT,
    start_time INTEGER,
    cpu_usage REAL,
    memory_usage INTEGER,
    status TEXT,
    executable_hash TEXT,        -- SHA-256 hash in hex format
    hash_algorithm TEXT,         -- Usually 'sha256'
    user_id INTEGER,
    group_id INTEGER,
    accessible BOOLEAN,
    file_exists BOOLEAN,
    environment_vars TEXT,        -- JSON string of environment variables
    metadata TEXT,               -- JSON string of additional metadata
    platform_data TEXT          -- JSON string of platform-specific data
);
```

## Common Query Patterns

### Basic Detection

```sql
-- Find processes by name
SELECT * FROM processes WHERE name = 'suspicious-process';

-- Find processes with pattern matching
SELECT * FROM processes WHERE name LIKE '%malware%';
```

### Resource Analysis

```sql
-- High CPU usage
SELECT * FROM processes WHERE cpu_usage > 80.0;

-- High memory usage
SELECT * FROM processes WHERE memory_usage > 2147483648; -- 2GB
```

### Hash-Based Detection

```sql
-- Known malicious hashes
SELECT * FROM processes
WHERE executable_hash = 'a1b2c3d4e5f6789012345678901234567890abcdef1234567890abcdef';
```

### Command Line Analysis

```sql
-- Suspicious command patterns
SELECT * FROM processes
WHERE command_line LIKE '%nc -l%'     -- Netcat listener
   OR command_line LIKE '%wget%'      -- Download tools
   OR LENGTH(command_line) > 1000;   -- Unusually long commands
```

### Path-Based Detection

```sql
-- Suspicious executable locations
SELECT * FROM processes
WHERE executable_path LIKE '/tmp/%'
   OR executable_path LIKE '/var/tmp/%'
   OR executable_path IS NULL;
```

## Performance Tips

### Use Indexes

- Time-based queries: `WHERE collection_time > ?`
- Process ID queries: `WHERE pid = ?`
- Name queries: `WHERE name = ?`

### Keep Result Sets Small

`LIMIT` is refused at rule load — the planner has no representation for it, so the pushed-down half would ignore it. Narrow the result set with predicates instead.

```sql
-- Narrow with predicates, not LIMIT
SELECT * FROM processes WHERE name LIKE '%test%' AND collection_time > ?;
```

### Avoid Complex Operations

```sql
-- Good: Simple conditions
WHERE name = 'process' AND pid > 1000;

-- Avoid: Complex nested operations
WHERE LENGTH(command_line) > INSTR(command_line, '/') + 50;
```

## Security Best Practices

### Use Parameterized Queries

```sql
-- Good: Parameterized
SELECT * FROM processes WHERE name = ?;

-- Bad: String concatenation
SELECT * FROM processes WHERE name = '" + user_input + "';
```

### Validate Input

- Always validate user-provided SQL fragments
- Use only approved functions
- Check for banned function usage

### Monitor Performance

- Watch for queries that consume excessive resources
- Use query timeouts
- Monitor memory usage
