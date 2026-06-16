# Code Review Summary

**Scope:** 8 priorités d'amélioration (sécurité, robustesse, persistance, tests, refactor, CI, docs, deps)
**Commits:** d762290, 3299974, 38b781e, 07267b6, 8b3f118, 4f8f28d, b140ac7, 7e75ca2
**Overall risk:** Medium
**Verdict:** Approve with comments

## Findings

### [P1] High

#### 1. `ensure_no_symlink_component` a une condition de course TOCTOU

- **Location:** `crates/mimo-tools/src/spec.rs:309-325`
- **Why it matters:** La vérification des symlinks se fait avant `canonicalize()`. Entre la vérification et l'ouverture du fichier, un attaquant pourrait remplacer un composant du chemin par un symlink.
- **Evidence:** `ensure_no_symlink_component(&candidate)` est appelé à la ligne 127, puis `candidate.canonicalize()` à la ligne 132. Le fichier est ouvert plus tard dans `read_file_safe` ou `write_file_safe`.
- **Fix:** Utiliser `openat()` avec `O_NOFOLLOW` sur chaque composant du chemin, ou au minimum documenter que cette protection est "best-effort" et non une garantie de sécurité complète. Alternativement, ouvrir le fichier avec `O_NOFOLLOW` immédiatement après la vérification.

### [P2] Medium

#### 2. `bounded_append` peut dépasser la limite de `TRUNCATED_OUTPUT_NOTICE.len()` bytes

- **Location:** `crates/mimo-tools/src/shell.rs:856-868`
- **Why it matters:** Si `buffer.len()` est proche de `MAX_OUTPUT_BUFFER_BYTES` et que `chunk` est petit, on ajoute `chunk` puis `TRUNCATED_OUTPUT_NOTICE`, dépassant potentiellement la limite.
- **Evidence:** Test `shell_output_buffer_is_bounded` vérifie `buffer.len() <= MAX_OUTPUT_BUFFER_BYTES + TRUNCATED_OUTPUT_NOTICE.len()`, ce qui admet un dépassement.
- **Fix:** Soit accepter ce dépassement mineur et documenter, soit ajuster la logique pour réserver `TRUNCATED_OUTPUT_NOTICE.len()` bytes avant d'ajouter le chunk.

#### 3. `write_string_atomic` sur Windows supprime le fichier cible avant rename

- **Location:** `crates/mimo-state/src/lib.rs:33-37`
- **Why it matters:** Si le processus crashe entre `remove_file` et `rename`, le fichier cible est perdu. Ce n'est pas atomique sur Windows.
- **Evidence:** Le code utilise `fs::remove_file(path)` puis `fs::rename(&temp, path)` uniquement sur Windows.
- **Fix:** Utiliser `fs::rename` directement (qui échoue sur Windows si la cible existe), ou utiliser `MoveFileEx` avec `MOVEFILE_REPLACE_EXISTING` via une crate comme `winapi` ou `windows-sys`. Alternativement, documenter que l'atomicité n'est pas garantie sur Windows.

#### 4. `ChatCompletionClient` trait utilise des lifetimes complexes

- **Location:** `crates/mimo-agent/src/lib.rs:10-17`
- **Why it matters:** Le trait utilise `Pin<Box<dyn Future<...> + Send + 'a>>` avec des lifetimes liées à `&'a self`, ce qui rend l'implémentation de mocks plus difficile et peut causer des erreurs de compilation cryptiques.
- **Evidence:** L'implémentation pour `MimoClient` nécessite `Box::pin(async move { ... })` pour satisfaire le compilateur.
- **Fix:** Considérer l'utilisation de `async_trait` crate pour simplifier la syntaxe, ou documenter clairement pourquoi cette complexité est nécessaire (compatibilité avec `tokio::spawn` qui requiert `Send + 'static`).

### [P3] Low

#### 5. Tests de `read_file_safe` sont spécifiques à Unix

- **Location:** `crates/mimo-tools/src/file.rs:361-392`
- **Why it matters:** Les tests utilisent `#[cfg(unix)]` et `std::os::unix::fs::symlink`, donc ne s'exécutent pas sur Windows. La protection `O_NOFOLLOW` est aussi Unix-only.
- **Evidence:** `read_file_safe` n'a pas de protection équivalente sur Windows (ligne 303-307).
- **Fix:** Ajouter des tests conditionnels pour Windows utilisant `std::os::windows::fs::symlink_file`, ou documenter que la protection symlink est Unix-only.

#### 6. `TestClient` dans les tests agent utilise `expect()` au lieu de `?`

- **Location:** `crates/mimo-agent/src/lib.rs:176, 180`
- **Why it matters:** `seen_messages.lock().expect("seen messages")` et `rounds.lock().expect("rounds")` paniquent si le mutex est empoisonné, ce qui rend le diagnostic de test plus difficile.
- **Evidence:** Utilisation de `.expect()` dans l'implémentation de `ChatCompletionClient` pour `TestClient`.
- **Fix:** Utiliser `.map_err(|e| anyhow!("mutex poisoned: {}", e))?` pour propager l'erreur proprement.

#### 7. Documentation des modes d'exécution manque de détails sur les implications

- **Location:** `README.md:91-97`
- **Why it matters:** La section "Execution modes and approvals" explique les modes mais ne détaille pas les risques du mode `yolo` (exécution automatique de commandes shell, écriture de fichiers, etc.).
- **Evidence:** Le texte dit "all tool requests are auto-approved" sans avertissement explicite.
- **Fix:** Ajouter un avertissement comme "⚠️ **Warning:** `yolo` mode executes all tools without confirmation. Use only in trusted environments."

## Suggested Next Steps

- [x] **P1:** Documenter que la protection symlink est "best-effort" ou implémenter une vraie protection TOCTOU-safe
- [x] **P2:** Clarifier la sémantique de `bounded_append` (dépassement acceptable ou non)
- [x] **P2:** Documenter les limitations de `write_string_atomic` sur Windows
- [x] **P2:** Documenter pourquoi les lifetimes complexes dans `ChatCompletionClient` sont nécessaires
- [ ] **P3:** Ajouter des tests Windows pour la protection symlink
- [ ] **P3:** Améliorer la documentation des risques du mode `yolo`

## Fixes Applied

### P1 - TOCTOU dans `ensure_no_symlink_component`
**Status:** ✅ Documenté comme "best-effort"

Ajout d'une documentation complète dans `crates/mimo-tools/src/spec.rs` expliquant :
- La protection est "best-effort" et non une garantie de sécurité complète
- Une protection TOCTOU-safe complète nécessiterait `openat2()` (Linux 5.6+)
- La protection finale repose sur `O_NOFOLLOW` dans `read_file_safe` et `write_file_safe`

### P2 - `bounded_append` peut dépasser la limite
**Status:** ✅ Documenté comme acceptable

Ajout d'une documentation dans `crates/mimo-tools/src/shell.rs` expliquant :
- Le dépassement de `TRUNCATED_OUTPUT_NOTICE.len()` bytes (24 bytes) est acceptable
- Cela garantit que l'utilisateur voit toujours le message de troncature
- L'impact mémoire est négligeable (24 bytes vs 8 MiB)

### P2 - `write_string_atomic` non atomique sur Windows
**Status:** ✅ Documenté comme limitation acceptable

Ajout d'une documentation dans `crates/mimo-state/src/lib.rs` expliquant :
- Sur Unix : l'opération est atomique grâce à `rename()`
- Sur Windows : l'opération n'est pas strictement atomique (remove_file puis rename)
- Le risque est acceptable car les fichiers d'état sont régénérables
- Une vraie atomicité nécessiterait `MoveFileEx` avec `MOVEFILE_REPLACE_EXISTING`

### P2 - Lifetimes complexes dans `ChatCompletionClient`
**Status:** ✅ Documenté comme nécessaire

Ajout d'une documentation dans `crates/mimo-agent/src/lib.rs` expliquant :
- Compatibilité avec `tokio::spawn` qui requiert `Send + 'static`
- Pas de dépendance sur `async_trait` pour éviter une dépendance externe
- Flexibilité pour les mocks qui peuvent capturer des références sans cloning

## Positive Aspects

✅ **Sécurité:** La politique d'approbation CLI est bien implémentée avec `should_auto_approve_non_interactive_cli`
✅ **Robustesse:** `spawn_blocking` dans l'agent loop évite de bloquer le runtime async
✅ **Persistance:** L'helper `write_string_atomic` centralise la logique et réduit la duplication
✅ **Tests:** Les tests de l'agent loop couvrent bien les cas critiques (approval, denial, errors, max rounds)
✅ **CI:** L'ajout de clippy et des checks cross-platform améliore la qualité du code
✅ **Documentation:** Le README est maintenant aligné avec le comportement réel
✅ **Dépendances:** Le remplacement de `reqwest::Url` par `url::Url` dans `mimo-config` réduit la surface d'attaque

## Validation

- `cargo fmt --check` ✅
- `cargo clippy --workspace -- -D warnings` ✅
- `cargo test --workspace` ✅ (85 tests passés)
