# Code Review Summary

**Scope**: Remove runtime yolo mode and make tool permissions config-only
**Overall risk**: Low
**Verdict**: Approve with comments

No remaining in-code `yolo` references were found in the reviewed scope.

## Findings

### [P0] Blocking

None.

### [P1] High

None.

### [P2] Medium

None.

### [P3] Low

- **`AppConfig::load` still accepts a non-file permission override**
  - **Location**: `crates/mimo-config/src/lib.rs:24-30,148-155`
  - **Why it matters**: The binaries no longer expose a permissions flag, but the configuration layer still allows callers to override the permission policy programmatically. That means the "permissions are configured only in config.toml" guarantee is not actually enforced at the API boundary.
  - **Evidence**: `ConfigOverrides` still exposes `pub permissions: Option<PermissionPolicy>`, and `AppConfig::load` prefers `(ConfigValueSource::Cli, overrides.permissions)` over `file_config.permissions`. Any caller can pass `ConfigOverrides { permissions: Some(PermissionPolicy::Auto), ..Default::default() }` and bypass the file value entirely.
  - **Fix**: Remove `permissions` from `ConfigOverrides` and drop the CLI override branch from `AppConfig::load`, then add a regression test that permissions only resolve from `config.toml` or the built-in default.

## Suggested Next Steps

- [x] No P0/P1 findings blocking merge
- [ ] Tighten the config API so permissions can only come from `config.toml` or the built-in default
- [ ] Add a regression test for permission-source resolution after the API change
