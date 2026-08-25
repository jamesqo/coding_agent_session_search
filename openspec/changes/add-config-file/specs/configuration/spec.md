## Purpose

Define one explicit, versioned configuration contract for CASS node identity,
provider roots, and federation membership.

## ADDED Requirements

### Requirement: Configuration discovery

<!-- claim: configuration/default-file-is-optional -->
Without `--config`, CASS SHALL inspect its platform configuration path for a
CASS JSON configuration file. Absence of that default file SHALL preserve
built-in provider roots and local-only search. `cass status` SHALL report the
resolved configuration path and whether a file was loaded.

If the platform does not provide an absolute configuration directory, CASS
SHALL fail with a typed configuration error rather than inspect a relative
working-directory path.

#### Scenario: First use without a file

- **WHEN** CASS is invoked without `--config` and no default configuration file exists
- **THEN** the command uses built-in local defaults without creating a configuration file

<!-- claim: configuration/explicit-file-is-required -->
An explicit `--config PATH` SHALL require a readable regular file. Failure to
read or parse it SHALL produce a typed JSON `configuration` error without
opening a database writer, scanning providers, or starting SSH.

<!-- claim: configuration/invalid-loaded-file-fails-before-effects -->
An existing default configuration file SHALL have the same validation and
pre-effect failure semantics as an explicit file. Each of the six public
commands SHALL resolve and validate configuration before performing command
effects. Hidden federation request workers SHALL neither discover nor load a
configuration file; explicitly combining a hidden worker flag with `--config`
or `--local-node` SHALL be a typed usage error.

#### Scenario: Explicit file is missing

- **WHEN** a command is invoked with `--config` naming a missing file
- **THEN** it exits unsuccessfully with `error.kind` equal to `configuration` before command work begins

#### Scenario: Explicit path is a directory

- **WHEN** a command is invoked with `--config` naming a directory
- **THEN** it exits with a typed configuration error before command work begins

#### Scenario: Default file is malformed

- **WHEN** the platform default configuration file exists but is malformed
- **THEN** the command fails before opening models, a database writer, or SSH

### Requirement: Versioned document validation

<!-- claim: configuration/document-version-and-fields-are-validated -->
A loaded configuration SHALL be a JSON object with `version` equal to `1`, an
explicit `local_node`, and a nonempty `nodes` array. Unknown fields,
unsupported versions, or invalid field types SHALL fail with a typed JSON
`configuration` error.

Configuration input SHALL be bounded to 1 MiB. Symlinks resolving to regular
files SHALL be accepted. The reported loaded path SHALL be absolute and
canonical; failure to resolve the path SHALL be a configuration error.
Exactly 1,048,576 bytes SHALL be accepted for parsing; any additional byte
SHALL be rejected before JSON parsing.

#### Scenario: Unsupported version

- **WHEN** a loaded document declares a version other than `1`
- **THEN** CASS rejects it without applying any values from the document

#### Scenario: Unknown provider

- **WHEN** a node's providers object contains an unsupported provider name
- **THEN** CASS rejects the configuration rather than ignoring that provider

### Requirement: Node inventory validation

<!-- claim: configuration/node-inventory-is-valid -->
Node names SHALL be unique. SSH destinations SHALL also be unique so two
logical nodes cannot fan out to the same machine. Each node SHALL contain a
bounded name and SSH destination, an explicit search-participation boolean, and
a providers object. Invalid or duplicate values SHALL fail with a typed JSON
`configuration` error.

Names and destinations SHALL contain 1 through 255 ASCII characters, start
with an ASCII letter or digit, and otherwise contain only ASCII letters,
digits, `.`, `_`, or `-`. The logical name `local` SHALL be reserved.
After resolving local identity, at most 16 nonlocal nodes MAY have `search`
enabled by default.

#### Scenario: Duplicate SSH destination

- **WHEN** two logical nodes configure the same SSH destination
- **THEN** CASS rejects the document rather than searching that destination twice

### Requirement: Provider roots validation

<!-- claim: configuration/provider-roots-are-valid -->
Provider keys SHALL be exactly `claude-code` or `codex`. Each present provider
SHALL contain a nonempty array of unique absolute root paths. Unsupported
providers and duplicate or relative roots SHALL fail with a typed JSON
configuration error.

An empty providers object SHALL be valid. Root uniqueness SHALL be lexical and
configuration loading SHALL NOT require any root to exist, because one shared
inventory may contain paths belonging to another machine.

#### Scenario: Remote root does not exist locally

- **WHEN** a nonlocal node contains a valid absolute root absent from this machine
- **THEN** configuration resolution succeeds without probing that root

### Requirement: Index horizon validation

<!-- claim: configuration/index-horizon-is-valid -->
Each node MAY contain an `index` object with `since_days` set to an integer from
1 through 36500 or `null`. Missing `index` or `since_days` SHALL resolve to 90
days; `null` SHALL select all history. Zero, negative, out-of-range, fractional,
or string values SHALL fail with a typed JSON configuration error.

#### Scenario: Horizon is omitted

- **WHEN** the resolved local node does not declare `index.since_days`
- **THEN** its resolved `since_days` value is 90

#### Scenario: All history is configured

- **WHEN** the resolved local node declares `"index": {"since_days": null}`
- **THEN** its resolved horizon has no day limit

### Requirement: Explicit local-node identity

<!-- claim: configuration/local-node-is-explicit -->
The configured `local_node` SHALL exactly identify one node entry. CASS SHALL
NOT infer local identity from the operating-system hostname or SSH destination.
Global `--local-node NAME` SHALL override the configured identity and SHALL
require an exact configured node name.

The document's own `local_node` SHALL be valid even when an override is
supplied. Supplying `--local-node` without a loaded configuration SHALL fail
with a typed configuration error.

#### Scenario: Configured identity is absent

- **WHEN** `local_node` does not match any configured node
- **THEN** CASS fails with a typed JSON `configuration` error without scanning or starting SSH

#### Scenario: Explicit override

- **WHEN** a valid configuration names Xenia locally and `--local-node dev-macbook` is supplied
- **THEN** command configuration resolves from the `dev-macbook` node entry

### Requirement: Configuration precedence

<!-- claim: configuration/cli-values-have-precedence -->
For configurable behavior, explicit CLI values SHALL replace corresponding
values from the resolved local-node entry, which SHALL replace built-in
defaults. Repeatable `--provider PROVIDER` SHALL restrict the run to the
deduplicated selected providers. `--since-days N` SHALL replace the configured
horizon with a positive number of days, while `--all-history` SHALL replace it
with no horizon. Supplying both horizon flags SHALL be a typed usage error.

#### Scenario: Horizon is overridden

- **WHEN** the local node configures 90 days and `cass index --since-days 30` is invoked
- **THEN** that run uses a 30-day source-activity horizon

### Requirement: One configuration system

<!-- claim: configuration/environment-inputs-are-ignored -->
`CASS_CLAUDE_ROOTS`, `CASS_CODEX_ROOTS`, `CASS_OPENCODE_ROOTS`,
`CASS_COPILOT_ROOTS`, `CASS_HERMES_ROOTS`, `CASS_PI_ROOTS`, and
`CASS_SEARCH_NODES` SHALL NOT alter provider discovery or federation selection.
Configuration files and supported CLI flags SHALL be the only user-controlled
inputs for those values.

#### Scenario: Legacy environment values are present

- **WHEN** legacy root and node environment variables are set but no configuration or matching CLI override is supplied
- **THEN** CASS uses built-in local roots and local-only search

### Requirement: Configuration status is explicit

<!-- claim: configuration/status-reports-resolved-settings -->
`cass status` SHALL include a `configuration` object containing `path`,
`loaded`, `local_node`, `providers`, and `index`. `providers` SHALL contain only
the resolved local node's enabled provider keys in deterministic Claude
Code-then-Codex order, each with its ordered `roots`; `index` SHALL contain
nullable `since_days`. Without a loaded file, `path` SHALL be the absolute
default path, `loaded` false, `local_node` null, `providers` SHALL contain the
two built-in provider root lists, and `since_days` SHALL be 90.

#### Scenario: Status without configuration

- **WHEN** no default file exists and `cass status` is invoked
- **THEN** its configuration object reports built-in roots, a 90-day horizon, and no local node

### Requirement: Configuration errors have a stable wire shape

<!-- claim: configuration/errors-are-stable-and-nonretryable -->
A configuration failure SHALL exit with code 9 and emit JSON with
`error.kind` equal to `configuration`, `retryable` false, and no
`recommended_action`.

#### Scenario: Invalid loaded document

- **WHEN** a loaded document violates any configuration invariant
- **THEN** CASS returns the stable nonretryable configuration error shape
