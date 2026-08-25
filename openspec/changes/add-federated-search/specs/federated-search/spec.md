## Purpose

Allow one CASS invocation to search independent local indexes on explicitly selected SSH/Tailscale nodes while preserving local-only behavior when federation is not selected.

## ADDED Requirements

### Requirement: Node selection precedence

<!-- claim: federated-search/node-selection-precedence -->
`cass search` SHALL accept repeated `--node <SSH_HOST>` values. When at least one explicit value is present, CASS SHALL use the deduplicated explicit values and ignore `CASS_SEARCH_NODES`. Otherwise it SHALL use the deduplicated, comma-separated nonempty values from `CASS_SEARCH_NODES`. With neither source present, search SHALL remain local-only and SHALL preserve the existing local response shape.

#### Scenario: Explicit nodes replace defaults

- **WHEN** `CASS_SEARCH_NODES=dev-macbook,personal-macbook` and search is invoked with `--node xenia --node xenia`
- **THEN** CASS searches the local database and Xenia once, without contacting either environment-default node

#### Scenario: No federation configured

- **WHEN** search has no `--node` value and `CASS_SEARCH_NODES` is unset or empty
- **THEN** CASS performs the existing local-only search

### Requirement: Node names are bounded SSH aliases

<!-- claim: federated-search/node-validation -->
Every selected node SHALL be a nonempty SSH host alias containing only ASCII letters, digits, `.`, `_`, and `-`, beginning with an ASCII letter or digit. `local` SHALL be reserved and rejected as a remote node. Invalid explicit or environment-provided nodes SHALL produce a typed usage error before any SSH process starts.

#### Scenario: Option-shaped node is rejected

- **WHEN** a selected node begins with `-` or contains whitespace or shell metacharacters
- **THEN** search fails with a typed JSON usage error without invoking SSH

### Requirement: Concurrent local and remote search

<!-- claim: federated-search/concurrent-fanout -->
When nodes are selected, CASS SHALL begin remote searches concurrently while performing the existing local search. Each remote request SHALL use noninteractive SSH, a fixed remote CASS command, structured JSON standard input, and a five-second execution deadline. Query text, filters, limits, and identifiers SHALL NOT be interpolated into a remote shell command.

#### Scenario: Two reachable nodes

- **WHEN** two selected nodes return compatible successful search responses within five seconds
- **THEN** one invocation returns merged results from local search and both nodes

#### Scenario: Remote command exceeds deadline

- **WHEN** a selected node does not finish its remote request within five seconds
- **THEN** CASS terminates that SSH child and records a timeout outcome for the node

### Requirement: Partial remote failure

<!-- claim: federated-search/partial-failure -->
A remote connection failure, timeout, nonzero exit, malformed response, or incompatible response SHALL NOT suppress successful local or remote results. Federated search SHALL exit successfully when local search succeeds and SHALL include one deterministic outcome for every selected remote node.

#### Scenario: Sleeping node

- **WHEN** local search succeeds, one remote node succeeds, and another node is unreachable
- **THEN** the response contains local and successful remote results plus an error outcome for the unreachable node

### Requirement: Deterministic federated merge

<!-- claim: federated-search/deterministic-merge -->
CASS SHALL merge node-local final rankings without comparing raw BM25 values between databases. It SHALL assign each candidate a reciprocal local-rank score, deduplicate candidates by provider, conversation identifier, and message identifier, retain the highest rank contribution for a duplicate, record every origin, and break equal aggregate scores deterministically by provider, conversation identifier, and message identifier. The final result count SHALL respect `--limit`.

#### Scenario: Message exists on two machines

- **WHEN** the same provider, conversation, and message identifiers appear in results from two nodes
- **THEN** the response contains one result whose origins list contains both nodes exactly once

### Requirement: Federated response identifies provenance

<!-- claim: federated-search/response-provenance -->
A federated search response SHALL identify its realized mode as `federated`, attach an ordered nonempty `origins` array and a federated rank score to every returned result, and include remote node outcomes. Local-only responses SHALL omit federated-only result and response fields.

#### Scenario: Mixed lexical and hybrid nodes

- **WHEN** successful nodes report different local realized modes
- **THEN** the federated response reports each node's realized mode in its outcome while the aggregate reports `federated`

### Requirement: Origin-aware remote view

<!-- claim: federated-search/remote-view -->
`cass view <id> --node <SSH_HOST> --context N` SHALL validate the node with the search node rules, send the identifier and context through structured standard input to the fixed remote CASS command, and return the compatible remote JSON view response. Without `--node`, view SHALL retain existing local behavior.

#### Scenario: View a remote result

- **WHEN** a federated result identifies `dev-macbook` as an origin and its identifier is passed to `cass view --node dev-macbook`
- **THEN** CASS returns that node's bounded adjacent context as ordinary view JSON
