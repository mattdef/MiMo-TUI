# Plan: Corriger le défilement vertical du menu slash

## Objective

Rendre le menu slash de MiMo-TUI entièrement navigable lorsque la liste des commandes dépasse la hauteur de la popup Ratatui, afin que les commandes situées après `/export` (`/config`, `/status`, `/retry`, `/context`, `/mcp`, etc.) soient visibles et sélectionnables.

## Requirements Snapshot

- **R1:** Identifier le composant responsable du rendu du menu slash.
- **R2:** Le menu slash doit permettre un défilement vertical complet dans Ratatui.
- **R3:** Les commandes au-delà de `/export` doivent être visibles et navigables.
- **R4:** La navigation au clavier doit rester intuitive avec `Up`/`Down`; `Tab` doit continuer à insérer la commande sélectionnée.
- **R5:** La solution doit rester ciblée sur le menu slash et ne pas réorganiser l'architecture du TUI.

## Scope

- Modifier principalement `crates/mimo-tui/src/app.rs`.
- Utiliser les données existantes de `crates/mimo-tui/src/slash_menu.rs` (`visible_entries`) et `crates/mimo-tui-core/src/commands.rs` (`COMMANDS`).
- Ajouter ou ajuster uniquement les tests nécessaires dans `crates/mimo-tui/src/app.rs` et, si utile, `crates/mimo-tui/src/slash_menu.rs`.
- Ne pas modifier la liste des commandes ni le parsing des slash commands, sauf si un test révèle une régression liée au filtrage.

## Assumptions and Constraints

- Aucun `.opencode/task.md` n'existe; ce plan est basé sur la demande utilisateur et l'exploration du code.
- Le TUI utilise Ratatui et Crossterm.
- Le rendu du menu slash est actuellement dans `App::render_slash_menu_overlay()` (`crates/mimo-tui/src/app.rs:1559`).
- L'état actuel du menu slash ne stocke que `slash_menu_selected` (`crates/mimo-tui/src/app.rs:236`), sans offset de scroll dédié.
- `Up`/`Down` changent déjà la sélection (`crates/mimo-tui/src/app.rs:602-608`), mais le `Paragraph` rendu n'applique aucun `.scroll(...)`, donc Ratatui coupe les lignes hors popup.
- Les helpers existants `adjust_selection_scroll(...)` et les patterns des overlays `model_picker`, `command_palette` et `attachment_picker` peuvent être réutilisés.
- Le support molette est optionnel et plus risqué, car `crates/mimo-tui/src/lib.rs` n'active pas actuellement `EnableMouseCapture` et `App::handle_terminal_event` ne traite pas `Event::Mouse`.

## Risks and Areas Requiring Care

- Éviter de laisser la sélection pointer hors de la liste filtrée après modification de l'input.
- Ne pas casser `Tab` pour l'autocomplétion slash ni l'attachement `@path`.
- Ne pas interférer avec les overlays prioritaires déjà gérés avant le menu slash: help, command palette, session/model picker, attachment picker, approval overlay.
- Si la molette est ajoutée, activer/désactiver MouseCapture proprement dans `TerminalGuard` pour éviter de perturber le terminal après sortie.

## Core concepts

- **Entrées visibles:** `slash_menu::visible_entries(self.input.trim())` filtre `COMMANDS` selon la commande en cours.
- **Sélection:** `slash_menu_selected` indique l'entrée active pour le style `> ...` et pour `Tab`.
- **Scroll offset:** ajouter un offset vertical, par exemple `slash_menu_scroll: u16`, puis le recalculer avec `adjust_selection_scroll(selected, scroll, visible_lines)` avant de rendre le `Paragraph` avec `.scroll((slash_menu_scroll, 0))`.
- **Hauteur visible:** calculer la hauteur intérieure du block du menu (`block.inner(popup).height`) au lieu de supposer que toute la popup peut afficher des lignes.

## Sub-Tasks

### Sub-Task 1: Ajouter l'état de scroll du menu slash

- **Status:** Pending
- **Objective:** Donner au menu slash un offset vertical persistant et initialisé.
- **Related Requirements:** R2, R3, R5
- **Dependencies and Preconditions:** Le champ `slash_menu_selected` existe déjà dans `App`.
- **In Scope for This Sub-Task:**
  - Ajouter `slash_menu_scroll: u16` à `App`.
  - Initialiser ce champ à `0` dans `App::new()`.
  - Remettre ce scroll à `0` quand la sélection slash est réinitialisée après insertion/autocomplétion.
- **Out of Scope for This Sub-Task:**
  - Support molette.
  - Changements dans `COMMANDS` ou le parser.
- **Instructions:**
  1. Ajouter le champ près de `slash_menu_selected`.
  2. Mettre à jour les endroits qui réinitialisent déjà `slash_menu_selected` (`handle_tab_key`, changements d'input si nécessaire) pour réinitialiser aussi `slash_menu_scroll`.
  3. Garder `clamp_slash_menu_selection()` responsable de la validité de l'index sélectionné.
- **Acceptance Criteria:**
  - L'état compile et `App::new()` initialise correctement le scroll.
  - Aucune logique de rendu n'est encore nécessaire pour ce sous-lot.
- **Cautionary Points (Risks & Edge Cases):**
  - Ne pas ajouter un état global partagé ou une dépendance nouvelle.
- **Implementation Suggestions:**
  - Copier le pattern des états `ModelPickerState`, `CommandPaletteState` ou `AttachmentPickerState`, qui possèdent déjà `scroll`.
- **Testing Suggestions:**
  - `cargo check` après ce sous-lot.
- **Done When:**
  - `App` possède un offset de scroll slash initialisé et utilisable par le rendu.

### Sub-Task 2: Appliquer le scroll dans `render_slash_menu_overlay`

- **Status:** Pending
- **Objective:** Faire défiler le contenu rendu pour que la sélection active reste visible dans la popup.
- **Related Requirements:** R1, R2, R3, R4
- **Dependencies and Preconditions:** Sub-Task 1 terminé.
- **In Scope for This Sub-Task:**
  - Refactor ciblé de `App::render_slash_menu_overlay()`.
  - Utilisation de `adjust_selection_scroll(...)`.
  - Application de `.scroll((self.slash_menu_scroll, 0))` sur le `Paragraph`.
- **Out of Scope for This Sub-Task:**
  - Changement visuel majeur du menu.
  - Remplacement par un widget `List` sauf si l'implémentation existante en `Paragraph` devient plus complexe que nécessaire.
- **Instructions:**
  1. Construire un `Block` unique pour le menu slash et calculer son `inner` avant le rendu.
  2. Calculer `visible_lines = inner.height.max(1) as usize`.
  3. Avant de rendre les lignes, mettre à jour `self.slash_menu_scroll = adjust_selection_scroll(self.slash_menu_selected, self.slash_menu_scroll, visible_lines)`.
  4. Rendre les lignes dans `inner` avec `Paragraph::new(Text::from(lines)).scroll((self.slash_menu_scroll, 0))`.
  5. Garder le style de sélection existant (`>`, bleu, gras).
  6. Ajouter un titre ou hint court du type `Slash menu · ↑/↓ scroll · Tab select` si cela reste lisible.
- **Acceptance Criteria:**
  - Quand la sélection descend après `/export`, la liste scroll automatiquement et les commandes suivantes deviennent visibles.
  - La ligne sélectionnée reste visible quand on monte et descend.
  - Le rendu continue à fonctionner sur petits terminaux.
- **Cautionary Points (Risks & Edge Cases):**
  - Le code actuel rend deux blocks sur `popup`; éviter les bordures doublées ou conserver le rendu sans duplication inutile.
  - Le scroll doit être calculé avec la hauteur intérieure, pas la hauteur totale du popup.
- **Implementation Suggestions:**
  - S'inspirer de `render_model_picker_overlay()` et `render_command_palette_overlay()` qui synchronisent `selected` et `scroll` avant le rendu.
- **Testing Suggestions:**
  - Ajouter un test de rendu avec `TestBackend` où `app.input` vaut `/`, `slash_menu_selected` pointe sur une commande après `/export`, puis vérifier que l'écran contient une commande tardive comme `/config`, `/status`, `/skill` ou `/mcp`.
- **Done When:**
  - Le menu slash n'est plus coupé uniquement aux premières commandes et suit la sélection.

### Sub-Task 3: Compléter la navigation clavier du menu slash

- **Status:** Pending
- **Objective:** Rendre la navigation clavier complète et prévisible.
- **Related Requirements:** R2, R3, R4
- **Dependencies and Preconditions:** Sub-Task 2 terminé.
- **In Scope for This Sub-Task:**
  - Étendre les branches `KeyCode` existantes dans `App::handle_terminal_event()` lorsque `self.slash_menu_visible()` est vrai.
  - Supporter au minimum `Up`, `Down`, `Home`, `End`, et idéalement `PageUp`/`PageDown`.
- **Out of Scope for This Sub-Task:**
  - Changement du comportement de `Enter` ou de l'exécution des commandes.
  - Support souris obligatoire.
- **Instructions:**
  1. Garder `Up`/`Down` comme navigation ligne par ligne.
  2. Ajouter `Home` pour sélectionner la première entrée et `End` pour sélectionner la dernière.
  3. Ajouter `PageUp`/`PageDown` pour déplacer la sélection par tranche visible ou par constante raisonnable si la hauteur n'est pas stockée dans l'état.
  4. Après chaque mouvement, laisser le rendu ajuster `slash_menu_scroll` via `adjust_selection_scroll`.
  5. Conserver `Tab` comme sélection/autocomplétion via `handle_tab_key()`.
- **Acceptance Criteria:**
  - `/` affiche le menu; `Down` permet d'atteindre toutes les commandes jusqu'à la fin.
  - `Up` permet de revenir vers le haut.
  - `Home`/`End` atteignent directement début/fin.
  - `Tab` insère toujours la commande sélectionnée, même si elle est hors de la première page initiale.
- **Cautionary Points (Risks & Edge Cases):**
  - Les branches globales `PageUp`/`PageDown` scrollent actuellement la conversation; elles ne doivent pas capter l'événement quand le menu slash est visible.
  - La sélection doit rester dans `0..entries.len()` même si le filtre change.
- **Implementation Suggestions:**
  - Factoriser en petites méthodes privées si le `match` devient chargé: `move_slash_menu_selection(delta)`, `set_slash_menu_selection(index)`.
- **Testing Suggestions:**
  - Ajouter des tests d'événements `Event::Key` pour `Down` répété, `Up`, `End`, `Home`, et `Tab` sur une commande tardive.
  - Vérifier que `PageDown` ne modifie pas `self.scroll` de conversation lorsque le menu slash est ouvert.
- **Done When:**
  - Le menu slash est entièrement navigable au clavier sans régression des raccourcis existants.

### Sub-Task 4: Option molette souris, uniquement si demandé pendant l'implémentation

- **Status:** Pending
- **Objective:** Ajouter un défilement à la molette si l'équipe veut un support souris en plus du clavier.
- **Related Requirements:** R2, R3
- **Dependencies and Preconditions:** Sub-Tasks 1-3 terminés; décision explicite d'accepter le risque MouseCapture.
- **In Scope for This Sub-Task:**
  - `crates/mimo-tui/src/lib.rs` pour activer/désactiver `EnableMouseCapture`/`DisableMouseCapture`.
  - `crates/mimo-tui/src/app.rs` pour traiter `Event::Mouse` avec `MouseEventKind::ScrollUp`/`ScrollDown` quand `slash_menu_visible()`.
- **Out of Scope for This Sub-Task:**
  - Clic souris pour sélectionner une ligne.
  - Drag scrollbar ou refonte du layout.
- **Instructions:**
  1. Ajouter MouseCapture dans `TerminalGuard::enter()` et son cleanup dans `Drop`.
  2. Importer et traiter les événements souris dans `handle_terminal_event`.
  3. Quand le menu slash est visible, mapper molette haut/bas aux mêmes changements que `Up`/`Down`.
  4. Ignorer les événements souris pour le menu slash quand un overlay prioritaire est ouvert.
- **Acceptance Criteria:**
  - La molette déplace la sélection et le rendu scroll pour garder la sélection visible.
  - Le terminal revient à son état normal après sortie du TUI.
- **Cautionary Points (Risks & Edge Cases):**
  - C'est une surface plus large que le bug initial; si le clavier satisfait l'acceptance, garder ce sous-lot non implémenté.
- **Implementation Suggestions:**
  - Préférer une méthode commune de navigation pour éviter de dupliquer la logique clavier/souris.
- **Testing Suggestions:**
  - Ajouter des tests unitaires d'événements souris si Crossterm les construit facilement; sinon valider manuellement dans `cargo run`.
- **Done When:**
  - La molette fonctionne sans casser les événements clavier ni l'état terminal.

### Sub-Task 5: Tests et validation finale

- **Status:** Pending
- **Objective:** Couvrir le bug signalé et vérifier l'absence de régressions.
- **Related Requirements:** R1, R2, R3, R4, R5
- **Dependencies and Preconditions:** Sub-Tasks 1-3 terminés; Sub-Task 4 si choisi.
- **In Scope for This Sub-Task:**
  - Tests ciblés de rendu et d'événements dans `crates/mimo-tui/src/app.rs`.
  - Validation manuelle du TUI.
- **Out of Scope for This Sub-Task:**
  - Tests nécessitant une API MiMo active.
- **Instructions:**
  1. Ajouter un test de rendu qui reproduit le bug: input `/`, sélection sur une commande après `/export`, rendu dans une hauteur limitée, assertion que la commande tardive est visible.
  2. Ajouter un test `Down` répété qui prouve que la sélection atteint la dernière commande de `COMMANDS`.
  3. Ajouter un test `Tab` sur une commande tardive pour vérifier l'insertion dans l'input.
  4. Si `PageUp`/`PageDown` sont ajoutés, tester qu'ils agissent sur le menu slash au lieu du scroll conversation.
  5. Lancer la validation workspace.
- **Acceptance Criteria:**
  - Le bug `/export` coupant la suite de la liste est couvert par un test.
  - Les commandes tardives (`/config`, `/skill`, `/mcp`, selon la sélection) sont visibles et sélectionnables.
  - Les commandes de validation passent.
- **Cautionary Points (Risks & Edge Cases):**
  - Les assertions de rendu doivent chercher des textes de commandes stables, pas des coordonnées exactes fragiles.
  - Utiliser `TestBackend` existant plutôt qu'une automatisation terminal externe.
- **Implementation Suggestions:**
  - Réutiliser les helpers de tests existants `test_app()` et `render_screen()`.
- **Testing Suggestions:**
  - `cargo test -p mimo-tui slash_menu`
  - `cargo fmt --check`
  - `cargo check`
  - `cargo clippy --workspace -- -D warnings`
  - `cargo test --workspace`
- **Done When:**
  - Les tests ciblés et la validation workspace confirment que le menu slash est entièrement navigable.

## Final Integration & Verification

- **System-Wide Test:**
  1. Lancer `cargo run`.
  2. Taper `/` dans l'input.
  3. Descendre avec `Down` au-delà de `/export`.
  4. Vérifier que `/config`, `/status`, puis les commandes plus basses deviennent visibles.
  5. Aller jusqu'à la fin de la liste et vérifier que `/mcp` est visible.
  6. Appuyer sur `Tab` sur une commande tardive et vérifier que la commande sélectionnée est insérée.
  7. Remonter avec `Up` et vérifier que le menu remonte correctement.
- **Completion Checklist:**
  - [ ] Composant identifié: `App::render_slash_menu_overlay()` dans `crates/mimo-tui/src/app.rs`.
  - [ ] État de scroll slash ajouté et initialisé.
  - [ ] Rendu Ratatui applique `.scroll(...)` sur le contenu du menu.
  - [ ] La sélection reste visible avec `adjust_selection_scroll(...)`.
  - [ ] `Up`/`Down` permettent d'atteindre toutes les commandes.
  - [ ] `Home`/`End` et éventuellement `PageUp`/`PageDown` fonctionnent sans voler le scroll conversation hors menu.
  - [ ] Les commandes après `/export` sont visibles et sélectionnables.
  - [ ] Tests ciblés ajoutés.
  - [ ] `cargo fmt --check`, `cargo check`, `cargo clippy --workspace -- -D warnings`, et `cargo test --workspace` passent.

## Open Questions

- Faut-il implémenter la molette immédiatement, ou considérer le défilement clavier complet comme suffisant pour ce correctif ciblé ? Recommandation: livrer d'abord le clavier; ajouter la molette seulement si le support souris est explicitement souhaité.
