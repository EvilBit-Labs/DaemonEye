# Update LLMs.txt File

## Description

Update the llms.txt file in the root folder to reflect changes in documentation or specifications following the llms.txt specification at <https://llmstxt.org/>.

## Steps

> SCOPE LIMITATION (IMPORTANT – READ FIRST)
>
> When executing this prompt you MUST restrict all repository content analysis exclusively to the already-provided attachments for `#file:../../spec/` `#file:../../docs` (including their subpaths such as `docs/src/` and `spec/*.md`), and the root `AGENTS.md` file. Treat those attachments as complete and authoritative for the purpose of updating `llms.txt`.
>
> DO NOT attempt to read, open, or re-scan any other project files (e.g. `AGENTS.md`, `.kiro/steering/development.md`, other `.github/` instructions, source code, lockfiles, coverage reports) even if tools are available. Avoid recursive or repeated attempts to fetch additional instruction files. If a step below would normally "scan the repo", interpret it narrowly: only enumerate Markdown files inside the provided `docs` and `spec` attachment trees plus top-level root Markdown files that are already known (you may assume `README.md`, `LICENSE`, `SECURITY.md` exist without re-reading them unless their content is required for a description — which it is not).
>
> If a tool invocation would cause broader traversal, SKIP it and proceed using the attachment lists. This prevents infinite loops and unnecessary I/O. The change detection algorithm should operate purely on:
>
> 1. Root-level Markdown files (assumed: `README.md`, `SECURITY.md`, `LICENSE`, and you MAY read `AGENTS.md` for authoritative project rules)
> 2. `spec/*.md`
> 3. `docs/src/**/*.md`
>
> Ignore generated HTML in `docs/book/` and any non-Markdown assets. Treat them as excluded artifacts automatically. Do not add them to `llms.txt`.

Update the existing `llms.txt` file in the root of the repository to reflect changes in documentation, specifications, or repository structure. This file provides high-level guidance to large language models (LLMs) on where to find relevant content for understanding the repository's purpose and specifications.

---

## TL;DR (Quick Start for the Coding Agent)

Perform these steps in order; do not skip validation:

1. Read existing `/llms.txt` (if missing, treat as new file creation).
2. Enumerate repo docs: top-level `*.md`, `docs/**`, `spec/**`, component crate `README` / key `Cargo.toml` metadata, security & contribution files.
3. Detect additions / removals vs current file (simple set diff on relative paths referenced).
4. Classify candidate files using Inclusion Heuristics (see below).
5. Draft updated sections preserving required structure (H1, optional blockquote, H2 category lists).
6. Ensure link syntax `[Readable Name](relative/path.md): concise description`.
7. Run internal validation checklist (structure, dead links, redundancy, ordering, diff sanity).
8. Output ONLY the new `llms.txt` file content (no commentary) when executing the update.

If any critical invariant fails (see Invariants) you MUST adjust before finalizing.

---

## Primary Directive

Update the existing `llms.txt` file to maintain accuracy and compliance with the llms.txt specification while reflecting current repository structure and content. The file must remain optimized for LLM consumption while staying human-readable.

NOTE ON SCOPE ENFORCEMENT: All subsequent references to "scan", "enumerate", or "discover" files are to be interpreted under the Scope Limitation above. Do not widen scope.

## Analysis and Planning Phase

Before updating the `llms.txt` file, you must complete a thorough analysis:

### Step 1: Review Current File and Specification

- Read the existing `llms.txt` file to understand current structure, if it exists yet
- Review the official specification at <https://llmstxt.org/> to ensure continued compliance
- Identify areas that may need updates based on repository changes

### Step 2: Repository Structure Analysis

- Examine the current repository structure using appropriate tools
- Compare current structure with what's documented in existing `llms.txt`
- Identify new directories, files, or documentation that should be included
- Note any removed or relocated files that need to be updated

### Step 3: Content Discovery and Change Detection

- Identify new README files and their locations
- Find new documentation files (`.md` files in `/docs/`, `/spec/`, etc.)
- Locate new specification files and their purposes
- Discover new configuration files and their relevance
- Find new example files and code samples
- Identify any changes to existing documentation structure

### Step 4: Create Update Plan

Based on your analysis, create a structured plan that includes:

- Changes needed to maintain accuracy
- New files to be added to the llms.txt
- Outdated references to be removed or updated
- Organizational improvements to maintain clarity

## Implementation Requirements

### Format Compliance

The updated `llms.txt` file must maintain this exact structure per the specification:

1. **H1 Header**: Single line with repository/project name (required)
2. **Blockquote Summary**: Brief description in blockquote format (optional but recommended)
3. **Additional Details**: Zero or more markdown sections without headings for context
4. **File List Sections**: Zero or more H2 sections containing markdown lists of links

### Content Requirements

#### Required Elements

- **Project Name**: Clear, descriptive title as H1
- **Summary**: Concise blockquote explaining the repository's purpose
- **Key Files**: Essential files organized by category (H2 sections)

#### File Link Format

Each file link must follow: `[descriptive-name](relative-url): optional description`

#### Section Organization

Organize files into logical H2 sections such as:

- **Documentation**: Core documentation files
- **Specifications**: Technical specifications and requirements
- **Examples**: Sample code and usage examples
- **Configuration**: Setup and configuration files
- **Optional**: Secondary files (special meaning - can be skipped for shorter context)

### Content Guidelines

#### Language and Style

- Use concise, clear, unambiguous language
- Avoid jargon without explanation
- Write for both human and LLM readers
- Be specific and informative in descriptions

#### File Selection Criteria

Include files that:

- Explain the repository's purpose and scope
- Provide essential technical documentation
- Show usage examples and patterns
- Define interfaces and specifications
- Contain configuration and setup instructions

Exclude files that:

- Are purely implementation details
- Contain redundant information
- Are build artifacts or generated content
- Are not relevant to understanding the project

---

## Inclusion / Exclusion Heuristics

Score each candidate (keep those scoring >= 2 unless intentionally excluded):

| Criterion              | +1 Signal                                          |
| ---------------------- | -------------------------------------------------- |
| Orientation Value      | Explains purpose, architecture, security model     |
| Specification          | Defines contracts, limits, protocols, data formats |
| Operator Critical      | Install, deploy, config, security hardening        |
| Cross-Cutting Policy   | Contribution, security, licensing, threat model    |
| Representative Example | Shows canonical usage or pattern                   |

Negative Exclusion Signals (any one usually drops): vendor lock file, autogenerated, code-only without explanatory context, temporary / experimental docs.

Prefer the smallest representative set when many similar files exist (e.g., keep `architecture.md` but not all derived slide decks or exports).

---

## Invariants (MUST Always Hold)

01. File name EXACTLY `llms.txt` at repo root.
02. Single leading H1 only (no multiple H1s).
03. All links are relative paths that exist at commit time.
04. No absolute filesystem paths, no external HTTP links inside file list sections (context isolation).
05. No duplicate file references.
06. Descriptions \<= 140 chars, imperative/concise, no trailing periods unless multiple sentences needed.
07. Section order: Documentation → Specifications → Examples → Configuration → Optional (omit empty ones without leaving gaps).
08. Deterministic ordering inside sections (alphabetical by display name unless logical order is strongly beneficial; if logical order used, it must be consistent and minimal).
09. Do not include compiled artifacts, coverage reports, `target/`, lockfiles (unless spec interest), or large binary assets.
10. Preserve semantic meaning: do not rewrite project intent.

---

## Output Contract

When finalizing, produce ONLY the complete desired contents of `/llms.txt` (no extra markdown fences, no commentary, no diff). If creation is not needed (no changes), you should explicitly state "NO CHANGE" instead of re‑emitting identical content (optimization for agents that may skip writes).

---

## Change Detection Algorithm (Deterministic)

1. Parse existing file, extract referenced relative paths.
2. Glob for candidate docs: `*.md` in root, `docs/**/*.md`, `spec/**/*.md`, crate root docs (`*/README.md`), security & community files (LICENSE, SECURITY.md, CONTRIBUTING.md, CODE_OF_CONDUCT\* if present), config guides.
3. Build two sets: CURRENT_REFERENCED, CANDIDATES.
4. NEW = CANDIDATES − CURRENT_REFERENCED filtered through heuristics.
5. STALE = CURRENT_REFERENCED − CANDIDATES (verify truly removed vs renamed via simple name match search).
6. For renames, map old → new path and update entry in place (preserve description with minor adjustments).
7. Recompute categories; if a section becomes empty, drop it.
8. Produce updated ordered lists, ensuring invariants.

---

## Failure Modes & Recovery

| Failure                   | Mitigation                                                                      |
| ------------------------- | ------------------------------------------------------------------------------- |
| Dead link introduced      | Re-scan path; if file newly added but uncommitted, note and omit until present. |
| Overly verbose list       | Apply heuristics; collapse by linking an umbrella doc instead of every subpage. |
| Missing critical spec     | Escalate by adding under Specifications with concise description.               |
| Duplicate classification  | Keep in first most appropriate section; remove from others.                     |
| Section bloat (>15 items) | Split logically or prune low-signal entries (score \<2).                        |

---

## Non-Goals

- Not a full index of source code.
- Not a changelog replacement.
- Not a substitute for inline docs.
- Avoid summarizing file contents; only describe purpose.

---

## Minimal vs Comprehensive Example (Illustrative)

Minimal (small repo): 1 H1, 1 blockquote, 1 Documentation section with 3–6 items. Comprehensive (larger repo like this): Up to 5 sections, each ≤ 12 items, Optional section is last and may be omitted in constrained contexts.

---

## Validation Checklist (Condensed)

Run prior to output:

1. Structure: Single H1, ordered sections, no empty lists.
2. Links: All referenced relative paths exist (limit scope to root *.md, spec/*.md, docs/src/\*\*/\*.md).
3. No duplicates: A file appears in exactly one section.
4. Descriptions: \<=140 chars, concise, no trailing period unless multi-sentence.
5. Coverage: Include `README.md`, `AGENTS.md`, key `spec/*.md`, core `docs/src/*.md` (architecture, security, deployment).
6. Exclusions: Omit generated `docs/book/**`, binaries, coverage, `target/`, lockfiles.
7. Ordering: Deterministic (alphabetical or intentional logical grouping documented in comments if used).
8. Delta sanity: NEW and STALE sets evaluated under constrained scope.

---

## Execution Steps

### Step 1: Current State Analysis

1. Read the existing `llms.txt` file thoroughly
2. Examine the current repository structure completely
3. Compare existing file references with actual repository content
4. Identify outdated, missing, or incorrect references
5. Note any structural issues with the current file

### Step 2: Content Planning

1. Determine if the primary purpose statement needs updates
2. Review and update the summary blockquote if needed
3. Plan additions for new files and directories
4. Plan removals for outdated or moved content
5. Reorganize sections if needed for better clarity

### Step 3: File Updates

1. Update the existing `llms.txt` file in the repository root
2. Maintain compliance with the exact format specification
3. Add new file references with appropriate descriptions
4. Remove or update outdated references
5. Ensure all links are valid relative paths

### Step 4: Validation

1. Verify continued compliance with <https://llmstxt.org/> specification
2. Check that all links are valid and accessible
3. Ensure the file still serves as an effective LLM navigation tool
4. Confirm the file remains both human and machine readable

## Quality Assurance

### Format Validation

- ✅ H1 header with project name
- ✅ Blockquote summary (if included)
- ✅ H2 sections for file lists
- ✅ Proper markdown link format
- ✅ No broken or invalid links
- ✅ Consistent formatting throughout

### Content Validation

- ✅ Clear, unambiguous language
- ✅ Comprehensive coverage of essential files
- ✅ Logical organization of content
- ✅ Appropriate file descriptions
- ✅ Serves as effective LLM navigation tool

### Specification Compliance

- ✅ Follows <https://llmstxt.org/> format exactly
- ✅ Uses required markdown structure
- ✅ Implements optional sections appropriately
- ✅ File located at repository root (`/llms.txt`)

## Update Strategy

### Addition Process

When adding new content:

1. Identify the appropriate section for new files
2. Create clear, descriptive names for links
3. Write concise but informative descriptions
4. Maintain alphabetical or logical ordering within sections
5. Consider if new sections are needed for new content types

### Removal Process

When removing outdated content:

1. Verify files are actually removed or relocated
2. Check if relocated files should be updated rather than removed
3. Remove entire sections if they become empty
4. Update cross-references if needed

### Reorganization Process

When restructuring content:

1. Maintain logical flow from general to specific
2. Keep essential documentation in primary sections
3. Move secondary content to "Optional" section if appropriate
4. Ensure new organization improves LLM navigation

Example structure for `llms.txt`:

```markdown
# [Repository Name]

> [Concise description of the repository's purpose and scope]

[Optional additional context paragraphs without headings]

## Documentation

- [Main README](README.md): Primary project documentation and getting started guide
- [Contributing Guide](CONTRIBUTING.md): Guidelines for contributing to the project
- [Code of Conduct](CODE_OF_CONDUCT.md): Community guidelines and expectations

## Specifications

- [Technical Specification](spec/technical-spec.md): Detailed technical requirements and constraints
- [API Specification](spec/api-spec.md): Interface definitions and data contracts

## Examples

- [Basic Example](examples/basic-usage.md): Simple usage demonstration
- [Advanced Example](examples/advanced-usage.md): Complex implementation patterns

## Configuration

- [Setup Guide](docs/setup.md): Installation and configuration instructions
- [Deployment Guide](docs/deployment.md): Production deployment guidelines

## Optional

- [Architecture Documentation](docs/architecture.md): Detailed system architecture
- [Design Decisions](docs/decisions.md): Historical design decision records
```

Note: The above example block uses illustrative file paths that may not exist in this repository; they are placeholders to demonstrate formatting only.

## Success Criteria

The updated `llms.txt` file should:

1. Accurately reflect the current repository structure and content
2. Maintain compliance with the llms.txt specification
3. Provide clear navigation to essential documentation
4. Remove outdated or incorrect references
5. Include new important files and documentation
6. Maintain logical organization for easy LLM consumption
7. Use clear, unambiguous language throughout
8. Continue to serve both human and machine readers effectively

---

## Agent Implementation Notes

- Prefer idempotent operations: if no change required, respond with "NO CHANGE".
- If changes small (≤3 edits), still re-emit full file (atomic replace model).
- Use stable naming: Convert file names to Title Case (minus extensions) unless a proper noun/acronym (e.g., "API", "IPC").
- For Rust workspace crates, generally include only root `README.md` or high-level `lib.rs` doc if it acts as specification (otherwise omit code internals).
- Security-critical docs (SECURITY.md, threat models) ALWAYS included unless empty.
- If both `docs/` and `spec/` contain overlapping material, prefer placing normative protocol details under Specifications, conceptual overviews under Documentation.
