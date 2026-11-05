## MODIFIED Requirements

### Requirement: Scheme registration, URI shape and accounts

The component SHALL register schemes `smtp`, `smtps`, `imap` and `imaps`
from one `MailBundle` (`config_key() == "mail"`). The URI shape SHALL be
`scheme://host[:port]?options`; omitted ports default to 587 (`smtp`),
465 (`smtps`), 143 (`imap`) and 993 (`imaps`). Credentials and trust
anchors SHALL come from named accounts in
`[components.mail.accounts.<name>]` (`username`, `password`, optional
`caFile`, optional `allowedHosts`), with `${env:}` placeholders allowed.
Account selection SHALL follow the rabbitmq broker rules: explicit
`?account=` must exist; omitted with exactly one account selects it;
omitted with several is an error naming `?account=`; omitted with none
means no authentication for `smtp`/`smtps`; for `imap`/`imaps` an account
with `username` and `password` is required and its absence SHALL fail
endpoint creation. Userinfo in the URI SHALL fail endpoint creation. URI
options SHALL be declared through the `UriConfig` descriptor.

Implicit single-account selection SHALL be bound to the account's
`allowedHosts` allow-list: the endpoint SHALL be created only when the URI
host exactly matches (case-insensitively) an entry in the selected account's `allowedHosts`. When the list is
absent or empty, when no entry matches, or when the URI carries no host,
implicit selection SHALL fail endpoint creation with that same typed
error (no host can be matched). Explicit `?account=<name>` selection
SHALL ignore `allowedHosts` and may target any host. The failure SHALL be
a typed endpoint-creation error whose message names the account and the
two remedies (`?account=<name>` or `allowedHosts`) and SHALL NOT contain
the host, port or server URL, in the error text or any log line.

#### Scenario: four schemes registered

- **GIVEN** a context booted with the mail bundle enabled
- **WHEN** the component registry is queried
- **THEN** `smtp`, `smtps`, `imap` and `imaps` all resolve

#### Scenario: default ports

- **GIVEN** URIs `smtp://h`, `smtps://h`, `imap://h`, `imaps://h`
- **WHEN** the endpoints resolve
- **THEN** the ports are 587, 465, 143 and 993

#### Scenario: single account resolves implicitly

- **GIVEN** one account `main` with `allowedHosts = ["h"]` and URI
  `smtp://h?from=a@x.test`
- **WHEN** the endpoint resolves
- **THEN** account `main` is selected

#### Scenario: implicit single account without allowed hosts fails

- **GIVEN** one account `main` with no `allowedHosts` and URI
  `smtp://mx.example.com?from=a@x.test`
- **WHEN** the endpoint is created
- **THEN** creation fails with an error naming `main` and both
  `?account=` and `allowedHosts`

#### Scenario: implicit single account host mismatch fails

- **GIVEN** one account `main` with `allowedHosts = ["other.example.com"]`
  and URI `smtp://mx.example.com?from=a@x.test`
- **WHEN** the endpoint is created
- **THEN** creation fails naming both remedies

#### Scenario: implicit single account host match succeeds

- **GIVEN** one account `main` with `allowedHosts = ["mx.example.com"]`
  and URI `smtp://mx.example.com?from=a@x.test`
- **WHEN** the endpoint is created
- **THEN** creation succeeds and account `main` is selected

#### Scenario: allowed hosts match is case insensitive

- **GIVEN** one account `main` with `allowedHosts = ["MX.Example.COM"]`
  and URI `smtp://mx.example.com?from=a@x.test`
- **WHEN** the endpoint is created
- **THEN** creation succeeds

#### Scenario: explicit account ignores allowed hosts

- **GIVEN** one account `a` with an empty `allowedHosts` and URI
  `smtp://mx.example.com?account=a&from=x@x.test`
- **WHEN** the endpoint is created
- **THEN** creation succeeds (documented escape hatch)

#### Scenario: implicit selection without uri host fails

- **GIVEN** one account `main` with `allowedHosts = ["mx.example.com"]`
  and URI `smtp:/x` with no authority
- **WHEN** the endpoint is created
- **THEN** creation fails with the typed endpoint-creation error naming
  `main` and both `?account=` and `allowedHosts`, with no URI error text

#### Scenario: ambiguous account selection errors

- **GIVEN** two accounts and a URI without `?account=`
- **WHEN** the endpoint resolves
- **THEN** resolution fails with an error naming `?account=`

#### Scenario: credentials in the URI are rejected

- **GIVEN** URI `smtp://user:secret@mail.example:587`
- **WHEN** the endpoint is created
- **THEN** creation fails with an error naming `[components.mail.accounts]`
  and the error text does not contain `secret`

#### Scenario: password never traverses Debug

- **GIVEN** an account with password `hunter2`
- **WHEN** the config is formatted with `{:?}`
- **THEN** the output does not contain `hunter2`

#### Scenario: host never traverses error or log

- **GIVEN** one account `main` with `allowedHosts = ["other.example.com"]`
  and URI `smtp://mx.example.com?from=a@x.test`
- **WHEN** the endpoint is created with a capture subscriber installed
- **THEN** no captured log line and no error `Display` text contains
  `mx.example.com`
