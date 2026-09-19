# cli-jobs Specification Delta

## MODIFIED Requirements

### Requirement: fail-closed consumer scheme allowlist

At load time, `camel job` SHALL reject documents whose discovered route
definitions consume (`from:` URI) from any scheme outside the job-safe
allowlist `{direct, seda, log, mock, stream}`, where the `stream` scheme
is admitted only with path `in` (`from: stream:in`). Job documents
declaring `from: stream:out` or `from: stream:err` SHALL be rejected at
load with an error naming `stream:in` as the only accepted stream
consumer path. Producer and sink `to:` URIs SHALL NOT be restricted.

#### Scenario: non-job-safe consumer scheme is rejected

- **GIVEN** a job document whose routes include a route with
  `from: kafka:topic` (or any scheme outside the allowlist)
- **WHEN** `camel job` loads the routes
- **THEN** the document is rejected with an error naming the offending
  route, its from-URI, and the allowlist, and the process exits with
  code 2

#### Scenario: producers are unrestricted

- **GIVEN** a job document whose target route consumes from
  `direct:in` and contains `to: http://...` and `to: log:out` steps
- **WHEN** `camel job` loads the routes
- **THEN** the consumer gate passes and the job runs

#### Scenario: from stream:in passes the gate

- **GIVEN** a job document whose route declares `from: stream:in`
- **WHEN** `camel job` loads the routes
- **THEN** the consumer gate passes and the job runs

#### Scenario: stream producer path rejected as consumer

- **GIVEN** a job document whose route declares `from: stream:out`
- **WHEN** `camel job` loads the routes
- **THEN** the document is rejected with an error naming `stream:in` as
  the only accepted stream consumer path, and the process exits with
  code 2
