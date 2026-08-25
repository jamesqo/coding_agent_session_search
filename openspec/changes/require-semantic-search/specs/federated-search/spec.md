## MODIFIED Requirements

### Requirement: Partial remote failure

A remote connection failure, timeout, nonzero exit, malformed response, incompatible response, or semantic-readiness failure SHALL NOT suppress successful local or remote hybrid results. Federated search SHALL exit successfully only when local hybrid search succeeds and SHALL include one deterministic outcome for every selected remote node.

#### Scenario: Remote node lacks semantic readiness

<!-- claim: federated-search/semantic-unready-node-is-partial-failure -->
- **WHEN** local hybrid search succeeds, one remote node returns hybrid results, and another remote node lacks models or current embeddings
- **THEN** the response contains local and successful remote results plus an error outcome for the semantically unready node

#### Scenario: Local node lacks semantic readiness

<!-- claim: federated-search/local-semantic-readiness-is-required -->
- **WHEN** the local database lacks models or current embeddings
- **THEN** federated search fails with the local typed readiness error even if a remote node could search successfully

### Requirement: Federated response identifies provenance

A successful federated search response SHALL identify its aggregate realized mode as `federated`, attach an ordered nonempty `origins` array and a federated rank score to every returned result, and include every remote node outcome. Every successful constituent node SHALL report hybrid realization; semantically unready nodes SHALL appear only as error outcomes. Local-only hybrid responses SHALL omit federated-only result and response fields.

#### Scenario: Ready and unready remote nodes

<!-- claim: federated-search/successful-nodes-are-hybrid -->
- **WHEN** local search and one remote search are semantically ready while another remote is not ready
- **THEN** the aggregate reports federated realization, successful outcomes report hybrid realization, and the unready outcome reports an error without results
