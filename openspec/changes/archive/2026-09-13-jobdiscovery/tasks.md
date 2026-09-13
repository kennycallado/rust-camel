# Tasks: jobdiscovery

## camel-config

### Task 1.1: Add ordered job discovery roots with legacy compatibility

**Files:**
- `crates/camel-config/src/config.rs` (modified)
- `crates/camel-config/src/config_tests/jobs_config_tests.rs` (modified)

**Steps:**
1. Replace `JobsCamelConfig.dir: String` with a serde-deny-unknown-fields model containing optional legacy `dir` and `dirs: Option<Vec<String>>`, preserving presence so absent `dirs` differs from explicit `dirs = []`.
2. Implement `JobsCamelConfig::resolved_dirs() -> Vec<String>` with precedence `dirs` whenever present, including an explicit empty list. Use `["jobs"]` only when both keys are absent, and fold legacy `dir` into one entry only when `dirs` is absent.
3. Update `CamelConfigBuilder::jobs` defaults and all config tests to assert normalized roots without changing unrelated configuration behavior.

**Tests:**
- `jobs_table_defaults_to_jobs`: setup TOML without `[jobs]`; action deserialize `CamelConfig`; assert normalized roots equal `["jobs"]`; command `cargo test -p camel-config --lib jobs_table_defaults_to_jobs`; expected: fails before implementation, passes after.
- `jobs_table_dirs_round_trip`: setup `[jobs]\ndirs = ["first", "second"]`; action deserialize; assert order preserved; command `cargo test -p camel-config --lib jobs_table_dirs_round_trip`; expected: fails before implementation, passes after.
- `explicit_job_dirs_override_legacy_dir`: setup both `dir = "legacy"` and `dirs = ["first", "second"]`; action deserialize and normalize; assert only `first`, `second`; command `cargo test -p camel-config --lib explicit_job_dirs_override_legacy_dir`; expected: fails before implementation, passes after.
- `empty_dirs_override_legacy_dir`: setup both `dir = "legacy"` and `dirs = []`; action deserialize and normalize; assert empty roots; command `cargo test -p camel-config --lib empty_dirs_override_legacy_dir`; expected: fails before implementation, passes after.
- `jobs_table_unknown_key_rejected`: setup `[jobs]\nbogus = true`; action deserialize; assert TOML error; command `cargo test -p camel-config --lib jobs_table_unknown_key_rejected`; expected: remains passing.
- `legacy_jobs_dir_alias_is_supported`: setup `[jobs]\ndir = "legacy-jobs"`; action deserialize and call `resolved_dirs`; assert exactly `legacy-jobs`; command `cargo test -p camel-config --lib legacy_jobs_dir_alias_is_supported`; expected: fails before implementation, passes after.

**Acceptance:**
- `cargo test -p camel-config --lib jobs_` exits 0.
- `cargo clippy -p camel-config --lib -- -D warnings` exits 0.
- Unknown fields remain rejected and default/legacy/dual-key precedence are covered by tests.

- [x] 1.1

## camel-cli

### Task 2.1: Implement bounded discovery-set listing and named lookup

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/tests/job_one_shot_test.rs` (modified)
- `crates/camel-cli/tests/job_signal_test.rs` (modified)
- `crates/camel-cli/tests/common/mod.rs` (modified)

**Steps:**
1. Replace single `jobs_root` resolution with ordered Camel.toml-rooted paths from `camel_config.jobs.resolved_dirs()`, preserving explicit document paths and exact bare `<name>.job.yaml` probing.
2. Add a lexical, non-symlink recursive metadata walker with root depth 0, depth limit 8, 512 encountered files per root, `is_job_document` filtering, and one root-specific truncation warning while continuing other roots.
3. Preserve the cheap description probe and root-level output. For nested files, render the configured-root-relative path, stem, and description. Keep malformed siblings non-fatal and missing roots successful.
4. Collect all exact bare-name matches before execution. Return exit 2 with every matching path for cross-root collisions. Select the only match regardless of root order. Keep listing outside route discovery, interpolation, and security setup.
5. Add integration fixtures and assertions for ordering, nested display, caps, symlink exclusion, legacy precedence, collisions, exact miss paths, and metadata-only behavior.

**Tests:**
- `configured_job_dirs_are_scanned_in_order`: setup two roots with `a.job.yaml` and `b.job.yaml`; action run no-argument `camel job`; assert exact output order; command `cargo test -p camel-cli --test job_one_shot_test configured_job_dirs_are_scanned_in_order`; expected: fails before implementation, passes after.
- `explicit_job_dirs_override_legacy_dir`: setup both config keys, with a legacy-only file and a first-root file; action list; assert exact first-root line and no legacy filename; command `cargo test -p camel-cli --test job_one_shot_test explicit_job_dirs_override_legacy_dir`; expected: fails before implementation, passes after.
- `default_jobs_root_is_used`: setup no jobs setting and one job in root `jobs`; action list; assert job line and exit 0; command `cargo test -p camel-cli --test job_one_shot_test default_jobs_root_is_used`; expected: remains passing as a regression guard.
- `named_job_uses_later_matching_configured_root`: setup only second root contains `report.job.yaml` with marker `second-marker`; action resolve bare name; assert stable report document and outcome fields, empty stderr, exit 0; command `cargo test -p camel-cli --test job_one_shot_test named_job_uses_later_matching_configured_root`; expected: fails before implementation, passes after.
- `listing_formats_descriptions_and_yml_is_display_only`: setup multiline `.job.yaml` and descriptionless `.job.yml`; action list then bare lookup yml stem; assert one-line description, `(no description)`, and `.job.yaml` miss path; command `cargo test -p camel-cli --test job_one_shot_test listing_formats_descriptions_and_yml_is_display_only`; expected: remains passing as a regression guard.
- `job_listing_recurses_and_skips_test_documents`: setup nested `domain/report.job.yaml` and sibling `.test.yaml`; action list; assert exact nested line and absent test file; command `cargo test -p camel-cli --test job_signal_test job_listing_recurses_and_skips_test_documents`; expected: fails before implementation, passes after.
- `malformed_job_sibling_does_not_abort_listing`: setup valid and malformed job files; action list; assert valid line, `(unparseable)`, exit 0; command `cargo test -p camel-cli --test job_one_shot_test malformed_job_sibling_does_not_abort_listing`; expected: remains passing as a regression guard.
- `missing_job_root_is_successful`: setup `dirs = ["missing"]` with no such root; action list; assert creation hint and exit 0; command `cargo test -p camel-cli --test job_one_shot_test missing_job_root_is_successful`; expected: fails before implementation, passes after.
- `job_listing_stops_at_depth_eight` and `job_listing_stops_at_512_files`: setup root depth 0 with boundary fixtures plus second root valid job; action list; assert boundary inclusion, beyond-bound exclusion, root-named one warning, continuation, exit 0; command `cargo test -p camel-cli --test job_signal_test job_listing_stops_at_`; expected: fails before implementation, passes after.
- `missing_first_root_does_not_hide_second`: setup `dirs = ["missing", "second"]` and valid job only in `second`; action list; assert second-root job line and exit 0; command `cargo test -p camel-cli --test job_one_shot_test missing_first_root_does_not_hide_second`; expected: fails before implementation, passes after.
- `job_listing_is_lexical_and_does_not_follow_directory_symlinks`: setup `a/`, `b/`, and in-root symlink to outside target; action list; assert exact lexical nested lines and absent target; command `cargo test -p camel-cli --test job_one_shot_test job_listing_is_lexical_and_does_not_follow_directory_symlinks`; expected: fails before implementation, passes after.
- `named_job_collision_reports_all_matching_paths`: setup same `report.job.yaml` in two roots; action `camel job report`; assert both exact paths on stderr and exit 2; command `cargo test -p camel-cli --test job_one_shot_test named_job_collision_reports_all_matching_paths`; expected: fails before implementation, passes after.
- `explicit_job_path_bypasses_roots_and_bare_miss_is_named`: setup `dirs = ["first", "second"]` with nested `ops/one-shot.job.yml` and no `missing.job.yaml` in either root; action explicit path and bare-name miss from the nested directory; assert the explicit document loads as-is (exit 0, marker reply) and the miss stderr names `first/missing.job.yaml` and `second/missing.job.yaml` with exit 2; command `cargo test -p camel-cli --test job_one_shot_test explicit_job_path_bypasses_roots_and_bare_miss_is_named`; expected: fails before implementation, passes after.
- `job_listing_does_not_boot_route_pipeline`: setup description plus invalid interpolation/security sentinels; action list; assert description output, no sentinel errors, exit 0; command `cargo test -p camel-cli --test job_one_shot_test job_listing_does_not_boot_route_pipeline`; expected: remains passing as a regression guard.

**Acceptance:**
- `cargo test -p camel-cli --test job_one_shot_test` exits 0.
- `cargo test -p camel-cli --test job_signal_test` exits 0.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- No `Discover::Patterns`, CLI glob, env interpolation, or security compile context is used by no-argument listing.

- [x] 2.1
