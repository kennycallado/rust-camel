# Test-count evidence (rc-v4u92)

Date: 2026-11-01
Base: main f5d36f9c1

## Before extraction

Command (worktree root, CARGO_TARGET_DIR=<worktree>/target):

    cargo test -p camel-dsl --test rest_xml_http_e2e --test rest_marshal_error_http_e2e

Results:

- rest_xml_http_e2e: 2 passed; 0 failed
- rest_marshal_error_http_e2e: 7 passed; 0 failed
- total: 9

Names:
- e2e_malformed_xml_returns_400
- e2e_valid_xml_returns_201_xml
- e2e_json_response_marshal_failure_returns_500
- e2e_form_response_marshal_failure_returns_500
- e2e_xml_response_marshal_failure_returns_500
- e2e_xml_user_step_body_xml_returns_declared_media
- e2e_xml_user_step_body_xml_uses_vnd_media
- e2e_json_malformed_request_still_returns_400
- e2e_authored_marshal_in_rest_user_steps_stays_400

## After extraction

Date: 2026-11-01

Command (worktree root, CARGO_TARGET_DIR=<worktree>/target), same as the
Before section:

    cargo test -p camel-dsl --test rest_xml_http_e2e --test rest_marshal_error_http_e2e

Results:

- rest_xml_http_e2e: 2 passed; 0 failed
- rest_marshal_error_http_e2e: 7 passed; 0 failed
- total: 9

Names unchanged.
