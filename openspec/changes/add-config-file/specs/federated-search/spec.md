## MODIFIED Requirements

### Requirement: Node selection precedence

<!-- claim: federated-search/node-selection-precedence -->
`cass search` SHALL accept repeated `--node NAME` values naming configured
remote nodes. When explicit values are present, CASS SHALL search the local
database plus the deduplicated named remote subset. Otherwise, with a loaded
configuration, it SHALL search locally plus every other node whose `search`
value is true. The resolved local node SHALL never be contacted through SSH;
explicitly naming it SHALL be a typed usage error before local search or SSH.
Without configuration or explicit nodes, search SHALL remain local-only and
preserve the local response shape.

#### Scenario: Configured default fanout

<!-- claim: federated-search/configured-default-fanout -->
- **WHEN** Xenia is local and dev-macbook and personal-macbook are search-enabled
- **THEN** a search without `--node` searches locally and contacts each enabled Mac once

#### Scenario: Explicit nodes replace defaults

- **WHEN** two remote nodes are enabled by default and search is invoked with `--node dev-macbook --node dev-macbook`
- **THEN** CASS searches locally and contacts dev-macbook once without contacting the other default node

#### Scenario: Disabled node is explicitly selected

- **WHEN** a configured remote node has `search` false and is explicitly named with `--node`
- **THEN** CASS searches that node because explicit selection replaces default membership

#### Scenario: No federation configured

- **WHEN** no configuration is loaded and search has no `--node`
- **THEN** CASS performs local-only hybrid search

#### Scenario: Explicit node without configuration

- **WHEN** no configuration is loaded and search is invoked with `--node dev-macbook`
- **THEN** CASS fails with a typed usage error before local search or SSH begins

### Requirement: Configured nodes are bounded SSH destinations

<!-- claim: federated-search/node-validation -->
Every configured node name and SSH destination SHALL contain at most 255 ASCII
characters, begin with an ASCII letter or digit, and contain only ASCII letters,
digits, `.`, `_`, and `-`. The name `local` SHALL be reserved. An unknown
explicit node or invalid configured value SHALL produce a typed error before
any SSH process starts. CASS SHALL pass the resolved SSH destination as one
process argument and SHALL never interpolate it into a shell command.

#### Scenario: Unknown configured node

- **WHEN** search is invoked with `--node unknown` and no node has that name
- **THEN** search fails before local search or SSH begins

#### Scenario: Local node is explicitly selected

- **WHEN** search is invoked with `--node` naming the resolved local node
- **THEN** search fails with a typed usage error before local search or SSH begins

#### Scenario: Option-shaped destination

- **WHEN** a configured SSH destination begins with `-` or contains whitespace or shell metacharacters
- **THEN** configuration loading fails without invoking SSH

### Requirement: Concurrent local and remote search

<!-- claim: federated-search/concurrent-fanout -->
When remote nodes are selected, CASS SHALL begin their configured SSH searches
concurrently while performing local hybrid search. Each remote request SHALL
use noninteractive SSH, the resolved node's SSH destination, a fixed remote
CASS command, structured JSON standard input, and the fixed bounded execution
deadline. Query text, filters, limits, and identifiers SHALL NOT be
interpolated into a remote shell command.

#### Scenario: Two reachable configured nodes

- **WHEN** two selected configured nodes return compatible successful hybrid responses within the deadline
- **THEN** one invocation returns merged results from local search and both nodes

#### Scenario: Remote command exceeds deadline

- **WHEN** a selected configured node does not finish within the transport deadline
- **THEN** CASS terminates that SSH child and records a timeout outcome for the node

### Requirement: Federation workers never recurse

<!-- claim: federated-search/remote-worker-is-nonrecursive -->
The hidden federation request mode SHALL execute exactly one local hybrid
search or one local view operation. It SHALL NOT load configured default nodes,
honor federation-selection environment values, or start SSH, even when a
configuration file exists on the remote machine.

#### Scenario: Remote machine also has default nodes

- **WHEN** a structured federation search request reaches a machine whose configuration enables remote nodes
- **THEN** that process searches only its local database and returns one protocol-v2 response without starting SSH

### Requirement: Origin-aware remote view

<!-- claim: federated-search/remote-view -->
`cass view <id> --node NAME --context N` SHALL resolve the configured node name,
send the identifier and context through structured standard input to the fixed
remote CASS command at that node's SSH destination, and return the compatible
remote JSON view response. The local node, an unknown name, or a node selected
without loaded configuration SHALL fail before SSH. Without `--node`, view
SHALL retain local behavior.

#### Scenario: View a configured remote result

- **WHEN** a federated result identifies `dev-macbook` as an origin and its identifier is passed to `cass view --node dev-macbook`
- **THEN** CASS returns that configured node's bounded adjacent context as ordinary view JSON
