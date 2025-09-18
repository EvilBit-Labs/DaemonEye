# SQL Dialect Quick Reference

## Allowed Functions

### String Functions

| Function                     | Description             | Example                            |
| ---------------------------- | ----------------------- | ---------------------------------- |
| `LENGTH(str)`                | String length           | `LENGTH(command_line)`             |
| `SUBSTR(str, start, length)` | Substring extraction    | `SUBSTR(executable_path, 1, 10)`   |
| `INSTR(str, substr)`         | Find substring position | `INSTR(command_line, 'malicious')` |
| `LIKE pattern`               | Pattern matching        | `name LIKE '%suspicious%'`         |

### Encoding Functions

| Function     | Description              | Example                |
| ------------ | ------------------------ | ---------------------- |
| `HEX(data)`  | Convert to hexadecimal   | `HEX(executable_hash)` |
| `UNHEX(hex)` | Convert from hexadecimal | `UNHEX('deadbeef')`    |

### Mathematical Functions

| Function    | Description    | Example                     |
| ----------- | -------------- | --------------------------- |
| `COUNT(*)`  | Count rows     | `COUNT(*) as process_count` |
| `SUM(expr)` | Sum values     | `SUM(memory_usage)`         |
| `AVG(expr)` | Average values | `AVG(cpu_usage)`            |
| `MAX(expr)` | Maximum value  | `MAX(memory_usage)`         |
| `MIN(expr)` | Minimum value  | `MIN(start_time)`           |

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
- `regexp()` - Regular expressions
- `match()` - Pattern matching

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

### Limit Result Sets

```sql
-- Use LIMIT for large queries
SELECT * FROM processes WHERE name LIKE '%test%' LIMIT 100;
```

### Avoid Complex Operations

```sql
-- Good: Simple conditions
WHERE name = 'process' AND pid > 1000;

-- Avoid: Complex nested operations
WHERE LENGTH(SUBSTR(command_line, 1, 100)) > 50;
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
