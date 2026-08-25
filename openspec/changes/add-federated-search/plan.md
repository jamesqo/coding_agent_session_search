# Federated Search Delivery Plan

Status: approved

## Scope

Add opt-in federated `search` and remote `view` over existing SSH/Tailscale connectivity. Preserve all local indexing, storage, semantic retrieval, reranking, and local-only JSON behavior. Do not add synchronization, discovery, a daemon, HTTP, database changes, an async runtime, or a generic transport abstraction.

Delivery is phased with verification at each behavior boundary and the complete repository gate once at final consolidation.

## Current Veritas Gate State

Project-bound `vtas` CLI access and the native `rust-test` evidence producer are available. The seven new `federated-search/*` claims are present as provisional Markdown claims. Current Veritas status reports seven expected `MISSING_LOCK_ENTRY` findings and rust-test provider drift because acceptance evidence has not yet been implemented or discovered. There are no unrelated advisory findings and no coverage exclusions.

The completion gate is fresh claim reconciliation, Rust evidence discovery, semantic link review, approval, and clean project-bound status/report after the implementation tests pass.

## Preservation and Change Contract

| ID | Disposition | Contract |
|---|---|---|
| PC-1 | Preserve | With no selected node, `search` and `view` retain their current execution and JSON shapes. |
| PC-2 | Evolve compatibly | `SearchHit` gains deserialization and conditionally omitted provenance fields; local storage/search semantics remain unchanged. |
| PC-3 | Evolve compatibly | `app/cli.rs` gains public node flags and hidden versioned federation request modes without adding operational commands. |
| PC-4 | Add | `app/federation.rs` becomes the sole owner of node selection, SSH child lifecycle, protocol envelopes, outcomes, and merge logic. |
| PC-5 | Preserve | Existing SSH/Tailscale configuration and deployment own connectivity and authentication; CASS never changes them. |

## Stack and Target Structure

```text
app/cli.rs
  ├─ local search/view (preserved)
  └─ federated composition
       └─ app/federation.rs
            ├─ node selection + validation
            ├─ version-1 stdin/stdout envelopes
            ├─ bounded SSH process execution
            └─ deterministic rank merge

app/storage.rs
  └─ concrete serializable/deserializable SearchHit

app/tests/cli_contract.rs
  └─ public CLI/process acceptance evidence
```

Rust standard-library threads and processes provide concurrency. Serde/serde_json provide the already-approved boundary format. No manifest or lockfile change is planned.

## Components

### Node selection and validation

- Outcome: deterministic `--node`/environment precedence and early rejection.
- Foundation: Clap parsing and typed JSON `AppError::usage`.
- Net-new: bounded alias grammar, stable deduplication, sixteen-node cap.
- Owns claims: `node-selection-precedence`, `node-validation`.
- Tests: private pure unit tests plus CLI integration cases proving precedence and rejection before fake SSH execution.

### Versioned SSH request runner

- Outcome: safe constant remote commands with bounded child lifetime and classified failures.
- Foundation: deployed `~/.local/bin/cass`, OpenSSH, JSON-only CLI boundary.
- Net-new: protocol v1 envelopes, hidden local-only modes, stdout/stderr drain threads, five-second kill/wait path.
- Owns claims: `concurrent-fanout`, `partial-failure`.
- Tests: fake `ssh` executables exercise success, malformed output, nonzero exit, and timeout without requiring a network.

### Search aggregation

- Outcome: origin-aware, deterministic, limited federated results.
- Foundation: current final local lexical/hybrid ranking.
- Net-new: reciprocal-rank calculation, identity deduplication, origin aggregation, node outcomes, conditional response fields.
- Owns claims: `deterministic-merge`, `response-provenance`.
- Tests: pure merge tests and end-to-end local-plus-two-node CLI acceptance.

### Remote view

- Outcome: retrieve context from a selected result origin through the same safe protocol runner.
- Foundation: current local `view` response.
- Net-new: `--node`, view request/response envelopes, compatible response validation.
- Owns claim: `remote-view`.
- Tests: local compatibility and fake-SSH remote context acceptance.

## Design Justification

The only cross-process consumer is a compatible CASS binary, so one concrete protocol and one concrete SSH runner are smaller than a trait, daemon, or transport registry. A standard-library thread per node is bounded at sixteen and isolates blocking process I/O without introducing Tokio. Final node rankings are merged by rank because raw BM25 is corpus-dependent; duplicates take the maximum rank contribution so synchronized copies do not gain artificial relevance.

## Delivery Plan

- [x] **PH-1 — Lock CLI selection behavior with failing tests.** Depends on: none. Add acceptance and pure tests for explicit-node precedence, environment fallback, local-only compatibility, deduplication, node cap, and invalid aliases. Exit: new tests compile and fail only because federation is absent.
- [x] **PH-2 — Implement node selection and protocol-safe SSH execution.** Depends on: PH-1. Add `app/federation.rs`, versioned request envelopes, hidden local-only endpoints, concurrent pipe draining, deadline kill/wait, and typed node outcomes. Exit: selection and subprocess boundary tests pass, including timeout and malformed/nonzero responses.
- [x] **PH-3 — Implement federated search merge and response provenance.** Depends on: PH-2. Refactor current local search into a concrete reusable result function, fan out while it executes, merge/deduplicate results, and conditionally serialize federated fields. Exit: local response golden shape is unchanged and search federation claims pass through CLI evidence.
- [x] **PH-4 — Implement origin-aware remote view.** Depends on: PH-2. Route explicit remote view through the versioned request runner while preserving local view. Exit: local and fake-remote view acceptance tests pass.
- [x] **PH-5 — Integration and Veritas consolidation.** Depends on: PH-3, PH-4. Run full Nextest, strict Clippy, rustfmt, doctests, lexical build/tests, reconcile provisional claims, discover/link/review Rust evidence, and require clean Veritas status/report. Push `main`, require CI/deployment success, then smoke-test real federated search and remote view across Xenia and both Macs. Exit: every plan checkbox is complete, all seven claims have approved runnable evidence, and no federation child remains running.

Dependency edges: `PH-1 → PH-2 → {PH-3, PH-4} → PH-5`.

Initial ready set: `PH-1`. PH-3 and PH-4 are structurally parallel after PH-2 but both touch `app/cli.rs`, so one implementation session should sequence their edits to avoid ownership conflict.

### PH-1 Execution Contract

Status: implemented. Depends on: none. Verification role: intermediate under plan-final cadence.

- **Outcome:** Public tests fix node-selection precedence, validation, deduplication, bounded count, and local-response preservation before production code changes.
- **Owns:** `app/tests/cli_contract.rs` and test-only helpers added there. No production symbols or generated Veritas state.
- **Execution:** Add a local database fixture and fake-SSH observation path; assert explicit flags replace environment defaults and duplicates contact once. Add a table of invalid aliases and a seventeen-node bound case that fail as typed JSON usage without executing SSH. Preserve a local-only response assertion proving federated fields are absent.
- **Focused proof:** `cargo nextest run -E 'test(federated) | test(local_search_response_omits_federated_fields)'`.
- **Expected transition:** Existing suite stays green; the new acceptance cases compile, then fail because `--node` and federation behavior do not exist. Runnable evidence remains pending until implementation turns them green.
- **Isolation:** No ready sibling exists; PH-2 consumes these tests.

Completion record: baseline Nextest passed 39/39 with one ignored real-model test. The focused four-test set produced one preserved local-only pass and three expected product-red failures for absent explicit federation, ignored environment defaults, and missing node validation. No production code changed; PH-2 is ready.

### PH-2 Execution Contract

Status: implemented. Depends on: PH-1 implemented. Verification role: intermediate under plan-final cadence.

- **Outcome:** One private federation module safely selects nodes, exchanges version-1 JSON with a compatible remote CASS, bounds output and process lifetime, and classifies every remote result without shell interpolation.
- **Owns:** new `app/federation.rs`; module declaration in `app/lib.rs`; hidden federation-request flags and versioned envelope variants in `app/cli.rs`; serialization derives in `app/storage.rs`; focused unit and CLI protocol tests. PH-3/PH-4 public routing remains excluded.
- **Execution:** Implement table-driven alias validation, stable deduplication, explicit-over-environment selection, and sixteen-node cap. Add concrete search/view request envelopes. Add a concrete SSH runner whose executable and timeout are private parameters for deterministic tests; production uses `ssh` and five seconds. Drain bounded stdout/stderr on reader threads while polling, kill/wait on deadline, and reject protocol mismatch or malformed output. Add hidden local-only request handlers that never consult node defaults.
- **Focused proof:** federation module unit tests plus CLI invocations of hidden request mode; fake executables cover success, nonzero, malformed, oversized, and short injected deadline behavior without network access.
- **Expected transition:** PH-1 selection cases become green only for selection/validation foundations; public federation stays pending PH-3. `concurrent-fanout` and `partial-failure` evidence remains provisional until public composition.
- **Isolation:** No sibling phase is ready. PH-3 and PH-4 consume the runner and envelopes.

Completion record: the hidden version-1 search endpoint passes its process-level acceptance test while ignoring configured federation defaults. Unit evidence passes for the alias grammar, stable deduplication, sixteen-node cap, malformed response classification, and injected deadline kill/reap path. The two remaining focused failures are the intentionally deferred public composition cases owned by PH-3.

### PH-3 Execution Contract

Status: implemented. Depends on: PH-2 implemented. Verification role: intermediate under plan-final cadence.

- **Outcome:** Public federated search executes local retrieval and bounded remote calls concurrently, preserves partial results, and returns one deterministic provenance-bearing result set.
- **Owns:** public search routing in `app/cli.rs`; merge logic and merge unit tests in `app/federation.rs`; optional federated-only fields on `SearchResponse` and `SearchHit`; fake-SSH search acceptance coverage in `app/tests/cli_contract.rs`.
- **Execution:** Spawn one scoped standard-library thread per validated node before local search. Join handles in selected-node order, record every outcome, and merge successful final-ranked lists with `1 / (rank + 1)`. Deduplicate by provider/conversation/message identity, retain the maximum contribution rather than adding duplicate copies, aggregate stable unique origins, use deterministic score/origin/identity ties, and enforce the requested final limit.
- **Focused proof:** pure merge tests for duplicate and tie behavior plus public CLI tests for provenance, partial failure, and concurrent fake nodes; the existing local-only omission assertion remains green.
- **Expected transition:** the two PH-1 public federation tests become green and search-related provisional claims gain runnable evidence. Remote view remains explicitly deferred to PH-4.
- **Isolation:** PH-4 is logically ready but shares `app/cli.rs`; execute it after this phase to avoid overlapping ownership.

Completion record: focused unit and process evidence passes. Two one-second fake nodes complete in about one second, mixed success/nonzero nodes return both usable results and ordered outcomes, synchronized identities retain a maximum score of one with both origins, and the original local-only response omits all federated fields.

### PH-4 Execution Contract

Status: implemented. Depends on: PH-2 implemented. Verification role: intermediate under plan-final cadence.

- **Outcome:** `cass view <id> --node <alias>` retrieves the remote node's compatible JSON context through the same bounded protocol runner while unqualified view remains local and unchanged.
- **Owns:** public view routing in `app/cli.rs` and fake-SSH view acceptance in `app/tests/cli_contract.rs`; it reuses PH-2 envelopes and child lifecycle unchanged.
- **Execution:** Validate the explicit alias, issue a version-1 `ViewRequest`, return the successful remote `ViewResponse` without presentation changes, and convert classified remote failure into a typed CASS error. Hidden view request mode must always execute locally and ignore federation defaults.
- **Focused proof:** process-level fake-SSH remote view plus hidden local request and existing local view coverage.
- **Expected transition:** `remote-view` gains runnable evidence and all implementation phases become ready for final consolidation.
- **Isolation:** no sibling implementation work remains; PH-5 consumes the integrated tree.

Completion record: the new public remote-view acceptance test first failed at the deliberate unimplemented route, then passed after routing through the shared version-1 runner. Its fake remote verifies the request ID, returns context unchanged, and records the fixed SSH command; the hidden view endpoint independently proves local execution.

### PH-5 Execution Contract

Status: implemented. Depends on: PH-3 and PH-4 implemented. Verification role: final consolidation.

- **Outcome:** the integrated feature satisfies repository quality gates, Veritas coverage gates, deployment CI, and real three-machine smoke behavior.
- **Owns:** only corrective edits revealed by full validation, generated Veritas state through `vtas`, final plan status, commit, push, and deployment/smoke evidence.
- **Execution:** run the complete semantic and lexical build/test matrix, strict Clippy, rustfmt, and doctests. Reconcile all provisional claims, discover/link/review/approve runnable evidence, and require clean project status/report. Review the final diff for scope and safety, commit and push `main`, observe deployment completion, then exercise federated search and origin-aware view against both Macs from Xenia.
- **Focused proof:** none; this is the single full repository gate required by the plan-final cadence.
- **Expected transition:** every checkbox and claim is complete, CI is green, and deployed binaries interoperate on all three nodes.
- **Isolation:** final serial integration only.

Completion record: semantic Nextest passed 53/53 with only the explicit real-model test skipped; lexical-only Nextest passed 46/46. Strict Clippy passed for both feature realizations, rustfmt and doctests passed, OpenSpec strict validation passed, and the refreshed Veritas report contained 35 covered claims, 53 approved links, and zero findings. Commit `a9632c6c` passed CI and the cloud cross-compile/deploy workflow. Installed binaries on Xenia, dev-macbook, and personal-macbook expose federation; a live Xenia search reported successful lexical outcomes from both Macs, and remote view returned compatible JSON from each.

## Traceability and Evidence Assignment

| Claim | Owning phase | Runnable evidence |
|---|---|---|
| `federated-search/node-selection-precedence` | PH-1/PH-2 | CLI integration tests with a fake SSH executable and environment isolation |
| `federated-search/node-validation` | PH-1/PH-2 | unit grammar table plus CLI rejection test proving fake SSH was not called |
| `federated-search/concurrent-fanout` | PH-2/PH-3 | timed fake-node integration showing parallel completion and stdin request capture |
| `federated-search/partial-failure` | PH-2/PH-3 | mixed success/nonzero/malformed/timeout CLI tests |
| `federated-search/deterministic-merge` | PH-3 | pure merge order/deduplication tests and final-limit CLI assertion |
| `federated-search/response-provenance` | PH-3 | local JSON compatibility plus mixed-mode federated JSON acceptance |
| `federated-search/remote-view` | PH-4 | local/remote CLI view integration tests |

Private timeout polling, pipe draining, protocol parsing, and merge facets receive ordinary unit/integration coverage but no synthetic claims. There are no non-falsifiable claims or `[[coverage.exclude]]` entries.

PH-5 owns `vtas` evidence discovery, link review, approvals, final status/report, repository gates, commit, deployment, and three-machine smoke evidence. Generated Veritas lockfiles are modified only through Veritas commands.

## Risks and Open Questions

- CI timeout tests must use generous scheduling margins while still proving the five-second product deadline; pure deadline classification tests should cover boundary arithmetic separately.
- Remote semantic inference may legitimately exceed five seconds on cold start. This is accepted behavior for the first version and appears explicitly as a node timeout rather than blocking local results.
- OpenSSH may format platform-specific stderr. Outcomes therefore classify stable categories and retain a bounded diagnostic without asserting exact SSH prose.
- No material product or architecture questions remain open for implementation.
