# Plan: Améliorations prioritaires du projet MiMo-TUI

## Objective

Produire un plan d’implémentation détaillé et exécutable pour les 8 axes d’amélioration identifiés sur MiMo-TUI, en privilégiant d’abord la sécurité, la robustesse et la testabilité, puis le refactoring structurel, la CI, la documentation et l’allègement des dépendances.

## Requirements Snapshot

- **R1:** Sécuriser l’exécution agentique côté CLI/TUI et outils, sans casser les usages légitimes actuels.
- **R2:** Réduire les risques techniques dans les outils (shell, édition de fichiers, exécution synchrone, réseau).
- **R3:** Fiabiliser la persistance locale (sessions, tâches, mémoire, MCP, diagnostics) et réduire les risques de corruption ou d’états partiels.
- **R4:** Renforcer la couverture de tests sur les zones les plus critiques et rendre la boucle agent testable.
- **R5:** Réduire la dette technique du TUI, en particulier le fichier `crates/mimo-tui/src/app.rs`.
- **R6:** Renforcer la qualité continue via la CI et des vérifications adaptées au workspace Rust 2024 multi-crates.
- **R7:** Corriger les incohérences de documentation et améliorer l’explicitation des comportements réels.
- **R8:** Simplifier certaines dépendances sans modifier le comportement fonctionnel attendu.

## Scope

- Sécurité et garde-fous des outils agentiques.
- Robustesse d’exécution shell/fichier/réseau.
- Persistance locale et helpers communs.
- Stratégie de tests unitaires/intégration ciblée.
- Refactoring structurel progressif du TUI.
- Renforcement de la CI.
- Mise à jour README/docs associées.
- Réduction d’une dépendance lourde dans `mimo-config`.

## Assumptions and Constraints

- Le workspace reste en **edition 2024**.
- Les commandes racine doivent continuer à fonctionner : `cargo check`, `cargo fmt --check`, `cargo test --workspace`, `cargo run`.
- La règle de précédence de configuration doit rester : **CLI > env > file > built-in**.
- `reqwest` doit rester en mode `rustls-tls` là où il est réellement nécessaire.
- Les changements doivent rester compatibles avec l’architecture multi-crates existante.
- Le comportement public ne doit pas être cassé sans justification explicite.

## Risks and Areas Requiring Care

- Réduire les permissions ou ajouter des confirmations peut changer l’expérience utilisateur du CLI/TUI.
- Les protections symlink/path traversal doivent être renforcées sans bloquer les cas valides dans le workspace.
- Le refactoring de `app.rs` peut introduire des régressions UI si les extractions ne sont pas progressives.
- Ajouter des tests sur l’agent demandera probablement une abstraction du client HTTP.
- Les écritures atomiques doivent être cohérentes entre Unix et Windows.
- Les changements CI peuvent faire émerger de la dette existante (clippy, portabilité).

## Core concepts

### 1. Sécurité outil = validation + exécution contrainte + approbation explicite

Exemple de direction pour isoler la logique d’approbation :

```rust
enum ToolPermission {
    Auto,
    Prompt,
    Deny,
}

fn resolve_permission(mode: AppMode, tool_kind: ToolKind, source: InvocationSource) -> ToolPermission {
    match (mode, tool_kind, source) {
        (AppMode::Plan, ToolKind::FileWrite | ToolKind::Shell | ToolKind::Network, _) => ToolPermission::Deny,
        (AppMode::Agent, ToolKind::FileWrite | ToolKind::Shell | ToolKind::Network, _) => ToolPermission::Prompt,
        (AppMode::Yolo, ToolKind::Shell, InvocationSource::CliAsk) => ToolPermission::Prompt,
        _ => ToolPermission::Auto,
    }
}
```

L’idée est de centraliser la politique, au lieu d’avoir plusieurs comportements implicites dans le CLI, l’agent et le TUI.

### 2. Testabilité de l’agent via abstraction du client

Exemple d’abstraction pour mocker la boucle `run_agent_turn` :

```rust
#[async_trait::async_trait]
pub trait ChatClient {
    async fn stream_chat_completion<F>(
        &self,
        messages: &[ChatMessage],
        tools: &[ApiTool],
        on_delta: F,
    ) -> anyhow::Result<AssistantResponse>
    where
        F: FnMut(&str) -> anyhow::Result<()> + Send;
}
```

Ensuite `MimoClient` implémente ce trait, et les tests injectent un faux client qui renvoie des réponses prévisibles.

### 3. Refactoring sûr d’un gros module

Pour `app.rs`, il faut éviter une réécriture massive. La bonne approche est :

1. extraire des types/états sans changer la logique,
2. déplacer des fonctions pures de rendu,
3. déplacer les handlers d’événements,
4. seulement ensuite simplifier les interfaces.

## Sub-Tasks

### Sub-Task 1: Sécuriser l’exécution agentique et les politiques d’approbation

- **Status:** Pending
- **Objective:** Corriger les écarts de sécurité les plus sensibles autour des approbations d’outils, des lectures hors workspace et des fetchs réseau.
- **Related Requirements:** R1
- **Dependencies and Preconditions:** Comprendre le comportement actuel de `mimo-cli`, `mimo-agent`, `mimo-tools`, `mimo-state`, `mimo-tui`.
- **In Scope for This Sub-Task:**
  - Revoir le comportement de `cargo run -- ask ...` qui auto-approuve tous les outils.
  - Définir une politique claire pour CLI one-shot, mode Agent, mode Plan et mode YOLO.
  - Renforcer la lecture de fichiers contre les symlinks sortant du workspace.
  - Compléter les protections SSRF sur IPv6 et redirections.
  - Ajouter un warning UX persistant ou une confirmation supplémentaire pour YOLO.
- **Out of Scope for This Sub-Task:**
  - Refonte complète UX du TUI.
  - Système avancé de sandbox OS.
- **Instructions:**
  1. Inventorier les points d’entrée d’exécution d’outils : `mimo-cli::ask`, `mimo-agent::run_agent_turn`, overlay d’approbation TUI.
  2. Décider du comportement cible pour le CLI one-shot :
     - option A recommandée : lecture/recherche auto, écriture/réseau/shell avec confirmation ou drapeau explicite `--yolo` futur.
     - option B transitoire : désactiver les outils mutables en CLI si aucun mécanisme d’approbation n’existe encore.
  3. Extraire ou centraliser la politique d’approbation pour éviter les divergences CLI/TUI.
  4. Dans `ReadFileTool`, empêcher les lectures via symlink externe (usage de `O_NOFOLLOW` ou vérification canonique finale atomique).
  5. Harmoniser et dédupliquer les helpers de filtrage d’hôte (`web_fetch`, skill install).
  6. Étendre le filtrage SSRF aux IPv6 link-local / adresses spéciales et revoir les redirections.
  7. Ajouter une protection UX sur le basculement YOLO.
- **Acceptance Criteria:**
  - Les outils mutables ne sont plus implicitement autorisés partout.
  - Une lecture via symlink hors workspace échoue clairement.
  - Les fetchs vers hôtes internes/locaux sont refusés pour IPv4 et IPv6.
  - Le comportement YOLO est explicite et visible.
- **Cautionary Points (Risks & Edge Cases):**
  - Attention à ne pas casser les lectures de fichiers valides à l’intérieur du workspace.
  - Vérifier les comportements différents Unix/Windows autour des symlinks.
  - Attention aux DNS/résolutions indirectes si la validation reste basée sur le host texte seulement.
- **Implementation Suggestions:**
  - Introduire un helper commun de politique d’approbation.
  - Introduire un helper commun `is_allowed_remote_host(...)` partagé entre crates, ou le déplacer dans un crate adapté.
  - Si aucune UX CLI interactive n’est souhaitée, documenter explicitement la restriction.
- **Testing Suggestions:**
  - Ajouter tests unitaires sur politique d’approbation.
  - Ajouter test symlink hors workspace dans `mimo-tools`.
  - Ajouter tests IPv4/IPv6 privés, loopback, link-local sur les validateurs réseau.
  - Vérifier `cargo test --workspace`.
- **Done When:**
  - Les risques de sécurité identifiés sont couverts par du code et des tests, avec comportement documenté.

### Sub-Task 2: Renforcer la robustesse des outils shell, édition et exécutions bloquantes

- **Status:** Pending
- **Objective:** Réduire les risques OOM, les remplacements de texte trop larges et les blocages liés aux appels systèmes synchrones.
- **Related Requirements:** R2
- **Dependencies and Preconditions:** Peut commencer après ou en parallèle de la sous-tâche 1, si les zones modifiées ne se chevauchent pas trop.
- **In Scope for This Sub-Task:**
  - Borner les buffers stdout/stderr du shell.
  - Clarifier la stratégie de troncation de sortie.
  - Corriger `EditFileTool` pour ne pas remplacer toutes les occurrences par défaut.
  - Migrer les appels `Command::output` sensibles vers `spawn_blocking` ou `tokio::process` selon le contexte.
- **Out of Scope for This Sub-Task:**
  - Refonte complète du shell manager.
  - Diff unifié riche si cela gonfle trop le scope.
- **Instructions:**
  1. Définir une limite mémoire raisonnable par buffer shell (ex. 4–8 MiB par flux).
  2. Choisir la politique en cas de dépassement : troncature circulaire, troncature tête, ou arrêt contrôlé du job.
  3. Faire évoluer `EditFileTool` pour un comportement plus sûr :
     - remplacer une seule occurrence par défaut,
     - ou échouer si plusieurs occurrences existent sans précision supplémentaire.
  4. Revoir `run_command` / `run_command_with_stdin` et autres exécutions synchrones déclenchées depuis du code async.
  5. Identifier les endroits où une exécution bloquante est acceptable et ceux où elle doit être isolée.
- **Acceptance Criteria:**
  - Une commande verbeuse n’entraîne plus une croissance mémoire non bornée.
  - Les remplacements de texte deviennent prédictibles.
  - Les appels bloquants critiques sont isolés du runtime async.
- **Cautionary Points (Risks & Edge Cases):**
  - La troncature ne doit pas empêcher le diagnostic utilisateur.
  - `EditFileTool` doit rester simple à consommer pour le modèle.
  - Attention aux écarts de comportement shell entre plateformes.
- **Implementation Suggestions:**
  - Préférer une structure “tail buffer” pour conserver les dernières sorties utiles.
  - Envisager `replacen(..., 1)` comme comportement par défaut minimal.
  - Pour les commandes ponctuelles non streamées, `spawn_blocking` peut suffire.
- **Testing Suggestions:**
  - Tests shell sur troncature/limite de buffer.
  - Tests `EditFileTool` pour 0, 1 et plusieurs occurrences.
  - Validation `cargo test --workspace`.
- **Done When:**
  - Les principaux risques de robustesse des tools sont couverts et testés.

### Sub-Task 3: Fiabiliser la persistance locale et factoriser les écritures atomiques

- **Status:** Pending
- **Objective:** Réduire les risques de corruption de fichiers d’état et homogénéiser la couche de persistance.
- **Related Requirements:** R3
- **Dependencies and Preconditions:** Idéalement après audit des stores dans `mimo-state`.
- **In Scope for This Sub-Task:**
  - Identifier tous les stores qui écrivent via `fs::write` direct.
  - Introduire un helper partagé d’écriture atomique et de création de parents.
  - Réutiliser ce helper pour sessions, tasks, memory, MCP, diagnostics, skills si pertinent.
  - Réduire la duplication `ensure_parent_dir`.
- **Out of Scope for This Sub-Task:**
  - Migration de format de stockage.
  - Chiffrement local des fichiers.
- **Instructions:**
  1. Cartographier les fichiers persistés par crate.
  2. Définir un helper commun soit dans `mimo-state`, soit dans un petit module partagé approprié.
  3. Gérer :
     - création du parent,
     - écriture dans fichier temporaire,
     - flush/sync si jugé nécessaire,
     - rename atomique.
  4. Uniformiser les messages d’erreur `Context(...)`.
  5. Évaluer si les permissions Unix doivent être harmonisées sur tous les fichiers de config/state.
- **Acceptance Criteria:**
  - Les writes des stores critiques passent par un chemin plus sûr et homogène.
  - La duplication évidente de helpers est réduite.
- **Cautionary Points (Risks & Edge Cases):**
  - Les garanties d’atomicité varient selon filesystem/OS.
  - Il faut éviter d’introduire des dépendances circulaires entre crates.
- **Implementation Suggestions:**
  - Conserver une API simple du style `write_string_atomic(path, contents)`.
  - Si besoin, ajouter un helper `ensure_parent_dir(path)` unique et privé au crate adapté.
- **Testing Suggestions:**
  - Ajouter tests de roundtrip sur chaque store modifié.
  - Vérifier création automatique des répertoires parents.
  - Vérifier `cargo test --workspace`.
- **Done When:**
  - Les stores critiques utilisent une stratégie d’écriture cohérente et testée.

### Sub-Task 4: Renforcer la couverture de tests et rendre l’agent testable

- **Status:** Pending
- **Objective:** Cibler les zones à fort risque avec des tests utiles, notamment la boucle agent et les stores peu couverts.
- **Related Requirements:** R4
- **Dependencies and Preconditions:** Peut dépendre partiellement des sous-tâches 1 à 3 si elles modifient les interfaces.
- **In Scope for This Sub-Task:**
  - Ajouter des tests sur `session_store`, `skill_store`, `mcp_store`.
  - Rendre `run_agent_turn` testable via abstraction de client.
  - Couvrir refus/acceptation tool, erreur tool, tours multiples, limite de rounds.
  - Ajouter au moins un test d’intégration ciblé de flux agentique si faisable sans surcomplexifier.
- **Out of Scope for This Sub-Task:**
  - Harness E2E complet du TUI interactif.
  - Tests réseau réels vers l’API MiMo.
- **Instructions:**
  1. Introduire l’abstraction minimale nécessaire pour mocker le client de chat.
  2. Adapter `run_agent_turn` pour dépendre d’une interface et non du type concret `MimoClient` quand c’est possible.
  3. Écrire des tests agent pour :
     - réponse simple sans tool,
     - demande de tool approuvée,
     - demande refusée,
     - erreur de tool,
     - dépassement `MAX_TOOL_ROUNDS`.
  4. Ajouter tests stores pour save/load/list/remove et cas invalides.
  5. Remplacer les tests temporaires fragiles par `tempfile` quand pertinent.
- **Acceptance Criteria:**
  - La logique centrale de l’agent est couverte par des tests déterministes.
  - Les stores peu couverts ont des tests de roundtrip et d’erreur.
- **Cautionary Points (Risks & Edge Cases):**
  - L’abstraction du client ne doit pas complexifier inutilement l’API publique.
  - Attention à ne pas sur-mocker au point de perdre le bénéfice des tests.
- **Implementation Suggestions:**
  - Préférer une interface locale au crate `mimo-agent` si possible.
  - Garder les tests centrés sur le comportement observable, pas les détails internes.
- **Testing Suggestions:**
  - `cargo test --workspace`
  - si nécessaire, tests filtrés par crate pendant le développement.
- **Done When:**
  - Les modules critiques disposent d’une couverture utile et les régressions majeures sont capturées.

### Sub-Task 5: Réduire la dette technique du TUI par refactoring progressif de `app.rs`

- **Status:** Pending
- **Objective:** Décomposer le module TUI principal sans casser le comportement existant ni lancer une réécriture totale.
- **Related Requirements:** R5
- **Dependencies and Preconditions:** Recommandé après sécurisation et premiers tests, pour réduire le risque de régression.
- **In Scope for This Sub-Task:**
  - Extraire des structures d’état secondaires.
  - Extraire les fonctions de rendu pures.
  - Extraire les handlers d’événements / slash commands par domaine.
  - Réduire la taille et la responsabilité de `App`.
- **Out of Scope for This Sub-Task:**
  - Refonte UX complète.
  - Changement profond de framework TUI.
- **Instructions:**
  1. Définir une stratégie par petites étapes, chacune compilable et testable.
  2. Commencer par extraire les fonctions sans dépendance forte : helpers de rendu, formatage, petits états.
  3. Regrouper ensuite les champs de `App` en sous-structures (`ConversationState`, `OverlayState`, `TaskState`, etc.) si cela simplifie réellement.
  4. Déplacer les handlers de slash commands par thème : config, plan, skills, MCP, jobs, tasks.
  5. Déplacer les handlers d’overlays/clavier spécialisés hors du corps principal.
  6. Vérifier après chaque étape que les tests TUI existants restent verts.
- **Acceptance Criteria:**
  - `app.rs` diminue sensiblement.
  - Les responsabilités sont mieux séparées.
  - Aucun changement fonctionnel involontaire n’est introduit.
- **Cautionary Points (Risks & Edge Cases):**
  - Ne pas casser les invariants de streaming (`assistant_index`, `streaming`, mise à jour delta).
  - Attention aux emprunts/mutabilités lors de l’extraction en Rust.
- **Implementation Suggestions:**
  - Préférer d’abord l’extraction de fonctions/modules avant d’introduire beaucoup de nouvelles abstractions.
  - Ne créer de nouveaux types que s’ils clarifient réellement l’état.
- **Testing Suggestions:**
  - Conserver/étendre les tests ratatui existants.
  - `cargo test --workspace`
  - `cargo check`
- **Done When:**
  - Le TUI est sensiblement plus navigable et maintenable, avec comportement inchangé.

### Sub-Task 6: Renforcer la CI et les vérifications de qualité

- **Status:** Pending
- **Objective:** Faire évoluer la CI pour détecter plus tôt les problèmes d’idiomatisme, de portabilité et de régression.
- **Related Requirements:** R6
- **Dependencies and Preconditions:** Les tests et le code doivent être suffisamment stables pour ne pas introduire une avalanche de bruit inutile.
- **In Scope for This Sub-Task:**
  - Ajouter `cargo clippy` au pipeline.
  - Étudier une matrice multi-plateforme minimale.
  - Évaluer l’ajout de `cargo audit` ou `cargo deny`.
  - Vérifier que les commandes documentées restent valides au niveau root.
- **Out of Scope for This Sub-Task:**
  - Mise en place d’une infra CI complexe ou coûteuse inutilement.
- **Instructions:**
  1. Faire évoluer `.github/workflows/ci.yml` par étapes.
  2. Ajouter d’abord `cargo clippy --workspace -- -D warnings` si acceptable pour le repo.
  3. Ajouter ensuite une matrice OS si le coût reste raisonnable.
  4. Décider si l’audit sécurité est bloquant ou informatif.
  5. Documenter toute nouvelle exigence développeur si nécessaire.
- **Acceptance Criteria:**
  - La CI couvre le formatage, la compilation, les tests, et au moins un niveau de linting.
  - Les divergences plateforme évidentes sont plus tôt détectées.
- **Cautionary Points (Risks & Edge Cases):**
  - `clippy -D warnings` peut nécessiter une phase de remise à niveau préalable.
  - La matrice multi-OS augmente le temps de CI.
- **Implementation Suggestions:**
  - Si besoin, commencer par Linux + clippy, puis ajouter les autres OS ensuite.
  - Garder la CI lisible et cohérente avec les commandes du projet.
- **Testing Suggestions:**
  - Vérifier localement, selon disponibilité :
    - `cargo fmt --check`
    - `cargo check`
    - `cargo test --workspace`
    - `cargo clippy --workspace -- -D warnings`
- **Done When:**
  - La CI est plus complète sans devenir disproportionnée par rapport au projet.

### Sub-Task 7: Corriger la documentation et aligner README / comportement réel

- **Status:** Pending
- **Objective:** Réduire les écarts entre la documentation, l’architecture réelle et les comportements effectifs du produit.
- **Related Requirements:** R7
- **Dependencies and Preconditions:** Mieux après les sous-tâches 1 et 6 si elles changent le comportement ou la CI.
- **In Scope for This Sub-Task:**
  - Corriger le README sur la liste réelle des crates.
  - Documenter les raccourcis existants non listés.
  - Clarifier le comportement d’approbation des tools selon les modes.
  - Clarifier les contraintes utiles (température, limites, commandes disponibles).
- **Out of Scope for This Sub-Task:**
  - Refonte marketing complète de la documentation.
- **Instructions:**
  1. Auditer le README par rapport au workspace réel et aux commandes CLI disponibles.
  2. Corriger la table des crates et la section commandes/contrôles.
  3. Ajouter un court paragraphe sur les modes `agent`, `plan`, `yolo` et leurs implications.
  4. Vérifier la cohérence avec `.github/copilot-instructions.md` et `AGENTS.md`.
  5. Si utile, ajouter une section “Known limits / safety model”.
- **Acceptance Criteria:**
  - Le README décrit correctement le projet tel qu’il fonctionne réellement.
  - Les principales commandes et raccourcis sont alignés avec le code.
- **Cautionary Points (Risks & Edge Cases):**
  - Ne pas documenter des comportements futurs non encore implémentés.
  - Garder la doc concise et fiable.
- **Implementation Suggestions:**
  - Utiliser les tests/commandes comme source de vérité secondaire.
  - Prioriser la correction des points qui impactent la sécurité et l’onboarding.
- **Testing Suggestions:**
  - Relecture croisée avec `mimo-cli/src/main.rs`, `mimo-tui-core/src/commands.rs`, `app.rs`, workflow CI.
  - Si besoin, lancer `cargo run -- --help` ou équivalent au moment de l’implémentation.
- **Done When:**
  - La doc ne contient plus d’écarts évidents avec l’état du code.

### Sub-Task 8: Alléger `mimo-config` en remplaçant `reqwest::Url` par une dépendance plus ciblée

- **Status:** Pending
- **Objective:** Réduire le poids conceptuel et technique de `mimo-config` en supprimant une dépendance réseau non nécessaire.
- **Related Requirements:** R8
- **Dependencies and Preconditions:** Peut être fait assez tôt, mais idéalement après les travaux de sécurité pour éviter les conflits sur la validation d’URL.
- **In Scope for This Sub-Task:**
  - Remplacer `reqwest::Url` par `url::Url` ou équivalent.
  - Mettre à jour `Cargo.toml` du workspace/crate concerné.
  - Vérifier que `normalize_base_url` garde exactement le comportement attendu.
- **Out of Scope for This Sub-Task:**
  - Refonte complète des règles de validation des URLs de config.
- **Instructions:**
  1. Introduire la dépendance la plus légère adaptée (`url`).
  2. Modifier `mimo-config` pour n’utiliser que ce parseur.
  3. Vérifier que les schémas autorisés restent `http` et `https` uniquement.
  4. Ajouter des tests sur `normalize_base_url` si absents.
- **Acceptance Criteria:**
  - `mimo-config` n’importe plus `reqwest` pour parser les URLs.
  - Le comportement de validation/normalisation reste inchangé pour les cas valides/invalides connus.
- **Cautionary Points (Risks & Edge Cases):**
  - Attention aux subtilités de parsing entre crates.
  - Vérifier les cas avec slash terminal, espaces et schémas invalides.
- **Implementation Suggestions:**
  - Ajouter tests explicites avant ou pendant le changement pour figer le comportement.
- **Testing Suggestions:**
  - Tests unitaires `normalize_base_url`.
  - `cargo test --workspace`
  - `cargo check`
- **Done When:**
  - La dépendance superflue est supprimée sans régression fonctionnelle.

## Final Integration & Verification

- **System-Wide Test:**
  1. `cargo fmt --check`
  2. `cargo check`
  3. `cargo test --workspace`
  4. si ajouté : `cargo clippy --workspace -- -D warnings`
  5. test manuel minimal :
     - `cargo run -- doctor`
     - `cargo run -- models`
     - `cargo run`
     - vérification TUI des modes Agent / Plan / YOLO
     - vérification d’une demande outil read-only et d’une demande outil mutante

- **Completion Checklist:**
  - [ ] Les politiques d’approbation sont cohérentes et sûres.
  - [ ] Les outils critiques sont plus robustes et couverts par des tests.
  - [ ] Les stores persistants utilisent des écritures plus sûres.
  - [ ] La boucle agent est testable et testée.
  - [ ] `app.rs` est significativement mieux découpé.
  - [ ] La CI reflète mieux les standards du projet.
  - [ ] Le README et les docs sont réalignés.
  - [ ] `mimo-config` est allégé sans changement de comportement.

## Open Questions

- Faut-il conserver un mode CLI `ask` pleinement autonome, ou le restreindre tant qu’un mécanisme d’approbation CLI explicite n’existe pas ?
- Souhaite-t-on traiter le refactoring de `app.rs` en plusieurs PRs dédiées, indépendantes des correctifs sécurité/tests ?
