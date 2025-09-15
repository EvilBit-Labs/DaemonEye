---
mode: agent
model: Auto (copilot)
tools: ['githubRepo', 'edit', 'search', 'new', 'runCommands', 'runTasks', 'usages', 'think', 'problems', 'changes', 'testFailure', 'openSimpleBrowser', 'fetch', 'extensions', 'todos', 'memory', 'brave_web_search', 'browser_click', 'browser_close', 'browser_console_messages', 'browser_drag', 'browser_evaluate', 'browser_file_upload', 'browser_fill_form', 'browser_handle_dialog', 'browser_hover', 'browser_navigate', 'browser_navigate_back', 'browser_network_requests', 'browser_press_key', 'browser_resize', 'browser_select_option', 'browser_snapshot', 'browser_tabs', 'browser_take_screenshot', 'browser_type', 'browser_wait_for', 'cancel_workflow_run', 'checkRepository', 'delete_workflow_run_logs', 'docker', 'download_workflow_run_artifact', 'fetch', 'get_code_scanning_alert', 'get_commit', 'get_dependabot_alert', 'get_discussion_comments', 'get_file_contents', 'get_global_security_advisory', 'get_issue', 'get_issue_comments', 'get_job_logs', 'get_latest_release', 'get_me', 'get_notification_details', 'get_pull_request', 'get_pull_request_comments', 'get_pull_request_diff', 'get_pull_request_files', 'get_pull_request_reviews', 'get_pull_request_status', 'get_release_by_tag', 'get_secret_scanning_alert', 'get_tag', 'get_workflow_run', 'get_workflow_run_logs', 'get_workflow_run_usage', 'get-library-docs', 'getRepositoryInfo', 'getRepositoryTag', 'list_branches', 'list_code_scanning_alerts', 'list_commits', 'list_dependabot_alerts', 'list_global_security_advisories', 'list_issue_types', 'list_issues', 'list_notifications', 'list_org_repository_security_advisories', 'list_pull_requests', 'list_releases', 'list_repository_security_advisories', 'list_secret_scanning_alerts', 'list_sub_issues', 'list_tags', 'list_workflow_jobs', 'list_workflow_run_artifacts', 'list_workflow_runs', 'list_workflows', 'rerun_failed_jobs', 'rerun_workflow_run', 'resolve-library-id', 'run_workflow', 'search_issues', 'search_pull_requests', 'search_repositories', 'submit_pending_pull_request_review', 'update_issue']
description: Work on the next unchecked task in the current task list.
---
1. Read the entire currently open task list document before beginning. Do not skip this step.
2. **Gather Context**: Before starting work on the task:
   - If the task list is in a folder, check for `requirements.md` and `design.md` files in the same directory and read them for essential context
   - If the task item contains a link to a GitHub issue, examine the issue thoroughly for additional context, acceptance criteria, and potential solutions
   - Review any referenced documentation or specifications
3. Identify the next unchecked task in the checklist. The task will typically have an associated github issue linked to it with additional context and a potential solution that should be reviewed as well.

> ⚠️ Important: Some tasks may appear implemented but are still unchecked.
> You must verify that each task meets all project standards defined in AGENTS.md.
> "Complete" means the code is fully implemented, idiomatic, tested, lint-free, follows SentinelD's security-first architecture, and aligns with all coding and architectural rules.

#### Task Execution Process

- Review the codebase to determine whether the task is already complete **according to project standards**.
- If the task is not fully compliant:
  - Make necessary code changes using idiomatic, maintainable approaches following SentinelD's security-first patterns.
  - Run `just fmt` to apply formatting rules.
  - Add or update tests to ensure correctness.
  - Run the test suites:
    - `just test`
  - Fix any failing tests.
  - Run the linters:
    - `just lint`
  - Fix all linter issues.
- Run `just ci-check` to confirm the full codebase passes comprehensive CI validation (format, lint, test, build, audit).

#### Completion Checklist

- [x] Code conforms to SentinelD project rules and security standards (AGENTS.md)
- [x] Tests pass (`just test`)
- [x] Linting is clean (`just lint`)
- [x] Full CI validation passes (`just ci-check`)
- [x] Task is marked complete in the checklist
- [x] A short summary of what was done is reported

> Update the current task list with any items that are implemented and need test coverage, checking off items that have implemented tests.
> ❌ Do **not** commit or check in any code
> ⏸️ Do **not** begin another task
> ✅ Stop and wait for further instruction after completing this task
