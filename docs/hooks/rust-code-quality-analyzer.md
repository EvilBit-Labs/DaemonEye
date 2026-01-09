# Rust Code Quality Analysis & Improvement

Analyze ONLY the changed files in the current diff for code quality issues and apply safe improvements.

## EXECUTION STEPS

1. **Identify Changed Files**: List all modified .rs, .proto, Cargo.toml, build.rs files in current diff
2. **Quality Analysis**: Analyze each changed file against the 8 focus categories below
3. **Apply Safe Edits**: Make only mechanical, non-breaking quality improvements
4. **Validate**: Run `just lint && just test` to ensure changes don't break anything
5. **Report**: Provide summary of applied changes and deferred items

## CODE QUALITY CATEGORIES

### 1. Code Smells

- ✅ Remove dead/unreachable code
- ✅ Split oversized internal functions (keep public APIs unchanged)
- ✅ Eliminate duplicate code blocks
- ❌ Flag complex functions for review

### 2. Design Patterns

- ✅ Add private helper traits for internal use
- ✅ Introduce builder patterns for complex constructors (internal)
- ✅ Apply newtype patterns for type safety (internal)
- ❌ Flag major architectural changes for review

### 3. Best Practices

- ✅ Replace blocking I/O with async equivalents in async contexts
- ✅ Follow Rust 2024 edition conventions
- ✅ Apply project-specific conventions
- ✅ Fix clippy warnings and suggestions

### 4. Readability

- ✅ Improve variable/function naming (internal only)
- ✅ Add rustdoc comments to public APIs
- ✅ Improve code structure and organization
- ✅ Add code examples to documentation

### 5. Maintainability

- ✅ Extract internal modules (preserve re-exports)
- ✅ Improve code clarity and documentation
- ✅ Add missing error context
- ❌ Flag major refactoring needs for review

### 6. Performance

- ✅ Eliminate unnecessary `.clone()` calls
- ✅ Add bounds to Vec growth patterns
- ✅ Replace blocking operations in async code
- ✅ Optimize memory allocations

### 7. Type Safety

- ✅ Replace string-based flags with private enums
- ✅ Strengthen type constraints
- ✅ Reduce unnecessary Option/Result nesting
- ✅ Add type aliases for clarity

### 8. Error Handling

- ✅ Add context with `.with_context()`
- ✅ Convert generic errors to structured types (internal)
- ✅ Ensure no silent failures
- ✅ Use thiserror for library errors, anyhow for applications

## SAFE EDITS TO APPLY AUTOMATICALLY

````rust
// ✅ Remove dead code
#[allow(dead_code)]
fn unused_function() {} // Remove entirely

// ✅ Add error context
.map_err(|e| format!("Error: {}", e))
// becomes:
.with_context(|| "Failed to process configuration")

// ✅ Replace blocking I/O in async
std::fs::read_to_string(path)
// becomes:
tokio::fs::read_to_string(path).await

// ✅ Eliminate unnecessary clones
let name = item.name.clone();
process_string(name);
// becomes:
process_string(&item.name);

// ✅ Add rustdoc to public APIs
pub fn process_data(data: &str) -> Result<String> {
// becomes:
/// Processes the input data and returns a formatted result.
///
/// # Examples
/// ```
/// let result = process_data("input")?;
/// ```
pub fn process_data(data: &str) -> Result<String> {
````

## CONSTRAINTS

- **Scope**: Only modify files that appear in the current diff
- **Safety**: All changes must pass `just lint && just test`
- **API Stability**: No changes to public function signatures or visibility
- **Performance**: Avoid changes that add significant overhead
- **Behavior**: Preserve all existing functionality

## OUTPUT FORMAT

```markdown
## Code Quality Analysis Summary
**Files Analyzed**: [list of changed files]
**Safe Edits Applied**: [count]
**Items Deferred**: [count]
**Approval Required**: [count]

### Applied Changes
- [file]: [description of change and rationale]
- [file]: [description of change and rationale]

### Deferred Items
- [file]: [reason for deferral]

### Requires Approval
- [file]: [quality concern requiring human review]

### Quality Assessment
[Overall code quality assessment]

### Next Steps
[Recommended follow-up actions]
```

## VALIDATION COMMANDS

```bash
just lint    # Must pass with zero warnings
just test    # All tests must pass
```

If any validation fails, revert the problematic changes and note in the deferred section.

## PROJECT CONTEXT

DaemonEye is a security-first process monitoring system with:

- Zero-warnings policy
- No unsafe code
- Async I/O throughout
- CLI-first design
- Memory efficiency focus
- Privilege separation architecture

Prioritize clarity and security over cleverness.
