# expanduser property investigation

Local investigation; nothing pushed or published. Original working tree and its pre-existing edits were left unchanged.

## Branches and production revisions

| Purpose | Branch | Commit | Worktree |
| --- | --- | --- | --- |
| Baseline | `main` | `9c37a10` | original checkout |
| Preserve existing dependency/README edits | `discovery/proptest-praxis` | `a82ad20` | `/tmp/expanduser-discovery` |
| Root account repair | `fix/root-account-home` | `a957682` | `/tmp/expanduser-fix-root` |
| Path semantics repair | `fix/preserve-path-semantics` | `b357def` | `/tmp/expanduser-fix-paths` |
| Combined production fixes | `discovery/proptest-praxis` | `a427d7f` | `/tmp/expanduser-discovery` |

Both focused fix branches start directly from `main`, contain standalone regressions and README updates, and exclude proptest/discovery machinery. Applying both involves resolving the shared parser hunk: use the byte-preserving parser without the root special case, as already done in discovery.

Discovery workers: `investigate/account-properties` (`41d3aa3`) and `investigate/path-properties` (`278bf2f`). Their harness commits are integrated into discovery. The final integration strengthens default filesystem assertions, removes incidental/oracle-only assertions and formats the retained suite; tested production remains `a427d7f`.

## Confirmed bugs

### Root home hardcoded

`~root/.config` returned `/root/.config`, while this host's account database specifies `/var/root/.config`. The fix removes the root special case. An independent `getpwnam_r` regression failed before and passed after; a separate consumer executable also confirmed the corrected output. Baseline property shrank to root with an empty suffix and str representation.

Vault: [root account finding](/Users/kit/Documents/vault/notes/20261105.expanduser-root-account-home.md).

### Component reconstruction changes filesystem semantics

`file` became `file/`, turning a successful regular-file lookup into `ENOTDIR`. Conversely, literal or expanded paths ending in `/` or `/.` could lose their directory requirement and accept a regular file. The same mechanism affects symlinks, nested paths and HOME pointing to a file.

The fix substitutes only the leading tilde expression and preserves the untouched suffix bytes. Non-expanding paths are returned unchanged. The standalone filesystem regression failed before and passed after; a consumer executable verified file reading and both directory-only suffixes. Baseline fixture matrix found 82 discrepancies in 256 cases; fixed matrix has zero.

Vault: [path semantics finding](/Users/kit/Documents/vault/notes/20261105.expanduser-path-semantics.md).

## Retained machinery

- `tests/account_properties.rs`: independent reentrant OS oracle; local account inventory; six public representations; weighted root generation; known/unknown/Unicode/NUL names; byte suffixes; foreign-libc histories; opt-in threaded stress.
- `tests/path_properties.rs`: byte grammar model, explicit HOME operation AST, exhaustive partitions, environment-isolated subprocess, byte/representation checks and independent filesystem observations (inode/device/type or OS error).
- `tests/*.proptest-regressions`: shrunk reproducible seeds. Readable input witnesses are in the reports and focused regression tests.
- `tests/account_properties.assessment.txt` and `tests/path_investigation.txt`: baseline/candidate commands, coverage, causal analysis and blind spots.

The component-equivalence check is only a partial lexical oracle. Default discovery now additionally asserts filesystem semantics. Strict raw mode is stronger than the original documented API and is retained separately; a lexical-only difference such as `//` versus `/` was not counted as a confirmed filesystem bug.

## Consolidated validation

Run from `/tmp/expanduser-discovery`:

```sh
PROPTEST_CASES=8192 PROPTEST_RNG_SEED=20261108 cargo test -- --nocapture
PATH_PROP_CHECK=raw PROPTEST_CASES=16384 PROPTEST_RNG_SEED=20261109 cargo test --test path_properties -- --nocapture
EXPANDUSER_STRESS_ITERS=20000 cargo test --test account_properties concurrent_expansions_with_foreign_libc_calls -- --exact --ignored --nocapture
cargo fmt --check
cargo clippy --all-targets
```

Observed on macOS arm64:

- All default unit/integration/doc suites passed; the optional stress test is ignored in the default run and passed when explicitly invoked.
- 8,192 HOME histories plus 12,560 exhaustive grammar queries yielded 638,990 path queries, zero byte discrepancies. HOME transitions included 67,233 sets, 22,226 empty values and 22,423 unsets.
- Saved semantic failure plus 8,192 generated semantic cases passed; all 256 filesystem matrix cases, seven relative witnesses and HOME=file witness passed.
- Saved lexical failures plus 16,384 fresh raw-byte cases passed.
- 133 inventoried accounts; 2,376 deterministic nonroot account/representation/suffix combinations, root witnesses and three 8,192-case account properties passed.
- Six-thread stress passed 80,000 expansions plus 40,000 foreign libc calls. This does not prove absence of races.
- Formatting check passed. Clippy exited successfully with four warnings: existing unused `std::env`, a manual C-string literal in the root regression, and two discovery-generator `repeat().take()` suggestions. No warning was suppressed.
- Standalone consumer smoke observed `/var/root/.config`, readable expanded `Cargo.toml`, and preserved `ENOTDIR` for `Cargo.toml/` and `Cargo.toml/.`.

### Actual mutation calibration

In an isolated detached worktree at `a427d7f`, replace suffix-byte append with an append of `Path::components().as_path()`. Running strict semantic mode with 256 cases and seed `20261108` fails, shrinking to `~/file/`: expected `ENOTDIR`, observed a regular file. Restore the mutation and its generated seed. No mutant is retained in the production branches. Initial oracle-only calibration assertions were removed rather than counted as evidence that production mutations are detected.

## Completed targets and unresolved leads

Both parser/HOME and named-account slices are complete for this local investigation. After initial failures, searches expanded to symlink/parent/dot interactions, arbitrary byte tails, repeated HOME changes, all account representations and foreign libc calls. No additional independent runtime defect was demonstrated.

Private safety lead: Linux documents `getpwnam` as MT-Unsafe; macOS instead uses per-thread storage but discourages its use inside libraries. No Linux runtime was available through Docker (client present, daemon absent), and no Linux race or exploit was reproduced. No speculative safety fix is included. See the [private vault assessment](/Users/kit/Documents/vault/notes/20261105.expanduser-account-lookup-safety-private.md) and [Linux contract](https://man7.org/linux/man-pages/man3/getpwnam.3.html). Keep this lead local/private pending platform investigation.

Other blind spots: missing current UID fallback, live account-database mutation, remote directory failures, giant records/ERANGE injection, null home pointers, locale changes, allocator failure, existing Unicode-named accounts, other Unix filesystems and permission-sensitive traversal. The crate currently imports Unix APIs unconditionally; Windows support was not assumed or tested.

Resumption priorities: run the account stress under Linux sanitizers, obtain a deterministic schedule for any shared-storage failure, and assess a caller-owned `getpwnam_r` repair. A crate-local mutex would not protect against foreign libc callers. Account error/fault injection is the next independent oracle opportunity.
