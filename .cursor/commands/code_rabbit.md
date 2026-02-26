# CodeRabbit Review

## Description

Use CodeRabbit to identify issues and follow its recommendations in the current code branch.

## Steps

1. Run `coderabbit --prompt-only`, let it take as long as it needs to identify issues with this code branch. It will output a large list of recommended fixes and considerations.
2. Evaluate the fixes and considerations. Fix major issues only, or fix any critical issues and ignore the nits.
3. Once those changes are implemented, run CodeRabbit CLI one more time to make sure we addressed all the critical issues and didn't introduce any additional bugs.
4. Do not change branches or mess with `git` at all. Just run the coderabbit tool, examine its output, fix its findings, and run it again to make sure you fixed everything.
5. Then run `just ci-check` to make sure you didn't break anything and, if it does not complete without failures, fix those problems.
6. Only run the loop (running coderabbit->fixing its recommendations->running `just ci-check`->fixing any failures) twice.
7. If on the second run you don't find any critical issues, ignore the nits and you're complete.
8. Give me a summary of everything that was completed and why.

## Completion Checklist

- [ ] Code conforms to DaemonEye project rules and security standards (AGENTS.md)
- [ ] Tests pass (`just test`)
- [ ] Linting is clean (`just lint`)
- [ ] Full CI validation passes (`just ci-check`)
- [ ] A short summary of what was done is reported
- [ ] CodeRabbit issues have been addressed
- [ ] CodeRabbit was run no more than twice
- [ ] No unnecessary changes were made beyond addressing critical issues
- [ ] No changes to git branches or history were made
